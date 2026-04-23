# Resource Collection Simulation

## Objective

Create a terminal-based graphical simulation using Ratatui that simulates autonomous robots

Collecting resources on a procedurally generated map.

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

# Resource Collection Simulation

## Objective

Create a graphical simulation in terminal using Ratatui that simulates autonomous robots

Collecting resources on a procedurally generated map.

## Requirements

### Card Generation

- Generate a map with noise-based obstacles

- Populate the map with two types of resources:

- Energy sources (represented by 'E')

- Crystal deposits (represented by 'C')

- Resources must have random quantities (50-200 units each)

### Types of Robots

Implement two types of robots with distinct behaviors:

1. Scout Robots (represented by 'x')

- Explore the map randomly

- Discover and share resource locations

- Avoid known obstacles

- Cannot collect resources

2. Collecting Robots (represented by 'o')

- Navigate to known resource locations

- Collect resources one unit at a time

- Return to the base by carrying resources

- Unload resources at the central base

### Basic System

- The central base acts as:

- Starting point for all robots

- Resource and Knowledge Storage Center

- Communication center to share discoveries

- Track the total energy and crystals collected

### Competitive Architecture and Knowledge Management

- Each robot operates as an independent entity with limited local knowledge

- Robots start without information on the map beyond their immediate environment

- Information sharing is done through asynchronous communication mechanisms

- Key distributed behaviors to be implemented:

- Scouts spread discovered resources and obstacles to other robots

- Collectors communicate collection events so that the database can be updated

The overall state

- The basic system coordinates the aggregation of knowledge of all discoveries

Robotics

- Robots must synchronize their actions without blocking the operations of others

### Technical Requirements

- Use Ratatui for the rendering of the terminal user interface

- Implement a real-time simulation

- Manage user inputs (any key press exits)

- Use Rust's competition features for robot coordination

- Generate obstacles using Perlin's noise

### Visual Layout

```bash

Obstacles: O (clear cyan)

Energy: E (green)

Crystals: C (light magenta)

Base: # (light green)

Scouts: x (red)

Collectors: o (magenta)

UI: Display the collected resource counter

```

### Success Criteria

- Robots navigate autonomously and avoid obstacles

- Scouts discover and share resource locations

- Collectors efficiently gather resources and return to the base

- Real-time updates on resource collection progress

- Clean terminal rendering with appropriate color coding

## Evaluation Scale

### Basic Implementation (60 points)

- Map Generation (10 points): Noise-based obstacle generation, placement of

Resources

- Robot Behaviors (20 points): Distinct behaviors of scout and collector,

Pathfinding

- Basic System (10 points): Resource storage, starting point functionality

- Communication System (20 points): Passing messages, sharing knowledge,

Synchronization

### Technical Quality (25 points)

- Concurrent Architecture (10 points): Independent robotic entities, non-

Blocking

- Ratatui integration (8 points): Real-time rendering, appropriate color coding

- Code Quality (7 points): Clean structure, appropriate error management, documentation

### Advanced Features (15 points)

- Optimization (5 points): Effective pathfinding, resource allocation strategies

- Robustness (5 points): Manage borderline cases, resource depletion, avoidance of

Collisions

- User Experience (5 points): Fluid simulation, clear visual feedback

Resource Collection Simulation: [https://md2pdf.netlify.app/](https://md2pdf.netlify.app/)
