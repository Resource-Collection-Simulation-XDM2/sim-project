# Resource Collection Simulation

## Objective

Create a terminal-based graphical simulation using Ratatui that simulates autonomous robots collecting resources on a procedurally generated map.

## Requirements

### Map Generation

- Generate a map with noise-based obstacles

- Populate the map with two types of resources:

    - Energy sources (represented as 'E')

    - Crystal deposits (represented as 'C')

- Resources should have random quantities (50-200 units each)

### Robot Types

Implement two types of robots with distinct behaviors:

1. Scout Robots (represented as 'x')

    - Explore the map randomly

    - Discover and share resource locations

    - Avoid known obstacles

    - Cannot collect resources

2. Collector Robots (represented as 'o')

    - Navigate to known resource locations

    - Collect resources one unit at a time

    - Return to base when carrying resources

    - Unload resources at the central base

### Base System

- Central base acts as:

    - Starting point for all robots

    - Resource storage and knowledge hub

    - Communication center for sharing discoveries

- Track total collected energy and crystals

### Concurrent Architecture & Knowledge Management

- Each robot operates as an independent entity with limited local knowledge

- Robots start with no information about the map beyond their immediate surroundings

- Information sharing occurs through asynchronous communication mechanisms

- Key distributed behaviors to implement:

    - Scouts broadcast discovered resources and obstacles to other robots

    - Collectors communicate resource collection events for the base to update global state

    - Base system coordinates knowledge aggregation from all robot discoveries

    - Robots must synchronize their actions without blocking each other's operations

### Technical Requirements

- Use Ratatui for terminal UI rendering

- Implement real-time simulation

- Handle user input (any key press exits)

- Use Rust's concurrency features for robot coordination

- Generate obstacles using Perlin noise

### Visual Layout

```bash
Obstacles: O (light cyan)
Energie: E (green)
Crystals: C (light magenta)
Base: # (light green)
Scouts: x (red)
Collectors: o (magenta)
UI: Display collected resources counter
```

### Success Criteria

- Robots autonomously navigate and avoid obstacles

- Scouts discover and share resource locations

- Collectors efficiently gather resources and return to base

- Real-time updates of resource collection progress

- Clean terminal rendering with proper color coding

## Grading Rubric

### Core Implementation (60 points)

- Map Generation (10 points): Noise-based obstacle generation, resource placement

- Robot Behaviors (20 points): Distinct scout and collector behaviors, pathfinding

- Base System (10 points): Resource storage, starting point functionality

- Communication System (20 points): Message passing, knowledge sharing, synchronization

### Technical Quality (25 points)

- Concurrent Architecture (10 points): Independent robot entities, non-blocking operations

- Ratatui Integration (8 points): Real-time rendering, proper color coding

- Code Quality (7 points): Clean structure, proper error handling, documentation

### Advanced Features (15 points)

- Optimization (5 points): Efficient pathfinding, resource allocation strategies

- Robustness (5 points): Handle edge cases, resource depletion, collision avoidance

- User Experience (5 points): Smooth simulation, clear visual feedback

Resource Collection Simulation: [https://md2pdf.netlify.app/](https://md2pdf.netlify.app/)
