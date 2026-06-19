use std::cmp::Ordering;
use std::collections::BinaryHeap;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use tokio::sync::mpsc;

use crate::simulation::Simulation;
use crate::world::{Cell, Position, ResourceKind, World};

// ---------------------------------------------------------------------------
// Messages sent from collectors to the base
// ---------------------------------------------------------------------------

/// A message sent by a collector when it unloads resources at the base.
/// The base processes these to update the global inventory.
#[derive(Debug, Clone)]
pub struct CollectorMessage {
    pub collector_id: u16,
    pub kind: ResourceKind,
    pub amount: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectorState {
    Seeking,
    Collecting,
    Returning,
    Unloading,
    Idle,
}

#[derive(Debug, Clone, Copy)]
pub struct CollectorConfig {
    pub carry_capacity: u16,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self { carry_capacity: 16 }
    }
}

#[derive(Debug, Default, Clone)]
pub struct CollectorStats {
    pub ticks: u64,
    pub moves: u64,
    pub replans: u64,
    pub collected_units: u64,
    pub unloaded_units: u64,
}

pub struct Collector {
    pub id: u16,
    pub position: Position,
    pub state: CollectorState,
    config: CollectorConfig,
    base: Position,
    target: Option<Position>,
    cargo_kind: Option<ResourceKind>,
    cargo: u16,
    path: Vec<Position>,
    path_next: usize,
    pub stats: CollectorStats,
    // Channel sender to the base for unloading communication.
    tx: mpsc::Sender<CollectorMessage>,
    // Per-collector RNG so target selection is not deterministic across
    // collectors — prevents every collector clustering on the same resource.
    rng: StdRng,
    // Reusable A* scratch — allocated once, reset before each plan.
    // prev needs no fill() reset: stale values are never read because
    // we only follow prev[x] for cells marked visited in the current run.
    astar_visited: Vec<bool>,
    astar_prev: Vec<usize>,
    astar_g: Vec<usize>,
}

// ---------------------------------------------------------------------------
// A* pathfinding helpers
// ---------------------------------------------------------------------------

/// Node in the A* open-set priority queue.
#[derive(Copy, Clone, Eq, PartialEq)]
struct AStarNode {
    f_cost: usize,  // g (cost from start) + h (heuristic to goal)
    index: usize,   // flat cell index
}

impl Ord for AStarNode {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is max-heap; reverse for min-heap behaviour.
        other.f_cost.cmp(&self.f_cost)
    }
}

impl PartialOrd for AStarNode {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Manhattan distance heuristic (admissible for 4-directional grid movement).
fn manhattan(x1: usize, y1: usize, x2: usize, y2: usize) -> usize {
    x1.abs_diff(x2) + y1.abs_diff(y2)
}

// ---------------------------------------------------------------------------
// Collector
// ---------------------------------------------------------------------------

impl Collector {
    pub fn new(
        id: u16,
        base: Position,
        config: CollectorConfig,
        map_size: usize,
        tx: mpsc::Sender<CollectorMessage>,
    ) -> Self {
        Self {
            id,
            position: base,
            state: CollectorState::Seeking,
            config,
            base,
            target: None,
            cargo_kind: None,
            cargo: 0,
            path: Vec::new(),
            path_next: 0,
            stats: CollectorStats::default(),
            tx,
            rng: StdRng::seed_from_u64(u64::from(id).wrapping_mul(0x9E3779B97F4A7C15)),
            astar_visited: vec![false; map_size],
            astar_prev: vec![0_usize; map_size],
            astar_g: vec![0_usize; map_size],
        }
    }

    /// Advance the collector by one simulation tick.
    ///
    /// `sim` provides read access to the static world (via `sim.world`) and
    /// mutable access to resource stocks / base inventory.
    pub fn tick(&mut self, sim: &mut Simulation) {
        self.stats.ticks += 1;

        match self.state {
            CollectorState::Seeking => self.tick_seeking(sim),
            CollectorState::Collecting => self.tick_collecting(sim),
            CollectorState::Returning => self.tick_returning(sim),
            CollectorState::Unloading => self.tick_unloading(sim),
            CollectorState::Idle => {
                // Scouts may have discovered new resources — wake up if
                // there are live targets available now.
                if self.pick_target(sim).is_some() {
                    self.state = CollectorState::Seeking;
                }
            }
        }
    }

