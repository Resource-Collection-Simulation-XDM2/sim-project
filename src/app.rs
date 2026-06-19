use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph};
use ratatui::{Frame, Terminal};
use tokio::sync::{mpsc, Barrier, RwLock};

use crate::occupancy::OccupancyGrid;
use crate::collector::{Collector, CollectorConfig, CollectorMessage};
use crate::concurrency::{self, SharedOcc, SharedSim};
use crate::map::{MapPreset, VisualTheme};
use crate::scout::{Discovery, Scout, ScoutConfig, ScoutMessage};
use crate::simulation::{RevealMode, Simulation};
use crate::world::{Cell, Position, World};

const NUM_SCOUTS: u16 = 4;
const NUM_COLLECTORS: u16 = 3;
const CHANNEL_CAPACITY: usize = 128;
const FRAME_DURATION: Duration = Duration::from_millis(33);

/// Which type of robot occupies a cell (for O(1) rendering lookup).
#[derive(Clone, Copy)]
enum RobotKind { Scout, Collector }

pub struct App {
    sim: SharedSim,
    sim_world: World,
    occupancy: SharedOcc,
    visual_theme: VisualTheme,
    scout_rx: mpsc::Receiver<ScoutMessage>,
    collector_rx: mpsc::Receiver<CollectorMessage>,
    position_rx: mpsc::Receiver<(u16, Position)>,
    barrier: Arc<Barrier>,
    scout_positions: Vec<Position>,
    collector_positions: Vec<Position>,
    /// Per-cell robot occupancy bitmap — O(1) lookup during rendering.
    robot_at: Vec<Option<RobotKind>>,
    revealed_cache: Vec<bool>,
    /// Snapshot of inventory + remaining, updated between ticks.
    cached_energy: u64,
    cached_crystals: u64,
    cached_remaining: u64,
    /// Cached resource stock data — avoids lock contention during rendering.
    stock_at_cache: Vec<Option<usize>>,
    stock_remaining_cache: Vec<u16>,
    stock_initial_cache: Vec<u16>,
    resource_discovered_cache: Vec<bool>,
    cached_initial_total: u64,
    tick: u64,
    done: bool,
}

