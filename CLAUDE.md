# CLAUDE.md

Engineering notes for this repo: how it fits together, the invariants worth
protecting, and the known defects. [DIRECTION.md](DIRECTION.md) is the current
design argument — why the next few chunks are being built at all — and carries
the live state of that plan. Gameplay and design work lives in
[TODO.md](TODO.md) — this file is for things that are wrong or fragile in the
code rather than things not yet built. Where an item is already tracked in
TODO.md it is cross-referenced, not repeated. Finished work and the reasoning
behind it lives in [DONE.md](DONE.md); read it before undoing a decision that
looks arbitrary.

**Start here:** `.claude/skills/tactics-dev/SKILL.md` is the working guide —
the instruments this project has for answering questions about itself, the
change loop, and the specific ways it has fooled people before. This file
covers what the code *is*; that one covers how to work on it.

## Commands

```sh
cargo run -p stahlsenshamädchen     # the game (starts on the overworld)
cargo run --bin validate-mods       # validate assets/mods; prints the scale table
cargo test -p tactics_core          # headless engine tests (the real suite)
cargo run -p tactics_core --example playthrough [seed]   # narrated AI battle
cargo run --release -p tactics_core --example perf       # hot-path timings
cargo run --release -p tactics_core --example balance    # what the data does
cargo run --release -p tactics_core --example balance -- --sim   # ...fought out
cargo run --release -p tactics_core --example balance -- --sim --points 100  # richer armies
```

`balance` is the content-iteration loop, and it is built around the kill chain
rather than around damage. The analytic pass is instant and answers "what did
that number just do" by standing two vehicles on an empty field and asking the
*real* combat code — `preview_attack` with the round under test forced into the
racks through `set_loadout`, not a reimplemented formula, so the report cannot
drift from the game. It prints P(pen) per gun × round × target × range band,
expected shots to knock out, shell flight times, and a "worth a look" section
that judges a gun on blast overmatch as well as penetration. `--sim` fights
whole battles and adds what killed them (brewed / wrecked / abandoned / crew
out), what it cost the girls, the ammunition economy, artillery's hit rate on
occupied ground, the delegation tax, the mustered-forces table and the
skill-gap table.

**Mustered forces is the only table that does not fight a given order of
battle.** `tactics_core::force::muster` hands each doctrine the same
requisition budget (`--points`, default 60) and lets it buy its own army:
roles are recognised off the chassis — indirect weapon, on foot, carries
somebody, sees farther than it shoots, otherwise armour — and each is wanted
in proportion to the doctrine weight that already names that appetite, with
`concentration` deciding how repeatable a buy is. It deliberately **pays the
asking price rather than hunting for value per point**, because dividing
appetite by cost makes every doctrine buy a swarm of the cheapest chassis and
hides the thing the table is for: a doctrine that wins reliably at equal
points is one whose preferred hardware is underpriced. Its first verdict was
blunt — massed armour's three heavy vehicles beat elastic defence's seven
mixed ones 34–1 while losing 2.4 points a battle.

It earns its keep immediately. It reports that the 105 mm shell's blast
overmatches even the Löwe's thinnest plate, so a direct hit wrecks a heavy tank
without consulting the penetration gate at all; that half of all deaths are
ammunition fires; and that a difficulty-5 side still does not beat a
difficulty-1 one cleanly.

### Objectives

A battle map may declare `objectives` — named sets of hexes, each either ground
to `hold` or an `exit` to leave by — and optionally a `victory_score`. The
rules are small and the consequences are not:

- **Objectives are map data, not battle state.** They live on `HexMap`
  (`map.objectives()`), because which hexes are the bridge cannot change during
  a fight. The battle carries only `objective_held` (parallel to that list) and
  `score` (by side). That split is why both setup paths — `from_map` and the
  overworld's `from_placements` — get objectives with no new arguments.
- **Control persists and contest cancels.** Standing on a hex takes it; driving
  away does not give it back; two sides on it makes it nobody's. Points are
  paid once at the end of a round, never per tick, or the size of every score
  would be an accident of `ticks_per_round`.
- **`EndReason::Stalemate` no longer implies a draw.** Losing contact ends the
  shooting; `BattleState::leader()` then says who won it. `winner: None` now
  means the score was actually level. Anything reading a battle result must
  handle `(Some(winner), Stalemate)`.
