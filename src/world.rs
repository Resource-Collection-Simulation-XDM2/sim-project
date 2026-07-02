use anyhow::{Result, anyhow, bail};
use noise::{NoiseFn, Perlin};
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::{Rng, SeedableRng};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub x: usize,
    pub y: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Energy,
    Crystal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceNode {
    pub kind: ResourceKind,
    pub position: Position,
    pub quantity: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    Walkable,
    Obstacle,
}

/// Maps a biome noise value (0..=255) to a palette entry for obstacle variety.
/// Low values → water/wet, mid values → forest/vegetation, high values → rock/mountain.
pub type BiomeId = u8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct World {
    width: usize,
    height: usize,
    cells: Vec<Cell>,
    /// Per-cell biome identifier for obstacle cells. `0` for walkable cells
    /// (but the value is only meaningful when `cells[idx] == Cell::Obstacle`).
    obstacle_biome: Vec<BiomeId>,
    resource_at: Vec<Option<usize>>,
    resources: Vec<ResourceNode>,
    base: Position,
}

#[derive(Debug, Clone, Copy)]
pub struct WorldConfig {
    pub width: usize,
    pub height: usize,
    pub obstacle_threshold: f64,
    pub obstacle_frequency: f64,
    pub base_safety_radius: usize,
    pub energy_nodes: usize,
    pub crystal_nodes: usize,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self {
            width: 96,
            height: 40,
            obstacle_threshold: 0.05,
            obstacle_frequency: 0.06,
            base_safety_radius: 5,
            energy_nodes: 14,
            crystal_nodes: 14,
        }
    }
}

// ---------------------------------------------------------------------------
// World generation
// ---------------------------------------------------------------------------

/// Compute multi-octave fractal Brownian motion (fBm) noise.
///
/// Sums `octaves` layers of Perlin noise at increasing frequencies, producing
/// coherent, natural-looking terrain with both large-scale structure and
/// small-scale detail.
fn fbm_noise(perlin: &Perlin, x: f64, y: f64, base_freq: f64, octaves: u32) -> f64 {
    let mut value = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = base_freq;
    let mut max_value = 0.0;

    for _ in 0..octaves {
        value += amplitude * perlin.get([x * frequency, y * frequency]);
        max_value += amplitude;
        amplitude *= 0.5;
        frequency *= 2.0;
    }

    value / max_value
}

