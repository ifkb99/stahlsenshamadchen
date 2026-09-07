# DONE

Finished work, kept for its reasoning rather than its status. [TODO.md](TODO.md)
is what is left; this is why the built things are the shape they are — the
measurements that settled an argument, the guesses that turned out wrong, and
the traps that are only obvious once you have fallen into them.

Nothing here needs doing. Read it when a decision looks arbitrary, or before
undoing one.

## Scale and maps

**A tile and a unit have a size.** 1 battle hex = 100 m, 1 round = 60 s,
1 tick = 5 s, 1 elevation level = 10 m, 1 overworld hex = 4 km = one battle
map. Movement points were already calibrated for this (5 MP tracked on grass =
30 km/h; 7 MP wheeled on road = 42 km/h) — only ranges, vision and reloads had
to move. The AW-style 10 HP model is what implicitly sets round length, so
committing to a 60 s round commits to the ballistics rewrite: lethality has to
come from penetration rolls, not HP attrition.

**Scale lives in data, not Rust.** A `scale` block in mod.json (`hex_meters`,
`round_seconds`, `ticks_per_round`, `elevation_meters`, plus
`overworld_hex_meters` and `overworld_turn_hours`), typed as `data::Scale`.
`TICKS_PER_ROUND` is gone as a const, so round resolution and cooldowns take a
registry; `reload_ticks` had to become `Option<u32>` because a serde default fn
cannot see the mod being loaded — read it via `weapon.reload(&scale)`. Panels
lead with "1.6 km" / "30 km/h" / "elev 30 m" and keep hexes and MP in parens. A
sibling `balance` block does the same for crew stats: the old `awareness / 4`
and `driving / 5` divisors became +5% of the vehicle's base per point, so a
gifted crew is worth about a quarter of what they are sitting in.
`validate-mods` prints the whole roster through the scale, which is what makes
a wrong number visible.

**A battle map is a hexagon of one overworld tile**, since that is how the
overworld draws it. Radius derives from the scale (4 km / 100 m = 41 across,
1261 tiles), `MapKind::Battle` implies `MapShape::Tile`, and validation rejects
anything that is not that hexagon (`"shape": "free"` opts out for scenario
maps). It needed no map-format change — `HexMap` was already a sparse hash and
a space already meant "no tile". `river_crossing` was rebuilt as a river
valley: meandering river, road bridge on the centre row, two mud fords, woods,
a town on the east bank, elevation falling from 3 at the rim to 0 along the
water. Cost 1261 tiles against 768, which took the engine suite from 9.5 s to
26 s — since paid back several times over by the fog work.

## Cadets, crews and rules

**Cadets are instances, not definitions.** `tactics_core::roster` owns `Cadet`
(id, def, name, owner, stats, xp, status, battles) and `Roster`; `Unit.crew`
and `ArmyUnit.crew` are `CadetId`s, and crew bonuses read the instance — so a
wounded gunner already costs her vehicle its gunnery with no special case. The
overworld owns the roster and armies carry handles into it, so the same cadets
come out of a battle as went in.

**One roster for the whole world**, with `Cadet.owner` naming her academy —
not one per side. That keeps `CadetId` unambiguous everywhere and makes cadets
changing hands (recruited, poached, captured) a field change rather than a
renumbering, which is what an academy-scale mode will want. `Army.units` is
`ArmyUnit` rather than `UnitPlacement` for the same reason: a placement is
map-file data carrying a coordinate a unit inside an army has no use for, and
conflating the two produced the `at: [0, 0]` survivor hack.

**Cores, skills and traits are built**, and the scale question they were
blocked on is settled: cores are the full nine GURPS attributes centred on 10
(8–12 ordinary), not the old 0–5, which is what makes skill defaults like
`Hands - 4` mean anything. Thirteen skills, five seat roles, and traits that
are declarative and conditional. Abilities are *derived*, never stored — see
`data/cores.rs` and [the wiki](assets/wiki/reference/cadets.md).

**Every core must be named by at least one skill.** A core nothing reads is
dead weight, which is exactly what `morale` and `leadership` were in the old
`CrewStats`: declared for years and never once consulted.

**They are cadets, and there are forty-nine of them.** The oldest open naming
question in TODO — *"need to find better name for girls"* — closed on
2026-08-24. The objection that *cadet* names a low rank while the game runs the
whole chain of command does not bite: at an academy cadet is an **enrolment
status rather than a rung**, so the appointments layer on top of it and "Cadet
Krieger, commanding 1st Company" is not a contradiction. The cost is that it is
colder than the academy register wants, and it is paid in the right place — the
warmth in this game lives in the names, not in the collective noun, and nobody
in a common room says "the cadets" anyway. The argument is in
`assets/wiki/reference/tone.md` under *The word: cadet*.

Two things about the rename are worth keeping. It moved serde field names
(`Roster::cadets`, `CrewLoss::cadet`, `CrewHit { cadet }`), so `SAVE_VERSION`
went to 2. And the determinism baseline was **substituted textually and then
passed unregenerated** — which is the whole of the evidence that a tree-wide
rename changed no rules, and is the way to check the next one.

**Filling the seats was a mechanics fix wearing content's clothes.** The base
mod shipped ten characters and `frontier` spread them over eighteen vehicles,
so the campaign stamped three separate Rosa Steiners until *one cadet, one
seat* landed — after which most vehicles were two-thirds crewed or crewed by
nobody. That is not cosmetic: **substance counts people aboard**, so a medium
tank crewed by the two cadets a map happened to name died about twice as fast
as the identical tank crewed by four anonymous ones. Naming your characters was
a straight penalty, and an invisible one. The roll is now twenty-four at
Kuhlmann and twenty-five with the Iron Valkyries, every seat in `frontier`
filled by her own cadet in the seat her skills are for, pinned by
`every_seat_in_the_campaign_belongs_to_a_cadet_of_her_own`. It also makes the
wound model legible: from a full order of battle an empty seat means
`CrewCondition::Absent` and nothing else.

`river_crossing` was deliberately left with the old ten and its partial crews,
because it is the determinism baseline; crewing it up changes the fight and is
its own chunk (TODO, Content gaps).

**An order can be meant.** `Latitude` is `Delegated` (every order this engine
ever had) or `Binding` ("I mean it"), and it exists at both scales for the
same complaint: orders read as weighted opinions. On a **crew** it gates the
battle drill — she drives on through fire instead of breaking for cover. On a
**formation** it gates something else entirely, and the distinction is the
part worth keeping: it raises the floor of `(1.5 - delegation)` from 0.5 to
1.0, which says *`delegation` may make a subordinate more literal than she was
asked to be, never less*. A loose doctrine used to read "take the ford" at
four fifths of face value, and nobody issues an order meaning four fifths of
it.

The two things it deliberately is not:

- **Not the contact damping.** That is what separates `Advance` from
  `Assault`, and nothing else does, so a binding advance that skipped it would
  be an exact synonym for assault. The verb answers "will she halt and fight
  when shot at", the latitude answers "may her doctrine discount this"; they
  are orthogonal and compose.
- **Not "ignore the doctrine".** A tight doctrine reads an order at 1.2, so
  removing the doctrine would make insisting pull *less* than asking.

Player-side it is `X` on a crew and **Ctrl** on a mission key — Ctrl because
`X` is already the assault and there is no free key that would not lie. The
AI never issues `Binding`, spelled out at all four call sites, which is why
the determinism baseline passed unregenerated through both halves of this.

**The academy roll.** `R` on the campaign map: the school in order of battle,
seat by seat, with availability and battles behind her. Derived every frame
rather than cached (a roll that caches goes stale exactly when it is opened,
which is after a battle), worded differently from the after-action report
(that page reports an event, this one reports a state), and it keeps a cadet
whose vehicle did not come home — she survives her tank far more often than
not. The layout came from the screenshot, not the design: with the chassis on
every line, every line wrapped.

## Battles

**What an order is worth against the terrain, as four numbers a mod owns**
(2026-08-28). `MISSION_WEIGHT`, `contact_scale`, the `0.15` distance decay and
`PLATEAU` became `planner.mission_weight`, `pull_under_fire`, `distance_decay`
and `plateau`. One chunk rather than four, because they are four terms in one
sum and sweeping any of them alone says less than sweeping the shape.
Determinism snapshot passed **unregenerated** — the whole of the evidence that
the refactor changed no rules — and `perf` is unmoved at 1.41 ms a round,
138 µs a `roads()` call, 0.09 ms a utility order. Five tests: one per field,
each mutation-checked by pinning the field back to its old constant and
requiring the test to fail, plus
`an_order_and_an_objective_are_led_to_by_the_same_slope`.

*Why one slope and not two.* `distance_decay` is shared between
`objective_value` and `mission_value` on purpose. `mission_weight` is quoted
in objective-value units — "an order pulls about as hard as the ford" — and
that sentence is only true while the two gradients have the same shape; a
second slope would silently change the units the first number is stated in and
the two would drift. The centre-seeking `0.15` in `score_tile`'s `advance`
term is deliberately *not* folded in despite matching in magnitude: that is
the fallback for a map naming no ground at all, and a designer asking how far
an order reaches should not also change how a lost crew searches an empty map.

*The result.* Swept on the delegation tax table at 8 seeds × 36 = 288 battles
a cell, with `--set planner.devolved=1.1` so both doctrines assign ground,
elastic defence's delegation tax runs **37 / 36 / 35 / 27 / 2 / 10** at
`mission_weight` 0 / 1 / 2 / 4 / 8 / 16 — against a `both flat` control that
is *bit-identical* at every value, which is the additivity claim measured
rather than asserted. The table's stated target is a tax of zero and 8 reaches
it. The shipped 2.0 was left alone: the measurement needs `devolved=1.1`, and
in the game as it ships elastic devolves (0.7 ≥ 0.6) and issues no ground
missions, so its real tax is 11 of 288. Whether to raise the orders or keep
elastic devolved is a content decision, in TODO.

*The null, which is the more useful half.* `pull_under_fire` is
**bit-identical at 0, 0.25, 0.6 and 1.0** across 576 battles — not inside the
noise, identical. `contact_scale` applies to `Advance` and `Recon`;
`ai/command.rs` picks a posture off `aggression`, so massed armour at 0.85
orders `Assault` (exempt by design), elastic defence at 0.3 orders `Hold`, and
`Recon` is issued by nobody. No doctrine the table fights ever issues an
`Advance`. Proved rather than inferred: `--set
doctrine.massed_armor.aggression=0.6` makes it order one, and the same sweep
then moves 55 / 53 / 52 of 144 with the control still bit-identical. The term
is live, correct, and **player-facing** — the player presses `G` and meets it,
the AI never does. A sweep returning exactly zero is an instruction to find
out why: "no effect" and "never evaluated" look identical in the table and
mean opposite things.

