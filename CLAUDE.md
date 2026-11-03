# CLAUDE.md

Engineering notes: how the code fits together, the rules an editor can break
by accident, and the known defects. This file states rules; the evidence, the
measurements and the wrong first drafts are elsewhere:

- [DONE.md](DONE.md) — finished work and why it is shaped that way. **Read it
  before undoing a decision that looks arbitrary.**
- [TODO.md](TODO.md) — gameplay and design work not yet built.
- [STRUCTURE.md](STRUCTURE.md) — seams in the wrong place (a rule in the wrong
  crate, a contract nothing checks).
- [PARKED.md](PARKED.md) — why code with no callers is still in the tree.
- Design memos: [SLICE.md](SLICE.md) (live plan: the three-battle slice,
  the cadet card, legibility from ASL), [DIRECTION.md](DIRECTION.md)
  (the August argument it builds on),
  [PLANNING.md](PLANNING.md) (AI planning layer), [WORLD.md](WORLD.md) (one
  world at two resolutions), [ASSAULT.md](ASSAULT.md) (attack and defence),
  REIMAGINED.md (learned goals / RL).
- `assets/wiki/reference/` — design records (detection, ballistics, infantry,
  battlefields, cadets).

**Start here:** `.claude/skills/tactics-dev/SKILL.md` is the working guide —
the instruments, the change loop, and the traps this repo has sprung before.

## Commands

```sh
cargo run -p stahlsenshamädchen      # the game (generated campaign; STAHL_CAMPAIGN=frontier for the drawn one)
cargo run --bin validate-mods        # validate assets/mods; prints the scale table
cargo test -p tactics_core           # headless engine tests (the real suite)
cargo run -p tactics_core --example playthrough [seed]           # narrated AI battle
cargo run --release -p tactics_core --example perf               # hot-path timings
cargo run --release -p tactics_core --example balance -- --help  # the balance harness: every flag
cargo run --release -p tactics_core --example campaign [seed [to]]  # whole AI-vs-AI campaigns (--map frontier)
cargo run --release -p tactics_core --example worldgen [seed]    # a generated world, drawn
cargo run --release -p tactics_core --example command -- start <map> <seed> <dir>  # the player's seat, headless (then plan/turn <dir> orders.json)
cargo run --release -p tactics_core --example heatmap -- --map <map> --games 16      # where a map's battles happen (SVG); --help in its doc
cargo run --release -p tactics_core --example heatmap -- --sight --map <map>         # a map's sight lines, before anybody fights on it
cargo run --release -p tactics_core --example mirror -- --arena ridge_arena  # does a rule read the compass?
cargo run --release -p tactics_core --example prices -- --worlds 4096        # what each chassis is worth
cargo run --release -p tactics_core --example goal_dataset -- --games 1024   # utility AI goals as training data
(cd training && uv run imitate.py ../target/goal_dataset/train.jsonl --out ../target/goal_net.json)
(cd training && uv run loop.py --start ../target/goal_net.json --iterations 40)  # RL: rollouts in Rust, PPO in JAX
```

In the game's generated campaign: Space runs the clock, `[` `]` its speed,
Enter to dawn, Z zooms onto the ground.

### Reading a balance number

`balance` asks the real combat code (`preview_attack`), never a
reimplemented formula; `--sim` fights whole battles.

- `--set path=value` / `--sweep path=a,b,c` address any mod field by its json
  path (`balance.partial_penetration_percent`, `weapon.howitzer_105.dispersion`,
  `planner.horizon_rounds`) through a serde round trip. Sweeping must stay
  equivalent to editing `mod.json`; that is tested.
- Three axes are not fields: `mods=`, `points=`, and `seed=`. **Always sweep
  `seed=` beside a difference** — every win table prints its noise band and a
  sweep prints its `spread`; read differences against them. At 36 battles a
  level pairing wanders from 12–24 to 24–12.
- No printed number may move with `--jobs`: `Tally::merge` fields must be
  associative (`tests/harness.rs`).
- Every fighting table goes through `Grid` / `fought_grids`, shared by the
  single run and the sweep. `--only <tables>` selects; `--csv` emits all.
