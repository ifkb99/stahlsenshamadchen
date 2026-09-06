# One system, not several

**Temporary working file**, in the tradition of `CONTEXT-GAPS.md`: the durable
half of each item moves into `CLAUDE.md`, `STRUCTURE.md` or `DONE.md` as it
lands, and this file is deleted when the last box is ticked. What lives here is
the *plan* and the running measurements, so that a half-finished arc can be
picked up by somebody who was not in the room.

Written 2026-08-30, from an architectural read of the tree at the end of the
MVP. The designer's complaint, in full, because the wording matters:

> it seems like we have added more and more features that work separately,
> instead of coexisting in one system

## The diagnosis

The code structure is not the problem. The crate split holds, the tick
pipeline is a fixed order with a measured reason under each step, and most
"remember to" hazards are already compile errors. The problem is narrower and
has one address: **the resolver got deep and the evaluator did not.**

`ai/eval.rs::score_tile` is the single place a rule becomes behaviour. Two
checkable facts about it:

1. **The AI never reads `registry.balance`.** Not once.
   ```sh
   grep -rno "registry\.[a-z_]*" crates/tactics_core/src/ai/*.rs \
     | awk -F: '{print $3}' | sort | uniq -c | sort -rn
   ```
   prints `planner`, `vehicle`, `weapon`, `terrain`, `doctrine`, `command` —
   and no `balance`. The rules of the battlefield reach the AI through
   `expected_damage` alone, which is used for the offense term and nothing
   else.

2. **The threat term scores the wrong hex.** `eval.rs:72` asks
   `best_weapon_against(registry, state, enemy.id, enemy.pos, me)` where `me`
   is the unit **at her current position**. Inside, `dist`, `sight.clear`,
   `hit_chance` and `shot_profile` all resolve against `target.pos`. The only
   thing that varies with the candidate `tile` is `1/dist(enemy.pos, tile)`
   and a six-hex gate.

   So driving out from behind a ridge into an enemy's line of fire is priced
   *identically* to staying behind it. Cover, elevation, facing, concealment,
   motion and profile — the whole near side of the shot the resolver-depth arc
   built — never reach the decision about where to stand. The AI's opinion of
   cover at a candidate tile is `def.cover * 0.03 * doctrine.cover_value`
   (`eval.rs:161`), an independent model that `balance.cover_to_hit_percent`
   never touches.

That one fact explains a run of things already recorded as separate
mysteries: that removing the six-hex gate cost the skill table more than it
won (it extended the reach of a number describing the wrong hex); that
detection has "no reason" and stacking has "no reason"; that
`difficulty_foresight` had to be invented as a second axis because the value
function could not separate good ground from bad sharply enough for a blur to
be a handicap.

**The fix has a precedent in this tree.** `round_worth`'s blast half was made
`overpressure`'s twin, case for case, so the pricing and the resolver cannot
disagree. `hit_chance` / `shot_profile` / `expected_damage` already take the
attacker's hypothetical `from`; they take the target by id only. Adding the
symmetric parameter — the target's hypothetical `at` — makes the defensive
half of the evaluator read the real rules, and every future defensive rule is
picked up for free instead of needing its own hand-weighted term.

## Order of work

Decided with the designer 2026-08-30: **instruments first.** The evaluator
change is the headline, and the instruments as they stand cannot see it —
three doctrines exercising a narrow slice of the command model, and a
radius-10 arena with almost nothing for a better commander to be better at.
Changing what is measured before widening the measurement is how this project
has produced null results it then had to argue about.

Branch `feat/one-system`, off `feat/planner-block`.

---

## Phase 1 — the instruments — **DONE**

- [x] **1a. A terrain-varied arena.** A road down the axis, a hilltop village
      on the objective, woods on the flanks, soft going short of the middle,
      water to go around — and symmetric under **two** reflections rather than
      one, because point symmetry alone is not enough once left and right stop
      being worth the same. Two objectives rather than one, because a single
      one is a funnel and the funnel showed up in the control row.
      `4efab1c`.
- [x] **1a′. Line of sight is a mirror-symmetric relation.** Not on the plan,
      and the biggest thing in the arc. See below. `46ff581`.