- **An exit is not ground, and leaving is not dying.** A vehicle that reaches
  an `exit` it is entitled to use (`side` restricts it — an exit anyone may
  take is a lane both armies take on round one) leaves the board: `alive` goes
  false and `exited` goes true. **Never classify a unit at the end of a battle
  by `!alive`.** Use `surviving_units()` and `lost_units()`; `alive` means "on
  the battlefield", which is what targeting, movement and fog want, and reading
  it for "did she come home" records a successful withdrawal as a dead crew.
- **`check_victory` reads the score before the board.** A withdrawing force
  reaches its target on the same tick its last vehicle drives off; checking
  elimination first would hand that battle to whoever was still standing.
- **The AI will not run for an exit unless it is losing.** The pull is gated on
  the doctrine's `withdraw_threshold` against the vehicle's own damage, so an
  intact crew scores every exit at zero. Without that gate the lane is a free
  win — every unit drives off on the first round. Whether a unit is *permitted*
  to leave is a chain-of-command question and deliberately not the evaluator's.
- **A map that declares no objectives behaves exactly as before**, including in
  the evaluator — `a_map_that_names_no_objectives_is_fought_exactly_as_it_was_before`
  pins that by requiring two doctrines with wildly different `objective_value`
  to score every tile identically. This is the same additivity rule that
  difficulty-as-a-mod imposes, and it is the one to re-check when touching
  `Evaluator::objective_value`.

The reason they exist is not scenario variety, it is that the AI had no reason
to advance: with elimination as the only victory condition, holding the best
cover on the map is optimal play, and the stalemate rate *rose* as difficulty
noise fell (11/12 at zero noise). See TODO.md under Design Decisions.

### Infantry, passengers and concealment

Infantry are a `VehicleDef` like everything else — `MovementClass::Foot`,
armour 0/0/0, leadership seats in the ordinary crew model and the rest of the
platoon abstracted into a `ModuleEffect::Troops` module. The design record is
`assets/wiki/reference/infantry.md`; what follows is only the parts that will
bite someone editing the code.

