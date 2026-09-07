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
- [x] **2b. The threat term reads the candidate tile.** `score_tile`'s danger
      becomes the real expected damage of the visible enemies shooting at her
      *there*. Watch the cost: `score_tile` is the hottest function the AI has
      (0.09 ms/order) and this makes the threat loop as expensive as the
      attack loop. `perf` before and after.
- [x] **2c. `terrain_value` shrinks to what is genuinely a preference.** Cover
      and elevation stop being a second opinion about what cover and elevation
      *do*, because 2b already priced that. What may remain is a doctrine's
      taste for high ground beyond its arithmetic worth.
- [x] **2d. Measure, and regenerate the baseline deliberately.** This one is
      *meant* to move the determinism snapshot; the diff is the evidence for
      the change rather than a cost of it. Read it before regenerating.

## Phase 2 — what it left behind

Found while building it, none of it blocking, all of it the next thing of its
kind:

- [x] **Cadence is not in the currency.** Every term — attack, threat, the
      overlay's total — is per *shot*. A machine gun at a shot every 10 s
      fires six times in a round and an 88 twice; `WeaponDef::reload` is a
      resolver fact the pricing never reads. Per-round expectation is one
      multiplication in `best_weapon_from`, but it changes every weight quoted
      in the currency (see `deviation_cost` above) and wants its own
      measurement.
- [x] **Pressure is not in the currency.** Fire that cannot beat her plate
      expects zero and is invisible to every planner, which is the designer's
      `threatened` note in DIRECTION.md. The resolver already turns a rattling
      bounce into `rules.bounced`; an expected-pressure twin of `incoming`
      would let a crew near Breaking value a quiet hex, and give the machine
      gun on every tank a job.
- [ ] **A `Shot` struct.** `hit_chance`, `hit_breakdown` and `expected_damage`
      carry `#[allow(clippy::too_many_arguments)]` at eight parameters; each
      end of the shot is a crew and a hex.
- [ ] **`fire_on` names one weapon per enemy**, so a coaxial that also bears
      is under-reported in the panel. A second bearing per enemy is a shape
      change to `danger.rs`.
- [x] **The overlay's bands are linear in a quantity that is not.** (A gradient since 2026-09-07.) Yellow is
      nearly unreachable in the base mod. Log scale, or a full-complement
      denominator.
- [x] **The arena is the instrument limit again.** +3 points on the skill
      rows at 576 battles a row is the right sign and not decisive; the terms
      that now discriminate (cover under a specific gun, sight, elevation)
      need ground where a wrong choice is punished harder than a radius-10
      hexagon with two objectives can. **Built: `ridge_arena`, `--arena`,
      `examples/mirror` (Wave 1 — ground, below).** What it found is the
      next item.
- [x] **The objective is not in the currency.** On `ridge_arena` the
      difficulty-5 commander loses to difficulty 1 (47.5% at 576 battles a
      row) because she prices the crest's danger honestly and nothing prices
      what holding it is *for*: the objective term is `value * decay` on a
      scale a single found gun outweighs. Reweight the arena so the safe
      flank scores and she wins 66.7% while drawing every equal battle.
      Orders have the same defect from the other end (`mission_weight` is
      quoted in objective units, not substance). Wave 2: quote both in the
      currency — what a point of score is worth in substance per round, and
      what a delegated order is worth — chosen by sweep on the ridge, where
      the win column punishes a crew who will not go where the points are.

## Phase 3 — the smaller seams

None of these move the baseline. All of them make the next change cheaper.

- [x] **3a. `save::rehydrate` becomes a compile-time obligation.**
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
- [x] **3d. `from_placements` validates.** `from_map` does;
      `from_placements` — the campaign's path — does not, and `spawn_unit`
      panics on content a mod removed (`battle/mod.rs:733`). As the campaign
      becomes the main mode this stops being theoretical.
- [x] **3e. Doc rot.** `BattleState::command` still says "Inert as of this
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
  13.6 expected, one shot each
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

## Phase 2b–2d — what landed

**The threat term is one line now.** `score_tile`'s danger is
`battle::danger::incoming(registry, state, unit, tile)` — the sum, over every
enemy this side has *found*, of what the resolver says she would take standing
on that tile. The six-hex gate and the `1/distance` falloff are gone rather
than retuned, because both were standing in for positional terms the
arithmetic could not see: a falloff guesses at "further off is safer" where
the range band states it exactly, and a gate guesses at "out of reach" where
sight and range answer it per gun. Range, line of sight, cover, elevation,
profile, facing and obliquity all reach the decision about where to stand now,
and every defensive rule added after this is picked up for free.

`caution * exposure` is untouched, and that split is the line the chunk is
drawn on: the sum is what the *rules* say can be put on her, which is the
resolver's to answer; `caution * exposure` is what she makes of it, which is
hers and her doctrine's.

**`incoming` sits beside `fire_on` rather than being spelled
`fire_on(..).iter().sum()`**, and both walk one private `guns_bearing_on`, so
there is still exactly one answer to *who can shoot her there and with what*.
The reason for the second shape is cost: `score_tile` asks this per candidate
tile per crew per round, so a `Vec` allocated and thrown away each time — and a
second `hit_chance` per enemy for a `hit_percent` the evaluator never reads —
is paid on every tile of every sweep. `fire_on`'s signature is untouched, as
the danger-overlay work asked.

### 2c: what remains of `terrain_value`, and why

**The term stays, gated: the prior speaks only where the arithmetic is
silent.** A tile no found gun can reach is priced by taste; a tile inside a
found gun's envelope is priced by the shot that gun would take — cover and
elevation included — and the flat bonus stands down rather than being added on
top. The two engine-side coefficients became `planner.cover_prior` (0.03) and
`planner.elevation_prior` (0.4), defaulting to exactly the constants they
replaced, so they are sweepable; **they ship unchanged**.

The alternative — keep the term and shrink the coefficients — was measured
first because it is the obvious move, and the measurement is the interesting
part. Skill gap on the mirrored arena, `--only skill --absolute`, 36 battles a
cell:

| terrain prior | 5 over 1 | 5 over 3 |
| --- | --- | --- |
| **4 seeds (288 battles a row)** | | |
| ungated, shipped 0.03 / 0.4 | 48.8% | 51.6% |
| ungated, halved | 53.0% | 50.7% |
| ungated, zero | **55.0%** | **56.3%** |
| gated, shipped | 50.4% | 52.0% |
| gated, cover 0 only | 48.9% | 50.5% |
| gated, elevation 0 only | 51.2% | 51.6% |
| **8 seeds (576 battles a row)** | | |
| gated, shipped | **52.5%** (+1.2 sd) | **51.4%** (+0.7 sd) |
| priors zero | 54.0% (+1.9 sd) | 52.0% (+1.0 sd) |

Read the two halves of that table together, because the four-seed half is a
trap and this project has fallen into it before. At four seeds zeroing the
prior looks like a 5-point win over gating it; at eight the gap is 1.5 and 0.6
points, which is nothing. The tell was already visible at four: zeroing cover
alone and zeroing elevation alone each did nothing, and only the pair did
anything — an interaction with no mechanism behind it is usually a draw of the
dice. Zeroing also costs the arena its side symmetry (`the ends` 46.7%,
−1.6 sd, against 51.7% gated), which is a second reason not to want it.

And zero is not available anyway: the prior is the **only** thing in the engine
that reads `DoctrineDef::cover_value` and `elevation_value`, so pricing it at
nothing retires two doctrine fields — exactly the dead-field failure this tree
keeps finding. The gate removes the double count where the double count
actually is, keeps both fields load-bearing, and needs no retuning, which also
keeps 2c separable from 2b's measurement.

Worth knowing for later: under the gun the resolver pays **3.5 substance
points** for the same timber the flat bonus was paying **0.9** for
(`the_ground_prior_stands_down_where_the_arithmetic_speaks` prints both). The
bonus can stand down without a crew forgetting what a wood is for, because the
thing replacing it is nearly four times larger and knows which gun is looking.

### The one content number that had to move: `planner.deviation_cost` 2.0 → 3.0

**Flagged for the designer rather than done quietly.** `deviation_cost` is
quoted in the evaluator's currency, and its stated job — the property
`the_shipped_doctrines_straddle_the_price_of_deviating` exists to pin — is to
sit between the shipped doctrines' `initiative` values. Phase 2 changed that
currency: threat used to be zero over most of the map and is now three to eight
substance points everywhere a found gun can reach. At 2.0 **all three** shipped
doctrines deviated on that stage, including massed armour, whose whole
character is driving at the hex it was given. Measured on `pressed_stage`:

| `deviation_cost` | massed 0.3 | elastic 0.7 | recon 0.9 |
| --- | --- | --- | --- |
| 2.0 (was shipped) | own idea | own idea | own idea |
| **3.0 (now shipped)** | **obeys** | own idea | own idea |
| 4.0 / 5.0 / 6.0 | obeys | own idea | own idea |
| 8.0 | obeys | obeys | own idea |

3.0 is the smallest value that restores the straddle and 3.0–6.0 all do, so it
is not a knife edge. It changed nothing measurable in the arena (the skill
table is byte-identical with and without it — the arena issues no missions).
DONE.md's "At `deviation_cost: 2.0` the …" paragraph and `planner.rs`'s
worked example both needed the new number; the second is updated, the first is
the lead's to fold.

This is also the (d) measurement the plan asked for: **subordinate initiative
finally has something to say.** The tile her own sweep offers is now priced
against the gun covering it, so at 2.0 it beat a distant map reference for
every doctrine in the mod. That is the mechanism working, not failing; the
price of acting on it simply had to be restated in the new currency.

### The autonomy line

The three protected tests pass unweakened —
`a_binding_march_presses_on_where_an_ordinary_one_takes_cover`,
`a_cut_off_unit_keeps_the_orders_she_had`,
`a_binding_mission_is_not_discounted_by_a_loose_doctrine`, the `mission_value`
tests and all of *the planner numbers are data*. One new test pins the half
that was prose: a delegated crew who breaks off raises `Decision::drill` so the
log can say so, and a binding one never reaches the drill at all.

Three existing tests failed and all three were knife-edge stages rather than
broken rules. Recorded because the margins are the interesting part:

- `an_assault_presses_through_what_an_advance_pauses_for`. `FORWARD` moved
  from seven hexes along the lane to ten. At seven the old threat term was
  *zero at both tiles* (both are more than six hexes from the gun), so the
  wood cost the advance only its cover bonus and the assault won by 0.48;
  now the wood is worth 1.5 substance points of avoided fire and the mission's
  slope has to be long enough to be worth that. At ten the assault presses on
  by 0.62 and the advance still halts by 1.63. Sixteen would break it the
  other way — the tank destroyer cannot reach that far and both orders drive
  forward for a reason that has nothing to do with either of them — so a new
  stage assertion pins the forward tile under the gun.
- `ground_the_enemy_reaches_first_is_worth_less_marching_for`.
  `contest_aversion` asserted at 5.0 instead of 3.0. The old `1/distance`
  falloff made the hill with a tank a hex away look five times as dangerous as
  ground five hexes off; the resolver disagrees, because a 75 mm loses two
  points of accuracy a hex and the far objective is only 3.7 against 5.2
  substance points. The term flips the choice from 4 upwards where it used to
  flip from 3; asserted at 5 so the stage is not sitting on its own threshold.
