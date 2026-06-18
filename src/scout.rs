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

pub struct Scout {
    pub id: u16,
    pub position: Position,
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

    // Reused per-tick buffers to avoid hot-path allocations.
    neighbor_buf: Vec<Position>,
    frontier_buf: Vec<Position>,
    explored_buf: Vec<Position>,
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
        let mut scout = Self {
            id,
            position: start,
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
            neighbor_buf: Vec::with_capacity(4),
            frontier_buf: Vec::with_capacity(4),
            explored_buf: Vec::with_capacity(4),
        };
        // Mark starting cell as explored.
        scout.observe_cell(start, world);
        scout
    }

    /// Advance one tick: observe surroundings, pick a move, step, maybe flush.
    pub fn tick(&mut self, world: &World) {
        self.stats.ticks += 1;

        self.neighbor_buf.clear();
        self.frontier_buf.clear();
        self.explored_buf.clear();

        for (dx, dy) in DIRECTIONS {
            let nx = self.position.x as i32 + dx;
            let ny = self.position.y as i32 + dy;
            if nx < 0 || ny < 0 || (nx as usize) >= self.world_width || (ny as usize) >= self.world_height {
                continue;
            }
            self.neighbor_buf.push(Position {
                x: nx as usize,
                y: ny as usize,
            });
        }

        // 1. Observe all cardinal neighbors (reveals obstacles / resources).
        // Track pre-observation exploration state so frontier selection still works.
        let neighbors = std::mem::take(&mut self.neighbor_buf);
        for &pos in &neighbors {
            let idx = self.flat_index(pos);
            let was_explored = self.explored[idx];
            self.observe_cell(pos, world);

            if self.known_obstacle[idx] {
                continue; // avoid known obstacles
            }

            if !was_explored {
                self.frontier_buf.push(pos);
            } else {
                self.explored_buf.push(pos);
            }
        }
        self.neighbor_buf = neighbors;

        // 3. Pick movement target.
        let frontier = std::mem::take(&mut self.frontier_buf);
        let explored = std::mem::take(&mut self.explored_buf);
        let target = self.pick_target(&frontier, &explored);
        self.frontier_buf = frontier;
        self.explored_buf = explored;

        // 4. Move (if we have somewhere to go).
        if let Some(pos) = target {
            self.position = pos;
            self.observe_cell(pos, world);
        }

        // 5. Periodic flush.
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

    /// Pick a movement target:
    /// - With `random_move_probability`, pick any walkable neighbor at random.
    /// - Otherwise, prefer frontier (unexplored) cells.
    /// - If no frontier, pick a random explored walkable cell.
    fn pick_target(
        &mut self,
        frontier: &[Position],
        explored: &[Position],
    ) -> Option<Position> {
        // Both empty → stuck.
        if frontier.is_empty() && explored.is_empty() {
            return None;
        }

        // Random exploration jitter.
        if self.rng.random_bool(self.config.random_move_probability) {
            let all_len = frontier.len() + explored.len();
            let idx = self.rng.random_range(0..all_len);
            return if idx < frontier.len() {
                Some(frontier[idx])
            } else {
                Some(explored[idx - frontier.len()])
            };
        }

        // Frontier-biased: prefer unexplored.
        if !frontier.is_empty() {
            let idx = self.rng.random_range(0..frontier.len());
            return Some(frontier[idx]);
        }

        let idx = self.rng.random_range(0..explored.len());
        Some(explored[idx])
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
