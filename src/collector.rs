use std::collections::VecDeque;

use crate::world::{Cell, Position, ResourceKind, World};

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

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BaseInventory {
    pub energy: u64,
    pub crystals: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct ResourceStock {
    pub kind: ResourceKind,
    pub position: Position,
    pub remaining: u16,
}

#[derive(Debug, Clone)]
pub struct CollectorWorld {
    width: usize,
    stocks: Vec<ResourceStock>,
    stock_at: Vec<Option<usize>>,
    base_inventory: BaseInventory,
}

impl CollectorWorld {
    pub fn from_world(world: &World) -> Self {
        let mut stock_at = vec![None; world.cell_count()];
        let mut stocks = Vec::with_capacity(world.resources().len());

        for node in world.resources() {
            let stock_idx = stocks.len();
            let flat = node.position.y * world.width() + node.position.x;
            stock_at[flat] = Some(stock_idx);
            stocks.push(ResourceStock {
                kind: node.kind,
                position: node.position,
                remaining: node.quantity,
            });
        }

        Self {
            width: world.width(),
            stocks,
            stock_at,
            base_inventory: BaseInventory::default(),
        }
    }

    pub fn base_inventory(&self) -> BaseInventory {
        self.base_inventory
    }

    pub fn total_remaining(&self) -> u64 {
        self.stocks.iter().map(|s| u64::from(s.remaining)).sum()
    }

    fn stock_index_at(&self, pos: Position) -> Option<usize> {
        let flat = pos.y * self.width + pos.x;
        self.stock_at.get(flat).and_then(|v| *v)
    }

    fn stock_remaining_at(&self, pos: Position) -> u16 {
        self.stock_index_at(pos)
            .and_then(|idx| self.stocks.get(idx).map(|s| s.remaining))
            .unwrap_or(0)
    }

    fn take_one_at(&mut self, pos: Position) -> Option<ResourceKind> {
        let idx = self.stock_index_at(pos)?;
        let stock = self.stocks.get_mut(idx)?;
        if stock.remaining == 0 {
            return None;
        }
        stock.remaining -= 1;
        Some(stock.kind)
    }

    fn unload(&mut self, cargo_kind: ResourceKind, amount: u16) {
        match cargo_kind {
            ResourceKind::Energy => self.base_inventory.energy += u64::from(amount),
            ResourceKind::Crystal => self.base_inventory.crystals += u64::from(amount),
        }
    }

    fn live_targets(&self) -> impl Iterator<Item = Position> + '_ {
        self.stocks
            .iter()
            .filter(|stock| stock.remaining > 0)
            .map(|stock| stock.position)
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
    // Reusable BFS scratch — allocated once, reset before each plan.
    // bfs_prev needs no fill() reset: stale values are never read because
    // we only follow prev[x] for cells marked visited in the current run.
    bfs_visited: Vec<bool>,
    bfs_prev: Vec<usize>,
}

impl Collector {
    pub fn new(id: u16, base: Position, config: CollectorConfig, map_size: usize) -> Self {
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
            bfs_visited: vec![false; map_size],
            bfs_prev: vec![0_usize; map_size],
        }
    }

    pub fn tick(&mut self, world: &World, cworld: &mut CollectorWorld) {
        self.stats.ticks += 1;

        match self.state {
            CollectorState::Seeking => self.tick_seeking(world, cworld),
            CollectorState::Collecting => self.tick_collecting(cworld),
            CollectorState::Returning => self.tick_returning(world),
            CollectorState::Unloading => self.tick_unloading(cworld),
            CollectorState::Idle => {}
        }
    }

    fn tick_seeking(&mut self, world: &World, cworld: &CollectorWorld) {
        if self.cargo > 0 {
            self.state = CollectorState::Returning;
            return;
        }

        let need_new_target = match self.target {
            Some(target) => cworld.stock_remaining_at(target) == 0,
            None => true,
        };

        if need_new_target {
            self.target = select_nearest_target(self.position, cworld.live_targets());
            self.path.clear();
            self.path_next = 0;
        }

        let Some(target) = self.target else {
            self.state = CollectorState::Idle;
            return;
        };

        if self.position == target {
            self.state = CollectorState::Collecting;
            return;
        }

        if self.path_next >= self.path.len() {
            if self.plan_path(world, self.position, target) {
                self.stats.replans += 1;
            } else {
                self.target = None;
                self.state = CollectorState::Idle;
                return;
            }
        }

        self.step_along_path();
    }

    fn tick_collecting(&mut self, cworld: &mut CollectorWorld) {
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

        if let Some(kind) = cworld.take_one_at(target) {
            if self.cargo == 0 {
                self.cargo_kind = Some(kind);
            }
            if self.cargo_kind == Some(kind) {
                self.cargo += 1;
                self.stats.collected_units += 1;
            }
        }

        if self.cargo >= self.config.carry_capacity || cworld.stock_remaining_at(target) == 0 {
            self.state = CollectorState::Returning;
            self.target = None;
            self.path.clear();
            self.path_next = 0;
        }
    }

    fn tick_returning(&mut self, world: &World) {
        if self.position == self.base {
            self.state = CollectorState::Unloading;
            return;
        }

        if self.path_next >= self.path.len() {
            if self.plan_path(world, self.position, self.base) {
                self.stats.replans += 1;
            } else {
                self.state = CollectorState::Idle;
                return;
            }
        }

        self.step_along_path();
    }

    fn tick_unloading(&mut self, cworld: &mut CollectorWorld) {
        if self.cargo == 0 {
            self.state = CollectorState::Seeking;
            return;
        }

        if let Some(kind) = self.cargo_kind {
            cworld.unload(kind, self.cargo);
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

    /// BFS pathfinding using reusable scratch buffers.
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

        // Reset visited flags only — prev needs no reset (explained on struct).
        self.bfs_visited.fill(false);

        let mut queue = VecDeque::with_capacity(64);
        self.bfs_visited[start_i] = true;
        queue.push_back(start_i);

        'bfs: while let Some(cur) = queue.pop_front() {
            if cur == goal_i {
                break 'bfs;
            }
            let x = cur % width;
            let y = cur / width;

            if y > 0 {
                let next = (y - 1) * width + x;
                if !self.bfs_visited[next]
                    && world.cell(Position { x, y: y - 1 }) == Some(Cell::Walkable)
                {
                    self.bfs_visited[next] = true;
                    self.bfs_prev[next] = cur;
                    queue.push_back(next);
                }
            }
            if x + 1 < width {
                let next = y * width + (x + 1);
                if !self.bfs_visited[next]
                    && world.cell(Position { x: x + 1, y }) == Some(Cell::Walkable)
                {
                    self.bfs_visited[next] = true;
                    self.bfs_prev[next] = cur;
                    queue.push_back(next);
                }
            }
            if y + 1 < height {
                let next = (y + 1) * width + x;
                if !self.bfs_visited[next]
                    && world.cell(Position { x, y: y + 1 }) == Some(Cell::Walkable)
                {
                    self.bfs_visited[next] = true;
                    self.bfs_prev[next] = cur;
                    queue.push_back(next);
                }
            }
            if x > 0 {
                let next = y * width + (x - 1);
                if !self.bfs_visited[next]
                    && world.cell(Position { x: x - 1, y }) == Some(Cell::Walkable)
                {
                    self.bfs_visited[next] = true;
                    self.bfs_prev[next] = cur;
                    queue.push_back(next);
                }
            }
        }

        if !self.bfs_visited[goal_i] {
            return false;
        }

        // Reconstruct path directly into self.path — no intermediate Vec.
        let mut cursor = goal_i;
        while cursor != start_i {
            self.path.push(Position {
                x: cursor % width,
                y: cursor / width,
            });
            cursor = self.bfs_prev[cursor];
        }
        self.path.reverse();
        true
    }
}