impl World {
    pub fn generate(seed: u64, config: WorldConfig) -> Result<Self> {
        validate_config(config)?;

        let mut rng = StdRng::seed_from_u64(seed);
        let perlin = Perlin::new(seed as u32);
        let mut cells = vec![Cell::Walkable; config.width * config.height];
        let mut obstacle_biome = vec![0u8; config.width * config.height];
        let mut resource_at = vec![None; config.width * config.height];
        let mut resources = Vec::with_capacity(config.energy_nodes + config.crystal_nodes);
        let base = Position {
            x: config.width / 2,
            y: config.height / 2,
        };

        for y in 0..config.height {
            for x in 0..config.width {
                let idx = y * config.width + x;
                let sample = fbm_noise(
                    &perlin,
                    x as f64,
                    y as f64,
                    config.obstacle_frequency,
                    3, // 3 octaves — good balance of coherence and detail
                );
                if sample > config.obstacle_threshold {
                    cells[idx] = Cell::Obstacle;
                    // Secondary low-frequency noise for biome classification.
                    // Single octave at very low frequency produces large coherent patches
                    // that look like natural terrain (lakes, forests, mountains).
                    let biome_noise = perlin.get([x as f64 * 0.015, y as f64 * 0.015]);
                    // Map [-1, 1] → [0, 255]
                    obstacle_biome[idx] = ((biome_noise + 1.0) * 127.5).clamp(0.0, 255.0) as u8;
                }
            }
        }

        carve_base_safety_zone(&mut cells, config.width, config.height, base, config.base_safety_radius);

        // Flood-fill from base to find all reachable walkable cells.
        let reachable = flood_fill_reachable(&cells, config.width, config.height, base);

        let mut candidates: Vec<usize> = cells
            .iter()
            .enumerate()
            .filter_map(|(idx, cell)| {
                if *cell != Cell::Walkable || !reachable[idx] {
                    return None;
                }

                let pos = index_to_position(config.width, idx);
                if pos == base {
                    return None;
                }

                Some(idx)
            })
            .collect();

        candidates.shuffle(&mut rng);

        // Place as many resources as reachable space allows (may be fewer
        // than requested if the map has isolated pockets).
        let energy_to_place = config.energy_nodes.min(candidates.len());
        let crystal_to_place = (config.crystal_nodes)
            .min(candidates.len().saturating_sub(energy_to_place));

        for _ in 0..energy_to_place {
            let idx = candidates
                .pop()
                .ok_or_else(|| anyhow!("candidate pool unexpectedly exhausted"))?;
            let position = index_to_position(config.width, idx);
            let resource_index = resources.len();
            resources.push(ResourceNode {
                kind: ResourceKind::Energy,
                position,
                quantity: rng.random_range(50..=200),
            });
            resource_at[idx] = Some(resource_index);
        }

        for _ in 0..crystal_to_place {
            let idx = candidates
                .pop()
                .ok_or_else(|| anyhow!("candidate pool unexpectedly exhausted"))?;
            let position = index_to_position(config.width, idx);
            let resource_index = resources.len();
            resources.push(ResourceNode {
                kind: ResourceKind::Crystal,
                position,
                quantity: rng.random_range(50..=200),
            });
            resource_at[idx] = Some(resource_index);
        }

        Ok(Self {
            width: config.width,
            height: config.height,
            cells,
            obstacle_biome,
            resource_at,
            resources,
            base,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    pub fn base(&self) -> Position {
        self.base
    }

    pub fn resources(&self) -> &[ResourceNode] {
        &self.resources
    }

    pub fn cell(&self, position: Position) -> Option<Cell> {
        self.index(position).map(|idx| self.cells[idx])
    }

    pub fn resource_at(&self, position: Position) -> Option<&ResourceNode> {
        let idx = self.index(position)?;
        self.resource_at[idx].and_then(|resource_idx| self.resources.get(resource_idx))
    }

    /// Return the biome identifier for the obstacle at `position`, or `None`
    /// if the cell is not an obstacle.
    pub fn obstacle_biome(&self, position: Position) -> Option<BiomeId> {
        let idx = self.index(position)?;
        if self.cells[idx] == Cell::Obstacle {
            Some(self.obstacle_biome[idx])
        } else {
            None
        }
    }

    pub fn obstacle_count(&self) -> usize {
        self.cells.iter().filter(|cell| **cell == Cell::Obstacle).count()
    }

    pub fn obstacle_ratio(&self) -> f64 {
        self.obstacle_count() as f64 / self.cells.len() as f64
    }

    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    fn index(&self, position: Position) -> Option<usize> {
        if position.x >= self.width || position.y >= self.height {
            return None;
        }
        Some(position.y * self.width + position.x)
    }
}

fn validate_config(config: WorldConfig) -> Result<()> {
    if config.width < 8 || config.height < 8 {
        bail!("world dimensions must be at least 8x8");
    }
    if !(0.0..=1.0).contains(&config.obstacle_frequency) || config.obstacle_frequency == 0.0 {
        bail!("obstacle_frequency must be in (0.0, 1.0]");
    }
    if !(-1.0..=1.0).contains(&config.obstacle_threshold) {
        bail!("obstacle_threshold must be in [-1.0, 1.0]");
    }
    Ok(())
}

fn carve_base_safety_zone(
    cells: &mut [Cell],
    width: usize,
    height: usize,
    base: Position,
    base_safety_radius: usize,
) {
    let r2 = (base_safety_radius * base_safety_radius) as isize;
    for y in 0..height {
        for x in 0..width {
            let dx = x as isize - base.x as isize;
            let dy = y as isize - base.y as isize;
            if dx * dx + dy * dy <= r2 {
                cells[y * width + x] = Cell::Walkable;
            }
        }
    }
}

fn index_to_position(width: usize, idx: usize) -> Position {
    Position {
        x: idx % width,
        y: idx / width,
    }
}

/// BFS flood-fill from `start`, returning a bitmap of all walkable cells
/// reachable without passing through obstacles.
fn flood_fill_reachable(cells: &[Cell], width: usize, height: usize, start: Position) -> Vec<bool> {
    let mut reached = vec![false; cells.len()];
    let start_idx = start.y * width + start.x;
    if start_idx >= cells.len() || cells[start_idx] == Cell::Obstacle {
        return reached;
    }
    let mut queue = std::collections::VecDeque::new();
    reached[start_idx] = true;
    queue.push_back(start_idx);

    while let Some(cur) = queue.pop_front() {
        let x = cur % width;
        let y = cur / width;
        // Cardinal neighbours.
        if y > 0 {
            let n = (y - 1) * width + x;
            if !reached[n] && cells[n] == Cell::Walkable {
                reached[n] = true;
                queue.push_back(n);
            }
        }
        if x + 1 < width {
            let n = y * width + (x + 1);
            if !reached[n] && cells[n] == Cell::Walkable {
                reached[n] = true;
                queue.push_back(n);
            }
        }
        if y + 1 < height {
            let n = (y + 1) * width + x;
            if !reached[n] && cells[n] == Cell::Walkable {
                reached[n] = true;
                queue.push_back(n);
            }
        }
        if x > 0 {
            let n = y * width + (x - 1);
            if !reached[n] && cells[n] == Cell::Walkable {
                reached[n] = true;
                queue.push_back(n);
            }
        }
    }
    reached
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn obstacle_ratio_stays_in_reasonable_bounds() {
        let config = WorldConfig::default();
        let mut min_ratio = f64::MAX;
        let mut max_ratio = f64::MIN;

        for seed in 0..100_u64 {
            let world = World::generate(seed, config).expect("world generation should succeed");
            let ratio = world.obstacle_ratio();
            min_ratio = min_ratio.min(ratio);
            max_ratio = max_ratio.max(ratio);
        }

        assert!(min_ratio >= 0.08, "min_ratio = {min_ratio}");
        assert!(max_ratio <= 0.60, "max_ratio = {max_ratio}");
    }

    #[test]
    fn resources_never_spawn_on_obstacles() {
        let world = World::generate(42, WorldConfig::default()).expect("world generation should succeed");

        for resource in world.resources() {
            let cell = world
                .cell(resource.position)
                .expect("resource position should always be in-bounds");
            assert_eq!(cell, Cell::Walkable);
        }
    }

    #[test]
    fn resource_quantity_is_within_bounds() {
        let world = World::generate(1337, WorldConfig::default()).expect("world generation should succeed");

        assert!(!world.resources().is_empty());
        for resource in world.resources() {
            assert!((50..=200).contains(&resource.quantity));
        }
    }

    #[test]
    fn base_safety_radius_is_walkable() {
        let config = WorldConfig::default();
        let world = World::generate(99, config).expect("world generation should succeed");
        let base = world.base();
        let r2 = (config.base_safety_radius * config.base_safety_radius) as isize;

        for y in 0..world.height() {
            for x in 0..world.width() {
                let dx = x as isize - base.x as isize;
                let dy = y as isize - base.y as isize;
                if dx * dx + dy * dy <= r2 {
                    let cell = world.cell(Position { x, y }).expect("position should be in-bounds");
                    assert_eq!(cell, Cell::Walkable);
                }
            }
        }
    }

    #[test]
    fn generation_is_deterministic_for_same_seed() {
        let config = WorldConfig::default();
        let world_a = World::generate(2026, config).expect("world generation should succeed");
        let world_b = World::generate(2026, config).expect("world generation should succeed");
        assert_eq!(world_a, world_b);
    }

    #[test]
    fn generation_timing_metrics() {
        let config = WorldConfig::default();
        let runs = 150;
        let mut samples_ms = Vec::with_capacity(runs);

        for seed in 0..runs as u64 {
            let start = std::time::Instant::now();
            let world = World::generate(seed, config).expect("world generation should succeed");
            assert!(world.cell_count() > 0);
            samples_ms.push(start.elapsed().as_secs_f64() * 1000.0);
        }

        samples_ms.sort_by(f64::total_cmp);
        let mean_ms = samples_ms.iter().sum::<f64>() / samples_ms.len() as f64;
        let p95_idx = ((samples_ms.len() as f64) * 0.95).ceil() as usize - 1;
        let p95_ms = samples_ms[p95_idx.min(samples_ms.len() - 1)];

        println!(
            "world_generation_ms mean={mean_ms:.4} p95={p95_ms:.4} runs={runs}"
        );
    }
}