- `--seed` is an offset; zero is the sample every quoted number used.
- `--arena skill_arena` (default, the historical baseline) or `ridge_arena`
  (where a wrong choice is punished). **Read skill/difficulty on the ridge,
  seed-swept.**
- `river_crossing`'s armies are badly unbalanced (24/76) and it is a fifth of
  the `--sim` sample. `battle_hills`/`battle_town` are mirror-symmetric, but
  their `ground` rows are noisy (spread ~12 of 72).
- `seats`, `attrition` and `mustered` have specific readings; see DONE.md.
  `mustered` pays the asking price on purpose: a doctrine that wins at equal
  points has underpriced hardware.

## Invariants worth protecting

- **`tactics_core` must not depend on Bevy.** Presentation, input and asset
  loading live in `crates/game`.
- **The simulation is deterministic given a seed.** `ChaCha8Rng`; no
  behaviour may depend on `HashMap`/`HashSet` iteration order — sort first or
  walk `state.units` in id order. `tests/determinism.rs` pins four seeds'
  event streams. Regenerate with `UPDATE_SNAPSHOTS=1` only after reading the
  diff: broad = you changed balance; small and about *ordering* = you
  introduced the bug the test exists for.
- **Content is data, not Rust.** The base game is a mod. A tuning constant in
  Rust a modder would want to change is a smell.
- **Difficulty is a mod.** Every harsh system is an additive rule whose
  absence is the gentle game; switching one off must not need an `if` in
  Rust. The determinism snapshot passing *unregenerated* is the evidence a
  chunk added a rule without changing one.
- **Fog must not leak through the order system.** An order is never refused
  in a way that reveals an unspotted enemy; use `spotted_enemy_at`, not
  `unit_at`.
- **Damage lands during a tick; death is reaped at the end.** Do not make
  `reap` eager.
- **A tiebreak may only read quantities a reflection preserves** (distances,
  terrain costs, dot products of differences). A coordinate does not; where a
  total order is needed, put the coordinate **last**. A greedy walk over a
  ranked list is a tiebreak too — take a tie group whole. Guarded by
  `a_march_is_the_same_march_from_either_end`,
  `a_reflection_leaves_a_bearing_alone_and_turns_a_coordinate_around`,
  `a_reflection_leaves_a_sight_line_alone`. `examples/mirror` finds breaches.
- **`Fate` separates "alive" from "standing on a hex".** `Fighting { doom }`
  (only this is `alive()`), `Destroyed(Destruction)` (only this is `lost()`),
  `Exited`. Classify with `surviving_units()` / `lost_units()`. Two
  destructions in a tick resolve by `Destruction::supersedes` (burning > crew
  left > crushed). Ask occupancy through `unit_at` / `occupants`, **never by
  scanning `units`**.
- **A unit id is a name, not a position.** Find units by `lookup(id)` /
  `slot_of(id)`, never `units[id.index()]`. Ids rise in placement order.

## Rules that will bite an editor

Each is defended by a test or the snapshot; the history is in DONE.md or the
named memo.

### Scenarios, objectives and endings

- **Objectives are scenario data** (`map::Scenario`), not battle state and not
  ground; `HexMap` is tiles only. Setup takes a `Battlefield` and keeps `map`
  and `scenario` apart.
- Objective points are paid once at round end, never per tick. Control
  persists; contest cancels.
- **`EndReason::Stalemate` does not imply a draw** (`BattleState::leader()`),
  and `Broken` is a reason too. Handle `(Some(winner), Stalemate)`.
- An exit is not ground and leaving is not dying (`Fate::Exited`). The AI
  only runs for an exit when losing (`withdraw_threshold`).
- `check_victory` reads the score before the board.
- A map declaring no objectives fights exactly as before
  (`a_map_that_names_no_objectives_is_fought_exactly_as_it_was_before`).
- Elevation levels run `0`-`9` then `a`-`z`. A battle map with
  `"shape": "free"` is a scenario, exempt from the regulation hexagon.

### Assaults (ASSAULT.md, `tests/assault.rs`)

All additive: a scenario declaring none of it is the game before.

- **No clock.** An assault ends when the fighting stops (a side breaks, is
  gone, or the attack stalls); then the ground decides. Score decides nothing.
