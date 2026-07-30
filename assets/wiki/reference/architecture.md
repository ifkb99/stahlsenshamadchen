---
id: architecture
title: Architecture
category: Reference
---

# Architecture

Two crates make up the project.

## `tactics_core`

The pure simulation. No Bevy dependency: battle state is plain data,
cloned and stepped headlessly. Battles are WEGO, so the boundary has two
halves: orders record intent, and ticks resolve it. Both return events.

```text
BattleState::apply(Order) -> Vec<Event>      // planning: record an intent
BattleState::step_tick(registry) -> Vec<Event> // resolution: one tick
```

Resolving a tick at a time is what lets the presentation layer animate a
round without the simulation racing ahead of the sprites; headless callers
use `resolve_round` instead. That boundary is what makes search-based AI
(MCTS), deterministic replays, and fast tests possible. The crate owns:

- The data / mod registry
- Map format
- Movement, fog of war, line of sight, combat
- The overworld sim (still turn-based: one side acts per day)
- AI planners, and the doctrine-weighted evaluator they share

## `game`

The Bevy presentation layer:

- Isometric hex rendering with elevation prisms
- Six-step view rotation and tile picking
- Battle and overworld UI
- The Lua campaign host (`mlua`, vendored Lua 5.4)