*What `plateau` turned out to govern.* Less than it used to. Since the goal
layer landed, the tile sweep's answer is one candidate among the goals rather
than a destination, and a chosen `Take(hex)` is walked by `step_toward`, which
never consults the band. It still decides between pieces of ground that mean
nothing in particular — the dispersion case it was introduced for — which is
why its test stages a map with no objectives on it, and why sweeping it on the
shipped maps produced nothing monotone.

**Battle victory conditions.** A map declares `objectives` (id, name, `at`
hexes, `value`, `kind`) and optionally a `victory_score`. Ground of kind
`hold` pays its value each round to whoever holds it; an `exit` is ground
worth *leaving* by, paid once, and the vehicle drives off the map. Reaching
`victory_score` wins outright (`EndReason::Objectives`); otherwise the points
decide a battle that would have been a draw, so **`EndReason::Stalemate` no
longer implies `winner: None`**.

The reason it jumped the queue was never the feature. `balance --sim` reported
**67% of battles ending in stalemate**, and the cause was not the standing
guess that "the sides lose each other" — measured, survivors were parked on
their own start rims 20+ hexes apart and *drifting further apart*. The tell was
an inversion: the stalemate rate **rose** as the AI got better (difficulty 5,
zero noise: 11/12; difficulty 1, ±6 noise: 6/12), because random noise was the
only thing causing contact. With elimination as the only win condition, sitting
in the best cover on the map is optimal play, and two sides doing that never
meet. After: **1 draw in 24, 652 shots against 302, 9.3 rounds against 16.5**.

Three things about it are easy to break and were expensive to get right:

- Objectives live on `HexMap`, not `BattleState`, so both setup paths pick them
  up with no new plumbing. The battle carries only `objective_held` + `score`.
- **Leaving is not dying.** `alive` means "on the battlefield"; `exited` means
  she went home. Classify end-of-battle units with `surviving_units()` /
  `lost_units()`, never `!alive`.
- **`deploy` must skip exit hexes.** A side deploys at the shallowest tiles of
  its own edge, which is exactly where its retreat lane is, so without the
  filter the attacker's leading vehicles spawned on their own way out and drove
  off on the first tick. Caught by
  `nobody_deploys_onto_their_own_way_off_the_map`, which is the game crate's
  first real test.

**Units no longer all spawn facing east.** They turn towards the centroid of
their enemies once every placement exists (`face_units_at_enemies`), and
`UnitPlacement` gained an optional `facing` so a scenario can still place
someone looking the wrong way — which is what makes an ambush authorable rather
than something the engine decides. Worth recording that the original note
overstated the damage: measured against the determinism baseline it was 6 rear
hits out of 59, not the whole first exchange, because on the 41-hex hexagon
units spend several rounds closing and facing updates on move. The bias was
real and entirely one-sided though — all 6 fell on side 1 — and after the fix
the same four seeds produce zero rear hits.

**A shell is priced against the plate it will strike.** `round_worth` — the
one value function behind both the loader's AP-or-HE choice and every
planner's shot pricing — added `0.3 * blast` for a round that did not
penetrate, flat, whatever it landed on. `overpressure` has always read the
struck plate. So the pricing and the physics disagreed about the most common
shell in the game, and the disagreement was invisible because both numbers
were plausible.

What it cost is the playthrough review's headline defect. A 105 put
**thirty-six shells into one tank destroyer's front**, twenty-nine of them
after the tracks and the antenna — the only two things a burst reaches from
outside a plate it cannot beat — were already destroyed. The AI was not being
stubborn; it re-planned every round and got the same answer, because the
answer never consulted the hull.

The fix is a pricing twin of `overpressure`, case for case: blast against
plate zero is worth its rating outright (splash cashes into casualty rolls at
the same rate a penetration's budget does), blast that overmatches is worth
the target's remaining substance (so `best_weapon_against` reads it as a kill
and the evaluator pays its kill bonus without either learning a special case),
and blast that does neither is worth one effect roll at the odds of getting
one — **zero when nothing reachable is left unbroken**. `blast_overmatches`,
`overpressure_chance` and `exterior_modules` are shared with the resolver
rather than restated, for the reason `los_clear` and `SightGrid::clear` share
`sight_line_clear`.

Two things worth keeping:

- **`BLAST_WORTH` is gone and nothing replaced it.** `points_per_effect`
  already states what a ledger point buys and `overpressure` already spends
  blast through it, so the exchange rate between blast and damage is a fact
  about the resolver rather than an opinion. One fewer magic number, arrived
  at by reading rather than by tuning.
- **The determinism diff is the evidence.** Across four seeds the only unit
  whose behaviour changed is the one firing the round whose price changed:
  every kill, every crew casualty and every module is identical, and the
  snapshot shrank by 96 lines because battles stopped running on after they
  were decided. A change to a shot value function has no business being
  tighter than that, and it is worth re-checking against if this is touched.

What the pair measured, `balance --sim` before and after (12 battles across
the three battle maps) plus game 1 of the review re-fought on its own seed:

| | before | after |
| --- | --- | --- |
| shots fired | 477 | 395 |
| of those, penetrated | 39% | 44% |
| bounced | 111 | 89 |
| artillery kills | 23 | 25 |
| 105 mm rounds spent per battle | 20.5 | 14.1 |
| cadets out per battle | 11.0 | 9.9 |
| rounds per battle | 14.5 | 14.0 |
| game 1: length | 30 rounds | 16 rounds |
| game 1: howitzer shells, and at how many hexes | 40 at 5 (36 at one) | 9 at 6 |

The howitzer takes **more kills from a third fewer shells**, which is the
whole claim in one line: nothing about the gun changed, only what the crew
believed it was worth firing. Guns across the board fire less and land more,
and the battle costs a cadet less. Nothing regressed — the delegation tax fell
(18.3 → 16.1 rounds for a commanded massed force, its standing complaint), the
mustered-forces and skill-gap tables sat still, and the win split moved
slightly toward parity, 8–4 to 7–5.

**A bounce that achieves nothing does not hold a battle open.** The stalemate
clock counted `ShotBounced` as progress, on the reading that the guns were
still trying. Trying is not progress: the barrage above reset the clock every
round for twenty-three rounds after its last useful shell, so one mispricing
bought eight rounds of wandering on top of itself. Bounces are off the list.
The livelock the list was written against is still shut out, because a bounce
that achieves something announces it separately — `ModuleHit` and `CrewHit`
are still there and overpressure raises both from outside the plate. The rule
is now the honest one: a gun *accomplishing* something keeps a battle alive, a
gun merely firing does not.

**A broken crew does something.** REVIEW.md's second fun tax, and the defect
was worse than the review read: a crew who would not advance would not retreat
*or take cover* either, because all three questions ran through one `obeys`
gate. "A broken unit that cannot retreat is free kills for the enemy." Morale
was narrating a death spiral and buying nothing.

The designer's shape for it: *defiance, and rallying from it — fight, flight
or freeze, depending on the cadet*. So the rung decides that she defies and
her temperament decides how. `morale.defiance` lists the responses with a
`core` and a `base`; the score adds trait modifiers and the highest wins, ties
to the first listed. Cores default to `AVERAGE`, so ordering `freeze` first is
what makes the whole feature additive — an unwritten cadet does exactly what
every crew did before it existed. `TraitEffect` gained an optional `defiance`
target beside its optional `skill`, because temperament under fire is not a
competence, and spelling it as one would have meant inventing a cowardice a
cadet could be trained in.

What each does: **flight** reverses away from contact (never toward an exit —
she is frightened, not navigating, and the map will not always have edges);
**fight** refuses to fall back and switches ambush discipline off, which is a
cost, not a bonus; **freeze** takes no opportunity fire either, so the third
state is a rule rather than a label. An ordered shot still reaches a frozen
crew: her initiative has gone, not her gun.

Three things that were harder than they look:

- **A crew cannot refuse her own decision.** Refusing throws away her ordered
  path and flight lays a new one, so without `UnitIntent::own_idea` the
  refusal check picks that up next tick, discards it, lays it again, and she
  shakes in place forever — a livelock that looks exactly like the freeze this
  removed.
- **Rallying reads sight, not the radio net.** `recovery_near_leader` — the
  twin `leader_lost` had wanted since it was added, because losing a commander
  cost a formation its nerve and still having one bought nothing. The first
  draft asked `in_contact` and was wrong twice: a commander steadies a crew by
  being visibly still in the fight rather than down a wire, and a zeroed
  `command` block puts a radioless crew out of contact where no block at all
  does not, so the rule would have broken the additivity pin. The second draft
  asked `fog.side(..).visible`, which is vacuous — a side always sees its own
  units' hexes. `fog::sees` is the per-unit question and was already there.
- **The scenario maps were anonymously crewed**, so every crew on them scored
  every response identically and froze. Correct by design and invisible in the
  instrument, so `battle_forest` now names 23 seats a side off the academy
  rolls — leaving each scout section anonymous, because Kuhlmann has 24 cadets
  against 25 seats and nobody crews two vehicles.

Measured, `balance --sim --games 36` before and after:

| | before | after |
| --- | --- | --- |
| foot units still on the field at the bell | 66 of 96 | **73 of 96** |
| rifle platoons lost | 30 | 23 |
| artillery lost | 34 | 25 |
| recon cars lost | 44 | 41 |
| hits landing on a side or rear arc | 15% | 20% |
| delegation tax, massed under command | 6 wins | **0** |

Morale buys survival now, and it buys it for exactly the vehicles that should
be buying it — the soft ones. The flank statistic is the price, and it is
emergent rather than designed: a crew reversing out of a fight shows somebody
her side. The delegation tax reaching its written target of zero was not
predicted; a commanded force whose crews may break contact loses less to the
command layer than one whose crews may only stand there.

The determinism baseline did **not** move, and it is worth knowing why rather
than trusting it: `river_crossing` has exactly one crew reach `Breaking` over
the four seeds — Anka's medium tank — and her temperament is `Fight`, which
differs from the old freeze only in ambush discipline, which does not apply to
a crew already spotted. A checkable coincidence, not a guarantee.

