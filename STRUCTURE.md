# Structural debt

Code-shape problems: things that are wrong about *how the tree is arranged*
rather than about what any rule does. [CLAUDE.md](CLAUDE.md) says what the code
is and lists the known defects in it; [TODO.md](TODO.md) is gameplay and design
work; [PARKED.md](PARKED.md) says why code with no callers is still here. This
file is for the fourth thing — a seam in the wrong place, a rule living in the
wrong crate, a contract asserted in prose that nothing checks. None of these
make the game play wrongly today. All of them make the *next* change more
expensive than it should be, which is the only reason to spend time on them
before there is a bug.

Each item states the evidence as something re-checkable — a grep, a count, a
file and line — because the failure mode of a document like this is an item
that was true once. Strike an item through when it lands and say what closed
it, the way DONE.md does.

Found 2026-08-26 by reading the tree after the harness-sweeps merge. Ranked by
how much a fix now saves later, which is not the same as how broken each one
is.

---

## 1. ~~A knowledge rule lives in the Bevy layer~~

**Fixed 2026-08-26**, in three commits, because investigating it turned up two
behaviour questions the move itself should not have decided.

1. **The move.** `Event::heard_by(&state, side)` now sits beside the event enum
   in `battle/orders.rs`, exhaustive over all 35 variants — a new event fails
   to compile until somebody has said who hears it. Every variant kept its
   existing answer, so the move changed nothing. Six tests it could never have
   had, with the negative halves being the point; verified by mutation, not by
   reading.
2. **`UnitSpotted`.** The finding understated the problem: there were *three*
   callers, and the third was a different rule. The renderer filtered spots by
   `sides[by_side].ai.is_none()` — "was the spotter human-controlled" — where
   `heard_by` asks "was the spotter mine". Those agree at one human side and
   diverge at two, which `battle.rs`'s own unit tests construct and a
   reinforced field battle reaches in play. Now `by_side == side`, in one
   place.
3. **Morale and defiance.** Both rode the catch-all, so the player's log
   printed enemy crews' morale rungs. Now own-unit, matching `CrewHit` and
   `ModuleHit`: you see her tank reverse out of the line and draw your own
   conclusion; you do not get to read the rung she is standing on.

Only the third is visible in play, and it is deliberately its own commit so it
can be reverted alone if the quieter log reads wrong. All ten tours pass, the
determinism snapshot is untouched throughout (it records unfiltered core
events, so none of this can reach it).

<details><summary>The original finding</summary>

`heard_by` — `crates/game/src/battle.rs:1747`.

It decides **which side is entitled to hear an event**: that a `WeaponDry` is
the quartermaster's secret and not something the sound of a gun gives away,
that mission traffic is own-formation only, that a brew-up is visible across
the field. That is a statement about what a side *knows*, which puts it in the
same family as `spotted_enemy_at`, `BattleState::picture` and the invariant
that fog must not leak through the order system — all of which live in core.

It is also not cosmetic where it sits. `drive_ai` filters with it *before*
queueing, because `accepting_orders` is false while the animation queue has
anything in it, so an event nobody will print still costs the player a beat.
The rule therefore has two callers that must not drift, which CLAUDE.md
already says — and says about a function no test can reach.

**What it costs.** `tactics_core` has 313 tests and none of them can exercise
this. `playthrough` cannot use it, so the narrated battle prints both sides'
secrets. A replay viewer or a second frontend would reimplement it, and the
reimplementation would be the drift the current note warns about.

**Fix.** Move it to `battle::` as `BattleEvent::heard_by(&state, side)` and
let the game crate call it. `shown_to` stays where it is — where to *draw* a
ghost is genuinely presentation.

</details>

---

## 2. ~~`Scale::elevation_meters` is shadowed by a constant in Rust~~