- `the_shipped_doctrines_straddle_the_price_of_deviating`. The
  `deviation_cost` change above.

### 2d: the determinism diff, read before regenerating

Broad and behavioural, which is what a rules change looks like: 517 changed
lines of 1503, +16 `Commit`, +18 `SetFire`, +19 `SetGoal`, +3 `SetMove`, and
six of the eight units picking different ground at some point. **Kills per
seed are unchanged** (7 / 6 / 6 / 5); what moved is where crews stood on the
way. Seed 2 no longer reaches `BattleEnded` inside the snapshot's twelve
rounds, joining seed 4, so three of four seeds now run past the window — the
battles got a little longer, which the fought-out digest agrees with
(13.2 → 13.5 rounds).

The clearest single example, seed 1, unit 4: she used to take
`Take(9, 18, -27)` and drive out to (11, 16), where side 0 spotted her and an
eight-hex duel opened across the open ground. She now takes
`Take(6, 19, -25)`, one row south, is first seen two rounds later at (10, 15),
and the answering medium engages from (12, 18) instead. Nobody was ordered to
do any of that; the tile she preferred simply stopped being priced as if the
enemy were shooting at the hex she had already left.

Regenerated with `UPDATE_SNAPSHOTS=1`.

### The measurements, before and after

Everything below is 36 battles at seed 0 unless it says otherwise, re-measured
on the same machine in the same session (the machine is contended enough that
the Phase 2 baseline's `perf` row had to be re-taken; the numbers here are the
median of three quiet runs).

| | baseline (738e417) | after 2b+2c |
| --- | --- | --- |
| outcome | Valkyries 24, Kuhlmann 12 | Valkyries 22, Kuhlmann 14 |
| draws / stalemates | 0 / 0 | 0 / 0 |
| length | 13.2 rounds (3–23) | 13.5 rounds (3–25) |
| first contact | round 3.5, 26 acquisitions at 11.0 hexes | round 3.4, 26 at 10.9 hexes |
| gunnery | 1894 shots, 618 pen (33%), 242 bounced, 953 missed | 1850 shots, 615 pen (33%), 218 bounced, 941 missed |
| on the move | 50% | 52% |
| crew cost | 2.7 wounded, 11.4 out | 3.2 wounded, 10.8 out |
| tank destroyer kills / losses | 72 / 30 | 75 / 27 |
| medium tank kills / losses | 55 / 66 | 67 / 64 |
| infantry surviving | 64 of 96 | 69 of 96 |

**Skill, 8 seeds × 36 battles = 576 a row.** This is the prediction Phase 2
wrote down, and it is the row to read.

| | 5 over 1 | 5 over 3 | the ends (A/B) |
| --- | --- | --- | --- |
| baseline | 283–289 **49.5%** (−0.3 sd) | 275–290 **48.7%** (−0.6 sd) | 283–283 50.0% |
| after | 292–264 **52.5%** (+1.2 sd) | 287–271 **51.4%** (+0.7 sd) | 286–267 51.7% |

**The prediction held, directionally and weakly.** Both rows rose, by 3.0 and
2.7 points, and both moved the same way — which is worth more than either
alone, since they share seeds and forces. Neither is individually decisive:
+1.2 sd and +0.7 sd are the kind of numbers this file has warned about
quoting. What can be said is that the sign is right and that nothing else in
the arc moved them: the same rows at four seeds read 51.2% / 47.5% before and
50.4% / 52.0% after, which is why the eight-seed pass exists.

Note also that the *four-seed baseline in this file was a high draw* on
5 over 1 (51.2% against 49.5% over eight), which is the fourth time a
single-draw number in this project has flattered itself.

**Delegation tax**, 36 battles a pairing:

| pairing | baseline | after |
| --- | --- | --- |
| massed flat | 12–24 | 14–22 |
| massed under command | 9–27 | 12–24 |
| elastic under command | 14–22 | 15–21 |
| bounding flat | 15–21 | 15–21 |
| bounding under command | 13–23 | 11–25 |

Massed's tax 3 → 2, bounding's 2 → 4, everything inside the ±6 band a level
pairing wanders in at 36 battles. **Not a measured regression, and nothing was
retuned to make it look like one.** The `mission_weight` question TODO.md
already carries is untouched.

**Performance.** `score_tile` is the hottest function the AI has and the
threat loop is now as expensive as the attack loop, which is what the plan
predicted. Median of three quiet runs:

| | baseline | after |
| --- | --- | --- |
| round resolution | 1.62 ms | 1.37–1.54 ms |
| utility order | 0.08 ms | 0.09 ms |
| `reachable()` | 14.5 µs | 18.0 µs |
| `roads()` | 106.4 µs | 136.1 µs |
| `unit_vision` (cold) | 90.5 µs | 88.5 µs |

Utility order paid 12%, which is the honest cost of the chunk and cheap for
what it buys. Round resolution went *down*, which CLAUDE.md already predicts —
it moves with how well the AI plays, and crews that stop driving into guns
spend fewer ticks manoeuvring. `reachable` and `roads` are not called by
anything this chunk touched; they move because the units they are measured on
are standing somewhere else by then, and `unit_vision` — which measures the
same work on the same map either way — is level, which is the check that the
machine is comparable.

### What the plan got wrong

- **"2c: cover and elevation stop being a second opinion"** reads as *delete
  the term*, and deleting it retires `cover_value` and `elevation_value`. The
  question is not how much of the term should survive but *where*, and the
  answer is a gate rather than a coefficient.
- **The plan expected 2b to be the whole of it.** Half the movement in the
  skill table came from the terrain gate, not from the threat term, and at
  the shipped prior 2b *alone* left 5 over 1 slightly worse at four seeds.
  Reading either half without the other would have produced a wrong
  conclusion in either direction.
- **Nothing in the plan anticipated that a shipped number would have to
  move.** Changing the currency a weight is denominated in is a thing that
  reaches every weight quoted in it; `deviation_cost` was the only one that
  crossed a threshold, but it will not be the last time the question comes up.
- **The plan's "watch the cost" was aimed at the wrong number.** `perf`'s
  `reachable` and `roads` rows moved more than `utility order` did, and
  neither is on this chunk's path — a reminder that those rows measure work
  *in a particular game state* and the game state is what changed.

## Scratch

The three temporary probes are gone (2026-09-07): `_arena_dump` and
`_los_probe` deleted, their census pinned by
`a_reflection_leaves_a_sight_line_alone` over every arena; `_mirror_probe`
promoted to `examples/mirror.rs` with a verdict.

## Wave 1 — seams: what landed

Phase 3 items **3a**, **3d** and **3e**. No rule changed and nothing moved:
`tests/snapshots/event_stream.txt` passes unregenerated, which is the evidence.
All eleven tours pass headless.

### 3a — the caches are a type, not a duty

`BattleState` is now `Battle<Built>`, a type alias over a struct with one extra
type parameter and one extra `#[serde(skip)]` field:

```rust
pub struct Built(());          // no Default, private field
pub struct Unbuilt;            // Default

pub struct Battle<C = Built> { /* …every field it always had… */
    #[serde(skip)] built: C,
}
pub type BattleState = Battle<Built>;
pub type SavedBattle = Battle<Unbuilt>;
```

Serde fills a skipped field with `Default::default()`, so `Battle<Unbuilt>`
deserializes and `Battle<Built>` **cannot** — the compiler says
`the trait bound `Built: Default` is not satisfied`. The one door between them
is `SavedBattle::rehydrate(&registry)`, which destructures the struct field by
field, so a cache added tomorrow stops the build until somebody has said
whether it travels in the file or is rebuilt on load. `save::SaveGame` takes
the same parameter (`SavedGame = SaveGame<SavedBattle>` is what serde reads,
`SaveGame` is what `from_json` hands back), so there is no route from a file to
a playable battle that does not pass a registry. The free function
`save::rehydrate` is gone.

**Why this shape.** The two alternatives the brief offered both cost more. A
`Caches` struct owning the skipped fields means `state.sight` and `state.moves`
become `state.caches.sight` at every call site in the engine — including
`combat.rs`, `movement.rs`, `eval.rs` and `goal.rs`, none of which this chunk
was allowed to touch — and it still cannot own the two skipped fields inside
`FogMap`. A hand-written saved-form mirror means a second copy of
`BattleState`'s sixteen fields, which is the drift this item exists to remove,
reintroduced one level up. The marker parameter costs one defaulted generic:
every signature in the workspace still reads `BattleState`, and the exhaustive
destructure in `rehydrate` is where the "cannot be forgotten" lives.

`tests/save.rs::a_battle_off_a_save_file_cannot_be_asked_anything_until_its_caches_are_back`
pins it, and its doc comment carries the line that no longer compiles.

### 3d — what `from_placements` now refuses

It returns `Result<Self, BattleSetupError>` — the same error `from_map`
returns — and refuses, collecting every problem rather than the first:

- a placement whose hex is not on this map;
- a placement naming a vehicle the registry does not have (this is the one that
  used to be `spawn_unit`'s `expect("placement validated against registry")`);
- a placement naming a side this battle does not have;
- a crew list naming a `CadetId` the roster handed in does not know — a seat
  that would be silently empty, and substance counts people aboard.

`spawn_unit` is fallible too, so neither setup path can panic on content.

In the game crate the campaign asks *before* it commits: `launch_battle` stages
the battle through `battle::field_battle_problem` and, on a problem, writes one
line to the overworld log and leaves the armies where they were —
`commit_to_battle` now runs only after the check passes. `choose_battle_map`
returns `Option<String>` instead of `expect`ing that some mod ships a battle
map. `setup_battle` handles the residual case by logging and returning to the
campaign rather than entering a battle it cannot build.

Tests: `content.rs` —
`a_placement_naming_a_vehicle_no_mod_ships_is_refused_rather_than_fatal`,
`a_placement_crewed_by_a_cadet_the_roster_never_heard_of_is_refused`,
`every_army_the_campaign_ships_can_be_put_on_a_battlefield`; `overworld.rs` —
`every_clash_the_campaign_map_can_produce_can_be_staged`,
`a_campaign_with_no_battlefield_to_fight_on_says_so_instead_of_panicking`.

### 3e — the doc rot that was fixed

- `BattleState::command` no longer claims to be "inert … read by nothing that
  makes a decision": `ai/eval.rs` reads it for the standing mission,
  `ai/command.rs` for doctrine and delegation, and rallying for a formation's
  leader.
- `data/ammo.rs` and `WeaponDef::ammo` no longer claim ammunition is inert and
  that combat resolves every shot from the weapon's own numbers. It does not;
  the fallback for a weapon that names no ammunition is what is left of that,
  and it now says so.
- Three intra-doc links to constants that became mod data years ago:
  `STALEMATE_ROUNDS`, `crate::battle::MIN_HIT` and `Balance::substitution_penalty`
  in `battle/mod.rs`, `data/balance.rs` and `roster.rs`. `cargo doc -p
  tactics_core --no-deps` now reports no unresolved links at all.

Grepping `threat`, "six hexes", "falloff" and `terrain_value` turned up nothing
else stale; the Phase 2 commit updated `data/planner.rs` and `ai/eval.rs` as it
went.

**Left for somebody who may touch `battle/movement.rs`:** `MoveGrid::tiles`'
doc still says "`[crate::save::rehydrate]` *must* refill it". That function no
longer exists. Rustdoc does not flag it because the field is private, so it
will sit there until the next person in that file fixes it. The rule it states
is still true; only the name is wrong, and the correct reference is
`[crate::battle::SavedBattle::rehydrate]`.

**Left for the docs the chunk may not edit:** `CLAUDE.md` says `save::rehydrate`
twice — under *The road, not the crow flight* ("rebuilt by `save::rehydrate`")
and under *Saving* ("**`save::rehydrate` must rebuild all three**"). `DONE.md`
says it twice more (the movement-grid entry and *The caches a save must
rebuild*). All four describe a function that has been replaced by
`SavedBattle::rehydrate`, and the *Saving* entry is the one worth rewriting:
the obligation it states in bold is now the thing the compiler checks.
## Wave 1 — ground: what landed

Phase 2's last open item was *"the arena is the instrument limit again"*: +3
points on the skill rows at 576 battles a row, the right sign and not decisive,
on a radius-10 hexagon with two objectives and little for a commander who now
prices cover under a specific gun to be better at. This chunk built the ground
and measured on it. **Nothing under `src/battle`, `src/ai` or `src/data` was
touched, and the determinism snapshot passed unregenerated.**

The headline is a null with a sign on it, and it is about the AI rather than
about the map: **on ground where a wrong choice is punished harder, the
difficulty-5 commander does slightly *worse* than the difficulty-1 one.** The
diagnosis and the evidence are below.

### `ridge_arena` — the second battlefield

Radius 12, 469 tiles, in `harness/arena.rs` beside the old one, which is now
`SKILL_ARENA` and is byte-for-byte the map it was.

**Written in coordinates the symmetry group is diagonal in.** The old arena
declares a shape and takes its four images through `symmetric`, which works and
is impossible to predict: the flip of the wood at relative `(-4, 4)` lands at
`(0, -4)`, which is not where anybody drawing on paper would put it. For a hex
at arena-relative `(x, y)`, put `s = 2x + y` (along-axis, doubled) and `t = y`
(lateral). Then `mirror` is `(s, t) -> (-s, -t)` and `flip` is `(s, t) ->
(s, -t)`, so the group is exactly *"negate either coordinate"* and **a set is
doubly symmetric if and only if it is described by `|s|` and `|t|` alone**.
`Arena::band(along, lateral)` is that description, and every feature of the
ridge arena is one. A band cannot come out asymmetric, which is a stronger
guarantee than the runtime assertion —
`a_band_is_the_same_band_from_either_end_and_either_flank` pins it, and also
pins that the two coordinates are what the module doc says they are.

**The shape.** A road down the axis over a level-2 knoll in the middle; open
valley floor either side; a level-2 ridge along each flank at `|t| = 7..8`,
stopping at `|s| <= 10` so there is a gap beside each forming-up line; woods on
the valley floor inside the ridges and more woods beyond them; mud in the two
gaps. Objectives: **The Crest**, thirteen hexes of level-2 grass in the middle,
worth 3; **The Spurs**, the ten level-2 hexes at the lengthwise middle of the
two flank ridges, worth 2, one objective across both flanks.

**What it discriminates on, measured with `los_clear` over the built map:**

| ground | tiles it can see, of 469 |
| --- | --- |
| the crest's summit hex | 68 |
| the crest's rim hexes | 148 |
| inner edge of a spur | 112 |
| outer edge of a spur | 88 |
| a reverse-slope wood | 9 |

The first two rows are the sharpest thing on the map and they are a choice
*within one objective*: the ray from a 22.5 m eye to a 2 m hull is already
under 20 m one step out, so a plateau blocks its own middle. `candidates`
offers all thirteen crest hexes and `score_tile` has to pick. The military
crest and the topographic crest are different hexes and the arithmetic can now
tell.

The woods split the same way. The crest rim sees into the near woods at four
and six hexes; the reverse-slope woods are blocked from the crest, from the
valley and from both edges of the ridge above them. A flat cover prior prices
those two identically; `danger::incoming` does not.

Two more properties fell out rather than being designed, and both are worth
knowing:

- **Everything commanding is bare and everything covered is blind.** The base
  mod has no cover without `vision_block`, and a wooded or built-up neighbour
  at your own level stands 20 m above the ray. So a crew in cover sees a hex or
  two; a crew that can see is in the open. This is true of `SKILL_ARENA`'s
  hilltop village too and nobody had noticed: a unit in the middle of it is
  nearly blind.
- **The knoll cuts the map in half lengthwise.** A gun on the east rim cannot
  see the west near woods, so each side's approach march is covered from the
  *far* rim rather than the near one, and the prize for winning the race to the
  crest is being able to shoot at the loser's approach.

A first draft had the level-1 shoulder at `|t| <= 3` and put the near woods in
dead ground from the crest — the exact opposite of what they are for. From a
22.5 m eye to a 2 m hull the ray is under 10 m for the last two fifths of its
length, so a level-1 lip that far out hides the ground just beyond it. Each
shoulder is now the *minimum* that satisfies `max_climb: 1`.

### `--arena`, and the tables that take it

`--arena <id>` (`skill_arena` default, `ridge_arena`) selects the battlefield
for `skill`, `brains` and `mustered`, and adds an arena row to `ground`.
`Arena` is a struct and the flag resolves to a `&'static Arena`, so the map,
the deployment and the symmetry checks are one choice — there is no way to
fight one arena's ground with another's order of battle. Everything still goes
through `fought_grids`.

**The default is unchanged and every quoted number reproduces**, checked rather
than asserted: the eight-seed skill table on `skill_arena` came back
`286-267 / 287-271 / 292-264` — the same three pairs of integers this file
records for after-Phase-2, not merely the same percentages.

`ground` gaining an arena row is the point rather than a side effect: that
table's question — *does an end of this battlefield pay?* — is exactly what the
skill table's `the ends` row has been standing in for. An arena's two orders of
battle are one force and its own reflection, so there is nothing to exchange;
the row is marked `†`, its `OB` columns print as `—` (a `NaN` cell, which
`GridColumn::cell` now renders as a dash rather than as the word NaN), and
`west`/`east` is the whole of it.

### `examples/mirror.rs` — the probe, promoted

`_mirror_probe.rs` became `examples/mirror.rs` with `--arena`, `--seeds`,
`--difficulty` and `--rounds`. What it gained is a **verdict**, and the verdict
is the interesting part, because most of what it prints is not a defect:

- **equal-key tiebreak** — the goal one crew took and the reflection of her
  twin's are the same distance from her. `candidates` sorts an objective's
  hexes by distance from the crew with the coordinate last, which the
  invariants permit. Expected; not a defect.
- **different key** — the two are at *different* distances, so a real ordering
  key disagreed. That is a rule reading the compass.
- **downstream** — pair order is decision order (planning walks unit ids, and
  the i-th vehicle of each side stands on the i-th deployment pair), so once
  one pair has broken the mirror everything decided after it that round
  inherits an unmirrored board: claimed hexes, occupied paths, spotted enemies.
  The report names the **root** — lowest pair in the first broken round — and
  lists the rest as downstream. The verdict counts roots only.

That last rule is what makes the report readable. Without it the ridge arena
reports four "a rule read the compass" findings that are all the same tie seen
four crews later.

**Both arenas come back clean.**

| | first break | root | verdict |
| --- | --- | --- | --- |
| `skill_arena` | round 3, pair 1 (medium tank), every seed | A at rel (1,-1) takes rel (0,-1), 1 away; B's reflects to rel (1,0), also 1 away | equal-key tiebreak |
| `ridge_arena` | round 1, pair 0 (medium tank), every seed | A at rel (-9,1) takes rel (-2,0), 7 away; B's reflects to rel (-2,1), also 7 away | equal-key tiebreak |

So the divergence this file's Phase 1a section left open — *"that residue is
real, is difficulty-5-only, and is the next thing of its kind if anybody wants
it"* — is **the documented coordinate-last tiebreak and nothing else**. Every
root on both arenas is two hexes of one objective the crew is equally far from.
The skill arena's round-3 pair-5 finding and the ridge arena's three extra
round-1 findings are all downstream of that one tie. Nothing in the tree is
reading the compass to break a tie it could have broken on distance.

Worth saying plainly because it closes a hunt: **this is not worth fixing by
adding a key.** Any invariant key that separates two hexes at equal distance
would have to be a fact about the ground, and the two hexes here are congruent
ground by construction. The residual is a coin, and it is the same coin on both
arenas.

### The measurements

All `--release`, `--games 36`, `--only skill --absolute`, seed-swept. 576
battles a row at eight seeds; the `sd` is the standard error of the eight
per-seed win rates.

**Skill, eight seeds.**

| | 5 over 1 | 5 over 3 | the ends (A/B) |
| --- | --- | --- | --- |
| `skill_arena` | 292–264 **52.5%** (+1.3 sd) | 287–271 **51.4%** (+0.6 sd) | 286–267 51.7% (+0.9 sd) |
| `ridge_arena` | 273–302 **47.5%** (−1.1 sd) | 273–303 **47.4%** (−1.2 sd) | 281–294 48.9% (−0.6 sd) |

**Difficulty discriminates *worse* on the new ground, and it changes sign.**
Five points on `5 over 1` and four on `5 over 3`, both rows moving together,
which is worth more than either alone since they share seeds and forces.
Neither is individually past 1.3 sd; the pair is the result. The four-seed
draw agreed (47.7% / 48.3%), so this is not a single-draw flatter.

**Ground, four seeds, 288 battles a row, difficulty 3 both sides.**

| arena | west | east | draws | side edge |
| --- | --- | --- | --- | --- |
| `skill_arena` † | 149 | 123 | 16 | **54.8%** to the western end |
| `ridge_arena` † | 146 | 142 | 0 | **50.7%** |

So the new ground is the better *instrument* on the axis the instrument is
supposed to be good at: no side edge worth the name and no draws at all, where
the old arena still leans about five points to side A at difficulty 3 and draws
one battle in eighteen. The skill table's own control agrees — `the ends` on
the ridge arena reads 144–144 at four seeds with a spread of 2, the tightest
control this project has recorded.

**Timing.** `perf` is unmoved, as it must be — nothing on its path changed:
round 1.43 ms (spread 1.15–2.07), `reachable` 17.8 µs, `roads` 135.8 µs,
`unit_vision` 89.1 µs, utility order 0.09 ms. The harness itself costs 40% more
on the bigger map: `--only skill --games 36` is 0.21 s on `skill_arena` and
0.30 s on `ridge_arena`. The sight-symmetry test now walks both arenas — 469²
ordered pairs on top of 331² — and still runs in 0.17 s.

### What the ground revealed about the AI

The negative sign is not noise about the map; it is a statement about
`score_tile`, and the objective weighting is the knob that proves it. Two
diagnostic configurations of the same arena, 36 battles a pairing:

| The Crest / The Spurs | 5 v 5 draws | 5 over 1 | 5 over 3 |
| --- | --- | --- | --- |
| **3 / 2 (shipped)** | 0 of 36 | 47.5% (8 seeds) | 47.4% (8 seeds) |
| 3 / 3 | 0 of 36 | 46.5% (2 seeds) | 46.5% (2 seeds) |
| 2 / 4 | **36 of 36** | **66.7% / 58.8%** (2 seeds) | 54.2% |
| 3 / 2, crest built up (town) | 17–23 of 36 | 60.0% / 54.7% | 49.6% |

Read the third row against the first. At 3 / 2 the crest is nearer than the
spurs for every crew the skill table deploys, so both sides must contest it —
and the difficulty-5 commander, who can now see exactly what a bare level-2
plateau costs, is the one who *declines* it. At 2 / 4 the safe flank outscores
the lethal middle, and then the difficulty-5 commander is enormously better
than the difficulty-1 one (66.7% at `5 over 1`, against 52.5% on the old
arena) — **and draws every single equal-skill battle**, because both sides take
their own flank, the split objective is contested, and neither ever attacks.

The fourth row says the same thing from the other side: give the scoring ground
cover and the skill signal comes back positive (57.4% mean at `5 over 1`), at
the price of half the battles ending in a draw and a nine-point side-A edge,
because a blind defensible objective goes to whoever arrives first.

The conclusion, and it is Phase 2's own success read as a cost: **the threat
term has become strong enough to beat the objective term.** A crew that prices
`danger::incoming` correctly and then refuses the mission is not misjudging the
ground — she is judging it right and valuing it wrong. There is nothing in the
evaluator that says *taking the objective is what the battle is for*, only a
`value * decay` pull that a single found 88 outweighs. `caution * exposure` is
supposed to be the crew's own opinion of the danger; on this ground it is the
whole decision.

Two things follow, neither of them this chunk's to do:

- **The shipped configuration is the one with the honest control** (3 / 2: zero
  draws, `the ends` 144–144). An arena that discriminates beautifully while
  drawing every equal battle is an instrument whose control is degenerate, and
  this file already has a paragraph about why that cannot be read.
- **The next planner question is not another term, it is the balance between
  two that exist.** Cadence and pressure (above) both add to the threat side.
  Whatever adds to it next should be measured on `--arena ridge_arena`, where a
  crew that will not go where the points are shows up in the win column instead
  of hiding.

### Scratch, revised

`_mirror_probe.rs` is now `examples/mirror.rs` and is no longer scratch.
`_arena_dump.rs` and `_los_probe.rs` remain untracked one-offs; the census
`_los_probe` produced is pinned by
`a_reflection_leaves_a_sight_line_alone`, which now walks **every** arena — a
second battlefield with a second set of ridges is a second sample of the
geometry rule, and it passes on the ridge arena unchanged.
## Wave 1 — currency: what landed

Phase 2's leftovers, both of them, in one arc: **cadence** (every term was per
shot, so a machine gun firing six times a round and an 88 firing three read
alike) and **pressure** (fire that cannot beat a plate expected zero and was
invisible to every chooser, including the shooter's own — the designer's
`threatened` note in DIRECTION.md). Two commits: `20fbd86` the mechanism at
neutral content values, `9ef6673` the base mod's numbers.

