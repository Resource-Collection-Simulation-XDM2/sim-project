use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal};
use tokio::sync::{mpsc, Barrier, RwLock};

use crate::collision::OccupancyGrid;
use crate::collector::{Collector, CollectorConfig, CollectorMessage};
use crate::concurrency::{self, SharedOcc, SharedSim};
use crate::map::{MapPreset, VisualTheme};
use crate::scout::{Discovery, Scout, ScoutConfig, ScoutMessage};
use crate::simulation::Simulation;
use crate::world::{Cell, Position, World};

const NUM_SCOUTS: u16 = 4;
const NUM_COLLECTORS: u16 = 3;
const CHANNEL_CAPACITY: usize = 128;
const FRAME_DURATION: Duration = Duration::from_millis(33);

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
    /// Snapshot of fog-of-war, updated between ticks so rendering never
    /// contends with robot write locks.
    revealed_cache: Vec<bool>,
    tick: u64,
    done: bool,
}

impl App {
    pub fn new(seed: u64, preset: MapPreset, visual_theme: VisualTheme) -> Result<Self> {
        let config = preset.config();
        let world = World::generate(seed, config)?;
        let base = world.base();
        let cell_count = world.cell_count();
        let sim_world_clone = world.clone();
        let sim = Arc::new(RwLock::new(Simulation::new(world)));
        let occupancy = Arc::new(RwLock::new(OccupancyGrid::new(&sim_world_clone)));

        let num_robots = NUM_SCOUTS as usize + NUM_COLLECTORS as usize;
        let barrier = Arc::new(Barrier::new(num_robots + 1));

        let (scout_tx, scout_rx) = mpsc::channel::<ScoutMessage>(CHANNEL_CAPACITY);
        let (collector_tx, collector_rx) = mpsc::channel::<CollectorMessage>(CHANNEL_CAPACITY);
        let (position_tx, position_rx) = mpsc::channel::<(u16, Position)>(CHANNEL_CAPACITY);

        let scout_config = ScoutConfig::default();
        for id in 0..NUM_SCOUTS {
            let scout = Scout::new(id, base, &sim_world_clone, scout_config, 1000 + u64::from(id), scout_tx.clone());
            tokio::spawn(concurrency::run_scout(
                scout, sim.clone(), occupancy.clone(), barrier.clone(), position_tx.clone(),
            ));
        }

        let collector_config = CollectorConfig::default();
        for id in 0..NUM_COLLECTORS {
            let collector = Collector::new(id + NUM_SCOUTS, base, collector_config, cell_count, collector_tx.clone());
            tokio::spawn(concurrency::run_collector(
                collector, sim.clone(), occupancy.clone(), barrier.clone(), position_tx.clone(),
            ));
        }

        drop(scout_tx);
        drop(collector_tx);
        drop(position_tx);

        Ok(Self {
            sim, sim_world: sim_world_clone, occupancy: occupancy.clone(), visual_theme,
            scout_rx, collector_rx, position_rx, barrier,
            scout_positions: vec![base; NUM_SCOUTS as usize],
            collector_positions: vec![base; NUM_COLLECTORS as usize],
            revealed_cache: vec![false; cell_count],
            tick: 0, done: false,
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut stdout = std::io::stdout();
        terminal::enable_raw_mode()?;
        stdout.execute(EnterAlternateScreen)?;
        let backend = ratatui::backend::CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

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
                // Snapshot fog-of-war for flicker-free rendering.
                self.revealed_cache.copy_from_slice(sim_guard.revealed_bitmap());
            }

            self.tick += 1;
            terminal.draw(|f| self.render(f))?;

            if event::poll(FRAME_DURATION)? && let Event::Key(_) = event::read()? {
                self.done = true;
            }

            {
                let sim_guard = self.sim.read().await;
                if sim_guard.total_remaining() == 0 {
                    terminal.draw(|f| self.render(f))?;
                    loop {
                        if event::poll(Duration::from_millis(100))? && let Event::Key(_) = event::read()? {
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
        let layout = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(area);
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
        if self.scout_positions.contains(&pos) {
            return (t.scout_char, t.scout_color);
        }
        if self.collector_positions.contains(&pos) {
            return (t.collector_char, t.collector_color);
        }
        if pos == self.sim_world.base() {
            return (t.base_char, self.base_pulse_color(t.base_color));
        }
        let idx = pos.y * self.sim_world.width() + pos.x;
        let revealed = self.revealed_cache.get(idx).copied().unwrap_or(false);
        if !revealed {
            return self.fog_display(pos, t);
        }
        if self.sim_world.cell(pos) == Some(Cell::Obstacle) {
            return (t.obstacle_char(pos), t.obstacle_color_for(pos));
        }
        if let Ok(sim_guard) = self.sim.try_read()
            && sim_guard.is_resource_discovered(pos) && let Some(resource) = self.sim_world.resource_at(pos) {
                let (remaining, initial) = sim_guard.stock_info_at(pos);
                if remaining > 0 {
                    let (ch, base_color) = t.resource_display(resource.kind);
                    return (ch, glow_color(base_color, remaining as f64 / (initial as f64).max(1.0)));
                }
            }
        (t.ground_char, t.ground_color)
    }

    fn base_pulse_color(&self, base: Color) -> Color {
        let phase = (self.tick % 30) as f64 / 30.0;
        fade_color(base, 0.75 + 0.25 * (phase * std::f64::consts::TAU).sin())
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
        let (energy, crystals, remaining) = self.sim.try_read()
            .map(|g| (g.base_inventory.energy, g.base_inventory.crystals, g.total_remaining()))
            .unwrap_or((0, 0, 0));
        let done = remaining == 0;
        let text = if done {
            format!("T:{}  E:{}  C:{}  Left:0   -   DONE!  Press any key to exit ", self.tick, energy, crystals)
        } else {
            format!("T:{}  E:{}  C:{}  Left:{}   -   running...", self.tick, energy, crystals, remaining)
        };
        f.render_widget(Paragraph::new(Span::styled(text, Style::default().fg(Color::White))), area);
    }
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
        let mut app = App::new(42, MapPreset::Default, VisualTheme::DEFAULT)
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
        let app = App::new(99, MapPreset::Default, VisualTheme::DEFAULT)
            .expect("app create");
        assert_eq!(app.scout_positions.len(), NUM_SCOUTS as usize);
        assert_eq!(app.collector_positions.len(), NUM_COLLECTORS as usize);
    }
}
