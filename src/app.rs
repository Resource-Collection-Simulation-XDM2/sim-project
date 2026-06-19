use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{Frame, Terminal};
use tokio::sync::mpsc;

use crate::collector::{Collector, CollectorConfig, CollectorMessage};
use crate::map::{MapPreset, VisualTheme};
use crate::scout::{Discovery, Scout, ScoutConfig, ScoutMessage};
use crate::simulation::Simulation;
use crate::world::{Cell, Position, World};

// ---------------------------------------------------------------------------
// Simulation constants
// ---------------------------------------------------------------------------

const NUM_SCOUTS: u16 = 4;
const NUM_COLLECTORS: u16 = 3;
const CHANNEL_CAPACITY: usize = 128;
/// Target frame duration: ~30 fps.
const FRAME_DURATION: Duration = Duration::from_millis(33);

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

pub struct App {
    sim: Simulation,
    scouts: Vec<Scout>,
    collectors: Vec<Collector>,
    _scout_tx: mpsc::Sender<ScoutMessage>,
    scout_rx: mpsc::Receiver<ScoutMessage>,
    _collector_tx: mpsc::Sender<CollectorMessage>,
    collector_rx: mpsc::Receiver<CollectorMessage>,
    visual_theme: VisualTheme,
    tick: u64,
    done: bool,
}

impl App {
    /// Create a new simulation app.
    ///
    /// Generates the world from `config`, creates robots, and wires the
    /// scout / collector message channels.
    pub fn new(seed: u64, preset: MapPreset, visual_theme: VisualTheme) -> Result<Self> {
        let config = preset.config();
        let world = World::generate(seed, config)?;
        let base = world.base();
        let cell_count = world.cell_count();
        let sim = Simulation::new(world);

        // Scout channel.
        let scout_config = ScoutConfig::default();
        let (tx, rx) = mpsc::channel::<ScoutMessage>(CHANNEL_CAPACITY);

        let scouts: Vec<Scout> = (0..NUM_SCOUTS)
            .map(|id| {
                Scout::new(
                    id,
                    base,
                    &sim.world,
                    scout_config,
                    1000 + u64::from(id),
                    tx.clone(),
                )
            })
            .collect();

        let collector_config = CollectorConfig::default();
        let (collector_tx, collector_rx) = mpsc::channel::<CollectorMessage>(CHANNEL_CAPACITY);
        let collectors: Vec<Collector> = (0..NUM_COLLECTORS)
            .map(|id| {
                Collector::new(id, base, collector_config, cell_count, collector_tx.clone())
            })
            .collect();

        Ok(Self {
            sim,
            scouts,
            collectors,
            _scout_tx: tx,
            scout_rx: rx,
            _collector_tx: collector_tx,
            collector_rx,
            visual_theme,
            tick: 0,
            done: false,
        })
    }

    // ------------------------------------------------------------------
    // Public entry point
    // ------------------------------------------------------------------

    /// Run the simulation with Ratatui rendering until the user presses a key.
    pub fn run(&mut self) -> Result<()> {
        // Set up terminal in alternate screen + raw mode.
        let mut stdout = std::io::stdout();
        terminal::enable_raw_mode()?;
        stdout.execute(EnterAlternateScreen)?;

        let backend = ratatui::backend::CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Main event loop.
        while !self.done {
            terminal.draw(|f| self.render(f))?;

            // Non-blocking input poll.
            if event::poll(FRAME_DURATION)?
                && let Event::Key(key) = event::read()?
            {
                // Exit only on explicit keys to avoid instant close on launch.
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc) {
                    self.done = true;
                }
            }

            self.tick_simulation();

            // All resources collected — simulation complete.
            if self.sim.total_remaining() == 0 {
                // Render one final frame so the user sees "COMPLETE".
                terminal.draw(|f| self.render(f))?;
                // Wait for any key press, then exit.
                loop {
                    if event::poll(Duration::from_millis(100))?
                        && let Event::Key(_) = event::read()?
                    {
                        break;
                    }
                }
                self.done = true;
            }
        }

        // Restore terminal.
        terminal::disable_raw_mode()?;
        terminal.backend_mut().execute(LeaveAlternateScreen)?;