**Fixed 2026-08-26.** `Heights::of` takes the scale and calls
`Scale::elevation`, and `los_clear` now goes through `Heights::of` instead of
inlining its own copy — so the module reads `elevation_meters` in one place
rather than the field being read nowhere and the number written three times.
The determinism snapshot did not move, which is what says the change is a
rearrangement and not a rule change.
`a_mod_that_flattens_a_level_flattens_the_skyline` is the check that the field
is read at all: on one map, at 10 m a level a three-digit ridge blocks and at
0.5 m the same ridge does not, asserted against both sight paths.

`EYE_HEIGHT` and `TARGET_HEIGHT` stayed in Rust — see item 8.

<details><summary>The original finding</summary>

`crates/tactics_core/src/battle/fog.rs:25` declares
`const ELEVATION_STEP: f32 = 10.0` and uses it for all sight geometry
(`Heights::of`, and again in the reference `los_clear`). `data::Scale` declares
`elevation_meters: 10.0` and a helper `Scale::elevation(levels)` to go with it.

The two agree today because both say 10. Nothing makes them agree. A mod that
sets `elevation_meters: 30` changes climb cost and changes **nothing** about
what a hill can see over.

**Evidence.** `elevation_meters` has exactly one consumer outside
`data/scale.rs` — the validator's print table (`registry.rs:647`).
`Scale::elevation()` has zero callers anywhere in the tree.

**What this is not.** An earlier reading of this had the whole scale contract
mostly unread. That was wrong: `round_seconds` is read through
`Scale::tick_seconds()` at `combat.rs:725`, `hex_meters` through
`Scale::meters()` at `combat.rs:724` and `:797`, and `overworld_hex_meters`
through `Scale::battle_map_radius()` at `map.rs:633`. `elevation_meters` is the
only field in the block that no rule reads.

**Fix.** `Heights::of` and `los_clear` take the scale and call
`Scale::elevation`. The numbers are identical, so **the determinism snapshot
must not move** — that is the check that the change is a rearrangement and not
a rule change.

**The pattern worth keeping.** The skill file already says every core must be
named by at least one skill, because a core nothing reads is dead weight. A
`Scale` field no rule reads is the same thing, and should be treated as a bug
rather than as documentation.

**Deliberately left alone:** `EYE_HEIGHT` (2.5) and `TARGET_HEIGHT` (2.0) in
the same file. They are modder-facing by the same argument, but promoting them
is a `mod.json` schema change with validation to match, and the reverse-slope
rule depends on their *ratio* rather than either value — so it wants its own
chunk and its own test. See item 8.

</details>

---

## 3. ~~`ScriptFacts` staleness is documented as a hazard and is live~~

**Fixed 2026-08-26.** Both publishers assign the whole struct through an
**exhaustive** literal — no `..default()` — so a field added to `ScriptFacts`
fails to compile in every publisher until each screen has said what it
answers. That is the house pattern already used by `Mission::slot`, and it is
stronger than the `..default()` this item originally proposed: a screen that
*should* answer a new fact cannot quietly take the default either. The
campaign map now says `selected: None` out loud, with a note that a map has a
selected *army* and that would want a fact of its own.

All ten tours pass. Note what they do **not** do: the script language has only
a positive `selected "<name>"` predicate, so no tour can assert the *absence*
of a selection, which is the instance this item was about. The compile-time
guard is what covers the class. A negated form would let a tour cover the
instance too — small, and worth doing next time the predicate language is
touched rather than on its own.

<details><summary>The original finding</summary>

CLAUDE.md warns: "a field a publisher leaves alone is still holding the
*previous* screen's answer — a script would wait on a muster prompt dismissed
two screens ago."

That is currently true. `battle::publish_script_facts` sets all eight fields.
`overworld::publish_script_facts` (`crates/game/src/overworld.rs:212`) sets
seven — it never touches `facts.selected`, and the campaign map has no notion
of a selected *unit* to publish. So after any battle, a campaign tour's
`selected` still names the last crew the player clicked, and an `expect
selected ...` on the map would pass for the wrong reason.