- **A passenger is `alive` but not on the field.** She has no independent
  position (hers mirrors the carrier's), she is invisible to spotting, and
  `unit_at` filters her out. That filter is the same shape as the exit rule
  above and has the same trap: **never decide "is anybody here" by scanning
  `units` yourself.** Go through `unit_at` / `spotted_enemy_at`, or a stack of
  a carrier and her platoon reads as two occupants of one hex and blocks
  movement onto a hex that is not full. `BattleState::passengers` is the
  reverse lookup, and it is the only correct way to ask what a carrier is
  carrying.
- **Shared fate is not optional.** A penetration into a loaded carrier rolls
  every passenger's girls and troops into the same interior pool, and a
  brew-up burns them. `effect_rolls` is where that happens, and it is shared
  with the plate-zero splash path — change one and you have changed both.
- **Plate zero is carved out of the overpressure overmatch rule.** Blast ≥
  twice the struck plate wrecks a vehicle, and against armour 0 that would
  mean one shell in a neighbouring hex deletes a dispersed platoon. For soft
  targets splash converts to casualty rolls instead. Artillery against
  infantry is attrition, brutal but never a single-event erasure.
- **The troops module means three things at once** and they are easy to
  separate by accident: interior weight (casualty rolls find the sections far
  more often than the two girls, which is the whole of the "leaders last"
  model), firepower (`mustered` scales every weapon's damage by hits
  remaining over toughness), and combat effectiveness (at zero the platoon is
  a remnant, alive and pulled hard toward withdrawal).
- **`concealment` scales the *spotter's* range, not the target's.** It is a
  per-target effective range inside the spotting pass, doubled when the
  target stands in cover ≥ 30, and bypassed entirely once she fires
  (`reveal_to_all` is untouched — an ambush is spent by springing it). The
  tile-vision cache is not involved and must not learn about it: that cache
  is a pure function of the map, which is what makes it exact rather than an
  approximation.
- **Ambush discipline applies to every unit, not just infantry.**
  `AMBUSH_PATIENCE` in `combat.rs` holds an *unseen* crew's opportunity fire
  below a quarter of the target's remaining substance — let them close. An
  ordered shot is exempt: discipline is about what a crew does on its own
  initiative. Leader-assignable postures are the designed future (TODO, under
  Chain of Command).
- **The AI reads what a formation is made of, and never what it is called.**
  `lays_indirect`, `goes_on_foot` and the balance instrument's taxi/foot
  columns all recognise a role off the hardware, so a mod that adds a mortar
  section or a paratroop platoon gets the behaviour on the day it is written.
  Follow that pattern rather than adding a `role` field; a chassis id in an
  `if` inside `ai/` is the smell.
- **A taxi run is two halves and the AI plans both.** The fare mounts when
  riding beats walking (rounds to cover the journey on foot against rounds
  to reach the tailgate, be driven, and get out, plus `BOARDING_ROUNDS`);
  the carrier drives to the pickup and holds the door while anybody has
  `boarding == Some(her)`. Without the driver's half a platoon at one hex a
  round never catches a carrier at six, so do not remove it as redundant.
  `BOARDING_ROUNDS` is 4 rather than the mechanical 2 because the cheap
  price let a delivered platoon re-board for a three-hex hop and thrash
  against the at-the-objective dismount reflex — see the constant's comment
  before retuning it.
- **What still is not planned is where an emptied carrier goes.** A mission
  belongs to a formation, so the taxi holds the ground her passengers were
  sent to hold, and roughly 21 of 24 die doing it whether the side fights
  flat or under command. That is per-unit tasking, tracked in TODO beside
  the assignable postures. A drop-off *short* of the objective was tried as
  a cheaper substitute and measurably lost platoons; the reason is in the
  passenger branch of `ai/utility.rs`.

### Latitude: an order a crew may not set aside

`Latitude` (`battle/command.rs`) is the per-unit twin of the
`Advance`/`Assault` distinction: `Delegated` is every order this engine has
ever had — she marches, and breaks off for cover under fire she has had time
to take in — and `Binding` is "I mean it", which the battle drill does not
preempt. The player says it with `X` on a selected crew, the same key that
orders a formation to assault.

- **It is read in exactly one place**, the drill gate in `ai/command.rs`. If a
  second `yields_to_drill()` appears, the model has drifted: latitude buys an
  order priority over the crew's *own judgment*, never over her nerve. Morale
  still refuses, `obeys()` is untouched, and a Binding order to a crew who has
  stopped listening is still not carried out.
- **It belongs to the destination, not to the girl.** Set only where
  `tasking` is set, cleared everywhere `tasking` clears (recall, arrival, a
  fresh formation mission), and carried in `WaitingOrders` so an order held at
  the radio arrives meaning what it meant. A radioed order with `to: None` says
  nothing about the march and must leave latitude alone —
  `an_order_about_her_gun_says_nothing_about_her_march` pins that.
- **`Delegated` is the default everywhere and the AI never issues `Binding`**,
  which is what keeps the determinism baseline valid across this change. If
  `event_stream.txt` moves when you touch latitude, something has leaked into
  AI-vs-AI play; do not regenerate it.
- **The drill can only preempt from round two.** `radio()` marches her itself
  the moment the order lands, so on the round she is ordered she is already
  planned and no planner is consulted. Any test of the drill-versus-order
  question has to fight a round first — this cost three test drafts, and the
  reasoning is in
  `a_binding_march_presses_on_where_an_ordinary_one_takes_cover`.
- **A deviation must announce itself.** `AiPlanner::last_was_drill` and
  `Decision::drill` exist so the presentation layer knows which orders were the
  planner's own idea. The game crate used to guess (keep only units whose
  formation had no mission) and thereby filtered out the single most important
  case — a personal march broken off for cover — leaving it silent. A vehicle
  that moves with no visible order behind it is indistinguishable from a bug;
  that is the bargain, and it is the whole reason the flag is plumbed rather
  than inferred.

### Saving

`tactics_core::save` serialises a game in progress; F5/F9 on the campaign map
drive it. The property that matters is not that the fields round-trip but that
**the future does**: `tests/save.rs` forks a battle in progress, sends one copy
through a save file, and requires both to produce the same events for the rest
of the fight. That is why the rng's stream position is saved rather than its
seed.

Two structures are `#[serde(skip)]` because they are caches: `SightGrid` and
the per-unit vision inside `FogMap`. They are pure functions of the map, and
the sight grid alone would be a thousand entries per save. The price is that
`save::rehydrate` *must* rebuild them — an empty sight grid answers every
line-of-sight question wrongly rather than loudly, and an empty `visible_key`
panics because recompute indexes it by side.

### Seeing the game without playing it

