---
id: modding
title: Modding
category: Reference
---

# Modding

Everything the engine reads lives in `assets/mods/<mod>/`. The base game
is itself a mod. Mods declare dependencies in `mod.json` and load in
dependency order; later mods override earlier definitions by re-declaring
an id.

```text
assets/mods/base/
  mod.json           id, name, version, dependencies
  characters/*.json  people: portrait, crew stats (gunnery, driving, ...)
  vehicles/*.json    chassis: armor facings, movement, vision, weapons[]
  weapons/*.json     damage, penetration, range, accuracy, indirect
  terrain/*.json     move costs per class, cover, vision block, income
  maps/*.json        palette + ASCII rows + elevation digits (+ scenario)
  campaigns/*.lua    campaign hooks (on_start, on_turn, on_battle_end)
```

## Maps

Map files are CDDA-style: a palette maps glyphs to terrain, `rows` paint
the map, and a parallel `elevation` grid of digits raises tiles:

```json
{
  "id": "river_crossing",
  "palette": { "g": "grass", "f": "forest", "w": "water" },
  "rows":      ["ggggffg", "ggwwggg"],
  "elevation": ["0011220", "0001100"]
}
```

Run `cargo run --bin validate-mods` to check every reference (weapons,
terrain, palettes, placements) without launching the game.

## Campaign scripts

Campaign scripts get a curated `game.*` API and read-only context
snapshots. See `assets/mods/base/campaigns/demo.lua` for the current
surface:

| Call | Effect |
| --- | --- |
| `game.message(text)` | Show a message in the campaign log |
| `game.set_funds(side, amount)` | Overwrite a side's funds |
| `game.give_funds(side, amount)` | Add to a side's funds |
| `game.start_battle(map_id)` | Launch a scenario battle map |

Hooks receive a `ctx` table with `turn`, `funds[side]`, `army_counts[side]`,
and `side_names[side]` (side indices are 0-based).
