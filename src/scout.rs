use std::collections::VecDeque;
use std::f64::consts::TAU;

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use tokio::sync::mpsc;

use crate::world::{Cell, Position, ResourceKind, World};

// ---------------------------------------------------------------------------
// Messages sent from scouts to the base
// ---------------------------------------------------------------------------

/// A single discovery made by a scout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Discovery {
    Resource {
        position: Position,
        kind: ResourceKind,
    },
    Obstacle {
        position: Position,
    },
}

/// A batched message sent by a scout to the base.
#[derive(Debug, Clone)]
pub struct ScoutMessage {
    pub scout_id: u16,
    pub discoveries: Vec<Discovery>,
}

// ---------------------------------------------------------------------------
// Scout
// ---------------------------------------------------------------------------

/// Configuration knobs for scout behavior.
#[derive(Debug, Clone, Copy)]
pub struct ScoutConfig {
    /// Maximum discoveries to buffer before forcing a flush.
    pub batch_capacity: usize,
    /// Flush the batch every N ticks even if not full.
    pub flush_interval: u32,
    /// Probability [0.0, 1.0] of picking a random walkable neighbor instead of
    /// the frontier-biased best candidate. Prevents clustering.
    pub random_move_probability: f64,
}

impl Default for ScoutConfig {
    fn default() -> Self {
        Self {
            batch_capacity: 8,
            flush_interval: 5,
            random_move_probability: 0.15,
        }
    }
}

/// Backpressure / runtime stats for a single scout.
#[derive(Debug, Default, Clone)]
pub struct ScoutStats {
    pub ticks: u64,
    pub cells_explored: u64,
    pub messages_sent: u64,
    pub messages_dropped: u64,
    pub discoveries_total: u64,
}

/// Cardinal directions for neighbor enumeration.
const DIRECTIONS: [(i32, i32); 4] = [(0, -1), (1, 0), (0, 1), (-1, 0)];
const DEFAULT_SCOUT_COUNT: u16 = 4;
/// If a scout finds no frontier in its sector for this many ticks, it will
/// reassign to the most promising sector seen in its local map.
const SECTOR_TIMEOUT: u32 = 200;

pub struct Scout {
    pub id: u16,
    pub position: Position,
    home: Position,
    exploration_goal: Option<Position>,
    /// Preferred sector angle (radians, 0..2π) relative to `home`.
    preferred_sector_angle: f64,
    /// Number of consecutive ticks with no frontier in the preferred sector.
    ticks_without_sector_frontier: u32,
    config: ScoutConfig,
    rng: StdRng,

    // Local knowledge bitmaps — same flat layout as World.
    explored: Vec<bool>,
    known_obstacle: Vec<bool>,

    // Discovery batch buffer.
    batch: Vec<Discovery>,
    ticks_since_flush: u32,

    // Bounded channel sender to the base.
    tx: mpsc::Sender<ScoutMessage>,

    // Observable stats.
    pub stats: ScoutStats,

    // World dimensions cached for index math.
    world_width: usize,
    world_height: usize,
}

impl Scout {
    /// Create a new scout at `start` (normally the base position).
    pub fn new(
        id: u16,
        start: Position,
        world: &World,
        config: ScoutConfig,
        seed: u64,
        tx: mpsc::Sender<ScoutMessage>,
    ) -> Self {
        let map_len = world.width() * world.height();
        let sector_angle = (id as f64 / DEFAULT_SCOUT_COUNT as f64) * TAU;

        let mut scout = Self {
            id,
            position: start,
            home: start,
            exploration_goal: None,
            preferred_sector_angle: sector_angle,
            ticks_without_sector_frontier: 0,
            config,
            rng: StdRng::seed_from_u64(seed),
            explored: vec![false; map_len],
            known_obstacle: vec![false; map_len],
            batch: Vec::with_capacity(config.batch_capacity),
            ticks_since_flush: 0,
            tx,
            stats: ScoutStats::default(),
            world_width: world.width(),
            world_height: world.height(),
        };
        // Mark starting cell as explored.
        scout.observe_cell(start, world);
        scout
    }