**Fix.** Have each publisher *construct* the whole value rather than assign
into it:

```rust
*facts = ScriptFacts { turn, idle, waiting: held, over, log, ..default() };
```

Then a field a screen has no answer for gets the default instead of the last
screen's answer, and omission stops being something a comment has to ask
people to remember. The prose warning stays — it explains *why* the shape is
what it is — but it stops being the only thing enforcing it.

</details>

---

## 4. ~~Test helpers are copy-pasted; there is no `tests/common/`~~

**Fixed 2026-08-26.** `tests/common/mod.rs` holds `mods_root`, `registry`,
`registry_wireless` and `seen`; all eight binaries import from it. The count
was worse than this item said: `determinism.rs` carried a ninth copy as
`load_registry`, with one word of its assert message changed.

Pure rearrangement, and the test counts say so — all twelve binaries green at
the same numbers as before, determinism snapshot untouched.

<details><summary>The original finding</summary>

`fn registry()` is **byte-identical in six** of the eight test binaries —
`save.rs`, `gunnery.rs`, `ammo.rs`, `modules.rs`, `content.rs`, `force.rs` —
with `engine.rs` carrying a seventh variant that goes through its own
`mods_root()`.

The duplication is not the real cost. `registry_wireless()` and `seen()` are,
per CLAUDE.md, **mandatory** for any staged test that needs two crews in plain
sight or a game without command rules — and they exist only in `engine.rs`
and (for `seen`) `save.rs`. A ninth test binary starts life without them, and
its author has no way to discover from the code that they were required. The
stage-setup contract is currently enforced by whether somebody read the right
paragraph.

**Fix.** A `tests/common/mod.rs` holding `mods_root()`, `registry()`,
`registry_wireless()` and `seen()`, with the doc comments that say why each
exists, and every binary declaring `mod common;`. Stage builders that are
genuinely local to `engine.rs` stay in `engine.rs` — the goal is to make the
*contract* importable, not to hollow out the file.

</details>

---

## 5. ~~The measurement instrument has no calibration test~~

**Fixed 2026-08-27.** `tactics_core::harness` holds the machinery whose
contracts were being claimed in prose — `parallel` (`run_all`, the job-order
guarantee), `overrides` (the `--set`/`--sweep` patch engine), `arena` (the
mirrored skill-gap map), `tally` (`Tally::merge`) — and `tests/harness.rs`
turns each claim into a test. The split is between the parts that have a
*contract* and the parts that have a *layout*: every table, column and duel
stayed in the example.

Verified as a pure rearrangement: `balance --sim --games 12` prints
byte-identical output, 399 lines, before and after. The two tests that matter
most were mutation-checked — reversing `run_all`'s output and turning
`deepest_stack` into last-value-wins each fail exactly one test.

**It found a wrong claim immediately.** `Tally::merge`'s doc comment said every
field must be "a sum, a concatenation or a union of sums" and listed *a
maximum* among the things that would make printed numbers depend on which core
finished first. `deepest_stack` has been a maximum all along and is perfectly
safe — max is associative, which is the actual requirement. The rule was
stated one notch too tight, and the comment now says associativity, notes that
commutativity is deliberately *not* required (the sample vectors concatenate,
which is why job order is guaranteed), and points at the test instead of
asserting.

<details><summary>The original finding</summary>

`crates/tactics_core/examples/balance.rs` is 4,321 lines. It holds the
mirrored arena fixture (`arena_map`, `assert_arena_is_mirrored`), the JSON
override engine (`patch_json`, `apply_override`), `run_all`'s
fold-in-seed-order determinism contract, and `Tally::merge`'s "every field is
a sum, a concatenation, or a union of sums" rule.

