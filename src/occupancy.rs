use crate::world::{Cell, Position, World};

// ---------------------------------------------------------------------------
// Occupancy grid — prevents robots from occupying the same cell
// ---------------------------------------------------------------------------

/// Tracks which cells are currently occupied by a robot.
///
/// The base cell is always exempt — multiple robots may occupy it
/// simultaneously (it is the starting point and unloading zone).
#[derive(Debug, Clone)]
pub struct OccupancyGrid {
    occupied: Vec<bool>,
    width: usize,
    height: usize,
    base: Position,
}

impl OccupancyGrid {
    /// Create a new grid matching the world dimensions.
    pub fn new(world: &World) -> Self {
        Self {
            occupied: vec![false; world.cell_count()],
            width: world.width(),
            height: world.height(),
            base: world.base(),
        }
    }

    /// Reset every cell to unoccupied.
    pub fn clear(&mut self) {
        self.occupied.fill(false);
    }

    /// Attempt to reserve `pos`.  Returns `true` on success.
    ///
    /// The base cell always succeeds (multiple robots allowed).
    /// Out-of-bounds positions always fail.
    pub fn try_reserve(&mut self, pos: Position) -> bool {
        if pos == self.base {
            return true;
        }
        let Some(idx) = self.index(pos) else {
            return false;
        };
        if self.occupied[idx] {
            return false;
        }
        self.occupied[idx] = true;
        true
    }

    /// Release a cell so another robot may occupy it.
    pub fn release(&mut self, pos: Position) {
        if pos == self.base {
            return;
        }
        if let Some(idx) = self.index(pos) {
            self.occupied[idx] = false;
        }
    }

    /// Check whether `pos` is currently reserved.
    pub fn is_occupied(&self, pos: Position) -> bool {
        if pos == self.base {
            return false;
        }
        self.index(pos)
            .map(|i| self.occupied[i])
            .unwrap_or(true) // out-of-bounds → treat as occupied
    }

    // ------------------------------------------------------------------
    // Movement resolution
    // ------------------------------------------------------------------

    /// Find the best vacant walkable cardinal neighbor of `old_pos`,
    /// preferring the cell closest (Manhattan) to `desired`.
    ///
    /// Returns `None` if the robot is fully boxed in — it should stay put.
    pub fn find_vacant_near(
        &self,
        old_pos: Position,
        desired: Position,
        world: &World,
    ) -> Option<Position> {
        let mut best: Option<Position> = None;
        let mut best_dist = usize::MAX;

        for (dx, dy) in &[(0i32, -1i32), (1, 0), (0, 1), (-1, 0)] {
            let nx = old_pos.x as i32 + dx;
            let ny = old_pos.y as i32 + dy;
            if nx < 0 || ny < 0 {
                continue;
            }
            let target: Position = Position {
                x: nx as usize,
                y: ny as usize,
            };
            // Must be walkable and not already occupied.
            if world.cell(target) != Some(Cell::Walkable) {
                continue;
            }
            if self.is_occupied(target) {
                continue;
            }
            let dist = target.x.abs_diff(desired.x) + target.y.abs_diff(desired.y);
            if dist < best_dist {
                best_dist = dist;
                best = Some(target);
            }
        }

        best
    }

    /// Deadlock breaker: returns the position one step *opposite* to the
    /// direction from `from` toward `desired` — i.e. backing up.
    ///
    /// Used when two robots meet head-on in a 1-wide corridor and no cardinal
    /// neighbor is vacant.  One robot backs up, letting the other pass.
    pub fn backward_pos(from: Position, desired: Position) -> Option<Position> {
        let dx = desired.x as i32 - from.x as i32;
        let dy = desired.y as i32 - from.y as i32;
        if dx == 0 && dy == 0 {
            return None;
        }
        // Step opposite to the intended direction.
        let bx = from.x as i32 - dx.signum();
        let by = from.y as i32 - dy.signum();
        if bx < 0 || by < 0 {
            return None;
        }
        Some(Position {
            x: bx as usize,
            y: by as usize,
        })
    }

    // ------------------------------------------------------------------
    // Internal
    // ------------------------------------------------------------------