    fn tick_seeking(&mut self, sim: &mut Simulation) {
        if self.cargo > 0 {
            self.state = CollectorState::Returning;
            return;
        }

        let need_new_target = match self.target {
            Some(target) => sim.stock_remaining_at(target) == 0,
            None => true,
        };

        if need_new_target {
            self.target = self.pick_target(sim);
            self.path.clear();
            self.path_next = 0;
        }

        let Some(target) = self.target else {
            // No live targets left anywhere — truly done.
            self.state = CollectorState::Idle;
            return;
        };

        if self.position == target {
            self.state = CollectorState::Collecting;
            return;
        }

        if self.path_next >= self.path.len() {
            if self.plan_path(&sim.world, self.position, target) {
                self.stats.replans += 1;
            } else {
                // Target unreachable — try a different one next tick.
                self.target = None;
                self.path.clear();
                self.path_next = 0;
                return;
            }
        }

        self.step_along_path();
    }

    fn tick_collecting(&mut self, sim: &mut Simulation) {
        if self.cargo >= self.config.carry_capacity {
            self.state = CollectorState::Returning;
            self.target = None;
            self.path.clear();
            self.path_next = 0;
            return;
        }

        let Some(target) = self.target else {
            self.state = CollectorState::Seeking;
            return;
        };

        if self.position != target {
            self.state = CollectorState::Seeking;
            return;
        }

        if let Some(kind) = sim.try_collect(target) {
            if self.cargo == 0 {
                self.cargo_kind = Some(kind);
            }
            if self.cargo_kind == Some(kind) {
                self.cargo += 1;
                self.stats.collected_units += 1;
            }
        }

        if self.cargo >= self.config.carry_capacity || sim.stock_remaining_at(target) == 0 {
            self.state = CollectorState::Returning;
            self.target = None;
            self.path.clear();
            self.path_next = 0;
        }
    }

    fn tick_returning(&mut self, sim: &mut Simulation) {
        if self.position == self.base {
            self.state = CollectorState::Unloading;
            return;
        }

        if self.path_next >= self.path.len() {
            if self.plan_path(&sim.world, self.position, self.base) {
                self.stats.replans += 1;
            } else {
                self.state = CollectorState::Idle;
                return;
            }
        }

        self.step_along_path();
    }

    fn tick_unloading(&mut self, _sim: &mut Simulation) {
        if self.cargo == 0 {
            self.state = CollectorState::Seeking;
            return;
        }

        if let Some(kind) = self.cargo_kind {
            // Send unload message to base — asynchronous, non-blocking.
            let msg = CollectorMessage {
                collector_id: self.id,
                kind,
                amount: self.cargo,
            };
            let _ = self.tx.try_send(msg);
            self.stats.unloaded_units += u64::from(self.cargo);
        }

        self.cargo = 0;
        self.cargo_kind = None;
        self.state = CollectorState::Seeking;
        self.path.clear();
        self.path_next = 0;
    }

    fn step_along_path(&mut self) {
        if self.path_next < self.path.len() {
            self.position = self.path[self.path_next];
            self.path_next += 1;
            self.stats.moves += 1;
        }
    }