It has **zero `#[test]`s**, and `cargo test` does not build tests in examples
at all. So every checkable claim CLAUDE.md makes about it — `--jobs 1 / 3 / 7`
byte-identical, `--sweep balance.x` equivalent to editing `mod.json`, the
arena being symmetric — is verified by hand, once, and recorded in prose.

Given that the house style is quoting this instrument's numbers in commit
messages, it is simultaneously the least-tested code in the tree and the most
load-bearing. The arena is also invisible to `tests/`, which is part of why
the side-B bias needed a bespoke investigation rather than falling out of a
test.

**Fix.** Move the machinery into `tactics_core` (a `harness` module) or a
small `tools/` crate, and leave the example as argument parsing and printing.
`Tally::merge`'s summability, the sweep/`mod.json` equivalence and
`assert_arena_is_mirrored` then become three ordinary tests that run on every
`cargo test`.

</details>

---

## 6. ~~Both screens serialise their systems with `.chain()`~~

**Fixed 2026-08-26.** `ScreenSet` in `main.rs` names the seven phases a
screen's frame runs in — `Animate → Simulate → Input → Sync → Present →
Lifecycle → Facts` — declared once for the whole app. Both screens turn out to
run the *same* phases in the *same* order, which is why it is one enum rather
than two; what differs is only which systems go in each.

The order is **not** the `Input → Simulate → Sync → Present` this item
originally proposed. Animation comes first, and for a real reason: the paced
event queue gates everything behind it, since both the simulation and the
keyboard refuse to act while anything is still animating. Naming the phases
after what the code does rather than after a tidy guess is the point of the
exercise.

Provably order-preserving: every system is in exactly one set, sets are
chained, and systems within a set are still chained — so the total order
matches the old chain element for element. Systems keep their own `run_if`,
because configuring one set twice with two state conditions would AND them and
the phase would never run at all. All ten tours pass.

<details><summary>The original finding</summary>

`BattlePlugin` chains twelve systems; `OverworldPlugin` chains fourteen.
Ordering is therefore positional — the constraint lives in where a name sits
in a tuple.

Two of those orderings are load-bearing enough to carry comments explaining
them (`debrief_input` and `roster_input` must run *after* `handle_input`,
because `Enter` and `Esc` each mean two things and the modal must have first
refusal). That is exactly the knowledge a named `SystemSet` would carry
structurally instead of by adjacency. This layer has already lost time to an
ordering fault — `until idle` coming true a frame early, four `key Enter`
presses advancing one round, every screenshot after them describing the wrong
turn while the script reported success.

**Fix.** Named sets — `Input → Simulate → Sync → Present` — with the systems
assigned to them. Insertion order then stops mattering, and a new system
declares which phase it belongs to rather than being dropped into a list.

</details>

---

## 7. ~~Two files are large in a way that has an obvious seam~~

**Fixed 2026-08-26.**

- **`battle.rs` 4,121 → 3,500 lines.** The pure formatters moved to
  `battle/panel.rs` (668 lines): twelve functions plus `MISSION_KEYS`, which is
  the keys-to-promises join and belongs beside `order_menu` rather than in a
  module of ECS systems. `every_mission_key_has_a_promise` moved with them.
  Nothing in the new file touches Bevy — that is the line the split was drawn
  on, and it is why `set_portrait` (takes a `Query`), `shown_to` (about
  drawing) and `aim_at` (closer to input) stayed behind.
- **`engine.rs` re-sectioned by subject.** 25 headers lost their chunk labels,
  9 new ones went into the 1,400 unlabelled lines at the top, and the module
  doc now carries a contents list of all 39 — titles rather than line numbers,
  so it cannot rot. Verified by diffing `--list` output before and after:
  byte-identical, so no test moved out of the run.

What is **not** done: `engine.rs` is still 13,000 lines in one binary. Splitting
it into several is now much cheaper than it was, because `tests/common/`
exists (item 4) and a new binary inherits the stage-setup contract by
importing it. That is a separate, larger job.

<details><summary>The original finding</summary>

- **`crates/game/src/battle.rs`, 4,121 lines.** Roughly 700 of them are
  `format_*` / `describe_*` functions that are pure over `&BattleState` and
  contain no Bevy at all: `format_unit`, `format_tile`, `format_attack`,
  `format_formation`, `format_net`, `format_contact`, `mission_sentence`,
  `hex_label`, `order_menu`. That is the same shape as `roster_page`, which
  the project already extracted into a free function over plain data for the
  stated reason that a page nobody can see in a diff is a page that rots. A
  `battle/panel.rs` is the mechanical version of the same move.

- **`crates/tactics_core/tests/engine.rs`, 13,100 lines, 313 tests.** It is
  sectioned by *development chunk* — "chunk 9b", "chunk 10c, second slice",
  "direction step 3" — with the first header at line 1408, so 1,407 lines are
  unlabelled. The file is organised as a changelog: the sections are
  meaningful to whoever was there and to nobody else, and "where are the fog
  tests" is a grep rather than a lookup. Re-sectioning by subject costs
  nothing and outlives the arcs that produced it.

Neither is urgent. Both get worse monotonically.

</details>

---

## 8. Content ids are `String` — measured, and half done

**Measured 2026-08-27, and the premise turned out to be half right.** The
constants half is **done**; the interning half is **not**, and should not be
attempted on the strength of the original argument.

### What the measurement says

Instrumented every string-keyed registry lookup and fought five rounds of
`river_crossing` at difficulty 3:

| kind | per round | share |
| --- | --- | --- |
| `module` | 15,082 | **61.8%** |
| `vehicle` | 3,362 | 13.8% |
| `role` | 1,948 | 8.0% |
| `weapon` | 1,501 | 6.1% |
| `terrain` | 1,015 | 4.2% |
| everything else | 1,503 | 6.1% |
| **total** | **24,413** | |

One lookup costs **12.2 ns** against the real registry, so the whole of it is
**0.30 ms/round** against 1.41 ms of round resolution — about **21%**, and that
is an *upper bound* on what interning could recover.

Three things follow, and they change the plan:

1. **The cost is real but not diffuse.** `module` alone is 62%, and it does not
   come from `module_ok` (three call sites) — it comes from `substance`,
   `troops` and `mobility_halves`, and `substance` is what the evaluator walks
   *per candidate tile*. Attacking that one path gets most of the win.
2. **`terrain` is already only 4%.** The premise that the grids are workarounds
   for a terrain-lookup problem is right about history and wrong about now:
   `SightGrid` and `MoveGrid` already took those, and they would still be worth
   keeping after interning because they also precompute *derived* values.
3. **Full interning has a real blocker.** Ids are serialised in saves and map
   files, and serde cannot resolve a string to an interned id without the
   registry. Every way around that is a cost the original item did not price: a
   `#[serde(skip)]` side table is a third cache `rehydrate` has to place (the
   compiler now insists, but it is still a cost) and a global interner is
   process-wide mutable state
   next to a determinism guarantee.

