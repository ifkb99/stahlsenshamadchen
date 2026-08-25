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