- [x] **1a″. `candidates` orders an objective's hexes by distance.** A
      tie-break with no invariant key in front of it. `9ffb8a6`.
- [x] **1b. A fourth shipped doctrine that issues a movement to contact.**
      Bounding Overwatch — aggression 0.6 (the `Advance` band), delegation 0.35
      (under `planner.devolved`). `planner.pull_under_fire` is reached for the
      first time. `f31c949`.
- [x] **1c. Re-baseline.** Below.

## Phase 2 — one currency

- [x] **2a. Symmetric previews, and the one shared currency function.**
      `hit_chance`, `hit_breakdown`, `hit_chance_inner`, `shot_profile`,
      `expected_damage` and `ai::best_weapon_against` all take the target's
      hypothetical hex beside the attacker's. Every existing caller passes the
      two real hexes, and the checks say the step was a rearrangement:
      determinism snapshot passed **unregenerated**, `--sim --games 12`
      **byte-identical**, perf unmoved (round 1.61 → 1.61 ms, `reachable`
      15.0 → 14.6 µs, `roads` 109.1 → 105.5 µs, `unit_vision` 89.4 → 88.6 µs,
      utility order 0.08 ms). See below.
- [ ] **2b. The threat term reads the candidate tile.** `score_tile`'s danger
      becomes the real expected damage of the visible enemies shooting at her
      *there*. Watch the cost: `score_tile` is the hottest function the AI has
      (0.09 ms/order) and this makes the threat loop as expensive as the
      attack loop. `perf` before and after.
- [ ] **2c. `terrain_value` shrinks to what is genuinely a preference.** Cover
      and elevation stop being a second opinion about what cover and elevation
      *do*, because 2b already priced that. What may remain is a doctrine's
      taste for high ground beyond its arithmetic worth.
- [ ] **2d. Measure, and regenerate the baseline deliberately.** This one is
      *meant* to move the determinism snapshot; the diff is the evidence for
      the change rather than a cost of it. Read it before regenerating.

## Phase 3 — the smaller seams

None of these move the baseline. All of them make the next change cheaper.

- [ ] **3a. `save::rehydrate` becomes a compile-time obligation.**
      `save.rs:213` is four manual duties where forgetting gives a silently
      wrong answer (an empty `SightGrid` answers every sight question wrongly)
      or a panic. This tree already converted `Mission::slot`,
      `Event::heard_by` and `ScriptFacts` from "remember to" into "will not
      compile"; this is the largest one left.
- [ ] **3b. `tasking` / `latitude` / `detached` become one noun.** Set and
      cleared as a triple at five sites in `orders.rs`. A `PersonalOrder`
      makes the clear-together rule structural instead of prose.
- [ ] **3c. `Unit`'s outcome stops being five booleans.** `alive`, `exited`,
      `abandoned`, `brewed`, `wrecked`, plus a prose warning never to classify
      by `!alive`. That is an enum in a costume, and the next fate — captured,
      immobilised and left behind — would be a sixth bool and a sixth warning.
- [ ] **3d. `from_placements` validates.** `from_map` does;
      `from_placements` — the campaign's path — does not, and `spawn_unit`
      panics on content a mod removed (`battle/mod.rs:733`). As the campaign
      becomes the main mode this stops being theoretical.
- [ ] **3e. Doc rot.** `BattleState::command` still says "Inert as of this
      chunk — populated, saved, and read by nothing that makes a decision".
      `eval.rs` reads it to find the standing mission.

## Phase 1a in progress — and it turned up a defect in a core rule

The arena is rewritten and is a real battlefield: a road down the axis, a
hilltop village on the objective, woods on the flanks, soft ground short of
the middle, and water to go around. Two things came out of building it, and
the second is much bigger than the first.

### Point symmetry was not enough

A varied arena that is only point-symmetric hands the battle to side B. Four
tiebreaks in `ai/` put the coordinate key last, which the invariants permit on
the stated ground that "the compass decides only a left-or-right choice off
the line of advance" — true only while left and right are worth the same. Put
a hill on one flank and water on the other and it stops being true.

The arena is therefore symmetric under **both** reflections now: `arena_mirror`
(through the centre, swapping the ends) and the new `arena_flip` (across the
axis of advance, swapping left and right). The force is flip-symmetric too —
`arena_deployment` returns flip pairs adjacent so the i-th vehicle of each side
can be matched to its lateral twin.

