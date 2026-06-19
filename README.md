# Resource Collection Simulation

Terminal simulation in Rust using Ratatui where autonomous scout and collector robots discover and gather resources on a procedural map.

## Setup

Requirements:
- Rust stable toolchain
- Cargo

Install dependencies and verify build: 

```bash
cargo check
```

Run:

```bash
cargo run
```

## Project Scope

- Procedural map generation with Perlin-noise obstacles
- Two robot roles (scout and collector)
- Central base for shared knowledge and resource storage
- Non-blocking robot coordination via asynchronous messaging
- Real-time terminal rendering with Ratatui and keypress exit
