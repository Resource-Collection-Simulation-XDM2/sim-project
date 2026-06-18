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
}

/// Owns the static world map plus the mutable resource / inventory state that
/// both scouts and collectors need to read and update during the simulation.
#[derive(Debug, Clone)]
pub struct Simulation {
    pub world: World,
    stocks: Vec<ResourceStock>,
    stock_at: Vec<Option<usize>>,
    pub base_inventory: BaseInventory,
}

impl Simulation {
    /// Create a new simulation from a freshly generated [`World`].
    ///
    /// Each resource node on the world is converted into a [`ResourceStock`]
    /// with its full initial quantity still available.
    pub fn new(world: World) -> Self {
        let cell_count = world.cell_count();
        let width = world.width();
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
            });
        }

        Self {
            world,
            stocks,
            stock_at,
            base_inventory: BaseInventory::default(),
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

    /// Iterator over positions of resource stocks that still have units left.
    pub fn live_targets(&self) -> impl Iterator<Item = Position> + '_ {
        self.stocks
            .iter()
            .filter(|stock| stock.remaining > 0)
            .map(|stock| stock.position)
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    fn stock_index_at(&self, pos: Position) -> Option<usize> {
        let flat = pos.y * self.world.width() + pos.x;
        self.stock_at.get(flat).and_then(|v| *v)
    }
}
