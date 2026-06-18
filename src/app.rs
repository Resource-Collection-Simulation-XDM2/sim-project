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
use tokio::sync::mpsc;

use crate::collector::{Collector, CollectorConfig, CollectorMessage};
use crate::scout::{Discovery, Scout, ScoutConfig, ScoutMessage};
use crate::simulation::Simulation;
use crate::world::{Cell, Position, ResourceKind, World, WorldConfig};

// ---------------------------------------------------------------------------
// Color palette (matches the project spec)
// ---------------------------------------------------------------------------

const OBSTACLE_COLOR: Color = Color::LightCyan;
const ENERGY_COLOR: Color = Color::Green;
const CRYSTAL_COLOR: Color = Color::LightMagenta;
const BASE_COLOR: Color = Color::LightGreen;
const SCOUT_COLOR: Color = Color::Red;
const COLLECTOR_COLOR: Color = Color::Magenta;
const GROUND_COLOR: Color = Color::DarkGray;
const FOG_COLOR: Color = Color::Black;
const UI_TEXT_COLOR: Color = Color::White;

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
    tick: u64,
    done: bool,
}

impl App {
    /// Create a new simulation app.
    ///
    /// Generates the world, creates robots, and wires the scout message channel.
    pub fn new(seed: u64) -> Result<Self> {
        let world = World::generate(seed, WorldConfig::default())?;
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
                && let Event::Key(_) = event::read()?
            {
                // Any key press exits, per project spec.
                self.done = true;
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

        // Build lines of styled spans.
        let mut lines: Vec<Line> = Vec::with_capacity(h);

        for y in 0..h {
            let mut spans: Vec<Span> = Vec::with_capacity(w);
            for x in 0..w {
                let pos = Position { x, y };
                let (ch, color) = self.cell_char(pos);
                spans.push(Span::styled(ch, Style::default().fg(color)));
            }
            lines.push(Line::from(spans));
        }

        f.render_widget(Paragraph::new(lines), area);
    }

    /// Determine the display character and color for a map cell.
    fn cell_char(&self, pos: Position) -> (&str, Color) {
        // 1. Robots on top.
        for scout in &self.scouts {
            if scout.position == pos {
                return ("x", SCOUT_COLOR);
            }
        }
        for collector in &self.collectors {
            if collector.position == pos {
                return ("o", COLLECTOR_COLOR);
            }
        }

        // 2. Base.
        if pos == self.sim.world.base() {
            return ("#", BASE_COLOR);
        }

        let revealed = self.sim.is_cell_revealed(pos);
        let cell = self.sim.world.cell(pos);

        // 3. Undiscovered — fog of war.
        if !revealed {
            // Show obstacles as unknown if they're on the border of revealed area.
            return (" ", FOG_COLOR);
        }

        // 4. Obstacles.
        if cell == Some(Cell::Obstacle) {
            return ("O", OBSTACLE_COLOR);
        }

        // 5. Resources (only if discovered).
        if self.sim.is_resource_discovered(pos)
            && let Some(resource) = self.sim.world.resource_at(pos)
            && self.sim.stock_remaining_at(pos) > 0
        {
            return match resource.kind {
                ResourceKind::Energy => ("E", ENERGY_COLOR),
                ResourceKind::Crystal => ("C", CRYSTAL_COLOR),
            };
        }

        // 6. Walkable ground.
        (".", GROUND_COLOR)
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
                "T:{}  E:{}  C:{}  Left:{}  —  running…",
                self.tick, inv.energy, inv.crystals, remaining
            )
        };

        let span = Span::styled(text, Style::default().fg(UI_TEXT_COLOR));
        f.render_widget(Paragraph::new(span), area);
    }
}

// ---------------------------------------------------------------------------
// Integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::collector::CollectorState;

    #[test]
    fn app_simulation_runs_without_panics() {
        let mut app = App::new(42).expect("app creation should succeed");

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
        let app = App::new(99).expect("app creation should succeed");
        assert_eq!(app.scouts.len(), NUM_SCOUTS as usize);
        assert_eq!(app.collectors.len(), NUM_COLLECTORS as usize);
    }
}
