use crate::world::{Position, ResourceKind, World};

// ---------------------------------------------------------------------------
// Shared simulation state: resource stocks + base inventory
// ---------------------------------------------------------------------------

/// Accumulated resources delivered to the base.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BaseInventory {
    pub energy: u64,
    pub crystals: u64,
}

/// Mutable tracking of a single resource node on the map.
#[derive(Debug, Clone, Copy)]
pub struct ResourceStock {
    pub kind: ResourceKind,
    pub position: Position,
    pub remaining: u16,
    /// Original quantity at generation time (for depletion glow).
    pub initial: u16,
}

/// Owns the static world map plus the mutable resource / inventory state that
/// both scouts and collectors need to read and update during the simulation.
#[derive(Debug, Clone)]
pub struct Simulation {
    pub world: World,
    stocks: Vec<ResourceStock>,
    stock_at: Vec<Option<usize>>,
    pub base_inventory: BaseInventory,

    // Fog-of-war / knowledge tracking.
    cells_revealed: Vec<bool>,
    resource_discovered: Vec<bool>,
}

impl Simulation {
    /// Create a new simulation from a freshly generated [`World`].
    ///
    /// Each resource node on the world is converted into a [`ResourceStock`]
    /// with its full initial quantity still available.
    pub fn new(world: World) -> Self {
        let cell_count = world.cell_count();
        let width = world.width();
        let height = world.height();
        let base = world.base();
        let mut stock_at = vec![None; cell_count];
        let mut stocks = Vec::with_capacity(world.resources().len());

        for node in world.resources() {
            let stock_idx = stocks.len();
            let flat = node.position.y * width + node.position.x;
            stock_at[flat] = Some(stock_idx);
            stocks.push(ResourceStock {
                kind: node.kind,
                position: node.position,
                remaining: node.quantity,
                initial: node.quantity,
            });
        }

        // Reveal cells in a small radius around the base so the simulation
        // doesn't start completely blind.
        let mut cells_revealed = vec![false; cell_count];
        let radius: isize = 4;
        let r2 = radius * radius;
        for y in 0..height {
            for x in 0..width {
                let dx = x as isize - base.x as isize;
                let dy = y as isize - base.y as isize;
                if dx * dx + dy * dy <= r2 {
                    cells_revealed[y * width + x] = true;
                }
            }
        }

        let mut resource_discovered = vec![false; stocks.len()];
        // Resources inside the initial reveal radius are pre-discovered.
        for (i, stock) in stocks.iter().enumerate() {
            let idx = stock.position.y * width + stock.position.x;
            if cells_revealed[idx] {
                resource_discovered[i] = true;
            }
        }

        Self {
            world,
            stocks,
            stock_at,
            base_inventory: BaseInventory::default(),
            cells_revealed,
            resource_discovered,
        }
    }

    /// Sum of `remaining` units across every resource stock still on the map.
    pub fn total_remaining(&self) -> u64 {
        self.stocks.iter().map(|s| u64::from(s.remaining)).sum()
    }

    /// How many units are left at `pos` (0 if the position has no resource or
    /// the resource is already depleted).
    pub fn stock_remaining_at(&self, pos: Position) -> u16 {
        self.stock_index_at(pos)
            .and_then(|idx| self.stocks.get(idx).map(|s| s.remaining))
            .unwrap_or(0)
    }

    /// Returns `(remaining, initial)` for the resource at `pos`, or `(0, 0)`
    /// if none exists.  Used for depletion-glow rendering.
    pub fn stock_info_at(&self, pos: Position) -> (u16, u16) {
        self.stock_index_at(pos)
            .and_then(|idx| self.stocks.get(idx).map(|s| (s.remaining, s.initial)))
            .unwrap_or((0, 0))
    }