- Strength lost is substance over full complement; wrecks and exits both
  count. A side **breaks on its will** (`will_lost`: cut-off/unnerved crews
  count at the larger of two percentages, never the sum). Unnerved ships at
  zero for a reason (ASSAULT.md).
- The stalemate clock counts assault progress (`assault_progressed`,
  `Battle::approach`).
- `prepared_cover` is dug at setup and round digging never fills it in.
- Mines roll only in `tread`, never for the sowing side, through
  `combat::mine_blast`. Roads price only fields the side has **found**
  (`MineState::found`).
- `BattleState::sight` is the one door for how far a crew sees (visibility).
- Standing orders are set at setup (`give_standing_orders`), not on the net.
- Surprise reads `found_before` (snapshotted at tick start). A planner's
  expectation never reads surprise.
- A reserve's flag is *derived* (`Formation::in_reserve`); any order commits
  it. Platoon breaks are nerve, never an order.

### The campaign (`tests/campaign.rs`, `tests/field.rs`)

- **Victory is map data** (`map::CampaignVictory`): `hold` (terrain held at
  dawn for `hold_days` dawns) and `decapitation`, both additive.
  `defeated(side)` is the one test.
- **`headquarters` is one flag read twice** (signals root and decapitation).
  Do not add a second flag.
- **`tactics_core::field` is the seam between campaign and battle**
  (`battlefield_for`, `Clash::muster/problem/stage`, `FieldBattle::report`).
  Anything deciding what a clash is belongs there, or the headless campaign
  stops being the campaign.
- A withdrawn army arrives a hex back; placement goes through `place_army`.
- The player orders through the same door as the AI (`OverworldOrder`);
  refusals come from the engine, not the screen.
- A campaign that has ended goes no further, even mid-tick
  (`a_campaign_that_has_ended_goes_no_further`).

### The world (`tactics_core::world`, WORLD.md, `tests/world.rs`)

- **`World` is the one owner**: `World::insert` writes a tile and both grids
  (`SightGrid`, `MoveGrid`); nothing writes a grid alone.
- Sight goes through `World::sight_clear` (and in a battle
  `BattleState::sees_through`, which adds smoke). `World::presence` has three
  answers: `Known`, `Outside`, `Unloaded`; a debug build panics on a line
  through `Unloaded`.
- A campaign hex is a chunk (radius 20, `chunk_of`).
- **Residency is a pure function of where everybody stands**
  (`needed_chunks`, `settle_world`).
- An engagement's dice are `world::engagement_seed` (SplitMix64, never
  `std::hash`); the value is pinned.
- A generated world saves as how to regenerate it plus the edit overlay; a
  chunked world with no generator refuses to serialise.
- **One front**: every army in contact is a formation in one `BattleState`
  (`engagement::Front`); contacts join it, armies leave on their own.
- **The order of battle is the chain of command** (`OverworldState::elements`).
  `Column` is derived on read and never stored; change the tree. A survivor
  is matched back to her node (`set_survivors`), never replaced.
- A detachment is a node with orders of its own; no "detached" state exists.
- On a generated world armies march over tiles; `reachable` must never offer
  what the march cannot deliver that day (keep the heuristic admissible).

### World generation (`tactics_core::worldgen`, `tests/worldgen.rs`)

- **The tiles are the truth**; a campaign hex is `summary(chunk)`.
- The skeleton (rivers, towns, roads) is made once; `tile(hex)` is a pure
  function with no chunk boundary in it.
- Integer noise only (no trig); shares are quantiles of the world's own noise.
- Ties go to a seeded `lot`, never a coordinate.
- **Bump `GENERATOR_VERSION` with any change that moves a tile.**
- World settings are json patches on `worldgen` (`data::patch`); a default
  option sets nothing; validation walks every combination.
- Tours that are not about the setup screen declare `STAHL_WORLD=default` on
  their `#!env` line.

### Fog, detection and knowledge (`fog.rs`, `assets/wiki/reference/detection.md`)

- Geometry decides whether a look is possible; the roll decides how long it
  takes. A held contact is not re-rolled; one look per target per tick
  (`SideFog::searched`); the best-placed spotter rolls once.
- `detection_certain_percent: 100` throws no die
  (`a_mod_that_asks_for_no_search_spots_exactly_as_it_always_did`).