### `candidates` broke the tiebreak invariant — fixed

`ai/goal.rs::candidates` pushed an objective's hexes in **map-file order**, and
the chooser breaks ties by list position. That is a tiebreak with no invariant
key in front of it at all, so both ends of the battlefield prefer the same
*absolute* hex, which is not the mirror choice. Now sorted by distance from the
crew, coordinate last.

Worth keeping straight: this is a real violation of a stated invariant and the
fix stands on its own, but it is **not** what was costing side A the battles
below. Measuring said so — the difficulty-5 rows did not move.

### Line of sight is not a mirror-symmetric relation

This is the one that matters. On the varied arena at difficulty 5 both sides —
where the blur is exactly zero, the map is symmetric, the force is symmetric,
and nothing else can break a tie — **side B took 46 of 72 equal battles**. At
difficulty 1 the same pairing is level, because a blur of 6.0 drowns it.

Bisected with `--set`, which is what that machinery is for:

| | `5 vs 5`, 2 seeds of 36 |
| --- | --- |
| as built | 25–47 |
| `balance.detection_certain_percent=100` (no search die) | 25–47, *identical* |
| `terrain.forest.concealment=0` | 25–47, *identical* |
| `terrain.forest.vision_block=0` | 32–40 |

So it is sight, and `_los_probe` measures it directly on the arena map:

| | ordered hex pairs, of 109,230 |
| --- | --- |
| `clear(a,b)` disagrees with `clear(b,a)` | 34 (0.03%) |
| `clear(a,b)` disagrees with **`clear(mirror(a), mirror(b))`** | **1,382 (1.27%)** |
| `clear(a,b)` disagrees with `clear(flip(a), flip(b))` | 30 (0.03%) |

`sight_line_clear` walks `from.line_to(to)`, and hex line drawing resolves a
point that lands exactly between two hexes with a fixed nudge. A point
reflection negates the direction of every segment, so the mirrored query
rounds the other way and 1.27% of pairs get a different answer. Elevation and
`vision_block` are what make that visible: on open grass nothing blocks, so
which hex the ray clips through never mattered.

This is the invariants section's own rule — *a tiebreak may only read
quantities a reflection preserves* — being broken **in the geometry rather
than in the AI**, which is why none of the three previous hunts found it. It
shipped twice in `ai/` and was fixed twice there; nobody checked the rule it
was stated about.

**It is a defect in the game, not only in the instrument.** Every battle
fought over woods or a ridge is decided partly by which absolute direction
the sight rays happen to round. `river_crossing` has both.

### The fix

A step that lands on a hex boundary now resolves to **every** hex it could be
in, and the ray is blocked if any of them blocks. That set is what a reflection
preserves — reflecting the ray reflects the whole set rather than picking its
other member — and so is reversing the ray. Census afterwards: **0 and 0**.

The boundary test is exact and costs three subtractions, so the seven-distance
scan only runs where it is needed: writing `d` for the offset to the nearest
centre and `e` for a neighbour direction, a neighbour is `2(d·e) + 2` further
off, so the widest of `|dx-dy|`, `|dx-dz|`, `|dy-dz|` reaching 1 *is* the point
being on a boundary. Cost: `unit_vision` 71.9 → 90.5 µs, round resolution
1.42 → 1.66 ms.

`a_reflection_leaves_a_sight_line_alone` pins it over every ordered pair of the
arena's 331 tiles under both reflections, and was mutation-checked (1,416
disagreements with the boundary test pinned off).

## Measurements

### The arc, step by step

Skill table, `--sim --only skill --absolute --games 36 --sweep
seed=0,1000,2000,3000`.

| | 5 vs 5 | 1 vs 1 | the ends | 5 over 1 | 5 over 3 |
| --- | --- | --- | --- | --- | --- |
| arena alone | 54.3% | 54.2% | 54.3% | 50.0% | 46.8% |
| + sight fix | 62.7% | 51.1% | 56.9% | 45.9% | 46.5% |
| + `candidates` fix | **54.2%** | **50.0%** | **52.1%** | **51.2%** | **47.5%** |