fn select_nearest_target(
    from: Position,
    targets: impl Iterator<Item = Position>,
) -> Option<Position> {
    targets.min_by_key(|target| {
        let dx = target.x.abs_diff(from.x);
        let dy = target.y.abs_diff(from.y);
        dx + dy
    })
}



#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::world::WorldConfig;

    #[test]
    fn collector_collects_and_unloads() {
        let world = World::generate(7, WorldConfig::default()).expect("world generation should succeed");
        let mut cworld = CollectorWorld::from_world(&world);
        let mut collector = Collector::new(0, world.base(), CollectorConfig::default(), world.cell_count());

        for _ in 0..2500 {
            collector.tick(&world, &mut cworld);
        }

        let inv = cworld.base_inventory();
        let unloaded = inv.energy + inv.crystals;
        assert!(unloaded > 0, "collector did not unload any resource");
        assert_eq!(collector.stats.unloaded_units, unloaded);
    }

    #[test]
    fn collector_never_steps_on_obstacle() {
        let world = World::generate(11, WorldConfig::default()).expect("world generation should succeed");
        let mut cworld = CollectorWorld::from_world(&world);
        let mut collector = Collector::new(0, world.base(), CollectorConfig::default(), world.cell_count());

        for _ in 0..1500 {
            collector.tick(&world, &mut cworld);
            let cell = world.cell(collector.position).expect("collector position should stay in-bounds");
            assert_eq!(cell, Cell::Walkable);
        }
    }

    #[test]
    fn collector_replans_on_events_not_every_tick() {
        let world = World::generate(9, WorldConfig::default()).expect("world generation should succeed");
        let mut cworld = CollectorWorld::from_world(&world);
        let mut collector = Collector::new(
            0,
            world.base(),
            CollectorConfig {
                carry_capacity: 8,
            },
            world.cell_count(),
        );
        let ticks = 2000_u64;

        for _ in 0..ticks {
            collector.tick(&world, &mut cworld);
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
        let mut cworld = CollectorWorld::from_world(&world);
        let num_collectors = 4_u16;
        let ticks = 1200_u64;

        let mut collectors: Vec<Collector> = (0..num_collectors)
            .map(|id| Collector::new(id, world.base(), CollectorConfig::default(), world.cell_count()))
            .collect();

        let mut samples_us = Vec::with_capacity(ticks as usize);
        for _ in 0..ticks {
            let start = std::time::Instant::now();
            for collector in &mut collectors {
                collector.tick(&world, &mut cworld);
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