**Recommended next step, when someone takes it:** make `substance` and its two
neighbours stop hashing, measure again, and stop. That is a bounded change
worth ~0.18 ms/round. Interning all twelve kinds is not justified by these
numbers.

### The constants: done

Five of the ten Rust tuning constants are now data, in `balance`:
`min_hit` (5), `max_hit` (95), `stalemate_rounds` (8), `eye_height_cm` (250)
and `target_height_cm` (200). Centimetres because `Balance` is deliberately
all-integer so the arithmetic stays exact — the module doc says so and derives
`Eq`, which caught an `f32` attempt at compile time.

Each `#[serde(default)]`s to exactly the constant it replaced, so a mod that
says nothing gets the game it always had; the determinism snapshot did not
move and `balance --sim` prints byte-identical output.
`how_much_luck_a_battlefield_has_is_a_mod_decision` and
`a_mod_that_raises_the_cupola_sees_over_the_rise` are the checks that they are
read at all. This also closes the note left open under item 2.

**The other five landed 2026-08-27** as the `planner` block:
`impatience`, `horizon_rounds`, `boarding_rounds`, `devolved` and
`exit_urgency`, which were the last tuning constants in `ai/`. A block of their
own rather than a home in `balance`, because they govern how the AI *thinks*
rather than what the rules *are* — nothing in it reaches a rule, so a mod that
rewrote all five would leave a human-versus-human battle bit-for-bit identical.
Determinism snapshot passed unregenerated, and `perf` is unmoved.

