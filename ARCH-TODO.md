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

## Phase 1 — the instruments

- [ ] **1a. A terrain-varied arena.** `harness/arena.rs` is a radius-10
      hexagon with a handful of forest hexes and no objectives worth arguing
      about, and it is now the third measurement to have blamed it by name.
      It must stay exactly mirrored — `assert_arena_is_mirrored` is the check,
      and the side-B edge is what happens when it is not — so every feature is
      declared once and reflected. Elevation, cover, a road, ground that is
      slow to cross: enough for a road-reader and a cover-reader to be visibly
      better than a crow-flight commander.
- [ ] **1b. A fourth shipped doctrine that issues a movement to contact.**
      `pull_under_fire` is bit-identical across 576 battles because no shipped
      doctrine ever orders an `Advance` — massed armour assaults, elastic
      defence holds, `Recon` is issued by nobody. Aggression in the `Advance`
      band with delegation below `planner.devolved`, in `assets/mods/base`
      beside the other three, existing values untouched.
- [ ] **1c. Re-baseline.** `balance --sim` and the skill table on the new
      arena, seed-swept, with the numbers written into this file. Everything
      Phase 2 claims is a difference from these rows.

## Phase 2 — one currency

- [ ] **2a. Symmetric previews.** `hit_chance`, `shot_profile` and
      `expected_damage` learn the target's hypothetical position, the way they
      already know the attacker's. Pure addition: every existing caller passes
      the target's real hex and the determinism snapshot must not move. That
      is the check that this step is a rearrangement.
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

### Where that leaves Phase 1

Blocked on the sight fix, and deliberately so: any re-baseline taken now is
contaminated by it, and so is any before-and-after the evaluator change is
judged on.

## Measurements

Skill table, `--sim --only skill --absolute --games 36 --sweep
seed=0,1000,2000,3000`, read off the `both ends` rows (288 battles each).

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

## Scratch

Temporary probes, to be deleted with this file:

- `crates/tactics_core/examples/_arena_dump.rs` — prints the arena, its terrain
  counts, deployment ground, and what a tracked vehicle can reach.
- `crates/tactics_core/examples/_mirror_probe.rs` — plays the arena at
  difficulty 5 both sides and reports the first round where a decision or a
  position stops being the mirror of its twin.
- `crates/tactics_core/examples/_los_probe.rs` — the sight-symmetry census
  above.