impl App {
    pub async fn new(seed: u64, preset: MapPreset, visual_theme: VisualTheme, reveal_mode: RevealMode) -> Result<Self> {
        let config = preset.config();
        let world = World::generate(seed, config)?;
        let base = world.base();
        let cell_count = world.cell_count();
        let sim_world_clone = world.clone();
        let sim = Arc::new(RwLock::new(Simulation::new(world)));
        sim.write().await.apply_reveal_mode(reveal_mode);
        let occupancy = Arc::new(RwLock::new(OccupancyGrid::new(&sim_world_clone)));

        let spawn_robots = reveal_mode != RevealMode::Full;
        let num_robots = if spawn_robots { NUM_SCOUTS as usize + NUM_COLLECTORS as usize } else { 0 };
        let barrier = Arc::new(Barrier::new(num_robots + 1));

        let (scout_tx, scout_rx) = mpsc::channel::<ScoutMessage>(CHANNEL_CAPACITY);
        let (collector_tx, collector_rx) = mpsc::channel::<CollectorMessage>(CHANNEL_CAPACITY);
        let (position_tx, position_rx) = mpsc::channel::<(u16, Position)>(CHANNEL_CAPACITY);

        let scout_config = ScoutConfig::default();
        if spawn_robots {
            for id in 0..NUM_SCOUTS {
                let scout = Scout::new(id, base, &sim_world_clone, scout_config, 1000 + u64::from(id), scout_tx.clone());
                tokio::spawn(concurrency::run_scout(
                    scout, sim.clone(), occupancy.clone(), barrier.clone(), position_tx.clone(),
                ));
            }
        }

        let collector_config = CollectorConfig::default();
        if spawn_robots {
            for id in 0..NUM_COLLECTORS {
                let collector = Collector::new(id + NUM_SCOUTS, base, collector_config, cell_count, collector_tx.clone());
                tokio::spawn(concurrency::run_collector(
                    collector, sim.clone(), occupancy.clone(), barrier.clone(), position_tx.clone(),
                ));
            }
        }

        drop(scout_tx);
        drop(collector_tx);
        drop(position_tx);

        let initial_remaining: u64 = sim_world_clone.resources().iter().map(|n| u64::from(n.quantity)).sum();
        let stock_count = sim_world_clone.resources().len();

        Ok(Self {
            sim, sim_world: sim_world_clone, occupancy: occupancy.clone(), visual_theme,
            scout_rx, collector_rx, position_rx, barrier,
            scout_positions: vec![base; NUM_SCOUTS as usize],
            collector_positions: vec![base; NUM_COLLECTORS as usize],
            revealed_cache: vec![false; cell_count],
            robot_at: vec![None; cell_count],
            cached_energy: 0,
            cached_crystals: 0,
            cached_remaining: initial_remaining,
            stock_at_cache: vec![None; cell_count],
            stock_remaining_cache: vec![0; stock_count],
            stock_initial_cache: vec![0; stock_count],
            resource_discovered_cache: vec![false; stock_count],
            cached_initial_total: initial_remaining,
            tick: 0, done: false,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut stdout = std::io::stdout();
        terminal::enable_raw_mode()?;
        stdout.execute(EnterAlternateScreen)?;
        let backend = ratatui::backend::CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Discard keys still buffered from launching (e.g. Enter after `cargo run`).
        while event::poll(Duration::from_millis(0))? {
            let _ = event::read()?;
        }

        while !self.done {
            // Prepare occupancy grid for this tick.
            {
                let mut occ = self.occupancy.write().await;
                occ.clear();
                for pos in self.scout_positions.iter().chain(&self.collector_positions) {
                    occ.try_reserve(*pos);
                }
            }

            self.barrier.wait().await;

            // Clear old robot positions from bitmap — index-based, no alloc.
            for i in 0..self.scout_positions.len() {
                self.set_robot_at(self.scout_positions[i], None);
            }
            for i in 0..self.collector_positions.len() {
                self.set_robot_at(self.collector_positions[i], None);
            }

            while let Ok((id, pos)) = self.position_rx.try_recv() {
                if id < NUM_SCOUTS {
                    self.scout_positions[id as usize] = pos;
                } else {
                    let cid = id - NUM_SCOUTS;
                    if (cid as usize) < self.collector_positions.len() {
                        self.collector_positions[cid as usize] = pos;
                    }
                }
            }

            // Rebuild bitmap from updated positions — index-based, no alloc.
            for i in 0..self.scout_positions.len() {
                self.set_robot_at(self.scout_positions[i], Some(RobotKind::Scout));
            }
            for i in 0..self.collector_positions.len() {
                self.set_robot_at(self.collector_positions[i], Some(RobotKind::Collector));
            }

            {
                let mut sim_guard = self.sim.write().await;
                while let Ok(msg) = self.scout_rx.try_recv() {
                    for d in &msg.discoveries {
                        match d {
                            Discovery::Resource { position, .. } => sim_guard.mark_resource_discovered(*position),
                            Discovery::Obstacle { position } => sim_guard.mark_obstacle_discovered(*position),
                        }
                    }
                }
                while let Ok(msg) = self.collector_rx.try_recv() {
                    sim_guard.unload(msg.kind, msg.amount);
                }
                for pos in self.scout_positions.iter().chain(&self.collector_positions) {
                    sim_guard.reveal_cell(*pos);
                }
                // Snapshot state for flicker-free rendering.
                self.revealed_cache.copy_from_slice(sim_guard.revealed_bitmap());
                self.cached_energy = sim_guard.base_inventory.energy;
                self.cached_crystals = sim_guard.base_inventory.crystals;
                self.cached_remaining = sim_guard.total_remaining();

                // Snapshot resource stock data so rendering never needs the lock.
                self.stock_at_cache.copy_from_slice(&sim_guard.stock_at);
                for (i, stock) in sim_guard.stocks.iter().enumerate() {
                    self.stock_remaining_cache[i] = stock.remaining;
                    self.stock_initial_cache[i] = stock.initial;
                    self.resource_discovered_cache[i] = sim_guard.resource_discovered[i];
                }
            }

            self.tick += 1;
            terminal.draw(|f| self.render(f))?;

            if event::poll(FRAME_DURATION)?
                && is_quit_key(&event::read()?)
            {
                self.done = true;
            }

            {
                if self.cached_remaining == 0 {
                    terminal.draw(|f| self.render(f))?;
                    loop {
                        if event::poll(Duration::from_millis(100))?
                            && is_quit_key(&event::read()?)
                        {
                            break;
                        }
                    }
                    self.done = true;
                }
            }
        }

        terminal::disable_raw_mode()?;
        terminal.backend_mut().execute(LeaveAlternateScreen)?;
        Ok(())
    }

    fn render(&self, f: &mut Frame) {
        let area = f.area();
        let layout = Layout::vertical([Constraint::Min(1), Constraint::Length(6)]).split(area);
        self.render_map(f, layout[0]);
        self.render_status(f, layout[1]);
    }

    fn render_map(&self, f: &mut Frame, area: Rect) {
        let w = self.sim_world.width();
        let h = self.sim_world.height();
        let cw = self.visual_theme.cell_width as usize;
        let mut lines: Vec<Line> = Vec::with_capacity(h);
        for y in 0..h {
            let mut spans: Vec<Span> = Vec::with_capacity(w * cw);
            for x in 0..w {
                let pos = Position { x, y };
                let (ch, color) = self.cell_char(pos);
                spans.push(Span::styled(ch, Style::default().fg(color)));
                if cw > 1 && ch.len() < 4 {
                    spans.push(Span::styled(" ", Style::default()));
                }
            }
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), area);
    }

    fn cell_char(&self, pos: Position) -> (&str, Color) {
        let t = &self.visual_theme;
        let idx = pos.y * self.sim_world.width() + pos.x;
        // O(1) bitmap lookup instead of Vec::contains scan.
        match self.robot_at.get(idx).copied().flatten() {
            Some(RobotKind::Scout) => return (t.scout_char, t.scout_color),
            Some(RobotKind::Collector) => return (t.collector_char, t.collector_color),
            None => {}
        }
        if pos == self.sim_world.base() {
            return (t.base_char, self.base_pulse_color(t.base_color));
        }
        let revealed = self.revealed_cache.get(idx).copied().unwrap_or(false);
        if !revealed {
            return self.fog_display(pos, t);
        }
        if self.sim_world.cell(pos) == Some(Cell::Obstacle) {
            return (t.obstacle_char(pos), t.obstacle_color_for(pos));
        }
        // Use cached stock data — no lock needed, zero flicker.
        if let Some(stock_idx) = self.stock_at_cache.get(idx).copied().flatten()
            && stock_idx < self.resource_discovered_cache.len()
            && self.resource_discovered_cache[stock_idx]
            && stock_idx < self.stock_remaining_cache.len()
        {
            let remaining = self.stock_remaining_cache[stock_idx];
            if remaining > 0 {
                let initial = self.stock_initial_cache[stock_idx];
                let ratio = remaining as f64 / (initial as f64).max(1.0);
                let resource = &self.sim_world.resources()[stock_idx];
                let (ch, base_color) = t.resource_display(resource.kind);
                return (ch, self.resource_pulse_color(base_color, ratio));
            }
        }
        (t.ground_char, t.ground_color)
    }

    fn base_pulse_color(&self, base: Color) -> Color {
        let phase = (self.tick % 30) as f64 / 30.0;
        fade_color(base, 0.75 + 0.25 * (phase * std::f64::consts::TAU).sin())
    }

    /// Smooth pulse + depletion fade for resources, like the base glow.
    fn resource_pulse_color(&self, base_color: Color, ratio: f64) -> Color {
        let phase = (self.tick % 30) as f64 / 30.0;
        let pulse = 0.80 + 0.20 * (phase * std::f64::consts::TAU).sin();
        let depleted = glow_color(base_color, ratio);
        fade_color(depleted, pulse)
    }

    fn fog_display(&self, pos: Position, t: &VisualTheme) -> (&str, Color) {
        let w = self.sim_world.width();
        let h = self.sim_world.height();
        let revealed = |p: Position| {
            let i = p.y * w + p.x;
            self.revealed_cache.get(i).copied().unwrap_or(false)
        };
        let adj = [
            (pos.x > 0).then(|| Position { x: pos.x - 1, y: pos.y }),
            (pos.x + 1 < w).then(|| Position { x: pos.x + 1, y: pos.y }),
            (pos.y > 0).then(|| Position { x: pos.x, y: pos.y - 1 }),
            (pos.y + 1 < h).then(|| Position { x: pos.x, y: pos.y + 1 }),
        ].iter().filter_map(|&p| p).any(revealed);
        if adj {
            let edge_char = if t.cell_width > 1 { "·" } else { "." };
            (edge_char, fade_color(t.fog_color, 0.15))
        } else {
            (t.fog_char, t.fog_color)
        }
    }

    fn render_status(&self, f: &mut Frame, area: Rect) {
        let t = &self.visual_theme;
        let done = self.cached_remaining == 0;
        let initial = self.cached_initial_total.max(1);
        let progress = if done {
            1.0
        } else {
            (initial - self.cached_remaining) as f64 / initial as f64
        };
        let pct = (progress * 100.0).round() as u16;

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if done {
                Color::LightGreen
            } else {
                Color::DarkGray
            }))
            .title(if done {
                Line::from(Span::styled(
                    " ✓ Mission complete ",
                    Style::default()
                        .fg(Color::LightGreen)
                        .add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(Span::styled(
                    " Resource Collection ",
                    Style::default().fg(Color::Cyan),
                ))
            });

        let inner = block.inner(area);
        f.render_widget(block, area);

        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

        let row1 = Line::from(vec![
            Span::styled("Tick ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:<6}", self.tick),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  │  "),
            Span::styled(
                format!("{} Scouts", NUM_SCOUTS),
                Style::default().fg(t.scout_color),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{} Collectors", NUM_COLLECTORS),
                Style::default().fg(t.collector_color),
            ),
        ]);
        f.render_widget(Paragraph::new(row1), rows[0]);

        let row2 = Line::from(vec![
            Span::styled(
                format!(" {} ", t.energy_char),
                Style::default().fg(t.energy_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("Energy ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:<6}", self.cached_energy),
                Style::default().fg(t.energy_color),
            ),
            Span::raw("  │  "),
            Span::styled(
                format!(" {} ", t.crystal_char),
                Style::default()
                    .fg(t.crystal_color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("Crystals ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:<6}", self.cached_crystals),
                Style::default().fg(t.crystal_color),
            ),
            Span::raw("  │  "),
            Span::styled("Left ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}", self.cached_remaining),
                Style::default().fg(if done { Color::LightGreen } else { Color::Yellow }),
            ),
        ]);
        f.render_widget(Paragraph::new(row2), rows[1]);

        let gauge_color = if done { Color::LightGreen } else { Color::Green };
        let gauge = Gauge::default()
            .gauge_style(Style::default().fg(gauge_color).bg(Color::DarkGray))
            .ratio(progress.min(1.0))
            .label(format!("{pct}% collected"));
        f.render_widget(gauge, rows[2]);

        let hint = if done {
            Line::from(vec![
                Span::styled("Press ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "Q",
                    Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" to exit", Style::default().fg(Color::DarkGray)),
            ])
        } else {
            Line::from(vec![
                Span::styled("● ", Style::default().fg(Color::Green)),
                Span::styled("Running", Style::default().fg(Color::White)),
                Span::styled("  —  press ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    "Q",
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                ),
                Span::styled(" to quit", Style::default().fg(Color::DarkGray)),
            ])
        };
        f.render_widget(
            Paragraph::new(hint).alignment(Alignment::Center),
            rows[3],
        );
    }

    #[inline]
    fn set_robot_at(&mut self, pos: Position, kind: Option<RobotKind>) {
        let idx = pos.y * self.sim_world.width() + pos.x;
        if idx < self.robot_at.len() {
            self.robot_at[idx] = kind;
        }
    }
}

fn is_quit_key(event: &Event) -> bool {
    matches!(
        event,
        Event::Key(key) if key.code == KeyCode::Char('q') || key.code == KeyCode::Char('Q')
    )
}

fn fade_color(c: Color, factor: f64) -> Color {
    let (r, g, b) = color_rgb(c);
    let f = factor.clamp(0.0, 1.0);
    Color::Rgb((f64::from(r) * f) as u8, (f64::from(g) * f) as u8, (f64::from(b) * f) as u8)
}

fn glow_color(c: Color, ratio: f64) -> Color {
    mix_color(c, Color::White, (1.0 - ratio.clamp(0.0, 1.0)) * 0.5)
}

fn mix_color(a: Color, b: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (ar, ag, ab) = color_rgb(a);
    let (br, bg, bb) = color_rgb(b);
    Color::Rgb(
        (ar as f64 + (br as f64 - ar as f64) * t) as u8,
        (ag as f64 + (bg as f64 - ag as f64) * t) as u8,
        (ab as f64 + (bb as f64 - ab as f64) * t) as u8,
    )
}

fn color_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0), Color::White => (255, 255, 255),
        Color::Red => (255, 0, 0), Color::Green => (0, 255, 0), Color::Blue => (0, 0, 255),
        Color::Yellow => (255, 255, 0), Color::Magenta => (255, 0, 255), Color::Cyan => (0, 255, 255),
        Color::Gray => (128, 128, 128), Color::DarkGray => (64, 64, 64),
        Color::LightRed => (255, 204, 203), Color::LightGreen => (144, 238, 144),
        Color::LightBlue => (173, 216, 230), Color::LightCyan => (224, 255, 255),
        Color::LightMagenta => (255, 224, 255), Color::LightYellow => (255, 255, 224),
        _ => (128, 128, 128),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn app_simulation_runs_without_panics() {
        let mut app = App::new(42, MapPreset::Default, VisualTheme::DEFAULT, RevealMode::Normal).await
            .expect("app create");
        for _ in 0..200 {
            app.barrier.wait().await;
            while app.position_rx.try_recv().is_ok() {}
            while app.scout_rx.try_recv().is_ok() {}
            while app.collector_rx.try_recv().is_ok() {}
            app.tick += 1;
        }
        assert!(app.tick > 0);
    }

    #[tokio::test]
    async fn app_creates_expected_number_of_robots() {
        let app = App::new(99, MapPreset::Default, VisualTheme::DEFAULT, RevealMode::Normal).await
            .expect("app create");
        assert_eq!(app.scout_positions.len(), NUM_SCOUTS as usize);
        assert_eq!(app.collector_positions.len(), NUM_COLLECTORS as usize);
    }
}
