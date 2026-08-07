# CLAUDE.md

Engineering notes for this repo: how it fits together, the invariants worth
protecting, and the known defects. Gameplay and design work lives in
[TODO.md](TODO.md) — this file is for things that are wrong or fragile in the
code rather than things not yet built. Where an item is already tracked in
TODO.md it is cross-referenced, not repeated.

## Commands

```sh
cargo run -p stahlsenshamädchen     # the game (starts on the overworld)
cargo run --bin validate-mods       # validate assets/mods; prints the scale table
cargo test -p tactics_core          # headless engine tests (the real suite)
cargo run -p tactics_core --example playthrough [seed]   # narrated AI battle
```

Rust edition 2024, resolver 3. `[profile.dev]` builds the workspace at
`opt-level = 1` and dependencies at 3, because AI search is slow at opt-level 0.
Anything that measures performance must be built `--release`; dev-profile
numbers are meaningless for the planners.

## The scale contract

Lives in the `scale` block of `assets/mods/base/mod.json`, typed as
`data::Scale` (`data/scale.rs`) and reachable as `registry.scale`. It is data,
not Rust: a mod that declares its own block replaces it wholesale, and a mod
that says nothing inherits what it extends. `cargo run --bin validate-mods`
prints the whole roster through it — every vehicle's speed, every weapon's
range and cadence, every map's real size — which is the check that did not
exist while these numbers lived only in this table.

| Quantity | Field | Value |
| --- | --- | --- |
| Battle hex | `hex_meters` | 100 m |
| Round | `round_seconds` | 60 s |
| Tick | `round_seconds / ticks_per_round` | 5 s (12 ticks) |
| Elevation level | `elevation_meters` | 10 m |
| Overworld hex | `overworld_hex_meters` | 4 km = one battle map |
| Overworld turn | `overworld_turn_hours` | 24 h (a "day", as the campaign banner already said) |

**A battle map is a hexagon, not a rectangle.** The overworld draws a tile as
a hex and a battle is that tile zoomed in, so the battlefield is the same
shape: `Scale::battle_map_radius()` derives radius 20 from 4 km ÷ 100 m, which
is 41 hexes across and 1261 tiles. `MapKind::Battle` implies
`MapShape::Tile` and validation rejects anything that is not that exact
hexagon; a scenario map that means to be some other shape sets
`"shape": "free"`. The map format needed no changes for this — `HexMap` is a
sparse `HashMap<Hex, Tile>` and a space in a row has always meant "no tile
here" — so the hexagon is visible in the ASCII of `river_crossing.json`.

There is deliberately no `TICKS_PER_ROUND` constant any more, which is why
round resolution, weapon cooldowns and mod validation all take a registry. The
one thing that could not follow: `WeaponDef::reload_ticks` is an
`Option<u32>`, because serde's `default = "..."` is a `fn() -> u32` and cannot
see the mod being loaded. Read it through `weapon.reload(&registry.scale)`,
never the field.

Consequences that are easy to violate by accident:

- **1 movement point ≈ 1 hex of clear terrain per round ≈ 6 km/h.** A vehicle's
  `points` is a speed, not an abstract budget. 5 tracked MP on grass is 30 km/h.
- **Weapon `range` is in hexes, so ×100 m.** The 88 reaches 16 hexes = 1.6 km.
- **`reload_ticks` is a practical aimed rate**, ×5 s. Not mechanical reload.
- **Vision is deliberately shorter than gun range for gun tanks.** The tank
  destroyer sees 10 and shoots 16 because needing a spotter is its character.
  Preserve that relationship when adding vehicles.
- **A small test map can no longer put units out of contact by distance.**
  Vision is 10–20 hexes; use a forest curtain. `tests/engine.rs::standoff` does
  this and explains why.
- **Crew bonuses are percentages of the vehicle's base**, from the sibling
  `balance` block (`data::Balance`): +5% sight per awareness, +5% speed per
  driving, +3 percentage points of hit chance per gunnery. Stats run 0–5, so a
  gifted crew is worth about a quarter of their vehicle. Never reintroduce a
  flat divisor — that is exactly what the scale change silently devalued.

## Invariants worth protecting

- **`tactics_core` must not depend on Bevy.** It is the reusable half. All
  presentation, input, and asset loading lives in `crates/game`.
- **The simulation is deterministic given a seed.** `ChaCha8Rng`, and no
  behaviour may depend on `HashMap`/`HashSet` iteration order. This has already
  been violated once (`fog::recompute` emitted spotting events in set order);
  it was invisible until vision ranges grew. When iterating collections to
  produce events or AI decisions, sort first or iterate `state.units` in id
  order. The replay viewer depends entirely on this.
- **Content is data, not Rust.** Vehicles, weapons, terrain, doctrines, and
  maps live in `assets/mods/<mod>/`. The base game is a mod. Adding a tuning
  constant to Rust that a modder would want to change is a design smell.
- **Fog must not leak through the order system.** An order is never refused in
  a way that reveals an unspotted enemy — that is why `destination_blocked`
  treats unspotted enemies as passable and lets the move resolve as an ambush.
  `spotted_enemy_at` exists so callers do not reach for `unit_at` and leak.
- **Damage lands during a tick; death is reaped at the end of it.** That is what
  lets two crews kill each other simultaneously. Do not make `reap` eager.

## Style

Comments explain *why*, in prose, and are often several lines — match that
rather than trimming to terse one-liners. Public items carry doc comments.
Errors are `thiserror` enums. Optional mod-facing fields get `#[serde(default)]`
and enums get `#[serde(rename_all = "snake_case")]`. Tests are end-to-end
against the real `assets/mods` content and named as full sentences stating the
rule they defend (`unspotted_enemies_still_ambush`).