Note the middle row: fixing sight made the *side* lean worse before the third
commit fixed it. That is not a contradiction — sight being exactly symmetric
keeps play mirrored for longer, so when a tie finally breaks it breaks on the
one remaining asymmetry (movement resolves in unit id order, and side A holds
the even ids) more cleanly and more consistently. That residue is real, is
difficulty-5-only (the `1 vs 1` row is level at 50.0%), and is the next thing
of its kind if anybody wants it.

### The Phase 2 baseline

Everything Phase 2 claims is a difference from these. `--sim --games 36`,
seed 0, full output saved outside the repo.

| | |
| --- | --- |
| outcome | Valkyries 24, Kuhlmann 12, 0 draws, 0 stalemates |
| length | 13.2 rounds mean (3–23) |
| contact | first found round 3.5, 26 acquisitions a battle at 11.0 hexes |
| gunnery | 1894 shots, 618 penetrated (33%), 242 bounced, 953 missed |
| on the move | 956 of 1894 shots (50%) laid from a moving vehicle |
| crew cost | 2.7 wounded and 11.4 out per battle |
| skill | 5 over 1 **51.2%**, 5 over 3 **47.5%** |
| perf | round 1.66 ms, `unit_vision` 90.5 µs, `reachable` 15.4 µs, `roads` 111.6 µs, utility order 0.08 ms |

**The prediction Phase 2 is making**, written down before the work so it can be
wrong: if the defensive half of the evaluator starts reading the resolver, the
skill rows should rise, because there will at last be something on this ground
for a commander to be better at. If they do not move, the currency argument is
wrong and the next place to look is the goal chooser rather than `score_tile`.

### Superseded

Read the rows below only for the shape of the argument; every one of them was
measured over a sight rule that is not mirror-symmetric.

| arena | 5 over 1 | 5 over 3 | the ends (A/B) |
| --- | --- | --- | --- |
| bare (before this work) | 61.5% | 52.6% | 50.5% |
| varied, point-symmetric only | 56.3% | 49.7% | 47.7% |
| varied, doubly symmetric, hills flanking the objective | 53.9% | 50.5% | 54.0% |
| varied, doubly symmetric, crest **under** the objective | 59.2% | 53.3% | 44.8% |

None of these is the number to keep — every row above is measured over a sight
rule that is not mirror-symmetric, which is exactly the confound the arena was
rebuilt to remove. Re-measure after the fix.

What the rows do already say, and it is worth having: **richer ground did not
by itself make skill discriminate.** `5 over 3` moved from 52.6% to 53.3%,
inside its own noise. That is the fourth measurement pointing at the evaluator
rather than at the map, and it is Phase 2's case restated.

## Phase 2a — what landed

**The symmetric parameter.** Six signatures gained `at: Hex` (or, in
`shot_profile`, `target_pos: Hex`) beside the target's id, exactly as they
already carried `from` beside the attacker's. Inside `hit_chance_inner` the
whole positional half now resolves against it — range, cover, downhill,
and through `shot_profile` the struck facing and obliquity; `best_weapon_from`
checks range and `sight.clear(from, at)` against it too.

**What deliberately did *not* follow the hypothesis, with the reasoning
written on it:**

- **`tgt.moved`.** The plan listed the moving-target term among the things
  that must resolve against `at`, and it should not: it is the mirror of
  `hexes_under_way`, which reads state and never the hypothetical `from` for a
  reason this tree already measured (three stalemates and +2.3 rounds when a
  draft charged the drive to a candidate tile). Charging the enemy a
  moving-target discount because she *would have driven* to the tile under
  discussion is that same bias with the sign flipped — far ground would read
  as systematically safer. So the term reads her state, and the doc comment
  says why.
- **Her facing at `at`** is the facing she has now, stated in the doc comment
  on `hit_chance`. The planner turns her toward the enemy with
  `face_units_at_enemies` after she moves, so her bearing at a tile is a
  consequence of the fight she finds there; predicting it is a later change
  and this is where it goes.
- **Which round the loader chambers.** `best_round_against` still judges from
  the real positions — as it already did for `from`, which nobody had noticed.
  Threading a hypothesis through it would price a rack against ground nobody
  has driven to.

