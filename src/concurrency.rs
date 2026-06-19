use std::sync::Arc;

use tokio::sync::{mpsc, Barrier, RwLock};

use crate::collision::{self, OccupancyGrid};
use crate::collector::Collector;
use crate::scout::Scout;
use crate::simulation::Simulation;
use crate::world::Position;

// ---------------------------------------------------------------------------
// Shared state types
// ---------------------------------------------------------------------------

/// The simulation state shared across all robot tasks and the main thread.
pub type SharedSim = Arc<RwLock<Simulation>>;
/// The occupancy grid shared across all robot tasks.
pub type SharedOcc = Arc<RwLock<OccupancyGrid>>;
/// Tick barrier — keeps all robots + main thread in lockstep.
pub type TickBarrier = Arc<Barrier>;

// ---------------------------------------------------------------------------
// Scout task
// ---------------------------------------------------------------------------

/// Run one scout as an independent async task.
///
/// On each tick the scout reads the world (via the shared simulation),
/// computes a move, resolves collisions, and sends discovery messages.
pub async fn run_scout(
    mut scout: Scout,
    sim: SharedSim,
    occupancy: SharedOcc,
    barrier: TickBarrier,
    position_tx: mpsc::Sender<(u16, Position)>,
) {
    loop {
        barrier.wait().await;

        let old = scout.position;
        {
            // Release old cell.
            let mut occ = occupancy.write().await;
            occ.release(old);
        }
        {
            // Scout reads the world (immutable).
            let sim_guard = sim.read().await;
            scout.tick(&sim_guard.world);
        }
        {
            // Resolve collision.
            let sim_guard = sim.read().await;
            let mut occ = occupancy.write().await;
            collision::resolve(&mut scout.position, old, &mut occ, &sim_guard.world);
        }
        // Notify main thread of new position.
        let _ = position_tx.try_send((scout.id, scout.position));
    }
}

// ---------------------------------------------------------------------------
// Collector task
// ---------------------------------------------------------------------------

/// Run one collector as an independent async task.
///
/// On each tick the collector reads/writes the simulation, resolves
/// collisions, and sends unload messages.
pub async fn run_collector(
    mut collector: Collector,
    sim: SharedSim,
    occupancy: SharedOcc,
    barrier: TickBarrier,
    position_tx: mpsc::Sender<(u16, Position)>,
) {
    loop {
        barrier.wait().await;

        let old = collector.position;
        {
            let mut occ = occupancy.write().await;
            occ.release(old);
        }
        {
            // Collector needs mutable access to the simulation.
            let mut sim_guard = sim.write().await;
            collector.tick(&mut sim_guard);
        }
        {
            let sim_guard = sim.read().await;
            let mut occ = occupancy.write().await;
            collision::resolve(&mut collector.position, old, &mut occ, &sim_guard.world);
        }
        // Notify main thread of new position.
        let _ = position_tx.try_send((collector.id, collector.position));
    }
}