Five tests, one per field, each named for the rule it defends
(`what_a_round_of_driving_costs_a_commander_is_a_mod_decision` and its four
siblings in `tests/engine.rs`), plus
`the_planner_numbers_can_be_swept_and_ship_at_the_values_they_replaced` in
`tests/harness.rs`, which asserts both halves at once: the block is addressable
from the command line, and what the base mod ships equals
`PlannerRules::default()`. Every one of the five was mutation-checked — pin the
field back to its old constant and its test fails — because a field-is-read
test that passes without reading the field is the exact failure it exists to
catch.

**The prize was `--sweep planner.horizon_rounds=2,4,6`, and it returned a null
result**: at 36 games across the three shipped maps a horizon of one round and
a horizon of eight are the same game, inside a seed noise floor of ±3 wins. The
table and what to read into it are in [DONE.md](DONE.md).

This closes item 8's constants half entirely. Only the interning half remains,
and the recommendation above stands.

<details><summary>The original finding</summary>

The registry exposes twelve `&str`-keyed lookups (`terrain`, `vehicle`,
`weapon`, `ammo`, `module`, `skill`, `role`, `trait_def`, `radio`,
`character`, `doctrine`, `map`) against roughly sixty call sites in `src`.

`SightGrid` and `MoveGrid` both exist **specifically** to hoist that String
hashing out of an inner loop — the notes on them say so: `los_clear` was doing
a `String`-keyed lookup per ray step on the order of a million times a round,
and `edge_cost` about 7,500 per `roads` call. Each cache costs a
`#[serde(skip)]` and a rehydrate obligation; the failure mode where forgetting
to rebuild gave a silently wrong answer (an empty sight grid answers every
question wrongly; an empty move grid says nobody can drive) is closed since
ARCH-TODO 3a, because `SavedBattle::rehydrate` destructures every field and a
new cache does not compile until it is placed.

That is two bespoke workarounds for one root cause, and the third hot path
will want a third. Interning ids at load time — `TerrainId(u16)` and friends,
resolved once when the mod tree is read — deletes the class of problem rather
than the instances. It is a large change and should not be attempted next to
anything else.

**Related, smaller:** ten tuning constants live in Rust against the stated
"content is data" rule — `BOARDING_ROUNDS`, `HORIZON`, `IMPATIENCE` (already
tracked in TODO), `DEVOLVED`, `EXIT_URGENCY`, `STALEMATE_ROUNDS`, `MIN_HIT`,
`MAX_HIT`, `EYE_HEIGHT`, `TARGET_HEIGHT`. Each is individually defensible and
collectively a drift; every one is a number a modder would want. They want a
sweep to prove nothing moved, which is why they are one chunk and not ten.

</details>

---

Found 2026-09-23 by a structural read of the tree at the end of the MVP
cluster, looking for places where two paths answer one question. The core is
not layered in the way it once was — the one-system arc closed the largest
case, and every write to a unit's movement state is in `battle/orders.rs` —
so what is left is at the seams: between what a side knows and what it is
shown, between the player's path and the AI's, and between the campaign and
the battle.

## 9. ~~The player's weapon choice is a second model of a shot~~