    /// Attempt to collect one unit from the resource at `pos`.
    ///
    /// Returns `Some(kind)` on success, or `None` if the position has no
    /// resource or the stock is already exhausted.
    pub fn try_collect(&mut self, pos: Position) -> Option<ResourceKind> {
        let idx = self.stock_index_at(pos)?;
        let stock = self.stocks.get_mut(idx)?;
        if stock.remaining == 0 {
            return None;
        }
        stock.remaining -= 1;
        Some(stock.kind)
    }

    /// Deposit `amount` units of `kind` into the base inventory.
    pub fn unload(&mut self, kind: ResourceKind, amount: u16) {
        match kind {
            ResourceKind::Energy => self.base_inventory.energy += u64::from(amount),
            ResourceKind::Crystal => self.base_inventory.crystals += u64::from(amount),
        }
    }

    /// Iterator over positions of resource stocks that are **discovered** and
    /// still have units left.
    pub fn live_targets(&self) -> impl Iterator<Item = Position> + '_ {
        self.stocks
            .iter()
            .enumerate()
            .filter(|(i, stock)| stock.remaining > 0 && self.resource_discovered[*i])
            .map(|(_, stock)| stock.position)
    }

    // ------------------------------------------------------------------
    // Fog-of-war / discovery
    // ------------------------------------------------------------------

    /// Record that a scout has observed a resource at `pos`.
    pub fn mark_resource_discovered(&mut self, pos: Position) {
        let width = self.world.width();
        if let Some(stock_idx) = self.stock_index_at(pos) {
            self.resource_discovered[stock_idx] = true;
        }
        // Also reveal surrounding cells.
        self.reveal_radius(pos, width, 1);
    }

    /// Record that a scout has observed an obstacle at `pos`.
    pub fn mark_obstacle_discovered(&mut self, pos: Position) {
        let idx = pos.y.checked_mul(self.world.width())
            .and_then(|v| v.checked_add(pos.x));
        if let Some(i) = idx
            && i < self.cells_revealed.len()
        {
            self.cells_revealed[i] = true;
        }
    }

    /// Whether a map cell has been revealed by any scout (or starts revealed).
    pub fn is_cell_revealed(&self, pos: Position) -> bool {
        let flat = pos.y.checked_mul(self.world.width())
            .and_then(|v| v.checked_add(pos.x));
        flat.and_then(|i| self.cells_revealed.get(i).copied())
            .unwrap_or(false)
    }

    /// Return a snapshot of the entire fog-of-war bitmap (for rendering).
    pub fn revealed_bitmap(&self) -> &[bool] {
        &self.cells_revealed
    }

    /// Whether a resource at `pos` has been discovered.
    pub fn is_resource_discovered(&self, pos: Position) -> bool {
        self.stock_index_at(pos)
            .map(|idx| self.resource_discovered.get(idx).copied().unwrap_or(false))
            .unwrap_or(false)
    }

    /// Reveal a single cell (called when a robot steps on it).
    pub fn reveal_cell(&mut self, pos: Position) {
        let idx = pos.y.checked_mul(self.world.width())
            .and_then(|v| v.checked_add(pos.x));
        if let Some(i) = idx
            && i < self.cells_revealed.len()
        {
            self.cells_revealed[i] = true;
        }
    }

    /// Reveal all cells and resources (for tests / debug).
    pub fn reveal_all(&mut self) {
        self.cells_revealed.fill(true);
        self.resource_discovered.fill(true);
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    fn stock_index_at(&self, pos: Position) -> Option<usize> {
        let flat = pos.y * self.world.width() + pos.x;
        self.stock_at.get(flat).and_then(|v| *v)
    }

    fn reveal_radius(&mut self, center: Position, width: usize, radius: isize) {
        let r2 = radius * radius;
        let max_idx = self.cells_revealed.len();
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                if dx * dx + dy * dy > r2 {
                    continue;
                }
                let nx = center.x as isize + dx;
                let ny = center.y as isize + dy;
                if nx < 0 || ny < 0 {
                    continue;
                }
                let idx = (ny as usize) * width + (nx as usize);
                if idx < max_idx {
                    self.cells_revealed[idx] = true;
                }
            }
        }
    }
}