- Tests needing crews in plain sight use `seen(..)` from `tests/common`.
- **`BattleState::known_enemies(registry, who)` is the one answer** to who
  knows of whom. `Knower::Commander` sees the reported picture;
  `Knower::Crew` adds what she sees herself. With no `command` block both are
  the pooled fog. Engine rules (opportunity fire, detection) read
  `fog.spotted`; `known_enemies` is for *deciding*.
- Rank governs orders down; information flows every way. Ranks are data;
  nobody ranked is everybody equal. `operational_commands` is derived on
  every call.

### The shot (`assets/wiki/reference/ballistics.md`)

- `hit_chance` names a target by id and takes both ends as ground (`from`,
  `at`). `tgt.moved` and the loader's round choice deliberately read real
  positions.
- **`danger::fire_on` is the one answer** to "what could the enemy put on her
  there"; `incoming` and `fire_on` share `guns_bearing_on`.
- Armour follows the hull; a turret lets guns traverse without turning it.
  Non-turreted guns pivot (`pivot_ticks`); turrets traverse
  (`traverse_seconds`).
- `Unit.moved` is incremented at the one place a unit changes hex.
  `hexes_under_way` reads state, never the hypothetical `from`.
- Dispersion rolls `impact` at firing; `shell_lands` resolves against
  `impact`. `impact` is not `serde(default)`.
- `penetration_roll` returns a share; `ShotHit::damage` is what was spent.
- **`round_worth` is the only shot price**; `blast_worth` mirrors
  `overpressure` case for case.
- The stalemate clock resets on accomplishment, never on a bounce.
- Ambush patience applies to every unit; ordered shots are exempt.
- Strays: only a miss is a lottery (`Event::ShotStrayed`).
- Cover comes from the ground crossed (`combat::ground_between`), read by
  `hit_chance` and the fog's search alike.

### Fear, pinning and movement (`tests/fire_and_movement.rs`, `tests/flanks.rs`)

- **`MoraleRules::pressure_for` is the one price list**, used to charge fear
  and to expect it (`fear_is_priced_by_the_same_arithmetic_that_charges_it`).
- Suppression is on the round (`AmmoDef::suppression`) and the ladder
  (`MoraleRung::accuracy`, `pinned`) — never a second fear system.
- Near misses, being targeted, flank fire and open tops are percentages whose
  neutral value is the game before. Small arms close a tank's hatches
  (`buttoned_ticks`), they do not frighten her.
- `movement::Pinning` is the one gate on a pinned crew; `reachable`,
  `path_to` and the step all ask it.
- Route price follows the order (`balance.route_exposure`, per mission verb).
- Ties go to friends before the compass (`movement::apart`).
- Stages about something else neutralise noise with `common::seen`, `calm`,
  `laid`, `head_on`.

### One currency: the evaluator reads the resolver

`score_tile` (`ai/eval.rs`) turns rules into behaviour, in **worth per round**
(substance points, plus fear at `morale.point_worth`).

- **`combat::expected_shot` prices one shot**; `worth_of` applies the
  exchange rate in exactly one place.
- Cadence comes from `combat::crewed_reload`, for both the resolver and the
  evaluator. Ground is priced per round; a trigger pull per shot.
- The threat term is `incoming(..).worth`: no distance falloff, no range gate.
- The terrain prior pays only where no found gun reaches.
- **`planner.deviation_cost` must sit between the shipped doctrines'
  `initiative` values**; `the_shipped_doctrines_straddle_the_price_of_deviating`
  fails when a term grows. Expect to retune it.
- `battle::threats` / `threatened` are the one walk and gate. **The drill
  minimises fire** (`battle::drill_destination`, used by both drills); **the
  rout maximises distance** (`flight_destination`).
- Symmetric weights (`score_worth` and friends) change what a battle is like,
  not who wins; the skill table cannot see them.

### Goals and the AI's seams

- **`ai/goal.rs::candidates` is shared knowledge; `GoalChooser` is
  judgement.** A learned policy replaces only the chooser.
- A goal lives on the `Unit` (forked saves must have the same future).
- A mission *replaces* the candidate list; initiative widens it by exactly
  one entry.
- Candidates are ordered by distance, coordinate last; one crew per piece of
  ground (claims count like parked vehicles).