**Difficulty is a blurry observer, not a blurry field.** The AI's mid-game
read as dead time and REVIEW.md called it a creep. Measured, it is not a
creep: on `battle_plains` a medium tank drove 53 hexes over 19 rounds to
finish 10 hexes further forward, a recon car drove 59 and ended *further* from
the objective than she deployed, and an artillery piece oscillated between two
adjacent hexes for 28 rounds. Nobody was slow — a medium tank makes five hexes
a round, which is the 30 km/h the scale contract chose. They were aimless. So
the review's proposed cure, more movement points, was backwards: it multiplies
the wander.

One cause was a selection bias hiding in `noisy_score`. Difficulty drew
independent noise **per candidate tile** and the planner then took an argmax
over every tile she could reach. The maximum of ninety draws from ±0.5 is
about +0.49 every time, against an objective gradient of 0.54 a hex — so the
winning tile was reliably whichever drew luckiest. And it scaled the wrong
way: more reachable tiles means more draws means a worse choice, which is why
the 7 MP recon car wandered hardest and the 3 MP howitzer only twitched.

The fix is one lean per unit per round, applied as a smooth function of where
a tile lies relative to her. Nothing can win by drawing well because there is
nothing to draw; what is left is a coherent misjudgement — today she favours
the left, and favours it consistently — which is what "the same candidates
through a blurrier lens" was always supposed to mean. Note the obvious
reading, a flat offset per unit, is a no-op: adding the same number to every
candidate changes no argmax.

Measured on `battle_plains` seed 7, path straightness (net displacement over
hexes driven), and `balance --sim --games 36`:

| | before | after |
| --- | --- | --- |
| straightness, difficulty 5 | 49% | 49% — *bit-identical* |
| straightness, difficulty 4 | 42% | **64%** |
| hexes driven, difficulty 4 | 196 | 104 |
| skill gap 5v1 exchange | 1:1.7 | **1:2.1** |
| skill gap 5v3 / 3v5 | 24–12 / 4–32 | 20–16 / **16–20** |
| engine test suite | 19.3 s | 6.7 s |

Two of those want reading carefully.

**The side-B artifact is largely gone.** CLAUDE.md has tracked "side B retains
a modest edge on the mirrored arena" as a suspected resolution-order problem
since B4. It was not resolution order: the same skill gap paid 24 wins from
one end and 32 from the other, and after this it pays 20 and 20. The bias
scaled with reachable-tile count, which is exactly what an argmax-over-draws
bias does.