The presentation layer used to be checkable only by running the game and
looking at it, which made every rendering and UI change unreviewable by anyone
not sitting at the keyboard. `crates/game/src/devtools.rs` fixes that: a script
of timed actions drives the real input path and captures the window along the
way.

```sh
STAHL_DEBUG=1 STAHL_BATTLE=river_crossing \
  STAHL_SCRIPT=scripts/dev/battle-tour.txt cargo run -p stahlsenshamädchen
```

On this project's Linux box the game needs `STAHL_PRESENT=immediate` or it
loses the GPU a few seconds in — an NVIDIA Vulkan driver bug, not ours, proved
with `cargo run -p stahlsenshamädchen --example minimal_window` (a stock Bevy
window that reproduces it with no game code). See DONE.md.

`scripts/dev/` holds a tour of each screen; the module doc lists every action.
Two things about it are load-bearing:

- **The scripted cursor is a resource, not the window's.** Writing to
  `Window::cursor_position` makes `bevy_winit` warp the real OS pointer, which
  fights the user for their mouse and fails silently when unfocused or on
  Wayland. `map_render::View` consults `ScriptedCursor` first instead.
  Scripts therefore name a **hex**, which also makes them independent of zoom,
  pan, window size and view rotation.
- **`run_script` must stay `.after(InputSystems)`.** Bevy clears `just_pressed`
  at the top of `PreUpdate`, so a press injected before that is wiped before
  any handler sees it — the symptom is a click that silently selects nothing.
- **Scripts wait on the game, not on a stopwatch.** `until <predicate>` and
  `expect <predicate>` read [`ScriptFacts`], which whichever screen is on
  publishes for them; a failed `expect`, a timed-out `until` or a degenerate
  screenshot makes the process exit nonzero, so a tour is a test. Prefer
  `until idle` over `wait N` for anything that waits on the simulation — a
  `wait` that guessed short photographs a half-played round and says nothing
  about it.
- **`idle` is `Battle::listening`, and both must stay one predicate.** It
  means "a keystroke would be acted on this frame", which is not the same as
  "the phase is planning": sprites finishing a walk hold the keyboard, and a
  side that has committed is done talking. When those drifted apart, `until
  idle` came true a frame early, four `key Enter` presses advanced the battle
  by one round, and every screenshot after them described the wrong turn
  while the script reported success. Anything new that makes `handle_input`
  refuse a keystroke belongs inside `listening`, not beside it.

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
- **`alive` and "standing on a hex" are two different questions.** `alive`
  goes false when she is destroyed *and* when she drives off by an exit, so
  classify outcomes with `surviving_units()` / `lost_units()` or a successful
  withdrawal is recorded as a dead crew. A passenger is the mirror case: she
  stays `alive` and her `pos` mirrors her carrier's, so she is on no hex that
  anybody may interact with — `unit_at` filters `aboard.is_none()` in one
  place precisely so every occupancy, targeting and collision read in the
  game inherits it. Never reimplement either check by scanning `units`.

## Style

Comments explain *why*, in prose, and are often several lines — match that
rather than trimming to terse one-liners. Public items carry doc comments.
Errors are `thiserror` enums. Optional mod-facing fields get `#[serde(default)]`
and enums get `#[serde(rename_all = "snake_case")]`. Tests are end-to-end
against the real `assets/mods` content and named as full sentences stating the
rule they defend (`unspotted_enemies_still_ambush`).

## Known issues

### Correctness

- ~~**Difficulty is inverted in practice: less noise plays worse.**~~
  Fixed, in two halves, and the fix is worth understanding before touching
  the utility planner's tile choice. The pathology: the greedy argmax
  broke score ties toward the first tile of a fixed (x, y) sweep, so on
  the broad plateaus open ground scores in, every identical unit drove to
  the SAME corner of every plateau — a noiseless side clumped, queued,
  and lost to anyone scattered by randomness (difficulty 5 lost to
  difficulty 1 in both orientations). The cure is deterministic, not more
  noise: among tiles within `PLATEAU` (0.3) of the best score, take the
  one nearest the unit's own position. Units standing apart stay apart
  when the ground between is all the same, and nobody burns movement
  crossing a plateau to park on identical grass. The second half was the
  instrument itself: the skill-gap table fought on `river_crossing`,
  whose sides field different vehicles, so it measured the map — it now
  fights on a mirrored arena inside `balance --sim`. Measured after: 5v1
  wins 29–7 / 28–8 across orientations at 1:1.9–2.1 exchange (B4's
  written success metric was "most battles at visibly better than 1:2"),
  equal-skill pairings sit at parity, and the deterministic 36–0 sweep is
  gone. Residual, tracked: side B retains a modest edge on the mirrored
  arena (33–3 vs 23–13 in the 5-vs-3 orientations), suspected
  resolution-order artifact worth a look in B4's remainder.