## Known issues

### Correctness

- **Army-contained unit placements are never validated.**
  `map.rs:312` passes `a.at` (the army's own hex) instead of `u.at` when
  checking each unit inside an `ArmyPlacement`, so a unit's own coordinates are
  neither validated nor used. `frontier.json` accordingly carries 14 `at`
  fields on army units that are leftover battle coordinates and mean nothing.
  Either drop the field from `ArmyPlacement`'s unit entries or start honouring
  it; right now it is misleading dead data that will bite whoever edits a
  campaign map next.
- ~~**`HexMap::center` is not a centroid.**~~ Fixed: it averages in floating
  point and rounds through `Hex::round`, which respects `x + y + z == 0`.
  This stopped being cosmetic the moment battle maps became hexagons — it is
  the rotation pivot for both map views, and `a_battle_map_is_one_overworld_tile`
  now asserts the centroid lands on the map.
- **Overworld elevation is priced at the battle scale.** `Scale` has one
  `elevation_meters`, so `frontier`'s mountains at elevation 2 read as 20 m.
  Harmless today — the overworld panel does not print elevation and only
  `max_climb` reads it — but a strategic map wants its own vertical scale, or
  its elevation digits want to mean something other than levels.
- **`raw_damage` has a `.max(1)` floor** (`combat.rs:217`), so any weapon that
  hits does at least 1 damage regardless of armour. An MG now fires 6 bursts a
  round, which means machine guns can grind down a heavy tank given time. This
  is the single most important thing for the ballistics rewrite to remove: a
  non-penetrating hit should do nothing, not a chip.

### Robustness

- **`spawn_unit` panics on unknown content.** `battle/mod.rs:252` expects the
  placement to have been validated, but `from_placements` — the path the
  overworld uses for field battles — never calls `validate_into`. A mod that
  removes a vehicle, or a loaded save referencing one, panics instead of
  erroring. Same shape at `game/src/overworld.rs:703`, where
  `choose_battle_map` expects the base mod to provide at least one battle map.
- **Elevation grids fail soft in a confusing way.** A missing or short
  `elevation` row silently defaults to 0 while a *mismatched* one only warns
  (`map.rs`), so a typo in a map file produces a playable but wrong battlefield
  rather than an error.

### Performance

- **MCTS is unusable at the new scale** — tracked in TODO.md under Bugs with the
  measurements and the three candidate fixes. Summary: ~35 s per unit order,
  the shipped `playthrough` example no longer completes, gameplay is unaffected
  because the scenario uses the `utility` planner.
- **`fog::recompute` rebuilds every side's vision from scratch after every
  single shot** (`combat.rs`, in `fire_at_unit` and `fire_at_tile`). At 20-hex
  vision that is ~1300 line-of-sight walks per unit per call, and a round has
  dozens of shots. Batching to once per tick would cost only that a unit
  revealed by firing becomes visible at end of tick rather than instantly. This
  is the largest single cost in round resolution and therefore in AI search.
- **`reachable()` is O(hexes × units) twice over** — tracked in TODO.md under
  Misc (the occupancy-index item). Measured at ~39 µs per call on the 768-tile
  map, which is fine in isolation and not fine inside a search that calls it
  thousands of times.

### Content gaps the scale decision exposes

- **Every overworld battle is fought on `river_crossing`.**
  `choose_battle_map` (`game/src/overworld.rs:693`) looks for a map named
  `battle_<terrain>` and otherwise returns the first battle map in the
  registry — and the base mod ships exactly one. With "1 overworld hex = 1
  battle map" now enforced as a shape, this wants a `battle_plains`,
  `battle_forest`, `battle_city` and so on, each the radius-20 hexagon (41
  across, 1261 tiles). `validate-mods` rejects any that are not, so the
  sizing cannot drift the way it did as a rectangle.
- ~~**`frontier` was not checked against the 4 km hex.**~~ Checked, and it
  holds without changes. It is 14 × 9 hexes = 56 × 36 km, and an army with
  `movement: 4` covers 16 km per turn. The turn length is now stated:
  `overworld_turn_hours: 24`, i.e. a day — not the half-day this note
  guessed, because the campaign layer had already committed to a day in its
  own UI (the banner counts "Day N", tile income is quoted `/day`) and 16 km
  is an ordinary *sustained* advance for an armoured formation even though
  the same tanks do 30 km/h in a battle. An operational turn is mostly not
  spent driving.
- `deploy` (`game/src/battle.rs:356`) sorts deployable tiles by depth from the
  map edge, so it scaled to the larger map with no changes. Worth knowing it is
  already scale-independent before anyone "fixes" it. On a hexagon the lowest
  columns exist only in the middle rows, so a side now deploys out of the
  hexagon's west or east vertex and fans inland — a column, which is what the
  ambush case in TODO wants anyway, but it is a shape change rather than a bug.

### Hygiene

- Clippy is not clean: 6 warnings in `tactics_core`, 26 in the game crate. Most
  are mechanical (`collapsible_if`, `manual_range_contains`, elided lifetimes,
  `--fix` handles them). Two are worth doing by hand: 11 instances of
  `type_complexity` in the game crate asking for `type` aliases, and a spread of
  `too_many_arguments` up to 11/7 — the worst offenders are the combat
  functions, which are threading `registry, state, attacker, from, weapon,
  target, blind, …` and would read better as a small `Shot` struct. Doing that
  before the ballistics work would pay for itself, since penetration adds more
  parameters to exactly those signatures.
- `crates/game/src/battle.rs` is ~1500 lines and `overworld.rs` ~1060. Not
  urgent, but they are the two files that will absorb the prep phase, objectives
  and menus work, and they are already the hardest to navigate.