**And the 5-vs-3 gap now discriminates less** — 67%/89% before, 56%/56%
after. The reading that fits both rows is that most of the old 5v3
"discrimination" *was* the artifact and the true edge at that gap is smaller;
the alternative reading is that the lean costs a good side something real. The
5v1 row argues for the first (it improved, and its exchange ratio reached
B4's written target of visibly better than 1:2), but this is a hypothesis and
it is written down here as one.

> **Corrected 2026-08-26, once the harness could sweep the seed.** Every
> skill-gap figure in the table above is a single draw, and the draws are much
> wider than this entry assumed. Sixteen seeds at `--games 36` put the 5-vs-3
> row anywhere from 16–27 wins of 36 and the 5v1 exchange from 1:1.2 to 1:1.8,
> so neither "1:1.7 → 1:2.1" nor "24–12 → 20–16" is evidence of anything: both
> moves are inside one seed's worth of wander. **The side-B claim above is
> wrong.** Adding the two equal-skill pairings over those sixteen seeds gives
> 494–652, side B on 56.6% of 1152 battles, in the same direction on 14 of
> them. The *mechanism* argued for here is still right — a per-tile draw does
> give a bias that scales with reachable tiles, and removing it was correct —
> but it did not remove the arena's own asymmetry, and "20 and 20" was one
> roll of a die that ranges 16–27. The numbers that supersede these are
> below, under "The skill arena's side-B edge"; re-draw them with
> `balance -- --sim --games 36 --only skill --sweep seed=0,1000,2000,3000`
> rather than trusting either set.

**Commitment was tried first and does not work — a negative result worth
keeping.** The other half of the wander is that a greedy planner re-decides
its destination every round with nothing carrying an intention between them,
and the obvious fix is to keep the destination until she arrives. It was
built: a `heading` on the unit beside `tasking`, set by `SetMove`, cleared on
arrival and wherever `tasking` clears. It made things *worse* — straightness
40% to 30% at difficulty 4 — and the reason is structural rather than a wiring
mistake. **The planner only ever scores tiles it can reach this round**, so a
heading is never more than one round away and the engine clears it on arrival.
Instrumented, commitment engaged 15 times in a whole battle and 12 of those
were to a tile one hex off: it fires only for units that *failed* to arrive,
and pins exactly the ones that were stuck. Real commitment needs the planner
to choose a goal several rounds out, which is a goal-selection layer and a
design chunk of its own — and the one that overlaps with the subordinate
initiative the designer has described wanting (`initiative` already exists as
a doctrine weight, unread). Reverted rather than shipped.

**The planner has a goal now, and the goal is the seam.** The other half of
the wander: a greedy planner re-decides where it is going every round, so
nothing carries an intention between them. Committing to the tile it picked
was tried first and failed structurally (above) — that tile is never more than
a round away. So the planner needed a destination worth committing to.

`Goal` is `Take(hex)` or `Hold`, kept on the unit and cleared when it
finishes. What it is chosen *from* is a short list of places that mean
something — objectives, and always the tile this round's sweep would have
picked, which is the guarantee that the layer can never leave a crew worse off
than having none. Not a raster: scoring a three-round radius would be seven
hundred tiles a unit a round, and more to the point "somewhere near the ford"
is not a different intention from "the ford".

The split is the deliverable, not the behaviour. `candidates` is shared
knowledge about the map; `GoalChooser` is judgement, and is the only thing a
learned policy replaces. That keeps a policy's action space at five or six
statements instead of 1261 hexes and lets it inherit pathing, boarding,
dismounting, opportunity fire and defiance from an executor that already
works. The vocabulary is the options framework's: candidates are the
initiation set, the chooser is the policy over options, `Goal::finished` is
the termination condition, and the utility planner is the intra-option policy.
Worth recording the correction that produced that shape — the original idea
was to use `initiative` as an RL epsilon, and epsilon is *exploration* noise
that anneals to nothing by deployment. A high-initiative crew under that
scheme would act randomly rather than independently, and would stop once
training converged. A disposition is a property of the policy: a feature it
conditions on, or a weight mixing two value estimates.

Four things this got wrong on the way, each worth keeping:

- **A mission must replace the candidate list, not join it.** The first draft
  made it one candidate among the objectives, and an objective could outbid
  it — so a crew under orders and a crew with none chose the same ground,
  which silently undid step 1 of DIRECTION.md.
  `a_cut_off_unit_keeps_the_orders_she_had` caught it. This is also exactly
  where subordinate initiative attaches: that chunk *widens* the list by
  doctrine and nothing else moves.
- **`SetMove` is refused past this round's movement budget.** That is the rule
  that made the planner only ever score reachable ground in the first place,
  and it means a long march is walked a leg at a time — through
  `movement::step_toward`, shared with a commander's personal `tasking`,
  because two implementations of "closest reachable" are two answers to where
  she is going.
- **The goal layer removed difficulty entirely on the first run.** Difficulty
  4 and 5 produced byte-identical battles, because the chooser never saw the
  blur and the sweep's noisy answer was only a candidate. Moving the blur into
  the chooser is the fix and is a better rule than the one it replaced: a
  worse commander now goes to the **wrong place**, which is a mistake a player
  can see and punish, rather than twitching between interchangeable hexes. A
  per-candidate draw is safe over six meaningful options and was not safe over
  ninety interchangeable ones — the argmax is the whole difference.
- **One crew per piece of ground**, said in the candidate list. That is the
  dispersion job the `PLATEAU` tie-break was quietly doing, now visible.

Measured, `--games 36`: battles 14.8 → 13.6 rounds, penetrating shots 41% →
43%, misses 513 → 425. More decisive, less wasted. The skill-gap rows moved
around inside their own noise floor — read the two same-brain control rows
first, which came in at 14–22 and 20–16 where parity is 18–18, so ±4 of 36 is
what any row has to beat before it means anything. 5v1 sits at 26–10 against
28–8 before, and its exchange ratio fell from 1:2.1 to 1:1.4.

**Difficulty still does not buy much intelligence, and now it is clear why.**
The chooser is shallow: it prices being *there* and the drive, and nothing
about the route, the risk on the way, or what the enemy will do about it. A
better commander has almost nothing to be better at yet. That is the argument
for what goes in the chooser next, and it is a much more tractable question
than it was when difficulty was jitter on a tile sweep.

**A crew says where she is going.** `Event::SetOut` fires when a goal changes
— not every round, because "still driving to the ford" is not news — and
carries the wording rather than the hex, so the log reads without a map in the
other hand. This is the first thing this AI has done that is explainable in a
sentence, which is most of the argument for the layer existing.

**The log became radio traffic.** One of ours speaks with her call sign in
front of her and says what she is doing; anything else is a spot report with
nobody's voice on it. Presentation only — the events keep every id, hex and
flag, so the harness and the replay are untouched, which is the property that
lets the voice be rewritten again later without checking whether the engine
still works.

It found a real bug on the first run, and the bug is worth knowing about
because it will recur. `SetOut` is emitted during the AI's *planning*, and
`drive_ai` pushed planning events into the same **paced** animation queue that
combat uses — while `accepting_orders` is false whenever that queue is
non-empty. One event per unit put nine beats in front of every planning phase,
and the player simply could not give orders: the infantry tour clicked into a
game that was not listening, selected nothing, and its unload never happened.
The fix is that `drive_ai` now filters with `heard_by` *before* queueing
rather than the log filtering after draining — an event nobody will be shown
must not cost the player a beat. It is also the fog-correct answer: where an
enemy crew has decided to go is on her net, not yours.

Worth recording how long that took to find, because the false trails are
instructive. The dismount worked headlessly on the first try, which ruled out
the engine; the tour then failed identically at the commit *before* the goal
layer, which briefly exonerated it — but that run had been given the wrong map
(the tour names `battle_plains` in its own header and the default battle map
is no longer `river_crossing`). Two tours were then "failing" for the same
reason. The lesson is the cheap one: a tour states its map at the top, and
running it without that is not a test result.

**MCTS is parked, on evidence.** REVIEW.md read "Kuhlmann won 3/3" as a sign
that MCTS beat the utility planner, and it could not be: `playthrough` gave
side 0 both MCTS *and* `massed_armor`, on a map whose sides field different
vehicles. Three candidate explanations, one observation.

`balance --brains` separates them — the mirrored arena, same forces, same
doctrine, same difficulty, both orientations, only the brain differs. At 64
battles a pairing:

| pairing (A vs B) | A won | B won | draws |
| --- | --- | --- | --- |
| utility vs utility | 33 | 31 | 0 |
| mcts vs mcts | 31 | 33 | 0 |
| mcts vs utility | 29 | 34 | 1 |
| utility vs mcts | 28 | 35 | 1 |

**MCTS 64, utility 62, out of 128.** Parity. The controls are what make that
believable: both same-brain rows land within two games of even, so the noise
floor is about ±3, and a real difference would have had to clear it in *both*
mixed rows. Neither does — both favour whichever side is B, by an amount well
inside a coin's spread.

So a planner costing ~1.8 s an order plays no better than one costing 0.05
ms. The likeliest reason is worth recording because it generalises: MCTS's
rollout policy **is** the utility planner, so it searches a tree whose leaves
are evaluated by the thing it is trying to beat. A search is only as good as
what it searches toward, and 900 iterations of a mediocre evaluator buys
almost nothing over asking that evaluator once. If MCTS is ever revived, the
evaluator is the thing to improve first — which is the same conclusion the
goal chooser reached from the other end.

Note the A/B was only answerable because the tables now fight across every
core: 256 battles took four minutes, where the first attempt at a quarter of
that sample had been framed as an overnight job and produced six battles a
pairing, which said nothing at all.

**`MoveGrid`: the same trick as `SightGrid`, one layer down.** `edge_cost`
resolved terrain by `String` through the registry and the searches called it
per edge of every tile they touched — about 7,500 string hashes per `roads`
call, for an answer no battle can change. Every tile's cost per movement class
is now resolved once, shared behind an `Arc` on `BattleState`, and rebuilt by
`save::rehydrate` (an empty one says every step is impossible, so a loaded
battle would have nobody able to move at all).

`roads` 322 → 219 µs, `reachable` 26.5 → 18.0, and **the event stream was
byte-identical** with the grid in and nothing else changed, which is the whole
claim: allowed to be faster, not allowed to price a step differently.
`movement::edge_cost` stays as the reference implementation — tests, one-off
queries, the campaign map — and the two share `step_cost` so the climb rule
cannot drift, exactly as `los_clear` and `SightGrid::clear` share
`sight_line_clear`.

Cutting `ai::goal::HORIZON` from 6 rounds to 4 took `roads` to 135 µs and the
planner's per-order cost from 0.17 to 0.09 ms. Six was chosen on the impatience
arithmetic without checking what it covered: six rounds is 30–42 movement
points and the battle map is a radius-20 hexagon, so the horizon was the entire
map and pruned nothing.

Two things worth keeping from the investigation. **Stacking is not a
performance question** — `roads` consults no occupancy at all by design, and
stripping the friend check out of `destination_blocked` entirely, infinite
stacking, moves `reachable` only 31.1 → 27.6 µs. And the grid is written for
the streamed world the design is heading for: keyed on tiles rather than on a
map's identity, folded in a region at a time through `extend`/`insert`, with
adding a new per-tile fact meant to be one field on `TileMove` and one line in
`TileMove::of`. `SightGrid` is the same structure and wants merging with it
when that day comes.

**The goal chooser reads the road.** It priced a march as `distance / speed`
and knew nothing about what happened on the way or about who else wanted the
ground. It now prices the real terrain cost (`battle::roads`, one Dijkstra out
to a horizon in rounds, shared by the whole candidate list), the share of that
road a spotted gun can see (`DoctrineDef::route_caution`), and how many rounds
later than the nearest visible enemy she would arrive (`contest_aversion`).

Four things in it were wrong first:

- **An A* per candidate** costs five to ten times a planned order. Every goal
  is a place to drive from the same hex, so one Dijkstra answers all of them.
- **Falling back to the crow flight** for ground with no road inside the
  horizon is exactly backwards: that ground is the ground whose road is
  longest, so the fallback made the far bank of an unfordable river the
  nearest thing on the map. It is priced at the horizon instead.
- **Occupancy has no business in a road.** A march takes rounds and the field
  does not hold still; a tank parked on the bridge is not a wall, it is a
  reason to expect a fight. Leaving units out also keeps the inner loop off
  the O(hexes × units) scan that `reachable` still pays.
- **Deepening a chooser makes a blur matter less**, which is the opposite of
  what deepening it was for. A value function that separates a good goal from
  a bad one more sharply is one a blurred commander still ranks correctly. So
  difficulty gained a second axis, `difficulty_foresight`: noise is misjudging
  what she has read, foresight is not having read it. At difficulty 1 it is
  zero and the chooser is exactly what it was, which makes every level above
  an addition.

And the measurement that did not come: on the mirrored arena the skill gap
barely moved (5-over-1 62.8% → 64.2%, 5-over-3 54.3% → 54.8% at 8 seeds × 36
battles, both inside the noise). The arena has a handful of forest hexes and
no objectives worth arguing about, so there is nothing there for a road-reader
to be better at — an instrument limitation, and the reason the three terms are
pinned by staged tests that isolate one apiece instead. Staging those turned
up the thing to remember about all of them: an objective offering a shot at a
visible tank is worth about seven points more to stand on than one that is
not, against route costs of one or two. These are tie-breaks between
comparable goals, and a coefficient big enough to overrule a destination would
be a doctrine that refuses every contested objective.

**Being looked at is no longer being seen.** Spotting keeps its geometry —
line of sight, range, and `VehicleDef::concealment` shortening a spotter's
reach against one target — and finding somebody inside your own field of view
now costs a die, once per tick, against the ground's own `concealment`, the
outer band of the spotter's reach, and how many hexes the target has driven
this round. Firing still bypasses everything. The full record is in
[detection.md](assets/wiki/reference/detection.md); four things in it were
wrong first and are worth not rediscovering:

- **A roll per spotter is a lie about the number.** At four crews looking, a
  nominal 2% is 8%, so the whole usable range of the knob collapsed into single
  digits and the instrument's "ticks to find" column was wrong by a factor
  nobody could see. One roll, by the best-placed crew. More eyes still pay,
  through watching more ground.
- **A near band is not optional.** Without `detection_certain_percent` two
  tanks three hexes apart on open grass had an 82% chance of noticing each
  other per tick. Nobody searches for the tank 300 m away in an open field, and
  it surfaced as two dozen staged tests failing at once.
- **The far-range term reads the spotter's own reach**, never the
  concealment-shortened one. Compounding them put a platoon at three hexes on
  75% of "reach" and charged the same fact twice.
- **The first instrument measured the wrong thing twice.** "Round of the
  battle's first contact" is dominated by one easy spot and reported no
  difference at any setting; splitting contacts by whether the found crew had
  driven is worse than useless, because `moved` is zeroed at the top of a round
  so "halted" means "has not driven *yet*" — a delayed contact leaves the
  halted bucket by construction, and the table cheerfully reported that
  detection rolls make stationary crews easier to find. What works is contact
  *range*, read at the tick the contact was made in.

And the finding that matters most is that it changed nothing measurable: 36
battles across four seeds put every fought-out column inside the seed noise
floor, no stalemates either way, because **nothing in the evaluator wants to be
unseen**. Same shape as stacking — the mechanism waits on a preference.

**The five AI numbers are the `planner` block** (2026-08-27): `impatience`,
`horizon_rounds`, `boarding_rounds`, `devolved` and `exit_urgency`, which were
the last tuning constants in `ai/`. Each `#[serde(default)]`s to exactly the
constant it replaced, so a mod that declares nothing gets the game it always
had — the determinism snapshot passed **unregenerated**, which is the whole of
the evidence that this refactor changed no rules, and `perf` measures the same
1.41–1.47 ms a round and 143–146 µs a `roads` call as `develop` does.

**A block of its own rather than five more fields on `balance`**, and the
distinction is the point. `balance` says what is *true on the battlefield* and
applies to a human's shot exactly as it does to a machine's; nothing in
`planner` reaches a rule, so a mod that rewrote all five would leave a
human-versus-human battle bit-for-bit identical. Filing them under `balance`
would have merged "what is true here" with "how well is this side played",
which is the same conflation difficulty spent a whole arc separating when it
stopped being a lottery and became a lens.

**And the prize was the sweep, which returned a null result.** What a horizon
is *worth to a commander* had never been measured, because until now the only
way to ask was to edit Rust and rebuild. Asked properly at 36 games across the
three shipped maps:

| `planner.horizon_rounds` | 1 | 2 | 3 | 4 | 6 | 8 |
| --- | --- | --- | --- | --- | --- | --- |
| wins | 20–16 | 19–17 | 19–17 | 19–17 | 20–16 | 19–17 |
| rounds | 13.3 | 13.5 | 13.3 | 12.9 | 13.1 | 13.1 |

Against a seed noise floor measured in the same session (`--sweep
seed=0,1000,2000`: ±3 wins, 1.2 rounds, heavy-tank kills 31–44), **a horizon of
one round and a horizon of eight are the same game.** Two sharper facts sit
under that. On the mirrored arena, `horizon_rounds` 4 and 6 produce *identical*
skill tables in every row — which turns the old note that six "pruned nothing"
from a tile count into a statement about behaviour. And the only consistent
signal anywhere is that 4 and 6 pay the better commander about three battles of
72 over 2 in the `both ends` rows, which is inside the band.

Read it as an instrument result rather than a result about the number, and it
is the same instrument limitation the road-reading chunk already recorded: the
shipped maps and the radius-10 arena have very little for a longer-sighted
commander to be better at. The value of moving a number into data is only
realised when somebody sweeps it — and the first sweep says the thing to build
next is ground worth reading, not a different horizon.

**Subordinate initiative** (2026-08-27), the chunk `ai/goal.rs` had been
holding a seam open for. `DoctrineDef::initiative` — declared for months and
read by nothing — now widens an ordered crew's candidate list by one entry:
the tile this round's own sweep picked, charged
`planner.deviation_cost * (1 - initiative)` for not being what she was told to
do. At `initiative: 0` the list is the two entries it always was, so a
doctrine with none plays the old game down to the rng stream position.

**Where the boundary ended up, and why it moved.** The written plan was that a
high-initiative doctrine "admits her own candidates alongside the ordered
one", which reads naturally as *her own objectives*. Built that way it fails
three tests, and the sharpest is `a_cut_off_unit_keeps_the_orders_she_had`: a
crew under orders and a crew with none went to the same hex, which is
DIRECTION.md's original complaint rebuilt inside the fix for it. The cause is
arithmetic rather than taste — a mission's ground is worth 2.0 on the
evaluator's scale and a shipped objective 2 to 5 — and no `deviation_cost`
repairs it, because a flat charge big enough to hold a 0.9-initiative crew
freezes a 0.3-initiative one. **That is the tell the skill guide names: a test
that fails at every setting of a new knob means the model is wrong, not the
tuning.**

So initiative governs **how she carries out an order, never whether she
believes it** — step 1 part 3 of the memo applied one layer down. Deciding
another objective matters more is `Unit::detached`, which already exists and
is a chain-of-command decision rather than an evaluator one.

**Calibration, which is the part worth keeping.** At `deviation_cost: 2.0` the
three shipped doctrines straddled the threshold on a staged march: massed
armour (0.3) drove at the hex she was given, elastic defence (0.7) and recon
pull (0.9) stopped to fight from ground of their own. (The number is 3.0
since 2026-09-06: when the threat term started reading the resolver, 2.0 no
longer held massed armour, and 3.0 is the smallest value that does — see
*The evaluator prices danger in the resolver's own arithmetic* below.) A number that separates the
content that ships is a number that means something; `balance.blind_penalty`
is the counter-example this was checked against.

**And in fought-out play it is inside the noise**, which is the third finding
of this shape after detection and stacking. Sweeping massed armour's own
`initiative` 0 → 0.3 → 0.9 at 36 games moves its wins under command by 1, its
carriers by 3 and its foot units by 2, against a seed spread on the same
columns of 1, 8 and 2. Two rows of that table are *byte-identical* across
every variant, and for checkable reasons rather than luck: `both flat` has no
missions to deviate from, and `elastic under command` issues none because its
`delegation` of 0.7 is at or past `planner.devolved`. Forcing every doctrine
to assign ground (`--set planner.devolved=1.1`, which the planner block made
possible the same afternoon) brings elastic into the sample and it moves by
one win.

**The determinism baseline passed unregenerated**, and the reason is
checkable: the snapshot contains zero `MissionAssigned` events, because
`river_crossing`'s only commanded side fights `elastic_defense`, which
devolves. There is nothing on that map for a subordinate to deviate from.

So on the shipped content this rule fires for exactly one doctrine and that
doctrine is calibrated not to use it — which points at the doctrine values and
at `devolved`, both of them data, rather than at the mechanism.

**Saves record which mods were playing.** `SaveGame.mods` stamps id and
version; mismatched ids are refused (the rules genuinely differ), version drift
on the same set warns and loads (a content patch must not cost the player their
campaign), and saves written before the field existed are trusted rather than
rejected.

**Asking what a number does, instead of what it is** (2026-08-26,
`feat/harness-sweeps`). `--set path=value` and `--sweep path=a,b,c` reach
every field of every block and every content map through a serde round trip of
the same representation a save file holds, so a field added to `Balance` is
sweepable the afternoon it lands and a path that names nothing is an error
listing what was there. Three things it settled:

- **It is the same thing as editing `mod.json`, and that is checked.** A
  hand-edited copy of the mod tree swept with `--sweep mods=a,b` and the
  equivalent `--sweep balance.moving_target_per_hex=5,40` print byte-identical
  difference rows. If that equivalence ever stops holding, the override
  machinery has become a second game.
- **The noise floor is bigger than several results this project quoted.** At
  36 games, four seeds alone move the tank destroyer's kills between 74 and 89
  and the mean battle length by 1.1 rounds. `28–8` and `26–10` were quoted at
  each other for months as evidence about a change, and at 36 battles a
  genuinely level pairing lands anywhere from 12–24 to 24–12 nineteen times in
  twenty; at the default 12 it lands as far out as 9–3. Every table with a win
  column now prints that band, computed from the battle count, and a swept
  table with three or more variants prints its own `spread` line, which under
  `--sweep seed=` *is* the measured floor. It is a floor rather than the
  answer: these battles share maps, forces and doctrines, so they scatter wider
  than a coin does.
- **`--jobs` moves no printed number.** 1 / 3 / 7 are byte-identical, and so
  is the parallel fought-out pass against the sequential one it replaced,
  because every job writes into the slot it owns and results fold in seed
  order regardless of which core finished. `Tally::merge`'s real requirement
  turned out to be associativity rather than summation — see STRUCTURE.md
  item 5 for the wrong claim the calibration test found.

**`ground` says what a battlefield is worth to the end that deploys on it.**
Every map is fought twice per seed with the two armies exchanged between the
ends — placements stay where the map put them and only the vehicles standing
on them swap — so summed that way `west`/`east` differ only by the ground and
`OB-0`/`OB-1` only by the force. Its control is built in: `battle_plains` and
`battle_forest` ship identical orders of battle, so their `OB` columns must
read level, and they do (36–36 and 37–35). Measured at 8 seeds × 36 battles
(`assets/wiki/reference/battlefields.md` has the method and the re-measure
command): `battle_forest` leans slightly east (45.8% west, −2.0 sd) and
`battle_plains` slightly west (55.6%, +2.7 sd), small, real and not worth
changing; `river_crossing`'s ground is level (49.5%) and its armies are not —
24.1% / 75.9% to the side fielding a tank destroyer where the other fields
artillery, −12.4 sd, by far the largest term on any map. It is the
determinism baseline *and* a third of the fought-out sample, so every doctrine
conclusion `--sim` prints is partly a conclusion about that tank destroyer.
The first reading of this table, before the coordinate tiebreaks were fixed,
said forest favoured the *west* 50–22 and plains the *east* 29–43 — one draw
of a build that made every crew edge west. Re-measure after anything that
changes how the AI moves.

**The near side of the shot** (resolver-depth arc, R1;
`assets/wiki/reference/ballistics.md`). The far side of a shot was always deep
and the near side was five lines. `hit_chance_inner` is now eight terms and
every one is data: `weapon.accuracy`, range falloff, `balance.accuracy(gunnery)`,
`downhill_bonus`, `cover_against_accuracy`, `vehicle.profile`, the two motion
terms, the rung's `accuracy`, and `blind_penalty`. Motion is priced per hex
because a hex is 100 m and a round 60 s, so one hex is a walking pace and
five is 30 km/h, and a flat "she moved" penalty throws away the only thing
that makes a fast chassis' speed a defence. Firing on the move costs more per
hex than being the thing fired at; if those ever cross, halting to shoot has
stopped being worth anything. Suppression is `MoraleRung::accuracy`, a number
on a rung the mod already declares, so a one-rung ladder has none and needs no
`if`.

*The draft that charged the drive, and what it cost.* `hexes_under_way` once
counted the distance from her real position to a candidate tile as driving, on
the reasonable ground that reaching a tile means crossing to it. But the
planner scores *ground*, and what makes a hill worth taking is the shooting
done from it over the rounds she sits there — almost all of it halted.
Charging every candidate tile except the one under her tracks put a standing
bias on staying put: 36 games gave **three stalemates where the baseline had
none, at 15.9 rounds against 13.6** — precisely the pathology land objectives
exist to remove, rebuilt one accuracy term lower down. Dropping the branch
restored 13.6 rounds and zero stalemates with the terms fully live. That is
why `hexes_under_way` reads state and never the hypothetical `from`.

*Instruments that came with it.* The `to hit` table shows the attacker's
motion as a gradient (1/3/5 hexes) rather than one "moving" column, because
the gradient *is* the rule; the roster gained a `profile` column; `--sim`
reports what share of shots were laid from a vehicle under way, because a
resolver term nobody's guns ever meet is a term that changed nothing; and
`Event::ShotFired` carries `moving` beside `blind` and `opportunity` so the
log can say why.

**Dispersion is a different fact from flight time, and the shell carries
both** (R2). `ShellInFlight` has `at` (the map reference the gunner laid on)
and `impact` (where the round comes down), rolled from `WeaponDef.dispersion`
when the shot is fired, because that is when the barrel, the charge and the
lay stop being adjustable. Flight time models the target moving; dispersion
models the gun — until it existed a shell aimed at a *parked* vehicle arrived
with certainty. A percentage of the range flown rather than a flat radius,
because dispersion grows with range, which is the whole reason a battery
registers before firing for effect: the base howitzer's 4% is exact at 300 m,
one hex at 2 km, two at 4 km. `scatter` takes the smaller of two ring rolls
and indexes `hexx`'s own ring order — a pure function of coordinates, unlike
iterating a set, which is the mistake this project has already made once —
because a ring at radius two holds twice the hexes of radius one and a uniform
draw over the disc puts most shells on the rim. There is deliberately still no
hit roll for a shell: a round that comes down on an occupied hex hits what is
on it, and a blind-fire penalty on top would price the same scatter twice.
`impact` is not `#[serde(default)]`, because the default is `Hex::ZERO` and an
old save's airborne shells would come down on the map corner — a silent wrong
answer where refusing to load is the loud one.

**A marginal penetration is worth less than a clean one** (R3). The pipeline
always had three outcomes — clean penetration, partial, bounce — and B2b
shipped two, so a round that scraped through spent exactly the budget of one
that vastly overmatched. That is the flattening the whole no-hit-points model
exists to avoid, one layer further in: margin is most of what separates a gun
that can just about manage a target from one that eats it. `penetration_roll`
now returns `Option<share>`, interpolated linearly by
`Balance::penetration_share` from `partial_penetration_percent` at parity up
to 1.0 at `clean_penetration_percent`, so there is no cliff for a marginal
shot to sit on and no threshold a modder discovers by bisection.
`clean_penetration_percent: 100` is the game before the band existed, checked
the strong way: the previous chunk's event stream passes byte-identical. The
analytic twin `penetration_share(balance, pen, armor, scatter)` averages over
the scatter outcomes that get through and `round_worth` multiplies by it,
because once a marginal penetration is worth less, "did it get through" no
longer prices a shot and a planner without this trades a certainty for a
technicality. `ShotHit::damage` is what was *spent*: rolled once in
`resolve_impact` and handed to `behind_armor_effects` as `spent`, which
`savage` also reads, so a round that broke up on the plate does not get the
overmatch that skips "wounded" — it replaced the whole `ShotProfile`
parameter there, the tell that the profile was only ever consulted for that
number. Tuning note, because the shape recurs: at a floor of 40% the band
swung the doctrine table to 25–11 and the tank destroyer to 99 kills — the
rule was right and the number was loud; 55% keeps it visible (TD 88 against
76 before the band) at 19–17, the parity the other chunks held. A penetration
cell reading `52·66%` gets through half the time and spends two thirds of its
budget when it does.

**Two crews on one hex** (2026-08-26). Stacking is `VehicleDef.footprint`
against `TerrainDef.capacity`, and `capacity: None` is *not* `1`: a terrain
that declares nothing keeps the rule this engine shipped with — one crew,
whatever size she is — which is not the same statement as "one footprint".
The latter would refuse a medium tank onto grass the moment anything declared
a footprint of 2, an additivity break disguised as a default. So stacking is
opt-in per terrain, and clearing the base mod's capacities in data reproduces
the old game exactly (0 stacked rounds, deepest stack 1, 0 strays, the old
infantry survival). Footprint has the mirror rule, read through
`VehicleDef::footprint()` where zero means one — the same field-versus-accessor
trap as `WeaponDef::reload`.

*`room_for` is fog-aware, and that is not an approximation.* An enemy the
moving side has not spotted takes up no room, because an order refused for a
full hex announces that somebody is standing there. The move resolves as an
ambush instead, and two crews can therefore end a tick over capacity — which
is correct, they have just driven into each other.
`unspotted_enemies_still_ambush` and
`hidden_enemies_do_not_show_up_as_holes_in_the_move_range` both failed the
moment this counted everybody. Crowding is a reason not to *stop*, never a
reason not to drive through: `passable` does not consult it and
`destination_blocked` does, and collapsing the two would make a wood holding
three platoons into a wall. A claim takes up room exactly as a parked vehicle
does, in both `claimed_by_friend` and `ai/goal.rs::claimed_by_another`, which
is what lets a section be ordered into one wood; `Goal::finished` reads the
same rule. A platoon dismounts onto the carrier's own hex, tried first —
`a_platoon_boards_rides_hidden_and_steps_off_where_the_ride_ends` used to
assert `distance_to(carrier) == 1`, which was the engine's limit rather than
anybody's intent. The campaign map made the same distinction the same day:
three places said a friendly army was impassable and only the A* cost inside
`move_army` moved anything; now only a *hostile* army blocks a route, and only
a march that means to avoid contact.

*The stray rule: the gunner aims, and only a miss is a lottery.* The to-hit
arithmetic answers for the vehicle she laid on exactly as it always did; what
is new is that a round which went past her has a hex full of other people to
end up among. `balance.stray_percent` is the chance per hundred points of a
bystander's `presence` — `100 + profile`, reusing the term that already means
"how much easier or harder she is to hit than a tank" rather than inventing a
second size field that would drift — rolled once per bystander in id order, so
several make a stray likelier without any one making it certain.
`Event::ShotStrayed` is its own event rather than a flag on `ShotHit`, after
the `ShotMissed` for the intended target, because a hit quietly naming a
different unit than the `ShotFired` before it reads as the log contradicting
itself. The observed rate is below the rolled rate on purpose: a platoon's
presence is 80, so 25% nominally means one miss in five, and measured on a
staged crowded wood it is 12%, because a stray that kills the bystander
leaves the rest of that round's misses with nobody to stray onto.
`a_round_that_goes_past_a_tank_can_find_the_platoon_beside_her` pins the band
rather than the number.

*What it changed.* Crews share ground in 69 of ~460 rounds, the deepest stack
seen is 2, strays fire 5 times in 1055 misses, and `river_crossing`'s four
determinism seeds stack literally never — which is why that snapshot did not
move, a checkable coincidence rather than a guarantee. Infantry survival went
from 79 of 96 to 65, and isolating it says the strays are not the cause (64
survivors with `stray_percent: 0`): getting out costs nothing now, so they
dismount more, stand with the vehicles, and vehicles attract fire. Nothing in
the evaluator values sharing cover or massing on ground, so a hex with room in
it is worth exactly what an empty one is; the mechanism waits on a preference,
the same shape as detection.

**The skill arena's side-B edge was three faults pointing the same way**
(2026-08-26), none of them resolution order, which is where the note had kept
saying to look. Side A held **42.9%** of equal-skill battles (494–652 of 1152,
−4.8 sd).

1. **The arena was not mirrored.** It was 25×13 rows of ASCII with forest at
   columns 8 and 16 — symmetric *as text* — and the odd-r offset conversion
   shears text into hexes. Six of 325 tiles had no mirror at all, 24 disagreed
   with their mirror on terrain, and side A had nine forest hexes within six
   of its deployment against side B's four. Rebuilt as a radius-10 hexagon
   whose every feature is declared once and reflected, asserted in
   `arena_map`: 45.6%.
2. **A coordinate tiebreak in `movement::step_toward`**: 46.4%.
3. **The same in the planner's plateau argmax**: **49.2%** (1133–1169 of
   2304, −0.8 sd).

The proof it is structurally gone rather than merely smaller: on the symmetric
arena, with difficulty 5 both sides (where the blur is exactly zero) and the
same planner seed, round one now produces eight of eight mirrored decisions
and positions. Before the tiebreak fixes the goals matched and three of four
positions did not — the planner agreed and the march did not, which is what
pointed at `step_toward`. The rule both broke is in CLAUDE.md's invariants: a
tiebreak may only read quantities a reflection preserves, and no total order
on coordinates can.

*The skill numbers, seed-swept rather than drawn once* (2026-08-26, 16 seeds
at `--games 36`, read off the `both ends` rows so being side A cancels):

| | wins | share | per 72-battle draw | exchange |
| --- | --- | --- | --- | --- |
| difficulty 5 over 1 | 824–326 | **71.5%** | 46–58 | 1:1.2–1.8 |
| difficulty 5 over 3 | 710–441 | **61.6%** | 36–54 | 1:0.9–1.6 |

Read the fourth column before quoting the third. One draw of 72 puts 5-over-1
anywhere between 64% and 81%, and puts 5-over-3 **level, at 36–36, on one
seed of sixteen**. Every figure this project used to quote — 29–7 / 28–8 when
the arena was built, 26–10 / 28–8 after blast pricing, 28–8 / 9–27 after the
per-round lean — was one such draw near the top of that band. After the
tiebreak fixes, 5-over-1 pays 61.1% (+10.6 sd) while 5-over-3 pays 51.9%
(+1.8 sd), which is the shallow-goal-chooser problem and not a bias. The
default `--games 12` put 5v1 at 6–6 and looked like a regression: twelve
battles cannot resolve a 70% edge, and this is the table most often quoted at
somebody.

*The earlier half of the same story: difficulty was inverted in practice.*
Less noise played worse, because the greedy argmax broke score ties toward the
first tile of a fixed (x, y) sweep, so on the broad plateaus open ground
scores in, every identical unit on a noiseless side drove to the same corner
of every plateau, queued, and lost to anyone scattered by randomness
(difficulty 5 lost to difficulty 1 in both orientations). The cure was
deterministic rather than more noise: among tiles within `plateau` of the
best, take the one nearest her own position. The second half was the
instrument: the skill table fought on `river_crossing`, whose sides field
different vehicles, so it measured the map. It fights on the mirrored arena
now, and the deterministic 36–0 sweep is gone.

**The ridge arena, and what it said about the evaluator** (2026-09-07, Wave 1
of the one-currency work). Phase 2 left "the arena is the instrument limit"
as its last open item: +3 points on the skill rows at 576 battles a row, on a
radius-10 hexagon with nothing for a commander who now prices cover under a
specific gun to be better at. `ridge_arena` is that ground — radius 12, a
level-2 crest in the middle worth 3, a spur on each flank ridge worth 2, near
woods the crest rim sees into at four and six hexes and reverse-slope woods
it cannot see at all (`los_clear` counts: summit 68 tiles, rim 148, spur inner
edge 112, a reverse-slope wood 9). The military crest and the topographic
crest are different hexes and the arithmetic can tell. It is written in the
coordinates its symmetry group is diagonal in — `s = 2x + y`, `t = y`, so
`mirror` negates both and `flip` negates the second, and a feature given as
`|s|`/`|t|` bands cannot come out asymmetric, which is a stronger guarantee
than the runtime check the old arena relies on. A first draft put the
level-1 shoulder three hexes out and hid the near woods in dead ground from
the crest, the opposite of their purpose: from a 22.5 m eye to a 2 m hull the
ray is under 10 m for the last two fifths of its length, so a lip that far out
hides the ground just beyond it. Each shoulder is now the minimum
`max_climb: 1` allows.

As an instrument it is the better one on the axis that matters. Its `ground`
row is 146–142 with no draws at difficulty 3, against the skill arena's
149–123 with sixteen, and `the ends` on it reads 144–144 at four seeds with a
spread of 2. `--arena` selects it for `skill`, `brains`, `mustered` and
`ground`; the default is unchanged and the eight-seed skill table on the old
arena came back with the same three pairs of integers ARCH-TODO records
(286–267 / 287–271 / 292–264), not merely the same percentages. The mirror
probe from Phase 1a became `examples/mirror` and gained a verdict: it names
the root (lowest pair in the first broken round; everything decided after it
inherits an unmirrored board) and says whether the root is an *equal-key
tiebreak* — two hexes of one objective the same distance from her, settled by
the coordinate the invariants allow to go last — or a *different key*, which
is a rule reading the compass. Both arenas come back with nothing but the
first, which closes the hunt Phase 1a left open: the residual is a coin, and
adding a key could not help because two hexes at equal distance on a mirrored
map are congruent ground by construction.

What it measured is a null with a sign on it. At eight seeds, 5 over 1 is
52.5% on the skill arena and **47.5% on the ridge**; 5 over 3 is 51.4% and
47.4%; both rows move together. The four-seed draw agreed. Two diagnostic
reweightings of the same map, 36 battles a pairing, say what it means. At the
shipped 3 / 2 the crest is nearer than the spurs for every crew and both sides
must contest it — and the difficulty-5 commander, who alone can see what a
bare plateau under a found gun costs, is the one who declines it. At 2 / 4 the
safe flank outscores the lethal middle and she beats difficulty 1 by 17 points
(66.7%) while **drawing every one of 36 equal-skill battles**, because each
side takes its own flank and nobody attacks. Building the crest up as a town
gives 60.0% at the price of half the battles drawn and a nine-point side
edge, since a blind defensible objective goes to whoever arrives first. So the
threat term, now priced honestly, outweighs an objective term that is still a
`value * decay` pull on a scale of its own: a crew who refuses the mission is
judging the ground right and valuing it wrong, and nothing in `score_tile`
says taking the objective is what the battle is for. The shipped 3 / 2 stays
because it is the configuration with an honest control (zero draws), and the
finding is the brief for the next chunk: quote the objective and the order in
the currency, and measure on the ridge.

Two properties fell out of building it. The base mod has no cover that does
not also block sight (forest 30 / 2, town 40 / 2), so everything commanding is
bare and everything covered is blind — true of the skill arena's hilltop
village too, where a crew in the middle can barely see out, and nobody had
noticed. And the knoll cuts the map in half lengthwise, so each side's
approach is covered from the *far* rim, and the prize for winning the race to
the crest is a shot at the loser's approach.

**A taxi run is two halves, and the AI plans both** (infantry employment,
2026-08-14; the design record is `infantry.md`). The fare mounts when riding
beats walking — rounds to cover the journey on foot against rounds to reach
the tailgate, be driven and get out, plus `planner.boarding_rounds` — and the
carrier drives to the pickup and holds the door while anybody has
`boarding == Some(her)`. Without the driver's half a platoon at one hex a
round never catches a carrier at six, so do not remove it as redundant.
`boarding_rounds` is 4 rather than the mechanical 2 because the cheap price
let a delivered platoon re-board for a three-hex hop and thrash against the
at-the-objective dismount reflex. A drop-off *short* of the objective was
tried as a cheaper substitute and cost two more platoons over 36 battles: it
traded dying in the back of a hull for walking the last stretch in the open,
and the taxi is usually killed by something the side has not spotted, so no
look-ahead could have seen it coming. What is still not planned is where an
emptied carrier goes — a mission belongs to a formation, so the taxi holds the
ground her passengers were sent to hold and roughly 21 of 24 die doing it,
under command or flat. That is per-unit tasking, in TODO.

**A wound outlives its battle** (direction step 3, 2026-08-24).
`CrewLoss::found` distinguishes "her vehicle did not come home" (`None`,
priced by what killed it through `resolve_crew_fate`) from "she was found like
this in a vehicle that did" (`Some(condition)`, priced by
`resolve_station_fate` — gentler, never `Lost`, never fatal without
permadeath). Before it, the entire in-battle crew model evaporated at the door
for every vehicle that survived. `CrewCondition::Absent` is "on the roll, not
in the vehicle", read once at spawn by `who_deploys`, and three things about
it are load-bearing: her seat leaves the substance reckoning entirely (neither
numerator nor denominator — charging it as a loss would make a short-handed
tank read as one already shot up, and every withdraw threshold and AI kill
estimate would price it that way); she stays in `Unit::crew` (the campaign
takes the crew list back at the end of the battle, so a cadet filtered out
here is deleted from her tank for good); and a vehicle nobody fit can crew
goes out with the walking wounded, because the campaign has no replacement
pool and a crewless vehicle is one nothing inside can kill — `crew_state`
stays empty in that case, which is also what keeps every scenario battle and
old save byte-identical. A battle never enlists anybody into an academy: the
anonymous crew a crewless vehicle gets is stamped into the *battle's* copy of
the roster, and `apply_battle_result` drops any crew id the campaign does not
know before writing survivors back, because an army holding ids that resolve
to nobody is not a crash and therefore sits there.