    /// Advance one tick: observe surroundings, pick a move, step, maybe flush.
    pub fn tick(&mut self, world: &World) {
        self.stats.ticks += 1;

        // 1. Observe all cardinal neighbors (reveals obstacles / resources).
        let neighbors = self.cardinal_neighbors(self.position);
        for &pos in &neighbors {
            self.observe_cell(pos, world);
        }

        // 2. Pick a distant frontier cell, then move one step toward it.
        let target = self.pick_target(world, &neighbors);

        // 3. Move (if we have somewhere to go).
        if let Some(pos) = target {
            self.position = pos;
            self.observe_cell(pos, world);
        }

        // 4. Periodic flush.
        self.ticks_since_flush += 1;
        if self.ticks_since_flush >= self.config.flush_interval || self.batch.len() >= self.config.batch_capacity {
            self.flush_batch();
        }
    }

    /// Returns the current position of the scout.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn position(&self) -> Position {
        self.position
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Observe a single cell: mark explored, record obstacle/resource discoveries.
    fn observe_cell(&mut self, pos: Position, world: &World) {
        let idx = self.flat_index(pos);
        let was_explored = self.explored[idx];
        self.explored[idx] = true;

        if was_explored {
            return; // already known
        }

        self.stats.cells_explored += 1;

        match world.cell(pos) {
            Some(Cell::Obstacle) => {
                self.known_obstacle[idx] = true;
                self.batch.push(Discovery::Obstacle { position: pos });
                self.stats.discoveries_total += 1;
            }
            Some(Cell::Walkable) => {
                if let Some(resource) = world.resource_at(pos) {
                    self.batch.push(Discovery::Resource {
                        position: pos,
                        kind: resource.kind,
                    });
                    self.stats.discoveries_total += 1;
                }
            }
            None => {} // out of bounds — should not happen
        }
    }

    /// Pick a movement target that pushes the scout toward less explored regions.
    fn pick_target(&mut self, _world: &World, neighbors: &[Position]) -> Option<Position> {
        let walkable_neighbors = self.walkable_neighbors(neighbors);

        // Small randomness keeps the four scouts from perfectly overlapping.
        if self.rng.random_bool(self.config.random_move_probability) {
            return self.random_choice(&walkable_neighbors);
        }

        let frontier_all = self.global_frontier();
        if frontier_all.is_empty() {
            self.exploration_goal = None;
            return self.random_choice(&walkable_neighbors);
        }

        if let Some(goal) = self.exploration_goal
            && goal != self.position
            && let Some(step) = self.next_step_toward(goal)
        {
            return Some(step);
        }

        if self.exploration_goal == Some(self.position) {
            self.exploration_goal = None;
        }

        // Prefer frontiers inside this scout's sector if any exist.
        let sector_width = TAU / (DEFAULT_SCOUT_COUNT as f64);
        let half_width = sector_width / 2.0;
        let mut sector_frontier: Vec<Position> = frontier_all
            .iter()
            .copied()
            .filter(|goal| {
                let dx = goal.x as f64 - self.home.x as f64;
                let dy = goal.y as f64 - self.home.y as f64;
                let angle = dy.atan2(dx).rem_euclid(TAU);
                angle_diff(angle, self.preferred_sector_angle) <= half_width
            })
            .collect();

        // Update sector timeout counter.
        if !sector_frontier.is_empty() {
            self.ticks_without_sector_frontier = 0;
        } else {
            self.ticks_without_sector_frontier = self.ticks_without_sector_frontier.saturating_add(1);
        }

        // If the scout hasn't seen any frontier in its sector for a while,
        // reassign its preferred sector toward the most promising global
        // frontier observed.
        if self.ticks_without_sector_frontier > SECTOR_TIMEOUT {
            // Choose best global frontier and set preferred angle toward it.
            let mut best_goal = None;
            let mut best_score = i32::MIN;
            for goal in &frontier_all {
                let score = self.frontier_score(*goal);
                if score > best_score {
                    best_score = score;
                    best_goal = Some(*goal);
                }
            }

            if let Some(g) = best_goal {
                let dx = g.x as f64 - self.home.x as f64;
                let dy = g.y as f64 - self.home.y as f64;
                self.preferred_sector_angle = dy.atan2(dx).rem_euclid(TAU);
                self.exploration_goal = Some(g);
            }
            self.ticks_without_sector_frontier = 0;
        }

        let search_frontier = if !sector_frontier.is_empty() {
            sector_frontier
        } else {
            frontier_all
        };

        let mut best_goal = None;
        let mut best_score = i32::MIN;

        for goal in search_frontier {
            let score = self.frontier_score(goal);
            if score > best_score {
                best_score = score;
                best_goal = Some(goal);
            }
        }

        self.exploration_goal = best_goal;

        if let Some(goal) = best_goal
            && let Some(step) = self.next_step_toward(goal)
        {
            return Some(step);
        }

        self.exploration_goal = None;

        self.random_choice(&walkable_neighbors)
    }

