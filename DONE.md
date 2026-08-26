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
> roll of a die that ranges 16–27. The numbers that supersede these are in
> CLAUDE.md under "Difficulty is inverted in practice"; re-draw them with
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

**Saves record which mods were playing.** `SaveGame.mods` stamps id and
version; mismatched ids are refused (the rules genuinely differ), version drift
on the same set warns and loads (a content patch must not cost the player their
campaign), and saves written before the field existed are trusted rather than
rejected.

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

## Tooling

**The balance harness**, `examples/balance.rs`, and it is two harnesses on
purpose. The analytic pass is instant — it stands two vehicles on an empty
field and asks `preview_attack` what one shot does, so it is the loop to sit in
while editing json — and `--sim` fights whole battles for the check at the end.
Everything goes through the real combat code rather than reimplemented
formulas, so the report cannot drift from the game. Its first run said two
useful things: `mg kills heavy_tank in 3 rounds`, which is the `.max(1)` floor
stated as a number rather than a worry, and the 67% stalemate rate above.

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