    fn index(&self, pos: Position) -> Option<usize> {
        if pos.x >= self.width || pos.y >= self.height {
            return None;
        }
        Some(pos.y * self.width + pos.x)
    }
}

/// Resolve a collision after a robot has attempted to move to `position`.
///
/// 1. Try the desired cell.
/// 2. If it's a **resource** cell that's occupied — wait (stay put).
/// 3. Try a detour through a vacant walkable neighbour.
/// 4. Deadlock breaker: back up one step (bridge / narrow corridor).
/// 5. Stay put as a last resort.
pub fn resolve(
    position: &mut Position,
    old: Position,
    occupancy: &mut OccupancyGrid,
    world: &World,
) {
    // 1. Desired cell is free → take it.
    if occupancy.try_reserve(*position) {
        return;
    }
    // 2. Resource cell already claimed — wait here, don't detour.
    if world.resource_at(*position).is_some() {
        *position = old;
        occupancy.try_reserve(old);
        return;
    }
    // 3. Find a vacant neighbour near the target.
    if let Some(alt) = occupancy.find_vacant_near(old, *position, world) {
        *position = alt;
        occupancy.try_reserve(alt);
        return;
    }
    // 4. Deadlock: try backing up one step (bridge / narrow corridor).
    if let Some(back) = OccupancyGrid::backward_pos(old, *position)
        && world.cell(back) == Some(Cell::Walkable)
        && occupancy.try_reserve(back)
    {
        *position = back;
        return;
    }
    // 5. Completely boxed in — stay put.
    *position = old;
    occupancy.try_reserve(old);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::world::{World, WorldConfig};

    #[test]
    fn base_always_free() {
        let world = World::generate(1, WorldConfig::default()).expect("world gen");
        let mut grid = OccupancyGrid::new(&world);
        let base = world.base();

        // Reserve base — always succeeds.
        assert!(grid.try_reserve(base));
        // Second reservation also succeeds (base is exempt).
        assert!(grid.try_reserve(base));
        // Releasing base is a no-op.
        grid.release(base);
        // Still can reserve.
        assert!(grid.try_reserve(base));
    }

    #[test]
    fn normal_cell_cannot_be_double_reserved() {
        let world = World::generate(42, WorldConfig::default()).expect("world gen");
        let mut grid = OccupancyGrid::new(&world);

        // Find a walkable cell that isn't the base.
        let pos = Position { x: 10, y: 10 };
        if world.cell(pos) != Some(Cell::Walkable) {
            return; // skip if obstacle
        }

        assert!(grid.try_reserve(pos));
        assert!(grid.is_occupied(pos));
        // Second reservation fails.
        assert!(!grid.try_reserve(pos));

        grid.release(pos);
        assert!(!grid.is_occupied(pos));
        // Now it works again.
        assert!(grid.try_reserve(pos));
    }

    #[test]
    fn find_vacant_returns_walkable_neighbor() {
        let world = World::generate(7, WorldConfig::default()).expect("world gen");
        let grid = OccupancyGrid::new(&world);
        let base = world.base();

        // Pick a walkable cell near the base.
        let pos = Position {
            x: base.x + 1,
            y: base.y,
        };
        if world.cell(pos) != Some(Cell::Walkable) {
            return;
        }

        let found = grid.find_vacant_near(pos, pos, &world);
        let Some(f) = found else {
            panic!("should find at least one walkable neighbor");
        };
        assert_eq!(world.cell(f), Some(Cell::Walkable));
        // Should be a cardinal neighbor.
        let dx = f.x.abs_diff(pos.x);
        let dy = f.y.abs_diff(pos.y);
        assert!(dx + dy == 1, "should be adjacent, got ({},{})", f.x, f.y);
    }

    #[test]
    fn backward_pos_goes_opposite_to_desired() {
        let from = Position { x: 5, y: 5 };
        let desired = Position { x: 7, y: 5 }; // moving right
        let back = OccupancyGrid::backward_pos(from, desired);
        assert_eq!(back, Some(Position { x: 4, y: 5 })); // should back left

        let desired_up = Position { x: 5, y: 3 }; // moving up
        let back_up = OccupancyGrid::backward_pos(from, desired_up);
        assert_eq!(back_up, Some(Position { x: 5, y: 6 })); // should back down
    }
}
