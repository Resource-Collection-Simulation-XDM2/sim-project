use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::Terminal;
use tokio::sync::{mpsc, Barrier, RwLock};

use crate::occupancy::OccupancyGrid;
use crate::collector::{Collector, CollectorConfig, CollectorMessage};
use crate::concurrency::{self, SharedOcc, SharedSim};
use crate::map::{MapPreset, VisualTheme};
use crate::scout::{Discovery, Scout, ScoutConfig, ScoutMessage};
use crate::simulation::{RevealMode, Simulation};
use crate::ui;
use crate::world::{Position, World};

const NUM_SCOUTS: u16 = 4;
const NUM_COLLECTORS: u16 = 3;
const CHANNEL_CAPACITY: usize = 128;
const FRAME_DURATION: Duration = Duration::from_millis(33);

/// Read-only simulation metrics for tests and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimMetrics {
    pub tick: u64,
    pub energy: u64,
    pub crystals: u64,
    pub remaining: u64,
    pub initial_total: u64,
    pub revealed_cells: usize,
    pub discovered_resources: usize,
}

impl SimMetrics {
    pub fn collected(&self) -> u64 {
        self.energy + self.crystals
    }

    pub fn is_complete(&self) -> bool {
        self.remaining == 0
    }
}

/// Which type of robot occupies a cell (for O(1) rendering lookup).
#[derive(Clone, Copy)]
pub(crate) enum RobotKind {
    Scout,
    Collector,
}

#[derive(Clone, Copy)]
pub(crate) struct AppSnapshot<'a> {
    pub(crate) sim_world: &'a World,
    pub(crate) visual_theme: &'a VisualTheme,
    pub(crate) robot_at: &'a [Option<RobotKind>],
    pub(crate) revealed_cache: &'a [bool],
    pub(crate) cached_energy: u64,
    pub(crate) cached_crystals: u64,
    pub(crate) cached_remaining: u64,
    pub(crate) stock_at_cache: &'a [Option<usize>],
    pub(crate) stock_remaining_cache: &'a [u16],
    pub(crate) stock_initial_cache: &'a [u16],
    pub(crate) resource_discovered_cache: &'a [bool],
    pub(crate) cached_initial_total: u64,
    pub(crate) tick: u64,
    pub(crate) num_scouts: u16,
    pub(crate) num_collectors: u16,
}

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

    pub fn world(&self) -> &World {
        &self.sim_world
    }

    pub fn scout_positions(&self) -> &[Position] {
        &self.scout_positions
    }

    pub fn collector_positions(&self) -> &[Position] {
        &self.collector_positions
    }

    pub fn metrics(&self) -> SimMetrics {
        SimMetrics {
            tick: self.tick,
            energy: self.cached_energy,
            crystals: self.cached_crystals,
            remaining: self.cached_remaining,
            initial_total: self.cached_initial_total,
            revealed_cells: self.revealed_cache.iter().filter(|&&r| r).count(),
            discovered_resources: self.resource_discovered_cache.iter().filter(|&&d| d).count(),
        }
    }

    /// Advance one simulation tick without terminal rendering or input.
    pub async fn step(&mut self) {
        {
            let mut occ = self.occupancy.write().await;
            occ.clear();
            for pos in self.scout_positions.iter().chain(&self.collector_positions) {
                occ.try_reserve(*pos);
            }
        }

        self.barrier.wait().await;

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
                        Discovery::Resource { position, .. } => {
                            sim_guard.mark_resource_discovered(*position)
                        }
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

            self.revealed_cache.copy_from_slice(sim_guard.revealed_bitmap());
            self.cached_energy = sim_guard.base_inventory.energy;
            self.cached_crystals = sim_guard.base_inventory.crystals;
            self.cached_remaining = sim_guard.total_remaining();
            self.stock_at_cache.copy_from_slice(&sim_guard.stock_at);
            for (i, stock) in sim_guard.stocks.iter().enumerate() {
                self.stock_remaining_cache[i] = stock.remaining;
                self.stock_initial_cache[i] = stock.initial;
                self.resource_discovered_cache[i] = sim_guard.resource_discovered[i];
            }
        }

        self.tick += 1;
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
            self.step().await;

            let snapshot = self.snapshot();
            terminal.draw(|f| ui::render(f, &snapshot))?;

            if event::poll(FRAME_DURATION)?
                && is_quit_key(&event::read()?)
            {
                self.done = true;
            }

            {
                if self.cached_remaining == 0 {
                    let snapshot = self.snapshot();
                    terminal.draw(|f| ui::render(f, &snapshot))?;
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

    #[inline]
    fn set_robot_at(&mut self, pos: Position, kind: Option<RobotKind>) {
        let idx = pos.y * self.sim_world.width() + pos.x;
        if idx < self.robot_at.len() {
            self.robot_at[idx] = kind;
        }
    }

    fn snapshot(&self) -> AppSnapshot<'_> {
        AppSnapshot {
            sim_world: &self.sim_world,
            visual_theme: &self.visual_theme,
            robot_at: &self.robot_at,
            revealed_cache: &self.revealed_cache,
            cached_energy: self.cached_energy,
            cached_crystals: self.cached_crystals,
            cached_remaining: self.cached_remaining,
            stock_at_cache: &self.stock_at_cache,
            stock_remaining_cache: &self.stock_remaining_cache,
            stock_initial_cache: &self.stock_initial_cache,
            resource_discovered_cache: &self.resource_discovered_cache,
            cached_initial_total: self.cached_initial_total,
            tick: self.tick,
            num_scouts: NUM_SCOUTS,
            num_collectors: NUM_COLLECTORS,
        }
    }
}

fn is_quit_key(event: &Event) -> bool {
    matches!(
        event,
        Event::Key(key) if key.code == KeyCode::Char('q') || key.code == KeyCode::Char('Q')
    )
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
            app.step().await;
        }
        assert!(app.metrics().tick > 0);
    }

    #[tokio::test]
    async fn app_creates_expected_number_of_robots() {
        let app = App::new(99, MapPreset::Default, VisualTheme::DEFAULT, RevealMode::Normal).await
            .expect("app create");
        assert_eq!(app.scout_positions().len(), NUM_SCOUTS as usize);
        assert_eq!(app.collector_positions().len(), NUM_COLLECTORS as usize);
    }
}