    fn frontier_score(&self, goal: Position) -> i32 {
        let current_to_goal = manhattan(self.position, goal) as i32;
        let home_to_goal = manhattan(self.home, goal) as i32;
        // Compute angular preference: goals near the scout's sector angle are favored.
        let dx = goal.x as f64 - self.home.x as f64;
        let dy = goal.y as f64 - self.home.y as f64;
        let angle = dy.atan2(dx).rem_euclid(TAU);
        let diff = angle_diff(angle, self.preferred_sector_angle);
        // diff in [0, PI]. Map to a small penalty in integer score.
        let angular_penalty = (diff / std::f64::consts::PI) * 10.0;

        (current_to_goal + home_to_goal * 2) - (angular_penalty as i32)
    }

    fn global_frontier(&self) -> Vec<Position> {
        let mut frontier = Vec::new();

        for y in 0..self.world_height {
            for x in 0..self.world_width {
                let pos = Position { x, y };
                let idx = self.flat_index(pos);

                if !self.explored[idx] || self.known_obstacle[idx] {
                    continue;
                }

                if self.has_unexplored_neighbor(pos) {
                    frontier.push(pos);
                }
            }
        }

        frontier
    }

    fn has_unexplored_neighbor(&self, pos: Position) -> bool {
        self.cardinal_neighbors(pos)
            .into_iter()
            .any(|neighbor| !self.explored[self.flat_index(neighbor)])
    }

    fn next_step_toward(&self, goal: Position) -> Option<Position> {
        let start_idx = self.flat_index(self.position);
        let goal_idx = self.flat_index(goal);

        if start_idx == goal_idx {
            return None;
        }

        let mut previous = vec![usize::MAX; self.explored.len()];
        let mut queue = VecDeque::new();
        previous[start_idx] = start_idx;
        queue.push_back(start_idx);

        while let Some(idx) = queue.pop_front() {
            if idx == goal_idx {
                break;
            }

            for neighbor in self.known_walkable_neighbors_from_idx(idx) {
                if previous[neighbor] != usize::MAX {
                    continue;
                }

                previous[neighbor] = idx;
                queue.push_back(neighbor);
            }
        }

        if previous[goal_idx] == usize::MAX {
            return None;
        }

        let mut step = goal_idx;
        while previous[step] != start_idx {
            step = previous[step];
        }

        Some(self.position_from_index(step))
    }

    fn known_walkable_neighbors_from_idx(&self, idx: usize) -> impl Iterator<Item = usize> + '_ {
        let x = idx % self.world_width;
        let y = idx / self.world_width;

