# 🤖 Resource Collection Simulation

A real-time terminal-based autonomous robotics simulation written in Rust. Scout and collector robots explore a procedurally generated map with obstacles, discover resources (Energy and Crystals), and coordinate asynchronously to return collected materials to a central base.

**Status:** Active development | **Language:**ls Rust (Tokio + Ratatui) | **Platform:** Cross-platform terminal

---

## Table of Contents

- [Features](#features)
- [Architecture](#architecture)
- [Setup & Installation](#setup--installation)
- [Running the Simulation](#running-the-simulation)
- [Command-Line Arguments](#command-line-arguments)
- [Examples](#examples)
- [Project Structure](#project-structure)
- [Robot Behaviors](#robot-behaviors)

---

## Features

### 🎮 **Autonomous Robot Behaviors**

- **Scout Robots** (`x` in default, 🔍 in emoji theme)
  - Explore the map randomly, discovering obstacles and resources
  - Share discoveries with the central base via async messaging
  - Navigate around obstacles intelligently
  - Cannot collect resources directly

- **Collector Robots** (`o` in default, 🚚 in emoji theme)
  - Navigate to known resource locations using shared knowledge
  - Collect one unit of a resource per action
  - Return to base when carrying resources
  - Unload resources to update global inventory

### 🗺️ **Procedural Map Generation**

- **Perlin Noise-based Obstacles** (`O` / 🪨) – impassable terrain
- **Energy Sources** (`E` / ⚡) – renewable resource (50–200 units each)
- **Crystal Deposits** (`C` / 💎) – rare resource (50–200 units each)
- **Central Base** (`#` / 🏠) – starting point and resource hub
- **Customizable Map Sizes** – small, medium, large presets

### 🔄 **Concurrent Architecture**

- **Tokio-based async runtime** – Each robot runs as an independent task
- **Non-blocking message passing** – MPSCs for scout discoveries, collector updates, position broadcasts
- **Distributed knowledge** – Robots start with no map data, learn through exploration
- **Synchronization barriers** – Coordinate tick-based simulation without blocking individual robot operations

### 🎨 **Visual Themes**

- **Default:** ASCII symbols with colors
- **Emoji:** Unicode emoji rendering for a more playful appearance
- Fog-of-war display modes for exploration visibility

### 📊 **Real-Time Statistics**

- Live resource inventory counter (Energy + Crystals collected)
- Remaining resources on the map
- Progress bar that tracks total crystal collection toward mission completion
- Simulation ends automatically when the crystal progress gauge reaches 100%
- Frame rate and tick counter

---

## Architecture

### High-Level Design

```
┌─────────────────────────────────────────────────────────────┐
│                    Ratatui Terminal UI                      │
│            (Real-time rendering, event handling)            │
└──────────────────────────┬──────────────────────────────────┘
                           │
        ┌──────────────────┼──────────────────┐
        │                  │                  │
        ▼                  ▼                  ▼
   ┌────────┐         ┌─────────┐        ┌──────────┐
   │ Scouts │         │ Base    │        │Collectors│
   │ (4x)   │         │Simulation         │ (3x)     │
   └────┬───┘         └────┬────┘        └──────┬───┘
        │ async msgs       │ shared state       │ async msgs
        └──────────┬───────┼──────────┬────────┘
                   │       │          │
              ┌────▼───────▼──────────▼────┐
              │   Shared Simulation State   │
              │  (World, Resources, FOW)    │
              │  Protected by RwLock<>     │
              └────────────────────────────┘
                          │
              ┌───────────▼────────────┐
              │  Procedural World      │
              │  (Perlin Noise Map)    │
              └────────────────────────┘
```

### Module Breakdown

| Module | Purpose |
|--------|---------|
| `main.rs` | CLI argument parsing and async runtime entry |
| `app.rs` | Main simulation loop, rendering, and event handling |
| `simulation.rs` | World state, resource tracking, fog-of-war |
| `world.rs` | Map generation, cell management |
| `map.rs` | Perlin noise generation, terrain presets |
| `scout.rs` | Scout robot logic and discovery broadcasting |
| `collector.rs` | Collector robot logic and resource gathering |
| `concurrency.rs` | Tokio task spawning and synchronization |
| `collision.rs` | Occupancy grid for efficient collision detection |

### Data Flow

```
User Input (Keyboard)
        │
        ▼
   [Event Loop]
        │
        ├─► Clear old robot positions
        │
        ├─► Synchronization Barrier (tick)
        │
        ├─► Receive position updates from all robots
        │
        ├─► Receive scout discoveries & collector updates
        │
        ├─► Update simulation state (inventory, FOW)
        │
        └─► Render to terminal (33ms per frame)
```

---

## Setup & Installation

### Requirements

- **Rust 1.93+** (stable channel recommended)
- **Cargo** (comes with Rust)
- **Terminal** with 80x24 minimum size (recommended 100x30+)

### Build & Verify

```bash
# Clone or navigate to the project directory
cd sim-project

# Verify the project compiles
cargo check

# Build in debug mode
cargo build

# Build optimized release
cargo build --release
```

---

## Running the Simulation

### Basic Commands

```bash
# Run with all defaults (randomized seed, default theme, normal FOW)
cargo run

# Run with specific theme (emoji mode)
cargo run -- d emoji

# Run with specific seed (reproducible simulation)
cargo run -- d default 12345

# Run with full map visibility (reveal all)
cargo run -- d emoji reveal

# Run with terrain-only visibility (hide fog but show resources)
cargo run -- d emoji nofog

# Combine seed + reveal mode
cargo run -- d emoji 12345 reveal
```

### Exiting

- Press **any key** to exit the simulation
- The simulation displays the final resource collection tally
- The simulation also ends automatically when the crystal progress gauge reaches 100%

---

## Command-Line Arguments

### Argument 1: Map Preset

Controls the size and layout of the generated map.

| Value | Map Size | Robots | Use Case |
|-------|----------|--------|----------|
| `default` | ~80×40 | 4 scouts, 3 collectors | Balanced exploration |
| `d` | ~80×40 | 4 scouts, 3 collectors | **Short alias for `default`** |
| `small` | ~40×20 | 4 scouts, 3 collectors | Quick testing |
| `s` | ~40×20 | 4 scouts, 3 collectors | Short alias for `small` |
| `large` | ~120×60 | 4 scouts, 3 collectors | Extended simulation |
| `l` | ~120×60 | 4 scouts, 3 collectors | Short alias for `large` |

**Default:** `default`

### Argument 2: Visual Theme

Controls the appearance of terrain, robots, and resources in the terminal.

| Value | Robot Display | Resource Display | Use Case |
|-------|---------------|------------------|----------|
| `default` | `x` (scout), `o` (collector) | `O` (obstacle), `E` (energy), `C` (crystal) | Classic ASCII |
| `d` | ASCII | ASCII | Short alias |
| `emoji` | 🔍 (scout), 🚚 (collector) | 🪨, ⚡, 💎 | Playful emoji mode |
| `e` | Emoji | Emoji | Short alias |

**Default:** `default`

### Argument 3: Seed / Reveal Mode

Controls map generation randomness and visibility.

| Value | Type | Effect |
|-------|------|--------|
| *(number)* | Seed | Use specific number to regenerate identical maps (e.g., `12345`) |
| `reveal` | Reveal Mode | Show entire map (no fog-of-war); robots still move |
| `nofog` | Reveal Mode | Show terrain but hide fog; resources and robots still discovered |
| *(omitted)* | – | Randomized seed, normal fog-of-war |

**Default:** Random seed, normal fog-of-war

### Argument 4: Reveal Mode (when Arg 3 is a seed)

Optional; applies reveal mode after specifying a seed.

| Value | Effect |
|-------|--------|
| `reveal` | Show entire map with all resources visible |
| `nofog` | Show terrain; resources/robots discovered gradually |
| *(omitted)* | Normal fog-of-war mode |

**Default:** Normal mode

---

## Examples

### 1️⃣ **Quick Start**
```bash
cargo run
```
- Randomized map (default size)
- ASCII theme
- Normal fog-of-war
- Observe robots discovering resources in real-time

### 2️⃣ **Emoji Visualization**
```bash
cargo run -- d emoji
```
- Default map size with emoji theme
- Scouts appear as 🔍, collectors as 🚚
- Resources: ⚡ (energy), 💎 (crystal)

### 3️⃣ **Reproducible Test (Same Map Every Time)**
```bash
cargo run -- d default 42
```
- Always generates the same map (seed 42)
- Great for debugging or replaying scenarios

### 4️⃣ **Large Map with Full Visibility**
```bash
cargo run -- l emoji reveal
```
- Large 120×60 map
- All resources and obstacles visible from the start
- Watch robots navigate freely

### 5️⃣ **Small Map + Terrain Only**
```bash
cargo run -- s emoji nofog
```
- Small 40×20 map
- Terrain (obstacles, base) always visible
- Resources/robots discovered as robots explore

### 6️⃣ **Seed + Custom Reveal Mode**
```bash
cargo run -- d emoji 999 reveal
```
- Generates map with seed 999 (reproducible)
- Full map visibility
- Emoji theme

### 7️⃣ **Release Build (Performance)**
```bash
cargo build --release
./target/release/resource_collection_simulation d emoji
```
- Optimized binary for faster simulation
- Useful for large maps or extended runs

---

## Project Structure

```
sim-project/
├── src/
│   ├── main.rs              # CLI parsing & async entry
│   ├── app.rs               # Main event loop & rendering
│   ├── simulation.rs         # Simulation state & mechanics
│   ├── world.rs             # Map cells & position tracking
│   ├── map.rs               # Perlin noise generation
│   ├── scout.rs             # Scout robot behaviors
│   ├── collector.rs         # Collector robot behaviors
│   ├── concurrency.rs       # Tokio task management
│   ├── collision.rs         # Occupancy grid
│   └── lib.rs               # Module declarations
├── Cargo.toml               # Dependencies & metadata
├── rust-toolchain.toml      # Rust version constraint
├── rustfmt.toml             # Code formatting rules
├── README.md                # This file
└── project-description.md   # Original specification
```

---

## Robot Behaviors

### Scout Robots 🔍

**Role:** Exploration and discovery

1. **Movement:** Random walk across the map, avoiding obstacles
2. **Discovery:** When a scout finds an energy source or crystal:
   - Broadcasts location to the base
   - Other scouts and collectors learn the position
3. **Obstacle Avoidance:** Updates occupancy grid; won't collide
4. **Knowledge Sharing:** Non-blocking async message to base

**Lifecycle:**
```
[Spawn at Base]
    ↓
[Random Movement]
    ↓
[Discover Resource? → Broadcast to Base]
    ↓
[Continue Exploring]
    ↓
[Exit on User Input]
```

### Collector Robots 🚚

**Role:** Resource harvesting and delivery

1. **Idle State:** Waits at base until resources are discovered
2. **Navigation:** Moves toward nearest known resource location
3. **Collection:** Picks up one unit per action when at a resource
4. **Return Trip:** Returns to base once carrying resources
5. **Delivery:** Unloads resources at base; updates global inventory
6. **Repeat:** Seeks next known resource

**Lifecycle:**
```
[Spawn at Base]
    ↓
[Wait for Resource Discovery]
    ↓
[Navigate to Resource Location]
    ↓
[Collect Resource (1 unit at a time)]
    ↓
[Return to Base]
    ↓
[Deliver Resources]
    ↓
[Inventory Updated]
    ↓
[Return to Step 2]
```

---

## Performance Notes

- **Frame Rate:** 30 FPS (33ms per frame)
- **Typical Runtime:** 2–10 minutes per simulation (depending on map size and resource density)
- **Optimization:** Occupancy grid provides O(1) collision lookup; shared state protected by RwLock
- **Bottleneck:** Terminal rendering is typically the limiting factor on large maps

---

## Troubleshooting

### "Cannot create expander for tokio_macros" Error

This is a Rust toolchain version mismatch. **Solution:**

```bash
# Remove old build artifacts
rm -r target

# Ensure stable toolchain is active
rustup override set stable

# Rebuild
cargo clean && cargo build
```

### Simulation Freezes or Is Unresponsive

- Press **any key** to exit
- Reduce map size: `cargo run -- s emoji`
- Check your terminal size (minimum 80×24)

### Terminal Rendering Issues

- Ensure terminal supports 256 colors
- Try default theme if emoji rendering looks corrupted:
  ```bash
  cargo run -- d default
  ```

---

## Development & Contributing

### Code Style

Follows Rust conventions with `rustfmt`:

```bash
cargo fmt
```

### Linting

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

### Testing

```bash
cargo test
```

---

## License

See [LICENSE](LICENSE) file.

---

## References

- **Ratatui:** https://ratatui.rs/
- **Tokio:** https://tokio.rs/
- **Perlin Noise:** https://en.wikipedia.org/wiki/Perlin_noise
- **Procedural Generation:** https://www.bosunsmate.org/perlin-noise/

---

**Happy simulating! 🚀**