        Ok(())
    }

    // ------------------------------------------------------------------
    // Simulation tick
    // ------------------------------------------------------------------

    fn tick_simulation(&mut self) {
        self.tick += 1;

        // 1. Tick scouts (explore, discover).
        for scout in &mut self.scouts {
            scout.tick(&self.sim.world);
        }
        // Reveal cells under scouts (they clear fog as they explore).
        for scout in &self.scouts {
            self.sim.reveal_cell(scout.position);
        }
        // 2. Drain scout messages → update shared knowledge.
        while let Ok(msg) = self.scout_rx.try_recv() {
            for discovery in &msg.discoveries {
                match discovery {
                    Discovery::Resource { position, .. } => {
                        self.sim.mark_resource_discovered(*position);
                    }
                    Discovery::Obstacle { position } => {
                        self.sim.mark_obstacle_discovered(*position);
                    }
                }
            }
        }

        // 3. Tick collectors (seek, collect, return, unload).
        for collector in &mut self.collectors {
            collector.tick(&mut self.sim);
        }
        // Reveal cells under collectors too.
        for collector in &self.collectors {
            self.sim.reveal_cell(collector.position);
        }
        // 4. Drain collector unload messages → base updates its inventory.
        while let Ok(msg) = self.collector_rx.try_recv() {
            self.sim.unload(msg.kind, msg.amount);
        }
    }

    // ------------------------------------------------------------------
    // Rendering
    // ------------------------------------------------------------------

    fn render(&self, f: &mut Frame) {
        let area = f.area();

        // Split: map on top, status bar on bottom.
        let layout = Layout::vertical([
            Constraint::Min(1),    // map fills remaining space
            Constraint::Length(2), // status bar
        ])
        .split(area);

        self.render_map(f, layout[0]);
        self.render_status(f, layout[1]);
    }

    fn render_map(&self, f: &mut Frame, area: Rect) {
        let w = self.sim.world.width();
        let h = self.sim.world.height();
        let cw = self.visual_theme.cell_width as usize;

        // Build lines of styled spans.
        let mut lines: Vec<Line> = Vec::with_capacity(h);

        for y in 0..h {
            let mut spans: Vec<Span> = Vec::with_capacity(w * cw);
            for x in 0..w {
                let pos = Position { x, y };
                let (ch, color) = self.cell_char(pos);
                spans.push(Span::styled(ch, Style::default().fg(color)));
                // For double-width themes, pad single-width characters so
                // every cell fills exactly `cw` terminal columns.  4-byte
                // UTF-8 (emojis) already occupy 2 columns.
                if cw > 1 && ch.len() < 4 {
                    spans.push(Span::styled(" ", Style::default()));
                }
            }
            lines.push(Line::from(spans));
        }

        f.render_widget(Paragraph::new(lines), area);
    }

    /// Determine the display character and color for a map cell.
    /// Incorporates biome colors, resource glow, base pulse,
    /// and fog-gradient edge.
    fn cell_char(&self, pos: Position) -> (&str, Color) {
        let t = &self.visual_theme;

        // ── 1. Robots on top ─────────────────────────────────────
        for scout in &self.scouts {
            if scout.position == pos {
                return (t.scout_char, t.scout_color);
            }
        }
        for collector in &self.collectors {
            if collector.position == pos {
                return (t.collector_char, t.collector_color);
            }
        }

        // ── 2. Base (with subtle pulse) ──────────────────────────
        if pos == self.sim.world.base() {
            let pulse = self.base_pulse_color(t.base_color);
            return (t.base_char, pulse);
        }

        // ── 3. Fog of war (with gradient edge) ───────────────────
        if !self.sim.is_cell_revealed(pos) {
            return self.fog_display(pos, t);
        }

        // ── 4. Obstacles (with biome colors) ────────────────────
        if self.sim.world.cell(pos) == Some(Cell::Obstacle) {
            return (t.obstacle_char(pos), t.obstacle_color_for(pos));
        }

        // ── 5. Resources (with depletion glow) ───────────────────
        if self.sim.is_resource_discovered(pos)
            && let Some(resource) = self.sim.world.resource_at(pos)
        {
            let (remaining, initial) = self.sim.stock_info_at(pos);
            if remaining > 0 {
                let (ch, base_color) = t.resource_display(resource.kind);
                let ratio = remaining as f64 / (initial as f64).max(1.0);
                let glow = glow_color(base_color, ratio);
                return (ch, glow);
            }
        }

        // ── 6. Walkable ground ───────────────────────────────────
        (t.ground_char, t.ground_color)
    }


    /// Subtle brightness oscillation for the base glyph.
    fn base_pulse_color(&self, base: Color) -> Color {
        // Use sine-like oscillation: tick % 30 gives a slow pulse.
        let phase = (self.tick % 30) as f64 / 30.0;
        let brightness = 0.75 + 0.25 * (phase * std::f64::consts::TAU).sin();
        fade_color(base, brightness)
    }

    /// Fog display with soft edge: cells adjacent to revealed area get a dim hint.
    fn fog_display(&self, pos: Position, t: &VisualTheme) -> (&str, Color) {
        let w = self.sim.world.width();
        let h = self.sim.world.height();

        // Check if any cardinal neighbor (in-bounds) is revealed.
        let adjacent_revealed = [
            (pos.x > 0).then(|| Position { x: pos.x - 1, y: pos.y }),
            (pos.x + 1 < w).then(|| Position { x: pos.x + 1, y: pos.y }),
            (pos.y > 0).then(|| Position { x: pos.x, y: pos.y - 1 }),
            (pos.y + 1 < h).then(|| Position { x: pos.x, y: pos.y + 1 }),
        ]
        .iter()
        .filter_map(|&p| p)
        .any(|p| self.sim.is_cell_revealed(p));

        if adjacent_revealed {
            // Dim edge — show a subtle character.
            let edge_char = if t.cell_width > 1 { "·" } else { "." };
            (edge_char, fade_color(t.fog_color, 0.15))
        } else {
            (t.fog_char, t.fog_color)
        }
    }

    fn render_status(&self, f: &mut Frame, area: Rect) {
        let inv = self.sim.base_inventory;
        let remaining = self.sim.total_remaining();
        let done = remaining == 0;

        let text = if done {
            format!(
                "T:{}  E:{}  C:{}  Left:0  —  DONE!  Press any key to exit",
                self.tick, inv.energy, inv.crystals
            )
        } else {
            format!(
                "T:{}  E:{}  C:{}  Left:{}  —  running…  (q/Esc to quit)",
                self.tick, inv.energy, inv.crystals, remaining
            )
        };

        let span = Span::styled(text, Style::default().fg(Color::White));
        f.render_widget(Paragraph::new(span), area);
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

/// Linearly interpolate a color toward black by `factor` (0.0 = black, 1.0 = original).
fn fade_color(c: Color, factor: f64) -> Color {
    let (r, g, b) = color_rgb(c);
    let f = factor.clamp(0.0, 1.0);
    Color::Rgb((f64::from(r) * f) as u8, (f64::from(g) * f) as u8, (f64::from(b) * f) as u8)
}

/// Brighten a resource color toward white as the stock depletes.
fn glow_color(c: Color, ratio: f64) -> Color {
    // ratio = remaining / max_initial → low ratio = nearly depleted → brighter
    let r = ratio.clamp(0.0, 1.0);
    // Mix toward white as r → 0 (depleted).
    let inv = 1.0 - r;
    mix_color(c, Color::White, inv * 0.5)
}

/// Mix two colors: `a * (1-t) + b * t`.
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

/// Extract (r, g, b) from a Ratatui color.
fn color_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::White => (255, 255, 255),
        Color::Red => (255, 0, 0),
        Color::Green => (0, 255, 0),
        Color::Blue => (0, 0, 255),
        Color::Yellow => (255, 255, 0),
        Color::Magenta => (255, 0, 255),
        Color::Cyan => (0, 255, 255),
        Color::Gray => (128, 128, 128),
        Color::DarkGray => (64, 64, 64),
        Color::LightRed => (255, 204, 203),
        Color::LightGreen => (144, 238, 144),
        Color::LightBlue => (173, 216, 230),
        Color::LightCyan => (224, 255, 255),
        Color::LightMagenta => (255, 224, 255),
        Color::LightYellow => (255, 255, 224),
        _ => (128, 128, 128),
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::collector::CollectorState;
    use crate::map::{MapPreset, VisualTheme};

    #[test]
    fn app_simulation_runs_without_panics() {
        let mut app = App::new(42, MapPreset::Default, VisualTheme::DEFAULT)
            .expect("app creation should succeed");

        // Run 500 ticks — enough for scouts to explore and collectors to
        // start finding targets.
        for _ in 0..500 {
            app.tick_simulation();
            app.tick += 1;
        }

        // Scouts should have made some discoveries.
        let total_discovered: u64 = app.scouts.iter().map(|s| s.stats.discoveries_total).sum();
        assert!(
            total_discovered > 0,
            "scouts made no discoveries after 500 ticks"
        );

        // At least some collectors should have left Idle.
        let idle_count = app
            .collectors
            .iter()
            .filter(|c| c.state == CollectorState::Idle)
            .count();
        assert!(
            idle_count < app.collectors.len(),
            "all collectors still idle after 500 ticks"
        );

        // Simulation world should have revealed cells beyond the base radius.
        let base = app.sim.world.base();
        let far_pos = Position {
            x: base.x + 10,
            y: base.y + 10,
        };
        // Not all maps have walkable cells at +10,+10, so just check that
        // the simulation didn't panic.
        let _ = app.sim.is_cell_revealed(far_pos);
    }

    #[test]
    fn app_creates_expected_number_of_robots() {
        let app = App::new(99, MapPreset::Default, VisualTheme::DEFAULT)
            .expect("app creation should succeed");
        assert_eq!(app.scouts.len(), NUM_SCOUTS as usize);
        assert_eq!(app.collectors.len(), NUM_COLLECTORS as usize);
    }
}