**The shared currency.** Two new things, and the split between them is the
point:

- `battle::combat::best_weapon_from(registry, state, attacker, from, target,
  at) -> Option<(usize, f32)>` — the three gates a shot has to pass (range
  band, sight unless indirect, expectation above zero) in **one** place.
  `ai::best_weapon_against` is now a five-line wrapper that adds the *would
  this kill her* judgment on top, which is the only part of it that was ever
  an AI question.
- `battle::danger::fire_on(registry, state, unit, at) -> Vec<Bearing>` with
  `Bearing { enemy, weapon, hit_percent, expected }` — every spotted enemy who
  could put fire on `unit` if she stood at `at`, in enemy id order, best
  weapon each. Fog-honest through `ai::visible_enemies`. No doctrine weight,
  no planner number, no falloff: 2b's threat term and the player's danger
  overlay both read this or they are two answers to one question.

Its own module rather than more of `combat.rs` because it is the currency the
rest of Phase 2 is denominated in, and a named file is where the next reader
looks.

**Tests** (`tests/engine.rs`, section *what the ground can put on her*), all
three mutation-checked:

| test | mutation that must fail it | result |
| --- | --- | --- |
| `a_bearing_taken_at_her_own_hex_is_the_shot_the_resolver_would_take` | `hit_chance` called with `blind: true` | FAILED as required |
| `where_she_would_stand_decides_what_can_be_put_on_her` | `fire_on` ignores `at` and uses `me.pos` | FAILED as required |
| `a_bearing_is_never_taken_from_an_enemy_nobody_has_found` | fog gate replaced by every alive enemy | FAILED as required |

The middle one is the one that matters: on its stage the open hex, the wood
and the far side of the belt priced *identically* before this chunk, because
the arithmetic resolved against the hex she was already on.

**One thing to know before 2b.** `hit_chance`, `hit_breakdown` and
`expected_damage` now trip `clippy::too_many_arguments` at 8, which
`combat.rs` never has before — CLAUDE.md's hygiene note predicted this exact
moment. They carry `#[allow]` with the reasoning, following the precedent of
`resolve_impact` next door. The cleanup it points at is a `Shot` struct
bundling each end of the shot (a crew and a hex); that is a readability
decision about every call site in the game and was deliberately not taken
inside a chunk whose whole claim is that nothing changed.

## Danger overlay — what landed

The player-facing half of "one currency". `battle::danger::fire_on` is now
read from the battle screen as well as from the evaluator, so the sentence in
DIRECTION.md — *friction the player can predict and price is drama; friction
she cannot see is a bug report* — is literally true of the most expensive
decision in the game, which is where to put a tank. Nothing in core changed;
this is presentation, and the determinism snapshot passed unregenerated.

**The panel line** is `panel::format_danger`, pure over a `BattleState` like
everything else in that file, and it leads the panel whenever the player has
one of her own crews selected and a hex under the cursor:

```
Danger at (15,20):
  Irma Krieger  45% for 3.0
    75mm KwK
  Nadja Orlov  82% for 10.6
    88mm PaK
  13.6 a round expected
  151% of what she has left
```

Two lines per gun because the panel is 300 px and a line that wraps to three
is one nobody reads: the name and the arithmetic lead, the gun that will do
it sits under them. On the hex she is already standing on the heading reads
`Danger where she stands:`, because "Danger at (10,20)" for her own tile sends
a player looking somewhere else. A hex nothing bears on says
`nothing spotted can reach her` rather than printing an empty heading. The
percentages and the expectations are `Bearing::hit_percent` and
`Bearing::expected` verbatim — the panel does no arithmetic of its own except
the total and its share of `substance().0`.

Three placement decisions worth knowing before moving it:

- **It leads rather than follows.** A full crew's description already fills
  the panel on its own, so appending put the one part that changes as the
  mouse moves below the fold on exactly the crews worth looking at. A
  datasheet loses less by being second than a live answer does by being
  invisible.
- **Never for an enemy crew**, the rule the unit panel's order hints already
  follow: an enemy's exposure is not the player's to read.
- **Not on the shot preview or the ghost report.** Both are already an answer
  about a hex somebody *else* is standing on, and "what could be put on you if
  you stood where that tank is" is a question nobody asked.

