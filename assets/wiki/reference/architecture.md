---
id: architecture
title: Architecture
category: Reference
---

# Architecture

Two crates make up the project.

## `tactics_core`

The pure simulation. No Bevy dependency: battle state is plain data,
cloned and stepped headlessly. Orders go in, events come out:

```text
BattleState::apply(Order) -> Vec<Event>
```

That boundary is what makes search-based AI (MCTS), deterministic replays,
and fast tests possible. The crate owns:

- The data / mod registry
- Map format
- Movement, fog of war, line of sight, combat
- The overworld sim
- AI planners

## `game`

The Bevy presentation layer:

- Isometric hex rendering with elevation prisms
- Six-step view rotation and tile picking
- Battle and overworld UI
- The Lua campaign host (`mlua`, vendored Lua 5.4)
