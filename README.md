# Resource Collection Simulation

A real-time terminal-based simulation in Rust using Ratatui. Autonomous scout and collector robots explore a procedurally generated map, discover energy and crystal resources, and coordinate asynchronously to return collected materials to a central base.

![status](https://img.shields.io/badge/status-complete-brightgreen)
![tests](https://img.shields.io/badge/tests-24%20passed-green)
![rust](https://img.shields.io/badge/rust-2024%20edition-orange)

---

## Quick Start

```bash
cargo run                          # random map, default theme
cargo run -- cavern retro          # cavern preset, retro ASCII theme
cargo run -- forest emoji 42       # forest preset, emoji theme, seed 42
cargo run -- archi symbols reveal  # archipelago, full map view (no fog)
```

**Any key exits.** Press any key to quit the simulation.

---

## CLI

```
cargo run -- [preset] [visual-theme] [seed|reveal|nofog] [reveal|nofog]
```

| Arg | Options | Default |
|-----|---------|---------|
| **preset** | `default` `cavern` `forest` `archipelago` `plains` | `default` |
| **visual-theme** | `default` `retro` `symbols` `emoji` | `default` |
| **seed** | any number, or `reveal`/`nofog` | random |
| **reveal mode** | `reveal` (full) `nofog` (terrain only) | normal fog |

### Examples

```bash
cargo run                                    # everything random
cargo run -- cavern retro                    # cave map, ASCII theme
cargo run -- forest emoji 42                 # reproducible map
cargo run -- archi symbols reveal            # full visibility, no robots
cargo run -- plains default nofog            # terrain visible, resources hidden
cargo run -- default default 99 reveal       # seed 99, full reveal
```

---

## Map Presets

| Preset | Obstacle % | Look |
|--------|-----------|------|
| `default` | ~30% | Balanced terrain |
| `cavern` | ~55% | Tight winding passages, dead-ends |
| `forest` | ~25% | Scattered obstacle clusters |
| `archipelago` | ~60% | Large water bodies, narrow land bridges |
| `plains` | ~12% | Wide-open terrain, sparse rocks |

All maps use **3-octave fractal Perlin noise** for obstacle placement plus a
**secondary low-frequency noise field** for coherent biome patches — obstacle
glyphs cluster naturally into rock fields, forests, or water bodies instead of
random scatter. A BFS flood-fill from the base ensures all placed resources are
reachable.

---

## Visual Themes

| Theme | Obstacles | Energy | Crystals | Base | Scouts | Collectors |
|-------|-----------|--------|----------|------|--------|------------|
| `default` | `O` | `E` | `C` | `#` | `x` | `o` |
| `retro` | `#` `%` `&` `@` | `E` | `C` | `H` | `S` | `R` |
| `symbols` | `◆` `▲` `■` `●` | `⚡` | `♦` | `⌂` | `●` | `○` |
| `emoji` | `🪨` `🌳` `💧` | `⚡` | `💎` | `🏠` | `🔍` | `🤖` |

Visual effects: biome terrain colors, resource depletion glow, base pulse animation, gradient fog edges.

---

## Robots

### Scouts — 4 units

- Frontier-biased random exploration
- Discover and broadcast resources/obstacles via async messaging
- Avoid known obstacles
- Cannot collect resources

### Collectors — 3 units

- **A\* pathfinding** with Manhattan heuristic
- Collects one unit per tick (capacity: 16)
- Returns to base when full, unloads via async message
- Auto-wakes from idle when scouts discover new resources
- Randomized target selection among nearest 3 resources (prevents swarming)

---

## Architecture

```
Main Thread                 7 Robot Tasks (tokio::spawn)
+--------------+            +---------------------------+
| barrier.wait |<--Barrier--| barrier.wait              |
| drain msgs   |            | sim.read() -> move        |
| update sim   |            | collision::resolve()      |
| render frame |            | position_tx.send()        |
| poll input   |            | barrier.wait              |
+--------------+            +---------------------------+
        |                                |
        +--------------+-----------------+
                       |
         +-------------v-------------+
         |  Arc<RwLock<Simulation>>  |
         |  Arc<RwLock<Occupancy>>   |
         +---------------------------+
```

| Component | Implementation |
|-----------|---------------|
| Concurrency | 7 `tokio::spawn` tasks + `Arc<Barrier>` lockstep |
| Shared state | `Arc<RwLock<Simulation>>` + `Arc<RwLock<OccupancyGrid>>` |
| Scout -> Base | `mpsc::try_send(ScoutMessage)` — non-blocking |
| Collector -> Base | `mpsc::try_send(CollectorMessage)` — non-blocking |
| Rendering | `ui.rs` renders from `AppSnapshot` — zero lock contention during draw |
| Pathfinding | A* with reusable scratch buffers (zero alloc per search) |
| Occupancy | `occupancy.rs` — per-cell reservation grid with deadlock-breaking backward step |

---

## Project Structure

```
src/
  main.rs          CLI entry point
  lib.rs           Crate root
  world.rs         Cell, Position, Perlin-noise generation, flood-fill
  simulation.rs    Resource stocks, base inventory, fog-of-war, reveal modes
  scout.rs         Scout exploration, discovery batching, messaging
  collector.rs     A* pathfinding, state machine, collector messaging
  occupancy.rs     Occupancy grid, collision resolution, deadlock breaker
  map.rs           Map presets, visual themes, biome colors
  ui.rs            Ratatui rendering — map, status bar, progress gauge
  app.rs           Event loop, simulation driver, AppSnapshot for UI
  concurrency.rs   Async robot task runners
```

---

## Design Decisions

| Decision | Rationale |
|----------|-----------|
| `try_send` over `send` | Non-blocking — channel full = backpressure, not deadlock |
| A* over BFS | Heuristic-guided — explores 30–50% fewer cells |
| `AppSnapshot` for rendering | `ui.rs` reads lock-free snapshot — zero contention during draw |
| Flood-fill reachability | Guarantees all placed resources are collectible |
| Randomized target selection | Prevents 3 collectors swarming the same resource |
| Base always free in occupancy grid | Multiple robots can occupy the starting point |
| Collector idle auto-wake | Avoids permanent deadlock when scouts discover late |
| `occupancy.rs` as dedicated module | Collision/occupancy logic separated from app and robots |

---

## Requirements

- Rust stable toolchain (edition 2024)
- Terminal with 96×42 minimum (for default preset)
- Dependencies: `ratatui`, `crossterm`, `tokio`, `rand`, `noise`, `anyhow`

```bash
cargo build --release
cargo test        # 24 tests
```
