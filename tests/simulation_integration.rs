//! Integration tests — full pipeline: scouts, collectors, base, async messages.

use resource_collection_simulation::app::{App, SimMetrics};
use resource_collection_simulation::map::{MapPreset, VisualTheme};
use resource_collection_simulation::simulation::RevealMode;
use resource_collection_simulation::world::Cell;

async fn make_app(seed: u64, preset: MapPreset) -> App {
    App::new(seed, preset, VisualTheme::DEFAULT, RevealMode::Normal)
        .await
        .expect("app should initialize")
}

async fn run_ticks(app: &mut App, count: u64) -> SimMetrics {
    for _ in 0..count {
        app.step().await;
    }
    app.metrics()
}

fn assert_robots_on_walkable(app: &App) {
    let world = app.world();
    for pos in app.scout_positions().iter().chain(app.collector_positions()) {
        let cell = world
            .cell(*pos)
            .unwrap_or_else(|| panic!("robot out of bounds at {pos:?}"));
        assert_eq!(cell, Cell::Walkable, "robot on obstacle at {pos:?}");
    }
}

fn assert_inventory_bounds(m: &SimMetrics) {
    assert!(
        m.collected() + m.remaining <= m.initial_total,
        "collected({}) + remaining({}) > initial({})",
        m.collected(),
        m.remaining,
        m.initial_total,
    );
}

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn integration_app_boots_with_robots_and_resources() {
    let app = make_app(42, MapPreset::Compact).await;
    let m = app.metrics();

    assert_eq!(app.scout_positions().len(), 4);
    assert_eq!(app.collector_positions().len(), 3);
    assert!(m.initial_total > 0);
    assert_eq!(m.remaining, m.initial_total);
    assert_eq!(m.collected(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn integration_same_seed_produces_identical_startup() {
    let app_a = make_app(2026, MapPreset::Plains).await;
    let app_b = make_app(2026, MapPreset::Plains).await;

    assert_eq!(app_a.metrics(), app_b.metrics());
    assert_eq!(app_a.world(), app_b.world());
}

// ---------------------------------------------------------------------------
// Tick pipeline
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn integration_tick_pipeline_runs_without_deadlock() {
    let mut app = make_app(42, MapPreset::Compact).await;
    let m = run_ticks(&mut app, 500).await;

    assert_eq!(m.tick, 500);
    assert_robots_on_walkable(&app);
}

#[tokio::test(flavor = "multi_thread")]
async fn integration_all_map_presets_run_successfully() {
    let presets = [
        MapPreset::Default,
        MapPreset::Cavern,
        MapPreset::Forest,
        MapPreset::Archipelago,
        MapPreset::Plains,
        MapPreset::Compact,
    ];

    for (i, preset) in presets.into_iter().enumerate() {
        let mut app = make_app(100 + i as u64, preset).await;
        let m = run_ticks(&mut app, 200).await;
        assert_eq!(m.tick, 200, "preset {preset:?} failed");
        assert_robots_on_walkable(&app);
    }
}

// ---------------------------------------------------------------------------
// Scout → base knowledge flow
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn integration_scouts_expand_fog_of_war() {
    let mut app = make_app(7, MapPreset::Compact).await;
    let start = app.metrics().revealed_cells;
    let m = run_ticks(&mut app, 300).await;

    assert!(
        m.revealed_cells > start,
        "fog should shrink (start={start}, now={})",
        m.revealed_cells
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn integration_scouts_discover_resources() {
    let mut app = make_app(11, MapPreset::Compact).await;
    let start = app.metrics().discovered_resources;
    let m = run_ticks(&mut app, 400).await;

    assert!(
        m.discovered_resources > start,
        "scouts should discover resources (start={start}, now={})",
        m.discovered_resources
    );
}

// ---------------------------------------------------------------------------
// Collector loop
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn integration_collectors_deliver_resources_to_base() {
    let mut app = make_app(21, MapPreset::Compact).await;
    let m = run_ticks(&mut app, 2_000).await;

    assert!(m.collected() > 0, "collectors should unload at base");
    assert!(m.remaining < m.initial_total, "map stock should decrease");
    assert_inventory_bounds(&m);
    assert_robots_on_walkable(&app);
}

#[tokio::test(flavor = "multi_thread")]
async fn integration_collection_never_decreases() {
    let mut app = make_app(33, MapPreset::Compact).await;
    let mut last = 0_u64;

    for _ in 0..20 {
        let m = run_ticks(&mut app, 100).await;
        assert!(m.collected() >= last, "collected must be monotonic");
        last = m.collected();
    }
}

// ---------------------------------------------------------------------------
// Full mission
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn integration_full_mission_completes() {
    let mut app = make_app(99, MapPreset::Compact).await;
    let initial = app.metrics().initial_total;
    let max_ticks = 40_000_u64;

    let mut m = app.metrics();
    for _ in 0..max_ticks {
        app.step().await;
        m = app.metrics();
        assert_inventory_bounds(&m);
        // Map empty AND all in-flight cargo delivered to base.
        if m.is_complete() && m.collected() == initial {
            break;
        }
    }

    assert!(
        m.is_complete(),
        "map resources should be depleted within {max_ticks} ticks (remaining={})",
        m.remaining,
    );
    assert_eq!(
        m.collected(),
        initial,
        "all units picked up must eventually reach the base"
    );
    assert_robots_on_walkable(&app);
}