### The design

**Suppression is a property of the round.** `AmmoDef::suppression` is what a
shot that *strikes* costs the crew it struck, penetration or not, on top of the
outcome prices `morale` already charges. On the round rather than on the ladder
because the loader chooses rounds: a belt and a solid shot from the same
coaxial mount are different experiences for the crew being shot at, and a gun
that can chamber both ought to be able to say so. `#[serde(default)]` to zero,
which is the game before.

**One price list.** `MoraleRules::pressure_for(ShotFelt, RoundPressure)` — a
penetration costs `hit + penetrated`, a bounce costs `bounced` unless the round
was small arms, and the round's suppression is charged on top in every case.
Two things spend it and they are the pair that must not drift:
`BattleState::apply_pressure` charging a tick's events, and
`combat::round_pressure` telling a planner what a shot is expected to be worth.
`ShotHit` and `ShotBounced` gained `ammo: Option<String>` so the event-reading
pass can price what arrived — events are the record, and a log that can name
the shell is worth more than one that says a shell.

**Worth is the currency.** `round_worth = round_damage + round_pressure *
morale.point_worth`, with the exchange rate applied in exactly one place
(`worth_of`, private, and mutation-checked — an earlier draft applied it twice
and a mutation to one site passed the suite). `expected_shot` returns
`ShotValue { expected, pressure, worth, shots }` off **one** hit chance and one
`shot_profile`, so the pressure half costs nothing measurable;
`expected_damage` and the new `expected_pressure` are its halves.
`best_weapon_from`'s third gate reads `worth` rather than damage, which is the
line that puts the machine gun on every tank back in the game;
`best_opportunity_shot` and `AMBUSH_PATIENCE` weigh worth too.

**Cadence.** `WeaponDef::shots_per_round(&Scale)` = `ticks_per_round /
reload(scale)`, read through the accessor and fractional so a piece that takes
longer than a round to load gets less than one shot. `Bearing` keeps its
figures **per shot** and gains `shots`, so the panel can name a gun and its
rate; `incoming` returns `Incoming { substance, pressure, worth }` **per
round**, which is the unit ground is held in.

Per caller, as the plan asked:

| caller | unit | why |
| --- | --- | --- |
| `best_weapon_from` | per round (ranks by `worth_per_round`) | every caller of it is pricing ground |
| `ai::best_weapon_against` | per round | the evaluator's attack term, which is subtracted from a per-round threat |
| its `kill` flag | per shot, damage only | the +4 bonus is for a *decisive* shot; times cadence an autocannon finishes everything it sees |
| `danger::incoming` | per round | "how bad is that hex" is a question about holding it |
| `danger::fire_on` | per shot + `shots` | the panel names a gun and its rate; the reader multiplies |
| `best_opportunity_shot` | per shot | one trigger pull now, and rate of fire is already modelled by how often `weapon_ready` lets her back |
| `best_round_against` | per shot | she is choosing what to load, and cadence cancels within one gun |
| `ai::threats` / `threatened` | boolean | untouched in shape; picks up the worth gate for free, so a crew under machine-gun fire now counts as under fire |

