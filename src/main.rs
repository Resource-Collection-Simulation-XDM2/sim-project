mod scout;
mod world;

use anyhow::Result;
use tokio::sync::mpsc;
use world::{Cell, ResourceKind, World, WorldConfig};

use scout::{Scout, ScoutConfig, ScoutMessage};

#[tokio::main]
async fn main() -> Result<()> {
    let config = WorldConfig::default();
    let world = World::generate(1, config)?;

    // Print world summary.
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

    // --- Scout simulation demo ---
    let num_scouts: u16 = 4;
    let sim_ticks: u64 = 300;
    let channel_capacity: usize = 64;
    let scout_config = ScoutConfig::default();

    let (tx, mut rx) = mpsc::channel::<ScoutMessage>(channel_capacity);

    // Create scouts, each with a unique seed derived from its ID.
    let mut scouts: Vec<Scout> = (0..num_scouts)
        .map(|id| {
            Scout::new(
                id,
                world.base(),
                &world,
                scout_config,
                1000 + id as u64,
                tx.clone(),
            )
        })
        .collect();

    // Drop the original sender so the channel closes when all scouts are done.
    drop(tx);

    // Run simulation ticks.
    let tick_start = std::time::Instant::now();

    for _tick in 0..sim_ticks {
        for scout in &mut scouts {
            scout.tick(&world);
        }
    }

    let tick_elapsed = tick_start.elapsed();

    // Drain messages from the channel.
    let mut total_discoveries = 0_usize;
    let mut resource_discoveries = 0_usize;
    let mut obstacle_discoveries = 0_usize;
    let mut messages_received = 0_usize;
    let mut messages_by_scout = vec![0_usize; num_scouts as usize];

    while let Ok(msg) = rx.try_recv() {
        messages_received += 1;
        let scout_idx = usize::from(msg.scout_id);
        if scout_idx < messages_by_scout.len() {
            messages_by_scout[scout_idx] += 1;
        }
        for discovery in &msg.discoveries {
            total_discoveries += 1;
            match discovery {
                scout::Discovery::Resource { .. } => resource_discoveries += 1,
                scout::Discovery::Obstacle { .. } => obstacle_discoveries += 1,
            }
        }
    }

    println!("\n--- Scout Simulation ({num_scouts} scouts, {sim_ticks} ticks) ---");
    println!(
        "Total time: {:.2}ms ({:.4}ms/tick)",
        tick_elapsed.as_secs_f64() * 1000.0,
        tick_elapsed.as_secs_f64() * 1000.0 / sim_ticks as f64
    );
    println!("Messages received: {messages_received}");
    println!("Messages by scout: {:?}", messages_by_scout);
    println!(
        "Discoveries: total={total_discoveries} resources={resource_discoveries} obstacles={obstacle_discoveries}"
    );

    for scout in &scouts {
        let s = &scout.stats;
        println!(
            "  Scout #{}: ticks={} explored={} discoveries={} sent={} dropped={}",
            scout.id, s.ticks, s.cells_explored, s.discoveries_total, s.messages_sent, s.messages_dropped
        );
    }

    Ok(())
}