**The overlay** is `D`, toggled, and it tints the selected crew's reachable
tiles — the same set the move highlight draws, through the same `HexOverlay` /
`MoveHighlight` machinery, *replacing* the blue rather than stacking on it
(two translucent fills over one tile make a third colour that means neither).
Four bands, as a share of what she has left to lose in one round: blue nothing
bears on it, yellow under a tenth of her, orange under a third, red a third or
more. `panel::DANGER_LEGEND` holds the words and `battle::DANGER_COLORS` the
colours, the second sized off the first so they cannot come apart; the legend
prints in the panel only while the overlay is up. `D` doubles as the camera's
pan-right key, which is the bargain `A` and `W` already make on this screen.

**Cost, and how it is paid.** `fire_on` over a 42-tile reach with four spotted
enemies is 228 µs in release — call it half a millisecond over a full
radius-20 reach, which is real money per frame and nothing at all once a
round. So it is computed in `update_highlights` under the existing
`range_dirty` gate, beside the reach it is about, and cached in
`Battle::danger`; every order already raises that flag and nothing else can
move a unit during planning. Toggling `D` raises it too.

**The tour** is `scripts/dev/danger-overlay.txt`
(`STAHL_BATTLE=river_crossing STAHL_SEED=7`) — the seed is pinned because the
tour asserts about positions. It commits one round so that somebody has
actually been *found* (an honest overlay has nothing to say on the deployment
round), re-selects the medium tank at (0,20), photographs the panel line on
open road to the east, toggles the overlay, photographs her own ground, and
toggles it off. `ScriptFacts` gained one field, `danger: Option<u32>` — how
many reachable tiles the overlay is painting as under fire, `None` when it is
not up — counted off the cached tint rather than recomputed, so what the tour
asserts is the arithmetic the tiles were painted from. Both publishers answer
for it; the campaign map says `None` out loud. The predicate is
`danger <cmp> <n>` and it is *false* when the overlay is off, so
`until danger >= 1` waits for the overlay and for it to have found something.

**What it cannot show yet, and why.**

- **Only what her side has spotted.** That is `fire_on`'s contract and not a
  gap, but it means a clear-looking tile can still hold an ambush, and the
  panel is not lying when it says so — it reports the picture.
- **The enemy where she is now, not where she will be.** `fire_on`'s `at` is
  hypothetical and the enemies' positions are real, which is the asymmetry the
  question has; a tile that is safe this round because the tank covering it
  has not arrived yet reads as safe.
- **Her facing at the tile is the facing she has now**, inherited from
  `hit_chance`'s own documented limitation, so the overlay cannot yet tell her
  that turning in would show a thinner plate.
- **The yellow band is nearly unreachable in the base mod.** In the tour's own
  screenshot the tiles go blue → orange → red with no yellow anywhere: an 88
  expects 10.6 points against a medium tank's ~9, so any tile it covers is
  instantly a third or more. The bands are linear in a quantity that is not,
  and the honest fix is either a log scale or a denominator that is the
  vehicle's full complement rather than what is left of it. Left as it is
  because the reading is *correct* — that ground really is lethal — and
  because picking the curve wants more than one map's worth of looking.
- **`fire_on` returns the best weapon per enemy, so the panel names one gun
  each.** A tank with a coaxial that also bears is under-reported. Adding a
  second bearing per enemy is a change to core's shape and belongs to whoever
  owns `danger.rs`.
- **No total was added to core.** The overlay sums `Bearing::expected` in the
  game crate rather than asking for a helper beside `fire_on`, deliberately:
  `danger.rs` is another agent's file this week and a one-line sum is not
  worth a merge conflict. If the evaluator ends up wanting the same sum, that
  is where it should live.

## Scratch

Temporary probes, to be deleted with this file:

- `crates/tactics_core/examples/_arena_dump.rs` — prints the arena, its terrain
  counts, deployment ground, and what a tracked vehicle can reach.
- `crates/tactics_core/examples/_mirror_probe.rs` — plays the arena at
  difficulty 5 both sides and reports the first round where a decision or a
  position stops being the mirror of its twin.
- `crates/tactics_core/examples/_los_probe.rs` — the sight-symmetry census
  above.