**A mission's promise lives beside the mission, not in the panel.**
`Mission::promise()` / `verb()` / `vocabulary()` and `Latitude::promise()` are
in `battle/command.rs` because a promise is a claim about the rules —
whoever changes what `Advance` does is then looking straight at the sentence
claiming what it does. One `VOCABULARY` table feeds all three accessors and
`slot()` is exhaustive, so a new mission without a promise fails to compile.
The game crate owns the *keys* and joins the two in `order_menu()`;
`every_mission_key_has_a_promise` exists because the failure mode of that
join is silent — rename a verb in core and the panel simply lists one order
fewer. The panel went 240 px → 300 px, because a promise that wraps to three
lines is one nobody reads.

**A shot has two ends, and both of them are ground** (2026-09-06, Phase 2a
of ARCH-TODO.md). `hit_chance`, `hit_breakdown`, `shot_profile`,
`expected_damage` and `ai::best_weapon_against` gained the target's
hypothetical hex `at` beside the attacker's `from`; every real firing path
passes her real hex, and the determinism snapshot and the balance output were
byte-identical, which is the whole claim that it was a rearrangement. Two
things deliberately did not follow the hypothesis: `tgt.moved` (charging a
moving-target discount for a drive she has not made is `hexes_under_way`'s
measured bias with the sign flipped — far ground would read as systematically
safer) and the loader's round choice (`best_round_against` had always judged
from real positions, which nobody had noticed). `combat::best_weapon_from`
holds the three gates a shot must pass in one place, and
`battle::danger::fire_on` is the currency: every spotted enemy who could put
fire on her there, the resolver's arithmetic and nothing else. Three tests,
each mutation-checked. `hit_chance` and its siblings now trip
`clippy::too_many_arguments` at eight, the first time `combat.rs` has, exactly
as the hygiene note predicted; a `Shot` struct is due and was not built inside
a chunk claiming nothing changed.