**The overlay** tints by worth per round and the panel prints each gun's
cadence beside its name, a total that is a round of fire ("expected this
round") and a pressure total when there is any. The AI and the player price the
same ground or they are playing different games.

### The check that made the rest readable

With `shots_per_round` pinned to 1.0 the determinism stream is **byte-identical
to the baseline** once the new `ammo:` field is stripped from it. So the whole
suppression/worth refactor is provably behaviour-neutral, and every line of the
mechanism commit's diff is attributable to cadence. Worth doing again the next
time two changes land together: it cost one build and turned an unreadable
1,193-line diff into a legible one.

### Content, and how it was chosen

| round | suppression | |
| --- | --- | --- |
| `ball_mg` | 2 | swept 0,1,2,3 |
| `he_105` | 2 | swept 0,2,4,6 |
| `ac_20_he` | 1 | interpolated by blast (1 against the 105's 6) |
| `he_75`, `he_88` | 2 | interpolated by blast (3 and 4) |
| `rifle_ball` | 0 | see below |

`morale.point_worth: 0.5`.
`planner.deviation_cost` 3.0 → **12.0**.
`ai/eval.rs`'s withdrawing `attack_scale` 0.25 → **0.0625**.

**The win column could not choose these.** `ammo.ball_mg.suppression=0,1,2,3`
at 36 battles with fear priced reads 24–12 / 24–12 / 24–12 / 21–15 against a
±6 band, and `morale.point_worth=0,0.5,1,2` reads 25–11 / 24–12 / 24–12 /
23–13. What moves is everything else: at `ball_mg: 2` cadets out per battle go
12.4 → 9.9, bounces as a share of shots 31% → 26%, artillery's share of the
shooting 75% → 66%. `he_105` at 4 and 6 shifts the win column by 4 and 6 and
adds a round to the battle, so 2.

**So a second instrument decided `point_worth`: how far each candidate moves
the currency relative to every other weight in the game, counted as engine
tests that stop holding.** 0.25 and 0.5 break 11; 1.0 breaks 14, the three
extra being `deviation_cost`'s family. The reasoning behind the number agrees:
at 1.0 a full ladder of fear (12 points) is worth almost a whole medium tank,
which is more than being frightened is; at 0.5 it is worth about a third of
one. This is a legitimate instrument and probably an underused one — it
measures exactly the thing CLAUDE.md warns about, that changing a currency
reaches every weight quoted in it — but it is a *count of broken calibrations*,
not a measure of whether the game is better, and it should not be mistaken for
one.

**`rifle_ball` ships at nothing and that is the one judgment here rather than a
measurement.** At 2 it tripled the infantry shot counts in the delegation table
(11 → 75 for massed flat) while the sweep could not tell 0 from 1 from 2 on the
win column, and a rifle section is not what the designer's note was about. Next
thing to sweep, against an instrument that can see it.

**`deviation_cost` was measured twice on `pressed_stage`, and that is the
interesting part.** The straddle it exists to keep — massed armour (0.3) obeys,
elastic defence (0.7) and recon pull (0.9) do not — holds from **8.5 to 19**
with suppression declared nowhere and from **12 to 29** with the base mod's
shipped values. One number has to serve both or the field means something
different in a mod that declines the new rule, so 12.0, the bottom of the
intersection, in a band still eight points wide. It is the third value this
field has had (2.0 → 3.0 → 12.0) and all three moves are the same event.

**`attack_scale`** is 0.25 divided by four, and the four is cadence. Its own
comment records the measurement it was set from (a shot worth ~4.6 against a
lane pulling ~0.5 a hex); a flat quarter of a *round* of fire put the same 4.6
back in front of a withdrawing crew and reinstated the defect the line exists
to remove — `an_ordered_withdrawal_needs_no_wounds` caught Anka Weiss planning
away from her lane, ten hexes off it to fifteen. The sentence is preserved
rather than the digit: a quarter of what one shot is worth to a crew who was
not ordered out. Anything at or below 0.15 keeps the withdrawal a withdrawal on
that stage; 0.20 does not.

### One modelling gap found and fixed

`mustered` scaled a remnant platoon's *damage* by the riflemen still standing
and not her round's suppression, so two cadets could pin a tank as hard as a
full platoon. It scales both now: volume of fire is volume of fire, and the
troops module already means firepower as well as interior weight.

### Test stages repaired, with their margins

Thirteen in all, none weakened. Grouped by what actually moved.
(`what_it_costs_a_subordinate_to_have_her_own_idea_is_a_mod_decision` is the
thirteenth and is only the shipped-value assertion, restated to 12.0.)

**Cadence: a weight that had to be restated.**

- `the_shipped_doctrines_straddle_the_price_of_deviating` — `deviation_cost`
  above.
- `an_ordered_withdrawal_needs_no_wounds` — `attack_scale` above.
- `an_assault_presses_through_what_an_advance_pauses_for` — the forward tile
  moves from ten hexes along the lane to twenty-two and the gun from (10, 0) to
  (10, 6). At ten the mission's slope no longer pays for the wood (the assault
  fell 0.54 short), and from the old firing position the only long-enough tiles
  sat *exactly* at the 88's maximum range — a stage balanced on a cliff, since
  one hex further is no fire at all and both orders press on for a reason that
  has nothing to do with either. Now `COVER` is 8 hexes from the gun and
  `FORWARD` 15 against a reach of 16: the advance halts by 2.97 and the assault
  presses by 1.98, against 3.24 and −0.54 before.
  **`the_stage_keeps_the_forward_tile_under_the_gun` is new** and pins the
  envelope through the danger arithmetic — Phase 2's comment promised that test
  by name and nobody had written it.
- `a_road_under_a_gun_is_worth_going_round` — its two (equal) objectives go
  from value 8 to 20. At 8 the crew took the firing position her own sweep
  offered and never chose between the two roads at all. Eighteen upwards
  restores the choice at the shipped `route_caution: 1.2`; twenty holds it from
  caution 1.2 to 16.
- `ground_the_enemy_reaches_first_is_worth_less_marching_for` —
  `contest_aversion` asserted at 15 instead of 5. The head start is a round of
  the medium's fire now, four times the size; the term flips from 12 upwards,
  and 15 keeps the stage off its own threshold, the margin Phase 2 left when it
  asserted 5 against a threshold of 4.

**A latent flaw cadence exposed.**

- `a_map_with_formations_but_no_missions_fights_exactly_as_the_flat_pool_did`
  zeroed `leader_lost` and not its mirror `recovery_near_leader`, so a crew who
  could see her formation leader shed two points the stripped run's crew could
  not. It was green only while nobody's pressure happened to cross a rung
  inside the eight-round window. Found at event 214: unit 0 brews up, everybody
  who saw it takes `ally_destroyed`, and unit 2 reaches Wavering in the flat
  run and not the other. Both fields zeroed now — that is what "a mod that
  never mentions the chain of command" means.

**Suppression: four stages that had to name their currency.**

`an_ordered_shot_that_cannot_penetrate_bounces_and_does_nothing` (now fights
the same battle twice, once with a belt that declares suppression and once with
one that does not, keeping its old assertion word for word on the second),
`a_gun_that_cannot_hurt_what_it_sees_holds_its_fire`,
`a_gun_with_nothing_left_to_break_expects_nothing`,
`a_remnant_platoon_is_a_story_not_a_gun`. The rule each defends — a shot worth
nothing is not taken, the damage ledger has no floor — is unchanged; what
changed is that "worth nothing" is a statement about a currency and the stage
now says which one.

**Three stages that stopped bruising and started killing.**

- `marching_under_fire` stands its crew at 4 rather than 5. Three 88 rounds a
  round at a crew whose nerve is now in the ledger beside her plate kills her
  in the staging round. The band is exactly {3, 4} — bounded by lethality above
  and by the gun's *sight* below, since at 2 the fire order the stage rests on
  is refused with `TargetNotSpotted` — and getting 4 cost two of the five
  callers a `seen(..)` they should always have had, CLAUDE.md's own rule for a
  stage that needs two crews in plain sight.
- `a_platoon_boards_rides_hidden_and_steps_off_where_the_ride_ends` drives two
  hexes rather than three. Three put the taxi at exactly six from the
  overwatch, the machine gun's maximum reach, and the platoon stepped off into
  a belt and abandoned before the last assertion could read her position. Only
  possible at all because a burst that cannot beat plate is now worth firing:
  the recon car used to hold its fire at the taxi's armour.
- `a_kinetic_round_that_beats_a_plate_up_close_fades_at_the_end_of_its_reach`
  silences the coaxial and gives the gun only AP. Both are the rest of the wave
  working: the coax now fires on every tick the main gun reloads and draws from
  the same rng stream, and at the far end of the reach the loader reaches for
  HE instead — correctly, since AP will not get through and HE at least
  rattles. It filters its counts on the new `ammo` field, which is a strictly
  better test than it was.

### Tests added

A new section of `tests/engine.rs`, *suppression and cadence join the
currency*, six tests, all mutation-checked:

| test | mutation that must fail it |
| --- | --- |
| `a_burst_that_cannot_get_through_still_counts_for_what_it_does_to_her_nerve` | `best_weapon_from` gates on `expected` instead of `worth`; `Round::loaded` drops suppression; `worth_of` drops `point_worth` |
| `fear_is_priced_by_the_same_arithmetic_that_charges_it` | `Round::loaded` drops suppression (the ladder and the twin then disagree by whole points) |
| `a_gun_that_fires_six_times_a_round_is_priced_six_times` | `incoming` sums per shot |
| `a_mod_that_says_nothing_about_suppression_plays_the_game_before` | `worth_of` drops `point_worth` |
| `the_loader_will_fire_a_belt_at_plate_she_cannot_beat_when_fear_is_worth_something` | `best_opportunity_shot` reads `.expected` |
| `suppression_and_what_fear_is_worth_are_data_and_are_read` | either of the above |

The second one is the one worth keeping: it fires several hundred bursts, sums
what `apply_pressure` actually charged, and requires the mean per shot *fired*
to land within a fifth of `expected_pressure`. Measured, it lands within a
couple of percent. Two price lists would be out by whole ladder points.

### Determinism, read before regenerating

**Mechanism** (1,193 of ~1,750 lines): ShotFired 120 → 113, ShotHit 26 → 36,
ShotMissed 59 → 46, ShotBounced 26 → 24, MoraleChanged 11 → 23, SetGoal 155 →
158, one `Defied` where there was none. Kills per seed 7/6/6/5 → 5/6/6/6.
Crews fire fewer shots and land more of them, which is what pricing a round of
fire ought to do to where they choose to stand.

**Content** (1,156 of ~1,500 lines): ShotFired 113 → 97, ShotHit 36 → 26,
ShotBounced 24 → 13, ShotMissed 46 → 47, MoraleChanged 23 → 19. Kills per seed
5/6/6/6 → 4/6/6/6, and `BattleEnded` 2 → 4: all four seeds now finish inside
the snapshot's twelve rounds where two used to run past it. Fewer shots landing
is the ladder's `accuracy` rung doing its job on crews who are now driven up
it.

Both broad and behavioural, which is what a rules change looks like; neither is
the small order-only diff that means an iteration-order bug.

### Measurements

`--sim --games 36 --sweep seed=0,1000,2000`, three columns per row.

| | baseline (`277838c`) | + mechanism | + content |
| --- | --- | --- | --- |
| outcome (Valkyries) | 22 / 18 / 19 | 24 / 21 / 18 | 22 / 17 / 26 |
| draws, stalemates | 0, 0 | 0, 0 | 0, 0 |
| length, rounds | 13.5 / 13.6 / 13.2 | 13.1 / 13.7 / 13.1 | 13.0 / 13.2 / 13.1 |
| hit% | 51 / 55 / 56 | 58 / 53 / 51 | 65 / 70 / 65 |
| bounce% | 33 / 33 / 33 | 33 / 34 / 33 | 24 / 24 / 25 |
| cadets out | 10.8 / 12.6 / 12.0 | 11.8 / 11.7 / 11.1 | 10.4 / 10.5 / 10.7 |
| medium tank | 67/64 | 58/66 | 63/74 |
| tank destroyer | 75/27 | 81/28 | 85/34 |
| artillery | 76/43 | 81/39 | 84/37 |
| 5 over 3, both ends | 37–32 | 38–32 | 34–37 (spread 7) |
| 5 over 1, both ends | 37–31 | 38–31 | 40–31 (spread 5) |

The skill rows say nothing: both move inside their own three-seed spreads and
in opposite directions. That is the fourth arc in a row where they have not
discriminated, and it is the arena's limit rather than this chunk's result —
see "the arena is the instrument limit again" above.

**Perf**, median of three quiet runs:

| | baseline | + mechanism | + content |
| --- | --- | --- | --- |
| round resolution | 1.49 ms | 1.97 ms | 1.84 ms |
| utility order (diff. 3) | 0.09 ms | 0.08 ms | 0.08 ms |
| `reachable()` | 18.9 µs | 14.8 µs | 15.4 µs |
| `roads()` | 143.1 µs | 109.9 µs | 111.3 µs |
| `unit_vision` cold | 93.5 µs | 90.0 µs | 93.4 µs |

**`score_tile` paid nothing for the second expectation**, which is the number
the plan asked to watch: `expected_shot` computes damage, pressure and worth
off one hit chance and one `shot_profile`, so the pressure half is a handful of
flops on a walk that was happening anyway. Round resolution is up a fifth over
the baseline and is the row CLAUDE.md already warns moves with how well the AI
plays rather than with how much work the loop does — crews now stand and trade
fire where they used to manoeuvre, and the shot census agrees.

**`playthrough 7`**: six machine-gun bursts, none of which could beat what they
were fired at — *"Irma Krieger (medium_tank) fires mg (opportunity)"*, bouncing
off a light tank's front. Before this wave the count was **zero on every seed
tried**, because a burst that could not penetrate was worth exactly nothing to
the crew holding the trigger. Seed 5 fires seven. Engagements on
`river_crossing` run to about eleven hexes and a coaxial reaches six, so it
speaks when somebody closes.

### What this wave leaves behind — the next things of their kind

- [ ] **Every landing shot is now worth something, and "a shot that
      accomplishes nothing" has stopped existing.** The morale ladder charges
      `hit + penetrated` for any round that gets through, so once fear is
      priced at all, a remnant platoon whose damage `mustered` has scaled to
      nothing still expects three points of pressure a shot against soft
      targets and opens up. `expected_pressure` is mirroring the resolver
      honestly — `apply_pressure` really does charge it — so this is a
      question about the *ladder*, not about the pricing: should the outcome
      price be scaled by what the round actually spent? It is the reason four
      test stages had to name their currency, and it is the single biggest
      behavioural surprise in the wave.
- [ ] **The fighting terms grew and the ground terms did not.** Attack and
      threat are a round of fire now, two to twelve times what they were;
      objective values (2–5 on the shipped maps), `mission_weight` (2.0),
      `impatience` (0.35), the mass band (−0.45 to −0.12) and the terrain
      priors were all quoted against a per-shot currency and are unchanged.
      `deviation_cost` and `attack_scale` were the two that crossed a
      threshold and had to move; the rest are simply worth proportionally less
      than they were, everywhere. The lead's planned re-quoting of orders ("a
      delegated order is worth a quarter of what she has left per round") is
      the right shape for the whole family, not just for orders.
- [ ] **`Bearing` still names one weapon per enemy**, so a tank whose coaxial
      now genuinely wants to fire is under-reported in the panel by the gun
      that is not its best. Phase 2 already listed this; suppression makes it
      bite, because the second weapon is exactly the one the new rule is about.
- [x] **The overlay's bands are worse than they were**, for the reason Phase 2
      predicted: they are linear in a quantity that is not, and worth per round
      is three to six times the per-shot figure they were sized against.
      Yellow is now unreachable rather than merely rare. Replaced by a
      gradient the same day (`panel::danger_tint`).
- [ ] **`rifle_ball` is unswept content.** See above.
- [ ] **`ai::threats` / `threatened` is a boolean over a currency that now has
      two halves.** It answers "is anybody shooting at me" and picks up the
      worth gate for free, which is why a crew under machine-gun fire now
      counts as under fire — probably right, and nobody has measured it. The
      planned rewrite onto `fire_on` should decide whether the battle drill
      wants a *threshold* rather than a predicate now that being shot at
      harmlessly is a thing the engine can express.
- [ ] **A `Shot` struct**, still. `expected_shot`, `expected_damage` and
      `expected_pressure` all carry `#[allow(clippy::too_many_arguments)]` at
      eight; there are three of them now rather than one.

## Wave 2 — readers, orders and objectives: what landed

Two commits: `ecea65a` the readers, `cc8ca23` the objective and the order.
The wave's brief was Wave 1's last open item — *"the objective is not in the
currency, and on real ground the best commander declines it"* — and the
headline is that the diagnosis was half right, in an instructive way.

**On `ridge_arena` at eight seeds and 576 battles a row, `5 over 1` goes
45.3% → 53.5% and `5 over 3` 48.3% → 52.3%.** Both rows move together and the
control row is honest (`the ends` 50.0% → 52.8%, zero draws at every
pairing). That is the largest, and the first decisive, movement the skill
table has recorded in five arcs.

**But two thirds of it came from the readers, not from the objective.** The
`ecea65a` commit alone — which touched no weight and added no field — took
`5 over 1` from 45.3% to 52.3%. What Wave 1 read as "nothing prices what
holding the crest is *for*" was at least as much "the reflex that decides
where a frightened crew stands was reading a different model from the one
that decides where she drives". A crew who broke for cover went to the
reachable tile with the highest terrain `cover`, which on ground built out of
woods a crest looks into is frequently the worst hex on the map.

### Part 1 — the readers walk one gate

`ai::threats` and `ai::threatened` were a second walk over the visible
enemies with a gate of their own, six callers between them. `threats` is
`fire_on(..)`'s membership now and `threatened` is `incoming(..).worth > 0`.
The two were arithmetically identical when they were joined —
`best_weapon_from` admits a gun precisely when one shot from it is worth
something, and a sum of positive terms is positive — so that half is a
refactor with a test on it rather than a behaviour change. What it buys is
that they stay identical, which is what
`there_is_one_answer_to_who_can_shoot_her` defends: the stage's seven
bearings run from 1.27 to 8.99 substance points a round, so any threshold
reintroduced above 1.27 fails it.

`danger::incoming_from(registry, state, unit, at, &[UnitId])` is the one new
public shape: the same private `guns_bearing_on`, restricted to a named list.
The drill needs it because it has to price ground against the guns this crew
has actually caught up with on her per-enemy `spotted_since` clock. Reacting
to a gun she has not noticed would be the reaction-latency defect rebuilt
inside the reflex that latency is about.

**The drill and the rout are two different things and the code now says which
is which**, which is the designer's own split:

| | first key | second key | what it means |
| --- | --- | --- | --- |
| the drill (`run_crew_drill`) | lowest `incoming_from` worth | cheapest drive, then the coordinate | *minimise the fire on me* — no distance term at all, so a hex nearer the gun that the gun cannot see beats a hex further off in the open |
| the rout (`flight_destination`) | most distance from every threat | lowest `incoming_from` worth | *get away* — the second key only sorts the hexes that tie on the first, which also keeps it cheap (a handful of hexes rather than ninety) |

`worth_key` quantises a round of expected fire to about a thousandth of a
substance point, so the tiebreak has a total order and near-identical ground
is settled by the cheaper drive rather than by a rounding artefact.

**The `perf` scare did not happen.** The drill now prices every reachable tile
per tick for a threatened crew, and round resolution went *down*, 1.90 →
1.78 ms. The drill only ever visits an idle crew, and a crew who goes where
the gun cannot see her spends fewer ticks being shot at. `reachable` 19.7 →
15.9 µs and `roads` 151.0 → 111.3 µs moved for the reason CLAUDE.md already
gives: they measure work in a particular game state, and the state changed.

`playthrough 7 battle_forest` has the clearest single before-and-after in the
wave. Raven 2, tick 10, under fire from a light tank two hexes off: she used
to break one hex to the nearest trees at (9, 25, −34), and now drives four
hexes to (6, 27, −33), out of the gun's sight altogether.

### Part 2 — the objective and the order in the currency

`planner.score_worth` is what one point of an objective's `value` is worth in
substance points a round; `planner.exit_urgency` rides on it, because it was
quoted in objective-value units and would otherwise have silently stopped
meaning that. `planner.order_worth` is the share of what a crew still has
aboard, per round, that being on the ground her commander named is worth to
her. `planner.mission_weight` is gone.

**A retired field is warned about, not refused.** Serde ignores what it does
not recognise, so a mod that had tuned `mission_weight` would have loaded
cleanly and played a different game from the one it wrote, with nothing to
grep for. It deserialises under its old name into a field nothing serialises,
so `--set planner.mission_weight=4` fails with the list of fields that do
exist (which names `order_worth`), and `validate-mods` prints one warning
naming the replacement and the arithmetic to port it. A warning rather than
an error because carrying a stale key from an older engine is a legitimate
thing for a mod to do; silence is not.

Three shape decisions, recorded because each had a live alternative:

- **The arrival rewards in `mission_value` are rebased 1.5 → 1.0 and 0.75 →
  0.5**, once, so that `order_worth` means the share it says it means rather
  than two thirds of it. Keeping 1.5 and setting `order_worth` to 0.167 would
  have reproduced the old behaviour *exactly* for a typical chassis — both the
  arrival step and the slope — and was rejected because the number would then
  have needed a mental multiplication to read, which is the whole thing this
  wave was undoing. What the rebase costs is stated on the field: arriving is
  worth about seven hexes of `distance_decay`'s slope where it was ten.
- **`distance_decay` stays one slope**, and its justification is better than
  it was. It used to be shared because `mission_weight` was quoted in
  objective-value units and the sentence "an order pulls about as hard as the
  ford" was only true while the gradients matched. The two are quoted in
  different things now, so the reason is that the slope is not a statement
  about what ground is worth at all — it is *how far off a crew can still tell
  which way to drive*, a fact about her and the map.
  `an_order_and_an_objective_are_led_to_by_the_same_slope` passes unweakened.
- **`Withdraw` is the one mission still priced on the objective scale.** Being
  ordered out is not worth *less* to a crew who is nearly finished, and
  quoting it as a share of what is left would have inverted exactly the thing
  the unordered version's flight gate exists to say.

`score_tile` hoists `left` out of `exposure`, and the comment says what the
two readings of it mean together: danger is priced as a *fraction* of her and
an order as a *share* of her, so the order/threat ratio scales as `left²`. A
half-destroyed crew weighs her orders against the guns covering them about
four times more cautiously than a fresh one. That is defensible — there is
simply less of her, twice over — but it is a design decision nobody has
argued with and it is listed below.

**The mechanism is provably behaviour-neutral.** At `score_worth: 1.0` the
determinism snapshot passed *unregenerated*, because 1.0 is exactly the game
before and the baseline's sides field no formations, so `mission_value` is
never called. Every line of the content diff is therefore attributable to the
one number. Worth doing again; it is the second wave running in which the
trick has turned an unreadable diff into a legible one.

### Content, and how it was chosen

`score_worth: 3.0` (1.0 is the game before; the Rust default follows the
shipped value, as `deviation_cost`'s has three times, because the base mod
must ship the defaults). `order_worth: 0.25`.

**The `score_worth` sweep is a null and the reason is the result.** On
`--arena ridge_arena`, `--only skill --absolute --games 36`, eight seeds:

| `score_worth` | 5 over 1 | 5 over 3 | the ends | 5v5 draws |
| --- | --- | --- | --- | --- |
| 0 | **9.2%** | 9.2% | 32.6% | **225 of 288** |
| 1 (the game before) | 52.3% | 51.6% | 51.4% | 1 |
| 2 | 53.3% | 50.5% | 52.8% | 1 |
| **3 (ships)** | **53.5%** | **52.3%** | 52.8% | 0 |
| 4 | 52.1% | 51.7% | 52.8% | 0 |
| 6 | 53.0% | 51.4% | 53.3% | 0 |
| 12 | 54.0% | 51.4% | 51.7% | 0 |

One spread of noise from 1 to 12. **This is not `pull_under_fire`'s kind of
null** — that term was never evaluated; this one is evaluated on every
candidate tile of every sweep. It is a *symmetric* number: both commanders get
the same rate, so it changes what a battle is about rather than which side is
better at it, and a table whose entire content is one side against another
cannot see it. The one value the table can see is zero, and it is
catastrophic: 9.2% and 225 draws of 288, because nothing then leaves cover.
**That is a third species of null worth naming, beside "no effect" and "never
evaluated": symmetric, and therefore invisible to every table this harness
has.**

What `score_worth` does move is the battle. Over the determinism baseline's
four seeds of `river_crossing`, 1.0 → 3.0 takes `ObjectiveTaken` from 5 to 8
and the rounds fought from 22 to 17, with two more vehicles destroyed. 3.0 is
also the rate at which the ridge's crest, worth three, prices at 13.5
substance points against the 8 to 29 a round a found gun puts on a crew
standing on it — inside a factor of two of the fire it has to argue with,
which is the comparison the field exists to make possible. Below that it is
arithmetic nobody consults.

**`order_worth`, swept on the delegation tax table** at 288 battles a cell
with `--set planner.devolved=1.1`, so that both commanded doctrines actually
assign ground. (In the shipped game elastic defence devolves and issues none,
which is why the same sweep without it moves three wins in 36 and says
nothing — the same caveat DONE.md records for `mission_weight`.)

| `order_worth` | massed armour's tax | bounding overwatch's tax |
| --- | --- | --- |
| 0.0 | +2 | +53 |
| 0.1 | −6 | +43 |
| **0.25 (ships)** | **−10** | **+35** |
| 0.5 | −15 | +29 |
| 1.0 | −11 | +30 |
| 2.0 | −18 | +30 |

A doctrine's tax is what it loses by fighting through missions instead of for
itself against the same flat opponent; zero is the target. **Both flat
controls are bit-identical at every value**, which is the additivity claim
measured rather than asserted: this number reaches commanded sides and
nothing else. Massed armour crosses zero under a tenth and overshoots;
bounding overwatch takes most of the improvement available to it by a quarter
and flattens. A quarter is the designer's number and it is where the first is
still near zero and the second has stopped improving.

### The autonomy line

`the_shipped_doctrines_straddle_the_price_of_deviating` passes unweakened and
**`deviation_cost` did not have to move**, which is the first time a wave has
changed the currency without moving it. The band did move, and the test's
comment now carries the re-measurement: on `pressed_stage` massed armour obeys
from **10** and elastic defence starts obeying at **24**, so the straddle is
10–23 where Wave 1 measured 12–29. The ordered ground on that stage is worth
3.25 to a thirteen-point medium where it was a flat 3.0, and its slope is half
again as steep. 12.0 sits inside with room at both ends.

`a_binding_march_presses_on_where_an_ordinary_one_takes_cover`,
`a_cut_off_unit_keeps_the_orders_she_had` and
`a_binding_mission_is_not_discounted_by_a_loose_doctrine` all pass
unweakened. `Latitude::Binding` reaches the same one number it always did.

### Part 3 — the rest of the family, surveyed

The brief for this part was to say, for every weight quoted in the
evaluator's currency, what unit it is in, whether cadence and the objective
rate left it proportionally cheaper, and either restate it with a sweep or
leave it with a written reason.

| number | quoted in | left cheaper? | what was done |
| --- | --- | --- | --- |
| `planner.impatience` 0.35 | `score_tile` points per round of driving | yes, by the full 2–12× | **swept, null.** Ridge, eight seeds, `5 over 1` / `5 over 3`: 0.35 → 53.5 / 52.3, 0.7 → 51.4 / 51.0, 1.4 → 52.4 / 53.8, 3.0 → 54.0 / 52.4. Left at 0.35. |
| `planner.plateau` 0.3 | `score_tile` points (a band width) | yes | **swept, null.** 0.1 → 53.5 / 51.6, 0.3 → 53.5 / 52.3, 1.0 → 52.3 / 52.3, 3.0 → 51.9 / 51.7. Mildly worse wide, inside the spread. Left at 0.3. |
| `planner.exit_urgency` 3.0 | objective-value points | it would have been | **restated by construction**: it is multiplied by `score_worth` with every other objective, so "an exit is worth about a good piece of ground" stays literally true. The relationship is the invariant, not the number. |
| `planner.pull_under_fire` 0.25 | a dimensionless share of the mission term | no | untouched. A share of a term is immune to that term's units. Still a number the AI never meets (DONE.md's null); still player-facing. |
| `planner.cover_prior` 0.03, `elevation_prior` 0.4 | `score_tile` points, gated to tiles no found gun reaches | partly | **left, with a reason.** The gate means they never argue with the fighting terms at all; the only thing they argue with is the objective pull, and the `score_worth` sweep above varies exactly that ratio twelve-fold and comes back null. Sweeping the prior would be sweeping the same ratio from the other end. |
| `doctrine.route_caution` 0.25–1.2, `contest_aversion` 0.15–0.7 | `score_tile` points per round of exposure / lateness | yes | **swept, null.** On the shipped maps, `massed_armor.route_caution` at 1.0 is a *bit-identical* game to 0.25, and 4.0 and 12.0 move nothing outside the ±6 band; `contest_aversion` is bit-identical at 0.6 and moves two wins at 7.0. Wave 1 already restated the *test* thresholds (asserting `contest_aversion` at 15 rather than 5); the shipped content still says nothing the instrument can hear. |
| `planner.deviation_cost` 12.0 | `score_tile` points | yes | **re-measured, not moved.** Band 12–29 → 10–23; see above. |
| `planner.horizon_rounds`, `boarding_rounds`, `devolved`, `distance_decay` | rounds, rounds, a delegation level, a per-hex fraction | no | a currency change cannot reach any of them. |
| the mass band, −0.45 / −0.15 at one and two hexes and −0.12 per hex beyond support, × `concentration` | `score_tile` points | **yes, and it is the largest one left** | **not restated, and not restatable: it is three constants in Rust.** See below. |
| the kill bonus +4.0, the advance slope 0.3 × `aggression`, the centre-seeking 0.15 × `scouting` | `score_tile` points | yes | same: bare Rust, unsweepable, unmeasured. |