- **Army-contained unit placements are never validated.**
  `map.rs:962` passes `a.at` (the army's own hex) instead of `u.at` when
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
- ~~**`raw_damage` has a `.max(1)` floor**~~ Fixed by the ballistics rewrite's
  B1 chunk: `raw_damage` is gone, a hit that does not penetrate does nothing
  structural, and the instrument that used to print "mg kills heavy_tank in 3
  rounds" now prints "mg cannot meaningfully hurt heavy_tank".

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

- ~~**`fog::recompute` rebuilds every side's vision from scratch after every
  single shot.**~~ Fixed, and without the batching trade-off the note here
  proposed — a firing unit is still revealed instantly. Three things did it,
  none of which changes what the fog *says*:
  - `SightGrid` resolves every tile's sight heights once. `los_clear` was
    doing a `String`-keyed `registry.terrain()` lookup per ray step, on the
    order of a million times a round, for an answer that cannot change
    because no battle alters its own terrain. Shared via `BattleState::sight`
    behind an `Arc` so search branching stays cheap.
  - Vision is cached per unit against `(pos, range)`. Since the map is
    immutable for the whole battle, that cache is not an approximation of the
    answer, it *is* the answer.
  - A side whose whole `(unit, pos, range)` list is unchanged skips the union
    outright, which is what makes the after-every-shot recompute nearly free:
    firing moves nobody.

  Round resolution went 14.05 → 1.02 ms/round and the engine suite 25.6 →
  2.6 s, with the event stream and final fog state **bit-identical** across
  four seeds. When touching this, keep the reference `los_clear` and
  `SightGrid::clear` sharing `sight_line_clear` so the fast path cannot drift,
  and keep `cached_vision_is_the_same_answer_as_computing_it_fresh` passing —
  it rebuilds every side's visible set from scratch and demands a match.

  That "bit-identical across four seeds" check is no longer done by hand:
  `tests/determinism.rs` records the event stream for the same four seeds into
  `tests/snapshots/event_stream.txt` and compares byte for byte. It exists
  because a self-consistency test cannot catch iteration-order bugs — a process
  agrees with itself whatever its hash seed is — which is precisely how the
  original `fog::recompute` ordering bug survived. Regenerate deliberately with
  `UPDATE_SNAPSHOTS=1 cargo test -p tactics_core --test determinism`, and read
  the diff first: a broad diff means you changed balance, a small one about the
  *order* of otherwise identical events means you introduced the bug this test
  is for.
- **The numbers below are reproducible.** `cargo run --release -p tactics_core
  --example perf` prints them; `--mcts` adds the slow ones. Measured on the
  1261-tile `river_crossing` with 8 units, four seeds:

  | | |
  | --- | --- |
  | round resolution | 1.07 ms (0.79–1.43 across seeds) |
  | `reachable()` per call | 24.6 µs |
  | `unit_vision` per unit, cold | 74.9 µs |
  | utility order | 0.05 ms |
  | mcts order, difficulty 3 / 4 | 1.84 s / 4.24 s |

  Utility order was 0.03 ms until the evaluator started pricing danger as a
  fraction of what a crew can absorb, which walks the unit list once more per
  candidate tile. Anything added to `score_tile` is paid for at that rate —
  it is the hottest function the AI has.

  Run it `--release` or the figures are meaningless. Note this supersedes the
  "~39 µs per call on the 768-tile map" figure that used to appear below: that
  map stopped existing when battle maps became the radius-20 hexagon.
- **MCTS is expensive but no longer impossible.** Was ~35 s per unit order and
  could not finish a round; the fog work brought it to ~3.3 s per order, and
  `cargo run --release -p tactics_core --example playthrough` now plays a full
  32-round battle in ~50 s. Still far too slow to plan a human's turn against,
  so the shipped scenario still names `utility`. The remaining cost is
  structural and unchanged: 900 iterations rolling out to depth 20, with every
  fifth step a `Commit` that runs the enemy's whole planning pass.
- **`reachable()` is O(hexes × units) twice over** — tracked in TODO.md under
  Misc (the occupancy-index item). 32.8 µs per call, which is fine in isolation
  and not fine inside a search that calls it thousands of times.

### Content gaps the scale decision exposes

- ~~**Every overworld battle is fought on `river_crossing`.**~~ Half fixed.
  `choose_battle_map` (`game/src/overworld.rs:870`) looks for a map named
  `battle_<terrain>` and otherwise returns the first battle map in the
  registry, and the base mod shipped exactly one. It now ships three —
  `river_crossing`, `battle_plains`, `battle_forest`, each the radius-20
  hexagon, and `balance --sim` samples all of them so every map added is free
  balance signal. What is still missing is *coverage*: the terrain ids the
  overworld actually uses have no `battle_` map of their own except plains
  and forest, so a fight on a mountain or in a town still falls through to
  whichever map iterates first. Adding `battle_city`, `battle_hills` and so
  on is content work with no engine question left in it.
  **`river_crossing` is the determinism baseline's ground and deliberately
  fields no infantry** — putting a platoon on it means regenerating the
  snapshot and losing the check that says a chunk changed no rules.
- ~~**`frontier` was not checked against the 4 km hex.**~~ Checked, and it
  holds without changes. It is 14 × 9 hexes = 56 × 36 km, and an army with
  `movement: 4` covers 16 km per turn. The turn length is now stated:
  `overworld_turn_hours: 24`, i.e. a day — not the half-day this note
  guessed, because the campaign layer had already committed to a day in its
  own UI (the banner counts "Day N", tile income is quoted `/day`) and 16 km
  is an ordinary *sustained* advance for an armoured formation even though
  the same tanks do 30 km/h in a battle. An operational turn is mostly not
  spent driving.
- `deploy` (`game/src/battle.rs:761`) sorts deployable tiles by depth from the
  map edge, so it scaled to the larger map with no changes. Worth knowing it is
  already scale-independent before anyone "fixes" it. On a hexagon the lowest
  columns exist only in the middle rows, so a side now deploys out of the
  hexagon's west or east vertex and fans inland — a column, which is what the
  ambush case in TODO wants anyway, but it is a shape change rather than a bug.

### Hygiene

- **The tree is clean and CI enforces it.** `cargo clippy --workspace
  --all-targets -- -D warnings` gates, and it denies rustc's own lints too (an
  unused import fails the build). `cargo fmt --check` gates alongside it, with
  `style_edition = "2024"` pinned in `rustfmt.toml` so a toolchain upgrade
  cannot turn CI red on its own. Run both before pushing; `cargo fmt` fixes the
  second automatically. Keep it this way. What
  the cleanup produced is worth knowing, because the shapes it introduced are
  the ones to reach for next time:
  - `map_render::View` bundles rotation, centre, window, camera and the dev
    harness's scripted cursor. Those five were threaded separately through nine
    systems; they are all answers to "how are we looking at the world", so a
    system now asks for `view` and calls `view.hovered(&map)` or
    `view.face_at(&map, hex)` instead of re-deriving the projection.
  - `map_render::TextSlot` / `MarkerQuery` and `battle::BattleHud` name the
    query types whose `Without` filters exist only so Bevy can prove two
    `&mut` queries do not alias.
  - `MapFile::validate`'s placement checker is a `PlacementCheck` struct rather
    than an eight-argument nested fn.
  - Two `#[allow(clippy::too_many_arguments)]` remain, on `battle::pump_events`
    and `overworld::enter_overworld`. A Bevy system's parameters are its
    dependency list, and those two do not decompose into any smaller noun;
    inventing a struct to satisfy the lint would make them worse. Both carry a
    comment saying so.

  Note the previous version of this note claimed the worst `too_many_arguments`
  offenders were the combat functions and proposed a `Shot` struct. That was
  wrong: `combat.rs` never tripped the lint. The offenders were all Bevy
  systems in the game crate. A `Shot` struct may still be a good idea when
  penetration adds parameters to `hit_chance`/`raw_damage`, but it is a
  readability choice, not a lint fix.
- `crates/game/src/battle.rs` is ~1500 lines and `overworld.rs` ~1060. Not
  urgent, but they are the two files that will absorb the prep phase, objectives
  and menus work, and they are already the hardest to navigate.