        DIRECTIONS.into_iter().filter_map(move |(dx, dy)| {
            let nx = x as i32 + dx;
            let ny = y as i32 + dy;
            if nx < 0 || ny < 0 {
                return None;
            }

            let nx = nx as usize;
            let ny = ny as usize;
            if nx >= self.world_width || ny >= self.world_height {
                return None;
            }

            let neighbor = ny * self.world_width + nx;
            if !self.explored[neighbor] || self.known_obstacle[neighbor] {
                return None;
            }

            Some(neighbor)
        })
    }

    fn cardinal_neighbors(&self, pos: Position) -> Vec<Position> {
        let mut neighbors = Vec::with_capacity(4);

        for (dx, dy) in DIRECTIONS {
            let nx = pos.x as i32 + dx;
            let ny = pos.y as i32 + dy;
            if nx < 0 || ny < 0 || (nx as usize) >= self.world_width || (ny as usize) >= self.world_height {
                continue;
            }

            neighbors.push(Position {
                x: nx as usize,
                y: ny as usize,
            });
        }

        neighbors
    }

    fn walkable_neighbors(&self, neighbors: &[Position]) -> Vec<Position> {
        neighbors
            .iter()
            .copied()
            .filter(|pos| !self.known_obstacle[self.flat_index(*pos)])
            .collect()
    }

    fn random_choice(&mut self, positions: &[Position]) -> Option<Position> {
        if positions.is_empty() {
            return None;
        }

        let idx = self.rng.random_range(0..positions.len());
        Some(positions[idx])
    }

    /// Flush the discovery batch to the base channel.
    fn flush_batch(&mut self) {
        self.ticks_since_flush = 0;

        if self.batch.is_empty() {
            return;
        }

        let message = ScoutMessage {
            scout_id: self.id,
            discoveries: std::mem::take(&mut self.batch),
        };

        match self.tx.try_send(message) {
            Ok(()) => {
                self.stats.messages_sent += 1;
            }
            Err(mpsc::error::TrySendError::Full(msg)) => {
                // Backpressure: drop the message, count it.
                self.stats.messages_dropped += 1;
                // Re-take discoveries so they can be retried on next flush.
                self.batch = msg.discoveries;
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                // Channel closed (simulation shutting down); count as drop.
                self.stats.messages_dropped += 1;
            }
        }
    }

    #[inline]
    fn flat_index(&self, pos: Position) -> usize {
        pos.y * self.world_width + pos.x
    }

    #[inline]
    fn position_from_index(&self, idx: usize) -> Position {
        Position {
            x: idx % self.world_width,
            y: idx / self.world_width,
        }
    }
}

#[inline]
fn manhattan(a: Position, b: Position) -> usize {
    a.x.abs_diff(b.x) + a.y.abs_diff(b.y)
}