So the survey's real finding is not a number. **Every weight in the family
that is *data* was swept and came back null; every weight that is still bare
Rust could not be swept at all, and between them they are now the largest
un-restated block in `score_tile`.** The mass band in particular is a
spacing rule denominated in a currency that has grown by an order of
magnitude since it was written, and the reason nobody has noticed is that
nobody can ask.

### Determinism, read before regenerating

Twice, once per commit, and both broad and behavioural.

**The readers** (223 of 1492 lines): `TookCover` 3 → 5, `ShotFired` 97 → 108,
`ShotBounced` 13 → 22, kills per seed 4/6/6/6 → 4/6/6/5. Seed 4 runs eleven
rounds where it ran five. The clearest single case is seed 1, unit 4: she
takes cover one hex north mid-round, is struck frontally instead of in the
side, and everything after that is a different battle.

**The objective** (1022 of 1371 lines): `ObjectiveTaken` 5 → 8,
`RoundStarted` 22 → 17, `UnitDestroyed` 21 → 23, `UnitSpotted` 72 → 58. Three
of four seeds finish sooner — seed 4 goes from eleven rounds to three, seed 3
from four to eight with the ford changing hands four times. Ground changes
hands more and the fights over it are settled faster, which is what pricing
the scoreboard in the currency is supposed to do.