**The evaluator prices danger in the resolver's own arithmetic** (2026-09-06,
Phase 2b–2d). `score_tile`'s threat had been the enemy's shot at the hex she
was *already* standing on, scaled by `1/distance` to the tile under discussion
and gated at six hexes — so cover, elevation, facing, profile and range never
reached the decision about where to stand, and the AI's opinion of a wood was
`cover × 0.03 × doctrine` in a model `balance` never touched. It is now
`battle::danger::incoming(registry, state, unit, tile)`, the sum over every
found enemy of what the resolver says she would take there; the gate and the
falloff were deleted rather than retuned, because each stood in for a
positional term the arithmetic can now state exactly. `caution * exposure`
stays: the sum is what the rules say, that is what she makes of it.

*What remained of the terrain term, and why it is a gate.* The obvious move —
keep `cover × 0.03` and shrink it — was measured first. At four seeds zeroing
the prior looked like a five-point win on the skill table; at eight seeds it
was 1.5 and 0.6 points, and the tell was already there at four, because
zeroing cover alone and elevation alone each did nothing and only the pair
did, an interaction with no mechanism. Zero also cost the arena its side
symmetry (46.7% against 51.7%), and zero is not available anyway: the prior is
the only reader of `cover_value` and `elevation_value`, so pricing it at
nothing retires two doctrine fields. So the prior *stands down where the
arithmetic speaks* — paid on a tile no found gun can reach, withheld inside a
found gun's envelope, where the resolver prices the same timber at 3.5
substance points against the flat bonus's 0.9. The coefficients became
`planner.cover_prior` (0.03) and `elevation_prior` (0.4) and ship unchanged.