**Fixed 2026-09-23.** `crates/game/src/battle.rs` carried its own
`best_weapon_from` — the same name as core's — ranking the weapons in range by
listed `damage`: no sight, no penetration, no round, no cadence, no fear. It
chose the gun for a click-to-engage and for the attack preview, so the player
and the AI priced the same shot with two functions. A shot at somebody now
goes through `weapon_against`, which is core's
`battle::best_weapon_from` and nothing else. Blind fire keeps a chooser of its
own, `area_weapon`, named for the one shot with nobody to price it against.

Where they parted, found by scanning every multi-gun chassis against every
target at one to eight hexes rather than by guessing (the first guess — a
medium tank on a platoon — was wrong; both pick the 75): a rifle platoon threw
the RPG at soft targets where rifle fire is worth more, and a light tank laid
the 37 on plate it cannot beat where the coaxial frightens the crew behind it.
If either of those reads wrong in play, it is wrong for the AI too, and now it
is one number to change. `the_player_lays_the_gun_the_ai_would`, mutation-
checked; all eleven tours pass.

## 10. ~~The campaign↔battle bridge lives in the Bevy crate~~

**Fixed 2026-09-23.** `tactics_core::field` holds the whole path:
`battlefield_for` (was `choose_battle_map`), `Clash::muster` (the
force-gathering half of `launch_battle`), `Clash::problem` / `stage` /
`inherit_missions`, `FieldBattle::report` (was `battle_outcome`, with the
*withdrew* rule), and `deploy`. `formation_exit` went to `battle::command`
beside `nearest_exit`, and the refused-order fallback of the campaign screen's
`drive_ai` became `overworld::step_planner`, so the one piece of judgment the
AI loop had is shared rather than copied. `PendingBattle::Field` carries a
`Clash`. The game crate's screens now only present it: `battle.rs` 4,438 →
3,346 lines and `overworld.rs` 2,887 → 2,677, tests included.

Nine tests moved with it into `tests/field.rs` (the game crate 28 → 19),
adapted to drive the new API and nothing else. Two hash-order shrugs went on
the way past: `battlefield_for`'s last resort took whichever battle map the
registry's `HashMap` yielded first, and so did the campaign screen choosing an
overworld map. Both take the lowest id now.

**The prize is `harness::campaign::play` and `examples/campaign`**: the whole
`frontier` campaign, every side a machine; 32 wars take about a tenth of a
second across every core. Its contract is two tests (it ends and stages every
clash it causes; the same seed is the same war). Its first reading, at 32
seeds, is a finding for the designer rather than a defect: **29 of 32 wars end
on day two, by decapitation, after one battle.** Each side's headquarters army
is also its vanguard (1st Company and Valkyrie Vanguard, five vehicles each),
the campaign planner prices the enemy's at `HEADQUARTERS_WORTH` (3.0) and
fights at parity, so the two headquarters meet on day two and that battle is
the war. Kuhlmann wins 21 of 32. Against a human the Valkyries' headquarters
will still come straight for hers.

Still in `deploy` and not changed here, because they are rules rather than
seams: every chassis is priced as `Tracked` when finding standing room, and a
side forms up by offset column.

## 11. ~~A side knows two things about the enemy, and each reader picks one~~

**Fixed 2026-09-23**, in two commits, because the first thing reading the
picture found was that it did not exist yet.

1. **The picture exists before round one.** Both setup paths computed the fog
   and stopped, so under command rules the picture was empty for the whole
   first planning phase and the player's board, which draws it, showed no
   enemy however plainly one stood in view — on `river_crossing`, two
   Valkyrie vehicles in the recon car's sight from the first frame
   (screenshotted on the same seed before and after). `open_the_net` computes
   who can speak and the picture at setup; contact still settles on the first
   tick, because three `net` tests said a crew who *starts* cut off is news.
   Snapshot: 16 opening `ContactReported` lines gone, nothing else.