### Test stages repaired, with their margins

Five, none weakened.

- `a_crew_caught_in_the_open_breaks_for_cover_before_the_round_ends` asserted
  `Some("forest")`. It asserts what the currency can claim instead — the hex
  she took is one the walker expects strictly less on — priced against **the
  board the drill was looking at** rather than the end of the round, since
  both crews move inside a tick and pricing after it reads "the gun expects
  nothing anywhere". `spring_the_ambush` returns that board as a fourth
  element.
- `a_crew_with_no_better_ground_in_reach_stands_where_she_is` asserted that
  nobody moves on a bare field. True of the terrain table, false of the
  currency: a gun loses accuracy with range, so the far corner of a billiard
  table really is quieter and a crew who has noticed the gun really ought to
  back off. It asserts the strictness itself now — every dash goes to strictly
  quieter ground — which is the rule the old stage was one consequence of,
  and it requires at least one dash so it cannot pass on an empty loop.
- `driving_is_counted_once_however_she_came_to_drive` (gunnery) asserted
  `moved == 2`. The ordered march ends inside the enemy's arc and the drill
  adds a third hex, so it counts the hexes she actually crossed and requires
  `moved` to equal them — a stricter reading of the same rule.
- `FLIGHT_SEED` 84 → 3. The rout's new second key sends her to a different hex
  of the same ring, she presents a different aspect, and on 84 the gun killed
  her in round two before there was anything to watch. Scanned rather than
  guessed: 63 of the first 400 seeds have her open the range in the first
  round and in two of three.
- `what_the_ground_a_commander_names_is_worth_is_a_mod_decision` sweeps
  `order_worth` (0 / 0.25 / 0.75) instead of `mission_weight`, and
  `ordered_against_the_ground` is parameterised by chassis so the
  heavy-versus-scout comparison is two runs of one stage rather than two crews
  on one, which would have put each in the other's mass term.

### Tests added

A new section of `tests/engine.rs`, *one walk, one gate: the readers of the
currency*, plus three in the order-versus-terrain section and one in
`tests/harness.rs`. All mutation-checked.

| test | mutation that must fail it |
| --- | --- |
| `there_is_one_answer_to_who_can_shoot_her` | any threshold in `threats` above 1.27, the cheapest of the stage's seven bearings |
| `the_drill_goes_where_the_gun_cannot_see_her_not_to_the_nearest_wood` | the drill reads terrain `cover` again |
| `a_frightened_crew_runs_from_the_gun_and_an_orderly_one_ducks_out_of_its_sight` | the rout drops its distance key and minimises fire |
| `an_order_is_worth_a_share_of_herself_so_it_asks_more_of_a_heavier_crew` | `order_worth * left` → `order_worth * 11.0` |
| `a_point_of_score_is_priced_in_the_currency` | `score_worth` dropped from `objective_value` |
| `a_delegated_crew_takes_ordered_ground_until_it_costs_more_than_her_share_of_herself` | the delegation floor stops moving with latitude |
| `a_mod_setting_a_field_that_no_longer_exists_is_told_which_one_replaced_it` | the retired field is dropped instead of captured |

The sixth measures the margins it asserts, which is the one worth keeping: a
recon car with nine substance points left, seven hexes from a tank
destroyer's 88, is looking at 29.2 points of expected fire a round, and
crosses at `order_worth` 1.20 delegated against 0.95 binding — four fifths of
it, the ratio of 0.8 to the floored 1.0 and nothing more. At the shipped
quarter she declines that order, which is the right answer.

### Measurements

`--only skill --absolute --games 36`, eight seeds, 576 battles a row.

| | 5 over 1 | 5 over 3 | the ends | 5v5 draws |
| --- | --- | --- | --- | --- |
| `ridge_arena`, baseline `1f7d76e` | 261–315 **45.3%** | 278–298 48.3% | 288–288 50.0% | 0 |
| `ridge_arena`, + readers | 301–275 52.3% | 297–279 51.6% | 296–279 51.4% | 1 |
| `ridge_arena`, + the currency | 308–268 **53.5%** | 301–274 **52.3%** | 304–272 52.8% | 0 |
| `skill_arena`, baseline | 301–268 52.3% | 285–283 49.5% | 236–331 41.0% | 7 |
| `skill_arena`, + readers | 282–287 49.0% | 278–285 48.3% | 277–293 48.1% | 3 |
| `skill_arena`, + the currency | 290–281 50.3% | 282–285 49.0% | 269–298 46.7% | 7 |

The old arena gives back two points on `5 over 1` and gains six on its own
control row, which is the trade this project has now made twice: ground that
punishes a wrong choice discriminates, and a radius-10 hexagon with two
objectives does not.

Delegation tax over three seeds: massed armour 8 → 1 → 4, bounding overwatch
14 → 13 → 19; all inside the ±6 band a single 36-battle pairing wanders in,
let alone three.

`perf`, median of three quiet runs:

| | baseline | + readers | + the currency |
| --- | --- | --- | --- |
| round resolution | 1.90 ms | 1.78 ms | 1.99 ms |
| `reachable()` | 19.7 µs | 15.9 µs | 15.8 µs |
| `roads()` | 151.0 µs | 111.3 µs | 111.5 µs |
| `unit_vision` cold | 99.7 µs | 93.1 µs | 93.3 µs |
| utility order (diff. 3) | 0.10 ms | 0.08 ms | 0.08 ms |

Five per cent over the baseline in all, against the twenty the brief allowed,
and the row that moved is the one CLAUDE.md already warns moves with how well
the AI plays. Nothing on either commit's path is hot: `objective_value` and
`mission_value` each gained one multiplication, and the drill's per-tick
sweep is paid only by an idle crew who has noticed a gun.

`playthrough 7 battle_forest`: "breaks for cover" 6 → 4 → 6. The count is not
the interesting number; where they go is.

### What this wave leaves behind — the next things of their kind

- [ ] **`score_tile`'s remaining bare-Rust weights are the largest
      un-restated block in the currency**, and the survey above is the
      argument: every weight that is data was swept and came back null, and
      every weight that is not could not be asked at all. The mass band
      (−0.45 / −0.15 / −0.12 × `concentration`), the +4.0 kill bonus, the 0.3
      advance slope and the 0.15 centre-seeking fallback are four numbers in
      the same sum as three that have each moved twice. They want to be
      `planner` fields for the reason the others are: *the prize is the
      sweep*.
- [ ] **A symmetric number is invisible to every table this harness has.**
      `score_worth` is evaluated on every candidate tile and moves
      `ObjectiveTaken` by sixty per cent, and the skill table cannot tell 1
      from 12 because both commanders get the same rate. The harness needs a
      table whose question is *what was this battle about* rather than *who
      won it* — objectives taken and held, rounds spent in contact, ground
      changing hands. `sim` has the columns for shots and casualties and none
      for ground.