    /// A* pathfinding using reusable scratch buffers.
    /// Writes the path directly into `self.path`; resets `self.path_next` to 0.
    /// Returns true if the goal is reachable. On failure `self.path` is empty.
    fn plan_path(&mut self, world: &World, start: Position, goal: Position) -> bool {
        self.path.clear();
        self.path_next = 0;

        if start == goal {
            return true;
        }

        let width = world.width();
        let height = world.height();
        let start_i = start.y * width + start.x;
        let goal_i = goal.y * width + goal.x;
        let goal_x = goal.x;
        let goal_y = goal.y;

        // Reset visited + g-cost for this search.
        self.astar_visited.fill(false);

        let mut open = BinaryHeap::with_capacity(64);
        self.astar_g[start_i] = 0;
        self.astar_visited[start_i] = true;
        open.push(AStarNode {
            f_cost: manhattan(start.x, start.y, goal_x, goal_y),
            index: start_i,
        });

        while let Some(AStarNode { index: cur, .. }) = open.pop() {
            if cur == goal_i {
                // Reconstruct path.
                let mut cursor = goal_i;
                while cursor != start_i {
                    self.path.push(Position {
                        x: cursor % width,
                        y: cursor / width,
                    });
                    cursor = self.astar_prev[cursor];
                }
                self.path.reverse();
                return true;
            }

            let x = cur % width;
            let y = cur / width;
            let next_g = self.astar_g[cur] + 1;

            if y > 0 {
                let next = (y - 1) * width + x;
                if !self.astar_visited[next]
                    && world.cell(Position { x, y: y - 1 }) == Some(Cell::Walkable)
                {
                    self.astar_visited[next] = true;
                    self.astar_prev[next] = cur;
                    self.astar_g[next] = next_g;
                    open.push(AStarNode {
                        f_cost: next_g + manhattan(x, y - 1, goal_x, goal_y),
                        index: next,
                    });
                }
            }
            if x + 1 < width {
                let next = y * width + (x + 1);
                if !self.astar_visited[next]
                    && world.cell(Position { x: x + 1, y }) == Some(Cell::Walkable)
                {
                    self.astar_visited[next] = true;
                    self.astar_prev[next] = cur;
                    self.astar_g[next] = next_g;
                    open.push(AStarNode {
                        f_cost: next_g + manhattan(x + 1, y, goal_x, goal_y),
                        index: next,
                    });
                }
            }
            if y + 1 < height {
                let next = (y + 1) * width + x;
                if !self.astar_visited[next]
                    && world.cell(Position { x, y: y + 1 }) == Some(Cell::Walkable)
                {
                    self.astar_visited[next] = true;
                    self.astar_prev[next] = cur;
                    self.astar_g[next] = next_g;
                    open.push(AStarNode {
                        f_cost: next_g + manhattan(x, y + 1, goal_x, goal_y),
                        index: next,
                    });
                }
            }
            if x > 0 {
                let next = y * width + (x - 1);
                if !self.astar_visited[next]
                    && world.cell(Position { x: x - 1, y }) == Some(Cell::Walkable)
                {
                    self.astar_visited[next] = true;
                    self.astar_prev[next] = cur;
                    self.astar_g[next] = next_g;
                    open.push(AStarNode {
                        f_cost: next_g + manhattan(x - 1, y, goal_x, goal_y),
                        index: next,
                    });
                }
            }
        }

        false // no path
    }

    /// Pick a resource target using Manhattan-distance-biased randomness.
    ///
    /// Sorts live targets by distance, then picks uniformly among the nearest
    /// few.  This prevents every collector from swarming the single closest
    /// resource while still preferring nearby targets.
    fn pick_target(&mut self, sim: &Simulation) -> Option<Position> {
        let mut targets: Vec<Position> = sim.live_targets().collect();
        if targets.is_empty() {
            return None;
        }

        // Sort by Manhattan distance from current position.
        let px = self.position.x;
        let py = self.position.y;
        targets.sort_by_key(|t| t.x.abs_diff(px) + t.y.abs_diff(py));

        // Choose randomly among the 3 closest (or fewer if there aren't 3).
        let top_n = 3.min(targets.len());
        let idx = self.rng.random_range(0..top_n);
        Some(targets[idx])
    }
}



#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::simulation::Simulation;
    use crate::world::WorldConfig;

