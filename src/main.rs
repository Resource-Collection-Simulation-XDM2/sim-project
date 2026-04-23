mod world;

use anyhow::Result;
use world::{Cell, ResourceKind, World, WorldConfig};

fn main() -> Result<()> {
    let config = WorldConfig::default();
    let world = World::generate(1, config)?;

    let (energy_nodes, crystal_nodes) = world
        .resources()
        .iter()
        .fold((0_usize, 0_usize), |(energy, crystal), node| match node.kind {
            ResourceKind::Energy => (energy + 1, crystal),
            ResourceKind::Crystal => (energy, crystal + 1),
        });
    let total_resource_units: u32 = world.resources().iter().map(|node| u32::from(node.quantity)).sum();
    let base = world.base();
    let base_cell_is_walkable = matches!(world.cell(base), Some(Cell::Walkable));
    let base_has_resource = world.resource_at(base).is_some();

    println!(
        "Generated {}x{} world: cells={}, obstacles={} ({:.1}%), resources={} (E={}, C={}) units={} base=({}, {}) base_walkable={} base_has_resource={}",
        world.width(),
        world.height(),
        world.cell_count(),
        world.obstacle_count(),
        world.obstacle_ratio() * 100.0,
        world.resources().len(),
        energy_nodes,
        crystal_nodes,
        total_resource_units,
        base.x,
        base.y,
        base_cell_is_walkable,
        base_has_resource
    );

    Ok(())
}