*The one content number that moved.* `planner.deviation_cost` 2.0 → 3.0.
Threat used to be zero over most of the map and is now three to eight points
wherever a found gun reaches, so at 2.0 all three shipped doctrines deviated
from an order on the staged march, massed armour included. 3.0 is the smallest
value that restores the straddle and 3.0–6.0 all do; at 8.0 elastic defence
obeys too. Changing the currency a weight is quoted in reaches every weight
quoted in it, and this was the one that crossed a threshold. It is also
subordinate initiative finally having something to say: the tile her own
sweep offers is now priced against the gun covering it.

*Three knife-edge stages, repaired rather than weakened.*
`an_assault_presses_through_what_an_advance_pauses_for` moved its forward tile
from seven hexes to ten — at seven the old threat was zero at *both* tiles and
the assault won by 0.48; at ten the assault presses on by 0.62 and the advance
still halts by 1.63, with a new assertion keeping the tile under the gun.
`ground_the_enemy_reaches_first_is_worth_less_marching_for` asserts
`contest_aversion` at 5 instead of 3, because the resolver disagrees with the
old falloff about how much more dangerous a hill with a tank one hex away is
than ground five hexes off. The protected order-versus-judgment tests
(`a_binding_march_presses_on_where_an_ordinary_one_takes_cover`,
`a_cut_off_unit_keeps_the_orders_she_had`,
`a_binding_mission_is_not_discounted_by_a_loose_doctrine`) passed unweakened,
and `a_crew_who_breaks_off_says_so_and_one_who_was_meant_does_not` pins the
half that had been prose: a delegated crew who breaks off raises
`Decision::drill`, a binding one never reaches the drill.

*The determinism diff, read before regenerating.* 517 of 1503 lines, six of
eight units picking different ground, kills per seed unchanged (7 / 6 / 6 / 5):
what moved is where crews stood on the way. Seed 1, unit 4, used to take
(9,18,−27), drive into the open at (11,16), get spotted and open an eight-hex
duel; she now takes (6,19,−25) one row south, is first seen two rounds later,
and the answering medium engages from elsewhere. Nobody ordered any of it.

*The measurements.* 36 battles at seed 0: 24–12 → 22–14, 13.2 → 13.5 rounds,
shots 1894 → 1850 at 33% penetration both, infantry surviving 64 → 69 of 96,
delegation tax inside the ±6 band (massed 3 → 2, bounding 2 → 4). **Skill at
8 seeds × 36 = 576 a row: 5 over 1 49.5% → 52.5% (+1.2 sd), 5 over 3 48.7% →
51.4% (+0.7 sd).** The prediction written down before the work — that the
rows would rise — held, directionally and weakly, both rows moving together;
neither is individually decisive, and half the movement came from the terrain
gate rather than the threat term. The four-seed baseline (51.2% / 47.5%) was
a high draw on one row and a low one on the other, the fourth single-draw
number in this project to mislead. Utility order paid 12% (0.08 → 0.09 ms);
round resolution went *down* (1.62 → about 1.5 ms), which CLAUDE.md predicts
— crews that stop driving into guns spend fewer ticks manoeuvring.

*What the plan got wrong, in the agent's words.* "2c: cover and elevation stop
being a second opinion" reads as delete the term; the question was *where* it
applies, not how much survives. The plan expected 2b to be the whole effect;
2b alone at the shipped prior left 5-over-1 slightly worse at four seeds.
Nothing anticipated a shipped weight having to move. And "watch the cost"
pointed at the wrong perf row: `reachable` and `roads` moved more than
`utility order`, and neither is on the chunk's path — they measure work in a
particular game state, and the state is what changed.

## Performance

**`fog::recompute` no longer rebuilds every side's vision after every shot** —
and without the batching trade-off the original note proposed, so a firing unit
is still revealed instantly. Three things did it: a `SightGrid` resolves tile
sight-heights once (`los_clear` was doing a String-keyed terrain lookup per ray
step, ~1M times a round); vision is cached per unit against `(pos, range)`,
which is exact because the map cannot change mid-battle; and a side whose
`(unit, pos, range)` list is unchanged skips the union entirely, which is what
makes the per-shot recompute free.