- [ ] **The order/threat ratio is quadratic in what she has left.** Danger is
      priced as a fraction of her (`exposure`'s fragility) and an order as a
      share of her (`order_worth * left`), so a crew at half substance values
      her orders at half and her danger at double. That follows from two
      separately defensible rules and nobody has decided it is the intended
      one. The alternative is to quote the order against her *full*
      complement, which keeps "the same order is worth more to a heavy tank"
      while making an order's weight constant through a battle. A question for
      the designer, not a defect.
- [ ] **The mid-round drill has no counterweight.** At the planning table a
      crew balances threat against the shot in front of her and the ground she
      was sent to; mid-round the drill reads danger alone, so an idle crew who
      has arrived where she was ordered and is under fire will back off it.
      That is right for a crew nobody has told anything and questionable for
      one who has just arrived at her objective, and it is the mechanism
      behind `driving_is_counted_once_however_she_came_to_drive`'s third hex.
      It is also unaffected by `Latitude`, which is read only at the planning
      table. Worth a decision before the player meets it.
- [ ] **`ai::threatened` is still a predicate over a currency with a
      magnitude.** Wave 1 asked whether the drill wants a *threshold* rather
      than a boolean now that being shot at harmlessly is expressible. Making
      the readers walk one gate did not answer it; it only made the question
      askable in one place. A `threatened_above(worth)` would be one line.
- [ ] **`Bearing` still names one weapon per enemy**, and **the overlay's
      bands are still linear in a quantity that is not** — both carried over
      from Wave 1 and both belong to the presentation layer the lead is
      rewriting.

## Wave 3 — enums: what landed

Phase 3 items **3b** and **3c**, on `wave3/enums` off `7553de7`. Two commits,
readable alone. **No rule changed:** `tests/snapshots/event_stream.txt` passes
unregenerated after each, which is the evidence. All eleven tours pass
headless; clippy `-D warnings`, `cargo fmt --check` and `validate-mods` clean.
Perf, which an enum in place of flags should not move, and does not: round
resolution 2.04 → 1.99 ms, `unit_vision` 88.7 → 93.5 µs, `roads()` 105.8 →
111.3 µs, `reachable()` 15.2 → 15.9 µs — all inside the run-to-run spread on
this machine (one round-resolution figure's own seed spread is 1.31–2.35 ms),
and the event stream is byte-identical, so the same decisions are being made
over the same work.

### 3b — a commander's personal order

```rust
pub struct March { pub to: Hex, pub latitude: Latitude }
pub enum PersonalOrder { Holding, Marching(March) }
// on Unit:
pub orders: Option<PersonalOrder>,
```

`None` is a crew under her formation's mission — the state every unit spawns
in and the one a fresh formation order returns her to — so *detached* is
`orders.is_some()`. `Unit::detached()` and `Unit::march()` are the two
questions the code was already asking; `PersonalOrder::march()` is the **only**
route from a unit to a `Latitude`, so the drill gate cannot read insistence
off a crew standing still. `WaitingOrders` carries the same `March`, which
retires its `destination` / `latitude` pair.

**Every site the triple used to be set or cleared at**, all in
`battle/orders.rs` unless noted:

| site | was | is |
| --- | --- | --- |
| `BattleState::spawn_unit` (`battle/mod.rs`) | `detached: false, tasking: None, latitude: default()` | `orders: None` |
| `Order::ClearIntent` — the recall | all three cleared | `orders = None` |
| `radio()`, with a destination | `tasking = Some(to)`, `latitude = latitude` | `orders = Some(Marching(March { to, latitude }))` |
| `radio()`, after either half | `detached = true` | `orders.get_or_insert(Holding)` |
| `radio()`, no contact | `command.hold_orders(id, to, fire, latitude)` | `hold_orders(id, to.map(March…), fire)` |
| `deliver_waiting_orders`, with a destination | `tasking`, `latitude` from the slot | `orders = Some(Marching(march))` |
| `deliver_waiting_orders`, always | `detached = true` | `orders.get_or_insert(Holding)` |
| `set_mission` — a fresh formation order collects everyone | all three cleared, per member | `orders = None` |
| `begin_round`'s arrival pass | `tasking = None`, `latitude = default()`, `detached` left alone | `orders = Some(Holding)` |
| `CommandState::hold_orders` (`battle/command.rs`) | `slot.destination`, `slot.latitude` under one `if` | `slot.march` |

`get_or_insert` rather than an assignment at the two detach sites is what
keeps `an_order_about_her_gun_says_nothing_about_her_march` true — and now
true structurally, since there is no separate latitude field left to leave
alone.

**Rejected.** A four-variant `PersonalOrder { Free, Holding, Marching, … }`
with `Free` in place of the `Option` — it makes "is she detached" a match over
two variants rather than an `is_some`, and puts the default state inside the
type where every `Option` idiom in the file already spells it outside.
Keeping `detached` as its own bool beside the order — that is the field that
could disagree, and the arrival transition (destination gone, detachment kept)
is exactly where it used to have to be remembered. Naming the field `tasking`
— it now holds the insistence and the detachment too, and the old word names
only the destination.

**Prose rules in CLAUDE.md's *Latitude* section the type now makes redundant**
(for the lead to delete):

- *"It belongs to the destination, not the cadet: set where `tasking` is set,
  cleared everywhere `tasking` clears, carried in `WaitingOrders`."* — the
  latitude lives inside `March`; there is no "where `tasking` is set" distinct
  from where the latitude is set, and `WaitingOrders` carries the same struct.
- *"A radioed order with `to: None` leaves it alone
  (`an_order_about_her_gun_says_nothing_about_her_march`)."* — an order with
  no destination cannot name a latitude to leave alone. Keep the **test**; the
  rule no longer needs stating.
- *"On a crew it is read in exactly one place, the drill gate in
  `ai/command.rs`. A second `yields_to_drill()` means the model drifted"* —
  still true and still worth saying, but the reachability half is now
  structural: `march()` is the only accessor that yields a `Latitude`, and it
  answers `None` for a crew who is merely holding.

Everything else in that section is about *what latitude means* (the crew
versus formation split, the contact damping, `Delegated` as the default, the
drill's round-two rule) and is not made redundant by a type.

`a_personal_order_is_taken_back_whole_or_not_at_all` (`tests/engine.rs`) pins
both exits on both halves at once.

### 3c — a vehicle's outcome

```rust
pub enum Destruction { Crushed, Abandoned, BrewedUp, CrewSpent }
pub enum Fate {
    Fighting { doom: Option<Destruction> },
    Destroyed(Destruction),
    Exited,
}
// on Unit:
pub fate: Fate,
```

`doom` is **inside** `Fighting` rather than a fourth variant beside it, so
that "is she on the field" stays one variant. Damage lands during a tick and
death is reaped at the end of it, so a doomed vehicle is genuinely still on
the board — targetable, blocking, shooting — and a `Doomed` variant would have
made `alive` a two-variant match that a reader written next year gets wrong.

Readers: `Unit::alive()`, `exited()`, `destruction()`; `Fate::lost()` behind
`surviving_units` / `lost_units` and `check_victory`'s decapitation pass.
Writers: `Unit::doomed_by(how)`, `destroy()`, `withdraw()` — three verbs where
there were five assignments. `ai/mcts.rs::determinize`, which had to set
`alive = false` **and** `exited = true` in the right combination or open its
search on a world where the enemy's commanding officer was already dead, is
now one `withdraw()`. `panel.rs`'s roll call matches `Fate` exhaustively, so
the next fate stops the UI compiling until somebody says what to call it.

**The one real decision, and it was measured first.** The old flags were
independent, all stayed set, and each reader had its own fixed order for
consulting them — an ordering written out three times and able to disagree. A
throwaway probe over 900 AI battles (9,131 vehicles lost) found two flags on
one hull about 250 times, in **all three** pairings, so this is a case that
happens rather than one to argue about. `Destruction::supersedes` makes it one
rule at the write: **burning beats the crew leaving beats the hull being
crushed**. The middle rung is the load-bearing one — `behind_armor_effects`
refuses to roll for a crew who has already gone, so an abandonment that a
later shell's blast overwrote would let the same cadets abandon the same tank
twice.

The only visible consequence anywhere in the tree is `balance`'s cause
histogram, where a hull that was both crushed and abandoned is now named by
the abandonment: `--sim --games 36` goes from `abandoned 96 brewed 95 crew out
86 wrecked by blast 78` to `abandoned 97 … wrecked by blast 77`. One loss in
355 changed label; nothing else moved.

**Rejected.** Storing the destruction as a set (or a struct of three bools
inside `Destroyed`) — lossless, and exactly the shape the item exists to
remove, one level down. The other severity order (brewed > crushed >
abandoned), which matches the old *read* precedence in `balance.rs` exactly
and would have kept that histogram byte-identical — it loses the abandonment
under a later crushing, which is a simulation difference (a second
`Event::Abandoned`) traded for a diagnostic one, the wrong way round.
Recording only whether she was lost and pushing the cause onto the event
stream — the campaign and the harness both read it off the unit after the
battle, and reconstructing it from events is the drift this item removes.

`a_hull_that_takes_two_ends_in_one_tick_is_remembered_by_the_more_telling_one`
pins the precedence in both orders for all three pairings plus the doom /
death / `CrewSpent` transitions; mutation-checked, with last-write-wins and an
inverted rank both failing it.

**Prose in CLAUDE.md now structural.** The *Invariants* bullet *"`alive` and
'standing on a hex' are two different questions… classify with
`surviving_units()` / `lost_units()`, and never reimplement either by scanning
`units`"* is half retired: the classify half is now the difference between
`Fate::Exited` and `Fate::Destroyed(_)`, and `lost_units` reads `Fate::lost`,
so `!alive` as a loss test is no longer expressible without going through the
accessor that says so. The *passenger* half — a passenger is alive and on no
hex — is untouched and still needs saying, because that is about `aboard`
rather than about fate. Likewise the *Objectives* bullet *"Never classify a
unit at the end of a battle by `!alive`"*: keep the sentence, drop the warning
tone. **Known issues → Robustness** should lose the *"`Unit`'s outcome is five
booleans"* entry entirely, and its "where is she going" half drops `tasking`
from the list of five fields.

### Saves

`SAVE_VERSION` 3 → 4 (3b) → **5** (3c). Old saves are **refused**, not
migrated, and deliberately: both new fields `#[serde(default)]` to the benign
value, so a version-3 file would open with every crew quietly back under her
formation's mission and a version-4 file with every wreck on the board
fighting again. `SaveError::Version` names the version found, which is the
loud failure worth having while there is no released build to migrate from.

### Left for whoever picks this up

- **`Unit`'s "where is she going" is still four fields** (`intent.path`,
  `orders`, `goal`, `boarding`) plus the formation mission. 3b took one of the
  five off the list; the remaining four are genuinely different time scales
  (this tick, this errand, her own plan, a rendezvous) and folding them wants
  a design decision rather than a refactor.
- **`behind_armor_effects`' bail-out guard reads two of the four
  destructions**, not all of them. A hull crushed by blast this tick can still
  roll for a bail-out. That is the rule as it stood and this chunk kept it
  bit-for-bit, but it now reads oddly next to the enum, and `supersedes` exists
  partly to protect it. Worth a designer's ruling.