    #[test]
    fn collector_collects_and_unloads() {
        let world = World::generate(7, WorldConfig::default()).expect("world generation should succeed");
        let base = world.base();
        let cell_count = world.cell_count();
        let mut sim = Simulation::new(world);
        sim.reveal_all();
        let (tx, mut rx) = mpsc::channel(16);
        let mut collector = Collector::new(0, base, CollectorConfig::default(), cell_count, tx);

        for _ in 0..2500 {
            collector.tick(&mut sim);
            while let Ok(msg) = rx.try_recv() {
                sim.unload(msg.kind, msg.amount);
            }
        }

        let inv = sim.base_inventory;
        let unloaded = inv.energy + inv.crystals;
        assert!(unloaded > 0, "collector did not unload any resource");
        assert_eq!(collector.stats.unloaded_units, unloaded);
    }

    #[test]
    fn collector_never_steps_on_obstacle() {
        let world = World::generate(11, WorldConfig::default()).expect("world generation should succeed");
        let base = world.base();
        let cell_count = world.cell_count();
        let mut sim = Simulation::new(world);
        sim.reveal_all();
        let (tx, mut rx) = mpsc::channel(16);
        let mut collector = Collector::new(0, base, CollectorConfig::default(), cell_count, tx);

        for _ in 0..1500 {
            collector.tick(&mut sim);
            while let Ok(msg) = rx.try_recv() {
                sim.unload(msg.kind, msg.amount);
            }
            let cell = sim
                .world
                .cell(collector.position)
                .expect("collector position should stay in-bounds");
            assert_eq!(cell, Cell::Walkable);
        }
    }

    #[test]
    fn collector_replans_on_events_not_every_tick() {
        let world = World::generate(9, WorldConfig::default()).expect("world generation should succeed");
        let base = world.base();
        let cell_count = world.cell_count();
        let mut sim = Simulation::new(world);
        sim.reveal_all();
        let (tx, mut rx) = mpsc::channel(16);
        let mut collector = Collector::new(
            0,
            base,
            CollectorConfig {
                carry_capacity: 8,
            },
            cell_count,
            tx,
        );
        let ticks = 2000_u64;

        for _ in 0..ticks {
            collector.tick(&mut sim);
            while let Ok(msg) = rx.try_recv() {
                sim.unload(msg.kind, msg.amount);
            }
        }

        assert!(
            collector.stats.replans < ticks / 6,
            "replans={} ticks={ticks}",
            collector.stats.replans
        );
    }

    #[test]
    fn collector_tick_timing_metrics() {
        let world = World::generate(42, WorldConfig::default()).expect("world generation should succeed");
        let base = world.base();
        let cell_count = world.cell_count();
        let mut sim = Simulation::new(world);
        sim.reveal_all();
        let (tx, mut rx) = mpsc::channel(64);
        let num_collectors = 4_u16;
        let ticks = 1200_u64;

        let mut collectors: Vec<Collector> = (0..num_collectors)
            .map(|id| Collector::new(id, base, CollectorConfig::default(), cell_count, tx.clone()))
            .collect();
        drop(tx);

        let mut samples_us = Vec::with_capacity(ticks as usize);
        for _ in 0..ticks {
            let start = std::time::Instant::now();
            for collector in &mut collectors {
                collector.tick(&mut sim);
            }
            while let Ok(msg) = rx.try_recv() {
                sim.unload(msg.kind, msg.amount);
            }
            samples_us.push(start.elapsed().as_nanos() as f64 / 1000.0);
        }

        samples_us.sort_by(f64::total_cmp);
        let mean = samples_us.iter().sum::<f64>() / samples_us.len() as f64;
        let p95_idx = ((samples_us.len() as f64) * 0.95).ceil() as usize - 1;
        let p95 = samples_us[p95_idx.min(samples_us.len() - 1)];

        println!(
            "collector_tick_us mean={mean:.2} p95={p95:.2} collectors={num_collectors} ticks={ticks}"
        );

        let total_unloaded: u64 = collectors.iter().map(|c| c.stats.unloaded_units).sum();
        assert!(total_unloaded > 0, "collectors unloaded no resources");
    }
}