2. **One question, two knowers.** `BattleState::known_enemies(registry, who)`
   replaced `ai::visible_enemies`. `Knower::Commander` is the fresh picture;
   `Knower::Crew` is the picture if she hears the net, plus what she sees or
   saw flash. The evaluator, goal chooser, both drills and `incoming` ask as
   the crew; the player's panel and tint ask as the commander through
   `fire_on_as`. With no `command` block both are the pooled fog.

**What it found on the way:** the picture was stricter than the fog it is
built from. The fog spots a gun off her muzzle flash with nobody looking at
her hex; the picture wanted an eyewitness, so a howitzer shelling the line
from out of sight was spotted and never reported, even on a perfect net.
`a_zeroed_command_block_is_the_game_without_one_with_a_commander_at_both_ends`
caught it the moment the brains read the picture.

**Measured:** determinism diff all `ContactReported` (51 → 70, the flash
reports) and no deed moved; `--sim --games 36` seed-swept, Iron Valkyries 75
→ 77 of 144 against a per-seed spread of 14–23, so inside the noise — both
commanders lost the same free radio, the symmetric null. The change that
matters is the human's: the AI no longer knows more than it has been told.
`perf` unmoved (utility order 0.09 ms); `mirror` clean.
`a_cut_off_scout_acts_on_what_she_sees_and_her_commander_never_hears_of_it`
and `the_commander_is_told_what_is_already_in_sight_when_the_battle_opens`,
both mutation-checked.

## 12. ~~"Take cover" is two functions~~

**Fixed 2026-09-23.** `battle::drill_destination` is the drill — the
reachable hex with the least `incoming_from` worth, strictly quieter only —
and both drills ask it: the engine's reflex against the guns she has noticed
on her reaction clock (`battle::noticed_threats`), the planning table's drill
against every gun bearing on her. The `drill` doctrine's evaluator only
chooses her shot from there now. `threats` and `threatened` moved from `ai::`
to `battle::danger`, because the engine's reflex and rout were calling into
the AI module for them.

**The first draft was wrong, and the probe said why.** It put the planning
drill on the reaction clock too, for "one clock" — and a crew who had been
looking at a tank since the battle opened sat under its gun for two ticks at
the planning table, had her running gear shot out, and never reached the
wood. Planning is a pause: nothing else planned there, the executors or the
evaluator's threat term, reads the clock, so the drill does not either. The
clock is for reacting while the round runs.

The two models parted on open ground, which is why the old test (a forest in
reach, where they happened to agree) could not have caught it: a scan of
stage layouts found the gun at column 9 sending the old drill to (3,2) and the
reflex's rule to (2,1). `the_planning_table_takes_cover_where_the_reflex_would`
pins it, mutation-checked against the old model. Only a human side's unordered
crews ever used the planning drill, so the determinism snapshot passed
unregenerated and no `balance` table can see it.

## 13. ~~The Lua campaign host is a second campaign-rule system~~

**Closed 2026-09-23** (the designer's call: park the host, drop the funds).
The host is behind `--features lua-campaigns`, off by default, with `mlua`
optional; PARKED.md says why and what reviving it properly would take. Funds
are gone from core, content and the HUD — the `Income` event, the treasury
on `OverworldSide`, the banner's "Funds 20" — and a terrain declaring
`income` or a map side declaring `funds` gets a `validate-mods` warning, the
`planner.mission_weight` courtesy (`a_campaign_has_no_treasury_and_content_that_funds_one_is_told`).

**`income` had a second, live job**: it was how the campaign planner ranked
ground (`4 + income / 2`). That job kept its numbers as terrain `value` —
city 3, factory 5 — so removing the treasury did not also change what the
AI marches on: `examples/campaign` over 32 seeds, plus two listed battle by
battle, is byte-identical before and after. `SAVE_VERSION` did not move: an
older save's `funds` key is simply not read, which is the right answer for it.