Round resolution 14.05 → 1.02 ms, engine suite 26 s → 2.6 s, event stream and
final fog state bit-identical across four seeds. The remaining LoS cost is that
vision raycasts every tile in range independently; shadowcasting would be
roughly another order of magnitude, but it changes *which* tiles are visible,
so it needs its own design pass alongside the detection-roll rework.

**MCTS went from impossible to merely expensive**: ~35 s per unit order → ~3.3
s, and `examples/playthrough.rs` plays a full 32-round battle at difficulty 4
in ~50 s instead of failing to produce a single round. Still too slow to plan
against a human, so the shipped scenario keeps `utility`. What is left is
structural: 900 iterations × depth 20, with roughly every fifth rollout step a
Commit that runs the enemy's whole planning pass and resolves a full 12-tick
round.

**The caches a save must rebuild.** `SightGrid`, `MoveGrid` and the per-unit
vision inside `FogMap` are `#[serde(skip)]` because they are pure functions of
the map, and either grid alone would be a thousand entries per save. The price
is that loading *must* rebuild all three: an empty sight grid
answers every line-of-sight question wrongly rather than loudly, an empty move
grid says every step is impossible so nobody can drive, and an empty
`visible_key` panics because recompute indexes it by side. `tests/save.rs`
pins the property that matters — not that the fields round-trip but that the
*future* does — by forking a battle in progress, sending one copy through a
save file, and requiring both to produce the same events for the rest of the
fight. That is why the rng's stream position is saved rather than its seed.

**Rebuilding them became a type (2026-09-07, ARCH-TODO 3a).** `save::rehydrate`
was four manual duties in prose, and the fourth cache added would have been
the one somebody forgot. Now `BattleState` is `Battle<Built>` and serde can
only produce `Battle<Unbuilt>`: a `#[serde(skip)]` field takes
`Default::default()`, `Unbuilt` has one and `Built` (a private unit field)
does not, so `serde_json::from_str::<BattleState>` fails to compile. The one
bridge, `SavedBattle::rehydrate(&registry)`, destructures the struct field by
field. Two shapes were rejected: a `Caches` struct owning the skipped fields
renames `state.sight` and `state.moves` at every call site in the engine and
still cannot own `FogMap`'s two skipped fields; a hand-written saved-form
mirror is a second copy of sixteen field names, the drift the item exists to
remove. The marker costs one defaulted generic and every signature still
reads `BattleState`. The snapshot passed unregenerated and all eleven tours
passed, which is the proof no rule moved.

**A campaign battle validates (3d, same day).** `from_placements` had never
called `validate_into`, so a chassis a mod stopped shipping reached
`spawn_unit`'s `expect` and took the run down. It now returns the same
`BattleSetupError` as `from_map`, collecting every problem rather than the
first, and `spawn_unit` is fallible. The game crate asks before it commits:
`launch_battle` preflights through `field_battle_problem` and declines the
clash with one log line, leaving the armies where they were, which is the
difference between a bad mod and a lost run. `choose_battle_map` returns an
`Option` instead of expecting that some mod ships a battle map.

## Tooling

**The balance harness**, `examples/balance.rs`, and it is two harnesses on
purpose. The analytic pass is instant — it stands two vehicles on an empty
field and asks `preview_attack` what one shot does, so it is the loop to sit in
while editing json — and `--sim` fights whole battles for the check at the end.
Everything goes through the real combat code rather than reimplemented
formulas, so the report cannot drift from the game. Its first run said two
useful things: `mg kills heavy_tank in 3 rounds`, which is the `.max(1)` floor
stated as a number rather than a worry, and the 67% stalemate rate above.

**Seeing the game without playing it.** `crates/game/src/devtools.rs` drives
the real input path from a script of timed actions and captures the window
along the way, which is what made rendering and UI changes reviewable by
anybody not sitting at the keyboard. Each of these was learned the hard way:

- **The scripted cursor is a resource, not the window's.** Writing to
  `Window::cursor_position` makes `bevy_winit` warp the real OS pointer, which
  fights the user for their mouse and fails silently when unfocused or on
  Wayland. `map_render::View` consults `ScriptedCursor` first instead, and
  scripts name a **hex**, which makes them independent of zoom, pan, window
  size and rotation.
- **`run_script` must stay `.after(InputSystems)`.** Bevy clears
  `just_pressed` at the top of `PreUpdate`, so a press injected before that is
  wiped before any handler sees it; the symptom is a click that silently
  selects nothing. It caught this bug in itself on its first run.
- **Scripts wait on the game, not on a stopwatch.** `until <predicate>` and
  `expect <predicate>` read `ScriptFacts`; a failed `expect`, a timed-out
  `until` or a degenerate screenshot exits nonzero, so a tour is a test. A
  `wait` that guessed short photographs a half-played round and says nothing
  about it.
- **A tour is a test only because `scripts/dev/run-tours.sh` exists.** Nothing
  else runs them — not `cargo test --workspace`, not CI, which has no display.
  Each tour declares its boot environment on a `#!env` line; that used to be
  prose at the top of the file, and three tours were written off as broken for
  weeks when they had only ever been invoked on the wrong map.
- **`idle` is `Battle::listening`, one predicate.** "A keystroke would be
  acted on this frame" is not "the phase is planning": sprites finishing a
  walk hold the keyboard, and a side that has committed is done talking. When
  the two drifted apart, `until idle` came true a frame early, four
  `key Enter` presses advanced the battle by one round, and every screenshot
  after them described the wrong turn while the script reported success.
  `waiting` is the strict complement — held behind something the player must
  answer or dismiss — and the academy roll sets it even though she opened it
  herself, which is what stops a tour photographing the map with a panel on
  top.
- **A click on a stacked hex cycles through its occupants.** `unit_at`
  answers with whoever comes first in id order, right for a HUD line and
  silently wrong for selection the day a hex could hold two crews: a platoon
  dismounts onto her carrier's own tile, so she sat behind the carrier and
  could not be selected by mouse at all. The infantry tour was the only thing
  in the project that ever tried to re-mount a platoon, which is the argument
  for the runner. `ScriptFacts::selected` lets a tour assert *who* a click
  selected rather than discovering three actions later that a keystroke went
  nowhere.
- **Every screen answers for every fact.** `ScriptFacts` is one resource
  shared by all screens, so a field a publisher left alone held the previous
  screen's answer — the campaign publisher set seven of eight and left
  `selected` naming the last crew clicked in a battle. Each publisher now
  assigns the whole struct through an exhaustive literal with no
  `..default()`, the bargain `Mission::slot` makes; the campaign map says
  `selected: None` out loud, because a map has a selected *army* and that is a
  different question.

**The clippy cleanup's shapes** (`-D warnings` gates, and denies rustc's own
lints too). `map_render::View` bundles rotation, centre, window, camera and
the scripted cursor — five things threaded separately through nine systems
that are all answers to "how are we looking at the world", so a system asks
for `view` and calls `view.hovered(&map)`. `TextSlot` / `MarkerQuery` /
`BattleHud` name the query types whose `Without` filters exist only so Bevy
can prove two `&mut` queries do not alias. `PlacementCheck` replaced an
eight-argument nested fn in `MapFile::validate`. Two
`#[allow(clippy::too_many_arguments)]` remain, on `battle::pump_events` and
`overworld::enter_overworld`, because a Bevy system's parameters are its
dependency list and those two do not decompose into any smaller noun. The
earlier note blaming the combat functions and proposing a `Shot` struct was
wrong: `combat.rs` never tripped the lint; every offender was a Bevy system in
the game crate. `cargo fmt --check` gates alongside, with
`style_edition = "2024"` pinned in `rustfmt.toml` so a toolchain upgrade
cannot turn CI red on its own.

**The danger overlay** (2026-09-06). `panel::format_danger` reads `fire_on`
verbatim and leads the tile panel whenever one of the player's crews is
selected and a hex is hovered — *Danger at (15,20): Irma Krieger 45% for 3.0,
75mm KwK; Nadja Orlov 82% for 10.6, 88mm PaK; 13.6 expected, one shot each;
151% of what she has left*. `D` tints the selected crew's reachable tiles by
the same total, replacing the move-range blue rather than stacking a third
colour on it, in four bands of what she has left. Decisions worth knowing: it
leads the panel because a full datasheet pushed the one mouse-dependent line
below the fold; never for an enemy crew and not on the shot preview or ghost
report; computed under `range_dirty` and cached rather than per frame, because
`fire_on` over a 42-tile reach with four spotted enemies is 228 µs in release;
the tour pins a seed because it asserts positions, and commits one round
first because an honest overlay has nothing to say before anybody is found.
What it cannot show: an ambush (it reports the *picture*), the enemy where
she will be rather than where she is, a thinner plate from turning in (her
facing at the tile is her facing now), and a coaxial that also bears
(`fire_on` names one weapon per enemy). The yellow band is nearly unreachable
in the base mod — an 88 expects 10.6 against a medium tank's ~9, so any tile
it covers is instantly "a third or more" — which is correct and reads as
harsh; a log scale or a full-complement denominator is the honest fix. The
total was first labelled "a round" and is per shot; a 75 mm at a shot every
15 s fires four times a round, so the label was wrong by the whole cadence of
the fastest guns.

## Bugs that turned out not to be ours

**Intermittent `DeviceLost`.** `Caught DeviceLost error: Unknown Unexpected
error variant (driver implementation is at fault)` a few seconds in, killing
the app. It looked like an overworld rendering bug; it is the NVIDIA Vulkan
FIFO present path on this box (2× RTX A4500, driver 580.126.20, X11).

The evidence: `crates/game/examples/minimal_window.rs` is a stock Bevy window
with none of our systems and it reproduces 3 runs out of 3. Under
`PRESENT=immediate|mailbox|nosync` it survives 3 out of 3; under
`autovsync`/`fifo` it dies every time. Workaround is `STAHL_PRESENT=immediate`
(see `main.rs`); the default stays vsync, because this is pixel art and tearing
is the one artefact it cannot hide, so the knob is opt-in for the affected
machine rather than a change for everyone.

False trail worth remembering: `WGPU_BACKEND=gl` looks like a fix but only
because the GL backend finds no device here and panics before rendering — it
produces zero `DeviceLost` lines for the wrong reason. The long-standing
`Couldn't get swap chain texture ... Cause: 'Outdated'` warning appears on
every run just before it, and is probably the same driver issue's first
symptom.
