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

## 5. The measurement instrument has no calibration test

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

---

## 6. Both screens serialise their systems with `.chain()`

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

---

## 7. Two files are large in a way that has an obvious seam

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

---

## 8. Content ids are `String`, and two caches exist to work around it

The registry exposes twelve `&str`-keyed lookups (`terrain`, `vehicle`,
`weapon`, `ammo`, `module`, `skill`, `role`, `trait_def`, `radio`,
`character`, `doctrine`, `map`) against roughly sixty call sites in `src`.

`SightGrid` and `MoveGrid` both exist **specifically** to hoist that String
hashing out of an inner loop — the notes on them say so: `los_clear` was doing
a `String`-keyed lookup per ray step on the order of a million times a round,
and `edge_cost` about 7,500 per `roads` call. Each cache costs a
`#[serde(skip)]`, a rehydrate obligation, and a failure mode where forgetting
to rebuild gives a silently wrong answer rather than a loud one (an empty
sight grid answers every question wrongly; an empty move grid says nobody can
drive).

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