- Difficulty applies to the chooser, not the sweep. `difficulty_noise` is
  misjudging; `difficulty_foresight` is not reading. One blur per unit per
  round (`UtilityPlanner::lean`), zero at difficulty 5 exactly.
- `roads` has no occupancy; ground beyond the horizon is priced at the
  horizon. Route terms are tie-breaks, not vetoes.
- **`planner` vs `balance`**: `balance` is what is true on the battlefield
  (reaches a human's shot); `planner` only reaches AI choices. That question,
  not file location, decides which block a new number goes in. Every planner
  field defaults to the constant it replaced and has a test that it is read.
- A number that differs per doctrine belongs on the doctrine.
- The AI reads what a formation is made of, never its name; a chassis id in
  an `if` in `ai/` is the smell.

### Plans (PLANNING.md, `ai/plan.rs`, `tests/plans.rs`)

- Templates are data; their algorithm is Rust. One planner per operational
  command. Planning strength is the commander's stats.
- A plan lives on the battle (`CommandState::plans`) and is never recalled
  once the word is given.
- Playouts are fought on what the commander has been told
  (`plan::known_world`), score `position_value` with `over` cleared, and
  nobody plans inside one.
- No shipped doctrine teaches a play yet; the one play built is neutral.

### Learned goals (REIMAGINED.md, `tests/learning.rs`)

- **`ai::features::describe` is all the network sees**, for both dataset and
  play; it holds no coordinate and no side number
  (`a_crew_and_her_mirror_twin_are_described_identically`).
- A network carries its feature names and a probe; `load_goal_net` refuses a
  mismatch. Adding a feature retires every old network.
- Recording is a tap that draws no dice. Rewards live in `training/`.
- **Quote trained results only from `balance --brains` on seeds no rollout or
  evaluation used.**
- One road search per decision (`goal::Survey`); don't hold a survey across a
  state change.
- Goal interruption (`planner.goal_events`) and unlabelled places
  (`goal_places`, `approach_places`) ship off.

### Defiance, latitude and orders

- The rung decides *that* she defies; temperament decides *how*
  (`DefianceResponse`; ties to the earlier entry). A crew cannot refuse her
  own decision (`UnitIntent::own_idea`).
- Rallying reads sight to the leader, not the radio.
- **Latitude**: a crew's answers "will she break off for cover", read only in
  `Unit::yields_to_drill`; a formation's reaches only `mission_value`'s floor.
  It travels with the mission. **The AI never issues `Binding`** — if the
  snapshot moves when you touch latitude, something leaked.
- Orders are general: the commander names a job and ground; crews choose
  where to stand.
- A deviation must announce itself (`Decision::drill`).

### Crews, infantry and stacking

- A passenger is `alive` but not on the field; ask `passengers` /
  `occupants`. Shared fate in a penetrated carrier is not optional.
- The troops module is interior weight, firepower and effectiveness at once.
- `concealment` scales the spotter's range; `profile` is about being hit.
- Capacity is `Option<u32>`; `None` is not `1`. Crowding stops a halt, never
  a drive-through. `room_for` is fog-aware by design. Sharing a hex with an
  enemy is impossible.
- `crew_skill` reads station state: absent/out is nobody; wounded pays
  `substitution_penalty`, which is the whole cost of a short crew.
- `CrewLoss::in_battle` is the one reading of who a battle hurt. Aid only
  helps. Field repair at chance zero throws no die.
- Casualty rules are the `casualties` block; the engine defaults to nobody
  dying and the base mod opts in.
- One cadet, one seat. A battle never enlists anybody.

### Saving (`tests/save.rs`)

- The future must round-trip (a battle forked through a save plays the same).
- Caches are rebuilt, and that is a type: `SavedBattle::rehydrate(&registry)`
  destructures every field. `World` saves as its tiles under the key `map`.
- **`SAVE_VERSION` is 10; older saves are refused, not migrated.** Bump it
  whenever an old file would load wrongly rather than fail.

### Presentation layer (`crates/game`)

- `Event::heard_by` (core) is the one log filter and its match is exhaustive.
  `drive_ai` filters *before* queueing — planning events cost the player a
  beat.
- Command traffic and a hull's insides are her side's business; fighting
  events are side-blind; a spot belongs to the side that made it.
- The danger overlay is `fire_on_as(Commander)` and nothing else, cached under
  `range_dirty`, a gradient via `panel::danger_tint`.
- The academy roll is derived every frame; a page over the map stops the
  world.
- **Tours** (`scripts/dev/run-tours.sh`, `STAHL_HEADLESS=1` without a GPU)
  are the only test of the UI — not `cargo test`, not CI. Scripts wait on
  `ScriptFacts` predicates, never a stopwatch or a count. `run_script` stays
  `.after(InputSystems)`. Every screen assigns `ScriptFacts` through an
  exhaustive literal. This machine needs `STAHL_PRESENT=immediate`.
- "Does it hold a Bevy type" is the line for extracting from `battle.rs`.

## The scale contract

The `scale` block of `assets/mods/base/mod.json` (`data::Scale`,
`registry.scale`); a mod's block replaces it wholesale.

| Quantity | Field | Value |
| --- | --- | --- |
| Battle hex | `hex_meters` | 100 m |
| Round | `round_seconds` | 60 s |
| Tick | `round_seconds / ticks_per_round` | 5 s (12 ticks) |
| Elevation level | `elevation_meters` | 10 m |
| Campaign elevation level | `overworld_elevation_meters` | 30 m |
| Overworld hex | `overworld_hex_meters` | 4 km = one battle map |
| Overworld turn | `overworld_turn_hours` | 24 h |

- A battle map is a hexagon of radius 20 (1261 tiles). A map declaring
  `elevation` must give every tile a level.
- There is no `TICKS_PER_ROUND` constant; read reload through
  `weapon.reload(&registry.scale)`, never the field.
- 1 movement point ≈ 1 hex of clear per round ≈ 6 km/h. Weapon range is in
  hexes.
- **A `Scale` field no rule reads is a bug.**
- Crew bonuses are percentages of the base — except where the base is small
  (1–3), where a percentage is a switch; use percentage *points* there.
  `Balance::scaled` floors at one; callers ask `base == 0` first.

## Known issues

- **The generated maps lean to an end** (`battle_town` ~1:2 to the east over
  eight seeds) and no compass tie found so far explains it. Until found, the
  `ground` rows of generated maps measure it.
- **Difficulty barely discriminates**: 5 over 1 is ~51% on the ridge, ~50%
  on the skill arena.
- **AI-vs-AI campaigns on `frontier` are decided on day two** by
  decapitation: each headquarters is its side's vanguard.
- **Four of five battle maps declare `"planner": "utility"`**, so the command
  layer (missions, plans, screening, withdrawal) is exercised only by
  `river_crossing`. Mission-related sweeps read null for that reason first.
- **Wiring a skill is not sufficient**: several crew rules are inert on
  shipped content because the quantities they scale are small integers. The
  designer's ruling is to raise content resolution, not ship coarse rates.
- Nothing in the AI reads `ground` (terrain reading) or fires smoke yet, and
  nothing gives it a reason to want concealment or a tor.
- "Where is she going" is still four fields (`intent.path`, `orders`, `goal`,
  `boarding`); folding them needs a design decision.
- `river_crossing` is the determinism baseline: no infantry, partial crews,
  no reserve — on purpose.
- `crates/game/src/battle.rs` (~3500 lines) and `overworld.rs` (~2200) want
  splitting.
- Performance: run `examples/perf --release`. `reachable`, `roads` and
  `unit_vision` measure work; round resolution moves with how the AI plays.
  `score_tile` is the hottest AI function; string-keyed registry lookups are
  ~21% of a round (STRUCTURE.md item 8). MCTS is parked (PARKED.md).

## Style

Comments explain *why*, in prose, often several lines. Public items carry doc
comments. Errors are `thiserror` enums. Optional mod-facing fields get
`#[serde(default)]`; enums get `#[serde(rename_all = "snake_case")]`. Tests
are end-to-end against real `assets/mods` content, named as sentences stating
the rule they defend. Shared stage builders live in `tests/common/stage.rs`.
Commit messages quote the instrument's numbers. CI enforces
`cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt
--check`. Rust 2024; dev profile is `opt-level = 1`, so measure in
`--release`.