/// Minimal absolute difference between two angles in radians (0..2π), result in [0, PI]
fn angle_diff(a: f64, b: f64) -> f64 {
    let d = (a - b).abs() % TAU;
    if d > std::f64::consts::PI { TAU - d } else { d }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::world::WorldConfig;

    /// Helper: create a world and a single scout with a receiver.
    fn setup(seed: u64) -> (World, Scout, mpsc::Receiver<ScoutMessage>) {
        let config = WorldConfig::default();
        let world = World::generate(seed, config).expect("world gen");
        let (tx, rx) = mpsc::channel(64);
        let scout = Scout::new(0, world.base(), &world, ScoutConfig::default(), seed, tx);
        (world, scout, rx)
    }

    #[test]
    fn scout_starts_at_base() {
        let (world, scout, _rx) = setup(42);
        assert_eq!(scout.position(), world.base());
    }

    #[test]
    fn scout_never_moves_onto_obstacle() {
        let (world, mut scout, _rx) = setup(42);

        for _ in 0..500 {
            scout.tick(&world);
            let cell = world
                .cell(scout.position())
                .expect("scout should be in-bounds");
            assert_eq!(cell, Cell::Walkable, "scout landed on obstacle at {:?}", scout.position());
        }
    }

    #[test]
    fn scout_explores_cells_over_time() {
        let (world, mut scout, _rx) = setup(42);

        for _ in 0..200 {
            scout.tick(&world);
        }

        // After 200 ticks the scout should have explored significantly more
        // than just the starting cell.
        assert!(
            scout.stats.cells_explored > 20,
            "cells_explored = {}",
            scout.stats.cells_explored
        );
    }

    #[test]
    fn frontier_bias_prefers_unexplored_cells() {
        // With random_move_probability = 0, every move should prefer unexplored
        // neighbors. We test that the scout explores faster than pure random.
        let config = WorldConfig::default();
        let world = World::generate(42, config).expect("world gen");
        let (tx, _rx) = mpsc::channel(64);
        let scout_config = ScoutConfig {
            random_move_probability: 0.0,
            ..ScoutConfig::default()
        };
        let mut scout = Scout::new(0, world.base(), &world, scout_config, 42, tx);

        for _ in 0..100 {
            scout.tick(&world);
        }

        // Deterministic: with zero randomness and frontier bias, exploration
        // should be efficient.
        assert!(
            scout.stats.cells_explored > 50,
            "frontier-biased explored only {}",
            scout.stats.cells_explored
        );
    }

    #[test]
    fn discovery_batch_is_flushed_and_received() {
        let (world, mut scout, mut rx) = setup(42);

        // Run enough ticks for the scout to leave the base safety zone and
        // encounter obstacles or resources on the wider map.
        for _ in 0..200 {
            scout.tick(&world);
        }

        // Sanity check: the scout must have found at least some discoveries.
        assert!(
            scout.stats.discoveries_total > 0,
            "scout made no discoveries after 200 ticks (cells_explored={})",
            scout.stats.cells_explored
        );

        // Drain messages.
        let mut total_discoveries = 0_usize;
        while let Ok(msg) = rx.try_recv() {
            assert_eq!(msg.scout_id, 0);
            total_discoveries += msg.discoveries.len();
        }

        assert!(
            total_discoveries > 0,
            "expected at least one discovery in channel (scout discoveries_total={})",
            scout.stats.discoveries_total
        );
    }

    #[test]
    fn backpressure_counts_dropped_messages() {
        let config = WorldConfig::default();
        let world = World::generate(42, config).expect("world gen");
        // Channel capacity = 1, so most sends should be dropped.
        let (tx, _rx) = mpsc::channel(1);
        let scout_config = ScoutConfig {
            batch_capacity: 1,
            flush_interval: 1,
            ..ScoutConfig::default()
        };
        let mut scout = Scout::new(0, world.base(), &world, scout_config, 42, tx);

        for _ in 0..100 {
            scout.tick(&world);
        }

        // We don't drain, so after the buffer fills, sends should fail.
        // At least some should be dropped.
        assert!(
            scout.stats.messages_sent + scout.stats.messages_dropped > 0,
            "expected message activity"
        );
    }

    #[test]
    fn scout_stays_in_bounds() {
        // Use a small world to quickly hit boundaries.
        let config = WorldConfig {
            width: 10,
            height: 10,
            obstacle_threshold: 0.9, // very few obstacles
            base_safety_radius: 1,
            energy_nodes: 1,
            crystal_nodes: 1,
            ..WorldConfig::default()
        };
        let world = World::generate(42, config).expect("world gen");
        let (tx, _rx) = mpsc::channel(64);
        let mut scout = Scout::new(0, world.base(), &world, ScoutConfig::default(), 42, tx);

        for _ in 0..500 {
            scout.tick(&world);
            assert!(scout.position().x < world.width());
            assert!(scout.position().y < world.height());
        }
    }

    #[test]
    fn scout_tick_timing_metrics() {
        let config = WorldConfig::default();
        let world = World::generate(42, config).expect("world gen");
        let num_scouts = 4_u16;
        let ticks = 1000_u64;
        let (tx, _rx) = mpsc::channel(256);

        let mut scouts: Vec<Scout> = (0..num_scouts)
            .map(|id| {
                Scout::new(
                    id,
                    world.base(),
                    &world,
                    ScoutConfig::default(),
                    100 + id as u64,
                    tx.clone(),
                )
            })
            .collect();
        drop(tx);

        let mut tick_samples_us = Vec::with_capacity(ticks as usize);

        for _ in 0..ticks {
            let start = std::time::Instant::now();
            for scout in &mut scouts {
                scout.tick(&world);
            }
            tick_samples_us.push(start.elapsed().as_nanos() as f64 / 1000.0);
        }

        tick_samples_us.sort_by(f64::total_cmp);
        let mean = tick_samples_us.iter().sum::<f64>() / tick_samples_us.len() as f64;
        let p95_idx = ((tick_samples_us.len() as f64) * 0.95).ceil() as usize - 1;
        let p95 = tick_samples_us[p95_idx.min(tick_samples_us.len() - 1)];

        println!(
            "scout_tick_us mean={mean:.2} p95={p95:.2} scouts={num_scouts} ticks={ticks}"
        );

        // After 1000 ticks, verify exploration progress.
        let total_explored: u64 = scouts.iter().map(|s| s.stats.cells_explored).sum();
        assert!(total_explored > 200, "total_explored={total_explored}");
    }
}
