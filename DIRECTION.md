# Direction

A design memo, and the live state of the work it spawned. Written 2026-08-24
after a big-picture review, at the point where the engine is close to an MVP
and the game does not yet have a spark.

[CLAUDE.md](CLAUDE.md) is what the code is. [TODO.md](TODO.md) is what is left.
[DONE.md](DONE.md) is why the built things are shaped as they are. **This file
is why the next few chunks are being built at all** — the argument, so that
when a step turns out to be wrong it is possible to see which premise failed
rather than just which patch did.

## The complaint

From the designer, in full, because the wording matters:

> the game still feels a bit off … there is a lot of life that needs to be
> poured in (UI, art, sounds, visual indications for shots, etc), but I don't
> feel a spark. Many things have certainly been done right, but the game feels
> a bit unintuitive. For example, the orders given never really seem to work
> out how I would like them to. I really love the idea of orders, but it
> really doesn't seem to be panning out … I fear we may be layering too many
> things on top of each other, instead of letting what works emerge and
> interact with each other.

Three claims: orders do not land, the game is unintuitive, and the layers are
stacked rather than interacting. The first is the concrete one and the other
two are downstream of it, so it is the one to attack.

## The diagnosis

### Orders are weighted opinions, not commitments

A formation mission reaches `Evaluator::mission_value` (`ai/eval.rs:462`) as a
term in a sum:

```
MISSION_WEIGHT(2.0) × strictness × doctrine.objective_value × (reward − 0.15·dist)
```

and is then added to cover, elevation, threat and shot value. Three
consequences, none of which is visible from the keyboard:

- **`strictness` is `1.5 − doctrine.delegation`** (`eval.rs:450`). The
  player's authority over her own tanks is a float in a mod file, and it
  differs per formation.
- **The order competes with the terrain.** "Take that hill" can lose to "this
  hedge scores better", and the unit ends up *near* where it was pointed.
  A player reads that as drift.
- **`contact_scale = 0.25`** (`eval.rs:277`): the instant anyone shoots, an
  `Advance`'s pull drops to a quarter and local appetites take the round.

That last one is the whole problem in miniature, and it is worth being precise
about, because **the design is right and the interface is what fails**.
`Advance` is movement to contact — halt and fight. `Assault` is the deliberate
attack that presses through fire. That is a real distinction, correctly
modelled, argued from a playtest in the code comments. At the keyboard it is
`G` versus `X` with no explanation anywhere in the game, so the player who
meant "attack" presses `G` and watches her attack stop halfway for reasons she
cannot see.

### What is already right, and was wrong in the first pass of this memo

The first draft of this diagnosis claimed player move orders were one-round
nudges cleared every round. **That is wrong**, and the correction matters
because it narrows the work considerably:

`Unit.tasking` (`battle/mod.rs:242`) is a *destination*, not a path. It
persists across rounds, re-paths from wherever she stands, and is cleared only
on arrival, on recall, or when her formation is given fresh orders.
`Unit.detached` (`:221`) excuses her from the formation's standing mission
entirely, and `eval.rs:241` honours it. The comment on `detached` even records
that the first playtest read the old behaviour as the game overriding the
player.

So the per-unit channel is **already literal and already persistent**. The
direct-order half of "make orders literal" is built. What is left is narrower
and sharper.

### The five overrides, and which of them are honest

An order can be turned into not-that-order by five independent systems. Each
is individually well-reasoned. The problem is the aggregate, and specifically
which ones announce themselves:

| override | where | announced? |
| --- | --- | --- |
| morale rung refuses | `obeys()` | yes — "refuses to advance" |
| mid-round battle drill (idle crew bolts for cover) | `orders.rs:1138` | yes — `TookCover`, logged |
| radio latency holds an order at the wire | `Order::Radio` | yes — `OrdersWaiting` |
| contact damping on `Advance` | `eval.rs:277` | **no** |
| planning-table drill preempts a personal march | `ai/command.rs:797` | **no — and actively filtered out** |

That last row is a defect, not a design choice. `crates/game/src/battle.rs`
(`drilled.retain(...)`) keeps the "takes cover on her own" line only for units
whose formation has **no** mission. A unit under the commander's personal
tasking is normally in a formation that *does* have one, so when her march is
broken off for cover the line is filtered away and **nothing is said at all**.
The player watches a tank she ordered to a ridge stop in a hedge, every round,
silently, forever.

That is very close to a literal restatement of the complaint.

### The governing principle

**Uncertainty about the enemy is drama. Uncertainty about your own orders is
noise.**

Combat Mission — the stated reference — is brutally literal here. The order
executes exactly as given; what is unpredictable is the world. That is what
makes a dead tank a story the player authored instead of a thing that happened
near her. This game currently simulates outcomes in extraordinary detail while
leaving the player unsure what her own input meant, which is backwards.

The corollary, which is the actual rule for this work: **friction the player
can predict and price is drama; friction she cannot see is a bug report.** A
crew that breaks under fire is a story *if the player saw the morale rung
before she gave the order*. The same event with no warning is the game
cheating.

### On "too many layers"

The instinct is right, but the layers are not the problem — the **ratio** is.
Fog, concealment, facing-dependent penetration, overpressure, crew stations,
morale rungs, reaction latency, radio latency, delegation, missions,
formations, objectives, exits, taxi runs, doctrine weights, difficulty noise.
Each was built well and measured. The surface the player touches is still
click-a-hex and read the log afterwards.

Two independent tells that this is real rather than a feeling:

- **The AI cannot play this game either.** [REVIEW.md](REVIEW.md), game 1: the
  howitzer fires all 40 rounds at a plate it cannot penetrate and never
  re-evaluates. When a thing that can score every tile on the map still cannot
  find the decision, the decision space is not legible.
- **The command layer costs tempo and currently buys nothing.** From
  `balance --sim`, 2026-08-24: massed under command 9 wins against 8 flat,
  elastic 8 against 8 — parity — while rounds per battle go **14.5 → 18.3**
  and carriers lost go 5 → 7. Delegation is presently buying slower battles at
  the same win rate.

Credit where it is due: the shot preview (`format_attack`,
`game/src/battle.rs:3090`) is genuinely excellent — hit % with every modifier
itemised, pen chance, facing, effective armour. It is the model for what every
other decision in the game should get. The gap is not information density; it
is that the *order verbs* got none of that treatment.

### On the missing spark

The designer's own list — UI, art, sounds — is real but is not why it feels
flat. **Nothing in this game costs the player anything yet.**

- Permadeath is off by default and the wound system has no teeth: TODO records
  that nothing stops a wounded cadet deploying, and the game crate never checks
  `CadetStatus::is_ready`.
- No XP, no progression, no roster screen, no post-battle "here is who came
  back".
- Requisition exists; nothing spends money.

Fire Emblem's battles are not mechanically richer than these. They are
*consequential*, because the player knows the people. [tone.md] already says
the emotional half of this game lives at the academy, and the academy does not
exist. The battle layer cannot carry the spark alone and no quantity of
sprites will make it.

Meanwhile the best writing in the project is the combat narration — "command
passes from X to Y", "ammo rack destroyed", "cadet #0 is OUT" — and it is
currently visible only to whoever runs the `playthrough` example.

## The plan

Ordered by spark per hour, not by dependency.

| # | step | state |
| --- | --- | --- |
| 1 | Orders mean what they say | **parts 1–2 done**, part 3 open |
| 2 | Say what a verb means before it is pressed | **done** |
| 3 | Close the consequence loop | **done**, roster screen open |
| 4 | Fix the fun taxes REVIEW.md found | **done**, bar the A/B |
| 5 | Park a layer (MCTS) | **answered: it is not better. Park it** |

### 1. Orders mean what they say

Not "remove the friction" — *make the friction legible and give the player a
verb that overrides it*. Three parts:

1. **The silent override becomes audible.** A personal march broken off by the
   planning-table drill says so in the log. Pure honesty; no behaviour change.
   This alone may account for a large share of the complaint.
2. **A way to say "I mean it."** The `Advance`/`Assault` distinction exists at
   formation level and has no per-unit twin: there is no way to tell one crew
   to press on through fire. Add order latitude so the player can, using the
   same grammar as the formation keys.
3. **A player's order is not scaled by her subordinate's doctrine.**
   `delegation` should govern *how* a subordinate achieves an order, never how
   much she believes it.

Constraints this chunk must respect:

- **Additive.** Today's behaviour is the default in every case, so a mod, a
  save and an AI side that never say otherwise are bit-for-bit the old game.
- **The AI never issues a binding order**, which is what keeps the determinism
  baseline untouched. If `tests/snapshots/event_stream.txt` moves, something is
  wrong — do not regenerate it to make this chunk pass.
- **Latitude is vocabulary, not tuning.** It belongs beside `Mission` in the
  order stream so saves, replays and future external brains carry it. Any
  *numbers* it introduces belong in mod data.

**What landed (2026-08-24, `feat/binding-orders`).** Parts 1 and 2, as
`Latitude` — `Delegated` (today's game) or `Binding` ("I mean it"), carried on
`Order::Radio`, stored on `Unit.latitude` beside the destination it qualifies,
and held with the order at the radio so one that waits arrives meaning what it
meant. It is read in exactly one place: the drill gate in `ai/command.rs`. The
player says it by selecting a crew and pressing `X` on the ground — the same
key that orders a *formation* to assault, because it is the same sentence at a
different scale and there is no second idiom to learn.

The silent override is audible now, and that turned out to be a defect rather
than a design choice: `AiPlanner::last_was_drill` and `Decision::drill` let the
planner say which orders were its own idea, replacing the game crate's guess
(keep only units whose formation has no mission) that had been filtering out
the one case that most needed saying. A broken-off march now reads *"Anka Weiss
breaks off her march to (14, 3) and takes cover"* instead of nothing at all.

Verification: 194 engine tests green,
`a_binding_march_presses_on_where_an_ordinary_one_takes_cover` pins the rule as
a comparison — same stage, same seed, latitude the only difference — and the
**determinism baseline passed unregenerated**, which is the additivity claim
holding: the AI never issues a binding order, so AI-vs-AI play is bit-identical.
`scripts/dev/press-on.txt` drives the real input path and asserts the order
lands. Three failed test drafts are recorded in that test's comments, because
each of them was a fact about the engine worth keeping: `radio()` marches her
itself so the drill can only preempt from round two; `threatened` asks whether
the enemy can *meaningfully hurt* her, so two mediums are never in danger from
each other; and cover works for whoever stands in it, so woods beside the enemy
let the enemy vanish instead.

**Part 3 landed (2026-08-25).** `Latitude` now rides on a formation's orders
as well as a crew's: on `Formation`, in `MissionChange` (so an order held on
the wire arrives meaning what it meant), in the `CutOff` snapshot (so a crew
who lost contact soldiers on the orders she was given *as she was given
them*), and on `Order::SetMission` / `Order::QueueMission`. The player says it
with **Ctrl** on a mission key, composing with Shift's "…and then this".

**What it governs, and what it deliberately does not.** Only the strictness
term — `(1.5 - delegation)`, whose floor goes from 0.5 to 1.0 under `Binding`.
That says the intended thing exactly: **`delegation` may make a subordinate
more literal than she was asked to be, never less.** A loose doctrine read
"take the ford" at 0.8 of face value, which is the complaint — an order quietly
worth less because of who it was given to — and insisting now buys the letter
of it and no more than the letter.

It does **not** lift the contact damping, and that is the interesting call.
Doing so was the obvious reading of "the same thing for missions", and it is
wrong: `Advance` and `Assault` differ in the contact damping and in nothing
else, so a binding `Advance` would have been an exact synonym for `Assault`.
Two idioms for one sentence is the thing this whole step exists to avoid. The
two axes are orthogonal and compose — the verb answers *will she halt and
fight when shot at*, the latitude answers *may her doctrine discount the order
at all* — and a binding assault is both.

Taking the doctrine out altogether was the other candidate and is also wrong:
a tight doctrine reads an order at 1.2, so "no doctrine" would make insisting
pull *less* than asking. `a_binding_mission_is_not_discounted_by_a_loose_doctrine`
pins all three claims, including that a binding order to a devolving doctrine
is worth exactly what an ordinary order to a neutral one is worth.

Verification: 205 engine tests green and the **determinism baseline passed
unregenerated** — the brain issues every mission at `Delegated`, spelled out
at all four call sites rather than defaulted, so AI-vs-AI play is bit-for-bit
the old game. `SAVE_VERSION` went to 3: everything added defaults, but
`MissionChange` went from tuple variants to struct ones, and an order caught
in transit in an older save is a formation that would silently forget what it
was told.

### 2. Say what a verb means before it is pressed

`Advance` versus `Assault` is the case in point: the distinction is built, and
free to explain. One line in the formation panel when a mission key is live.

**What landed (2026-08-24).** `Mission::promise()` and `Latitude::promise()`,
in the engine beside the enums they describe, because a promise is a claim
about the *rules* and the person changing what an order does should be looking
straight at the sentence claiming what it does. One shared `VOCABULARY` table
feeds `verb()`, `promise()` and `vocabulary()`, so a listed order and a given
one cannot say different things.

Three places read them:

- The formation panel lists every mission key with its price
  (`G advance - take it, halting to fight what shoots` beside
  `X assault - take it through fire, and pay for it`), which is the exact
  confusion the memo diagnosed.
- The standing order is read *back* with its promise under it, and so is an
  order still in the air — under a signals net that is the only place the
  player can see what she just decided before it lands.
- The unit panel does the same for the per-crew twin, which had no explanation
  anywhere in the game: step 1 added the `X` verb and only the changelog said
  so. It is drawn only for a crew the viewer may actually order — offering to
  press an enemy crew on through fire reads as an offer.

The panel went 240 px → 300 px, because a promise that wraps to three lines is
one nobody reads. Verified by `the_panels_explain_the_orders_they_offer` and
`every_mission_key_has_a_promise` (the keys live in the game crate and the
promises in the engine; the failure mode of that join is a menu that silently
lists one order fewer), plus `scripts/dev/orders-explained.txt`.

### 3. Close the consequence loop

Battle → named cadets wounded and lost → a roster screen the player looks at →
next battle. Wounds with teeth (`CadetStatus::is_ready` actually checked), one
post-battle screen. Small next to what is already built, and it is the whole
difference between a wargame and *this* wargame.

**What landed (2026-08-24).** Three things, and the first was a hole rather
than a missing feature.

1. **A wound taken at her station now survives the battle.** The battle
   tracked every cadet's condition seat by seat all fight, and the only
   casualties the campaign ever heard about were the crews of *destroyed*
   vehicles — so a gunner knocked out in round one of a battle her side won
   was fit again by the time the campaign screen drew. `CrewLoss` gained
   `found: Option<CrewCondition>` and `roster::resolve_station_fate` prices
   it: gentler than a wreck, never fatal without permadeath, and never
   `Lost`, because her tank came home and somebody got her to a doctor.
2. **`is_ready` is actually checked.** `CrewCondition::Absent` says a cadet is
   on the roll and not in the vehicle. Her seat leaves the substance
   reckoning entirely (an empty seat is not damage, or every withdrawal
   threshold in the game would price a short-handed tank as half dead),
   nothing inside can hit her, and whoever is left covers at the substitution
   penalty. She is *not* removed from `Unit::crew` — the campaign takes that
   list back at the end of a battle and a cadet filtered out here would be a
   cadet deleted from her tank for good. A vehicle nobody fit is left to crew
   goes out with the walking wounded rather than empty; the campaign has no
   replacement pool and a crewless vehicle is one nothing inside can kill.
3. **The campaign stops to show the bill.** An after-action page, worst news
   first, named: who was killed, who is wounded and for how long, who is
   walking back, and who came home with how many battles behind her. It
   freezes the campaign exactly as the muster prompt does, and for the same
   reason — the log already narrated all of this at three lines a second in
   among the tile income, which is to say the game already told the player and
   she certainly did not see it. The muster prompt is the other end of the
   same loop: it now names who is not fit to deploy, which is where the cost
   is actually paid.

The casualty numbers moved out of Rust into a `casualties` block in
`mod.json` while this was being written — they are the dial between "an
armoured skirmish costs nobody anything" and "half the school is in the
infirmary by Tuesday", and the whole of the fate model was written as bare
integers in `roster.rs`. That is a down payment on the designer's note below,
not a discharge of it: `ai/eval.rs` is still full of them.

**Found while photographing it, and half-fixed.** The first after-action page
ever drawn listed "Rosa Steiner — her first" twice. `frontier` names the base
mod's ten characters across eighteen vehicles, and the campaign stamped a
separate cadet per mention, so Kuhlmann fielded three Rosas. The engine half is
fixed and is a rule rather than a nicety — **one cadet, one seat**: a repeated
name is enlisted once per academy and the other vehicles crew anonymously,
which is exactly what a placement naming nobody has always got, and
`validate-mods` now names every dropped mention. The content half was put to
the designer as a fiction question — ten characters and eighteen tanks, so
either 2nd Company has no named cadets in it or the school needs more
students — and the answer, on 2026-08-24, was **more students**. See *The word,
and the school roll* below.

**The roster screen landed (2026-08-25).** `R` on the campaign map opens the
academy roll and `R` or `Esc` closes it: the school's whole strength in order
of battle, vehicle by vehicle and seat by seat, with each cadet's
availability — *fit*, *infirmary, 4 day(s)*, *walking back*, *killed in
action* — and how many battles she has behind her.

Three decisions in it worth keeping. **It is derived every frame, not
cached**, because a roll that kept its own copy of who is wounded goes stale
exactly when the player opens it. **It words availability differently from the
after-action report** — that page reports an event (*wounded today*), this one
reports a state (*not available for six days*), and using one vocabulary for
both would make the roll look like a report already dismissed. And **a cadet
whose vehicle did not come home is still on it**, under "Without a vehicle":
she survives her tank far more often than not, and a roll that only walked the
order of battle would drop her from the school on the day she most needs to be
on it.

The layout came from the screenshot rather than from the design. With the
chassis named on every cadet's line, every single line wrapped, and a wrapped
list of twenty-four people is not a list anybody reads; the vehicle became a
heading. So did dropping "0 battle(s)" — on day one that is twenty-four lines
saying the same thing about everybody, and a number now means somebody has a
history. `scripts/dev/the-roll.txt` is the tour, and its three `expect`s pass.

**Still open**: *doing* something about it. Moving cadets between vehicles and
a reserve is the other half of TODO's "move cadets around between tanks, and
see stats" — this is the seeing half.

### 4. Fix the fun taxes

From REVIEW.md, in its priority order: artillery target fixation, broken units
that cannot withdraw, the 19-round mid-game creep.

**Re-measured before starting, 2026-08-25, and three of the five diagnoses
were wrong.** Game 1 reproduces exactly (30 rounds, 28–0, Stalemate), so the
review is current — but watching a battle and measuring one are different
instruments, and the review only had the first.

- **Tax 1 was not a re-evaluation failure. Done.** The AI re-planned every
  round and got the same answer, because `round_worth` priced blast flat and
  never read the plate it was landing on while `overpressure` always has. The
  pricing and the resolver disagreed. Fixed by making the blast half
  `overpressure`'s twin, case for case; see DONE.md. It is worth knowing that
  the *first* draft of the test for this passed against the unfixed
  engine — the penetration half of the price has always been facing-aware, so
  comparing a howitzer's front and rear arcs proves nothing unless the gun is
  stripped of its penetration first.
- **Tax 2's chain reaction. Done.** Bounces no longer reset the stalemate
  clock. The two shipped together because they are one story: twenty-nine of
  the thirty-six wasted shells were fired after the only things blast could
  reach on that hull were already broken, and each of them bought another
  round of the battle.
- **Tax 3 is not a creep and more movement points would make it worse.**
  Measured over game 1: the medium tank drove **53 hexes in 19 rounds to end
  10 hexes closer** to the bridge; the recon car drove 59 and finished
  **three hexes further away than she deployed**; the howitzer spent
  twenty-eight rounds oscillating between two adjacent hexes. Nobody is slow.
  A medium tank already makes 5 hexes a round, which is the 30 km/h the scale
  contract chose on purpose. They are *aimless*, and the mechanism is
  specific: `noisy_score` draws difficulty noise independently per candidate
  tile and the planner then takes an argmax over every reachable one, so the
  chosen tile is whichever got the luckiest draw. The maximum of eighty draws
  from ±0.5 beats an objective gradient of 0.54 a hex, every round — and the
  faster the vehicle the more candidates she has, which is why the 7 MP recon
  car wanders hardest and the 3 MP howitzer merely twitches. **Done, half of
  it**: difficulty is now one lean per unit per round rather than a draw per
  tile, which took straightness at difficulty 4 from 42% to 64% and —
  unexpectedly — closed the side-B edge the skill-gap table has carried since
  B4, because that edge *was* this bias. The other half, carrying an intention
  between rounds, was built and measurably failed; see DONE.md. It needs the
  planner to have a goal several rounds out, which is a chunk of its own and
  the one that meets the designer's subordinate-initiative idea.
- **Tax 2's other half — nobody retreats. Done, and not as an exit.** Two
  defects, neither of them the one named: `resolve_movement` refused *all*
  path movement from a crew that would not obey, advancing and retreating
  alike, under a comment saying "they simply will not advance"; and the exit
  was invisible to the evaluator until `condition` fell below
  `1 - withdraw_threshold`, i.e. below 15% for massed armour.

  **The designer settled the model rather than the bug**: a broken crew goes
  into defiance — fight, flight or freeze, by temperament — and rallies out of
  it, and flight means *away from contact* rather than toward a lane, because
  the map is eventually not going to have edges. That is what shipped; see
  DONE.md. Exits therefore stay narrow and stay what they are: an ordered
  withdrawal, not a rout.
- **Tax 4's A/B is confounded three ways**, not one: `playthrough.rs` gives
  side 0 both MCTS *and* `massed_armor`, on a map whose sides field different
  vehicles. Swap them independently or it says nothing — and it is the same
  measurement step 5 needs.

### 5. Park a layer

MCTS is ~3.3 s per order and ships unused; the scenario names `utility`. It is
maintenance surface with no role. Either it becomes the enemy brain or it goes
on ice, deliberately and in writing.

`balance --brains` is the instrument that settled it: the mirrored arena the
skill-gap table uses, same forces, same doctrine, same difficulty, varying
only which planner is thinking, in both orientations. **256 battles at 64 a
pairing, difficulty 3, 2026-08-25:**

| pairing (A vs B) | A won | B won | draws |
| --- | --- | --- | --- |
| utility vs utility | 33 | 31 | 0 |
| mcts vs mcts | 31 | 33 | 0 |
| mcts vs utility | 29 | 34 | 1 |
| utility vs mcts | 28 | 35 | 1 |

**MCTS 64 wins, utility 62, out of 128 mixed battles. That is parity.** The
controls are the reason to believe it: both same-brain rows came in within
two games of even, so the noise floor is about ±3 and a real difference would
have had to clear it in *both* mixed rows. Neither row favours a brain —
both favour whichever side is B, by an amount (69 of 128 combined) that is
inside one standard deviation of a coin.

So MCTS is ~1.8 s per order against the utility planner's 0.05 ms, roughly
thirty thousand times the cost, and it does not win. **It goes on ice.** The
verdict rather than the code: the module still compiles and its tests still
run, but nothing ships it, no planner change owes it a budget, and anybody
proposing to revive it starts from this table. The likeliest reason it fails
is worth writing down — its rollout policy *is* the utility planner, so it
searches a tree whose leaves are evaluated by the thing it is trying to beat,
and 900 iterations of that buys very little over asking the evaluator once.
A search is only as good as what it is searching toward.

## The word, and the school roll

**Answered by the designer, 2026-08-24**, and both halves are done.

**They are cadets.** The engine says so (`Cadet`, `CadetId`, `CadetStatus`),
the UI says so, and TODO's oldest open question — *"need to find better name
for girls"* — is closed. The objection the designer raised alongside the
decision is worth keeping written down, because it is the one anybody will
raise again: *cadet* sounds like a low rank, and this game runs the whole chain
of command. It does not bite. At an academy, cadet is an **enrolment status
rather than a rung**: everyone enrolled is a cadet and the appointments —
gunner, platoon leader, captain of the school team — layer on top, which is how
real academies do it. "Cadet Krieger, commanding 1st Company" is not a
contradiction. The full argument, including the cost (it is colder than the
academy register wants, and the warmth belongs in the names rather than in the
collective noun), is in `assets/wiki/reference/tone.md` under *The word: cadet*.

The rename was mechanical and is worth one sentence of evidence: the
determinism baseline was substituted textually and then **passed without being
regenerated**, which says the change touched the token and nothing else.
`SAVE_VERSION` went to 2, because serde field names moved.

**The school needed more students, and now has them.** Forty-nine characters
where there were ten: twenty-four at Kuhlmann, twenty-five with the Iron
Valkyries, and every seat of every vehicle in `frontier` filled by a cadet with
her own name, in the seat her skills are for. This was not only a fiction
decision. Substance is counted per person aboard, so a medium tank crewed by
the two cadets a map happened to name died about twice as fast as the identical
tank crewed by four anonymous ones — **naming your characters was a straight
mechanical penalty**, and an invisible one. Filling the seats also makes the
wound model legible: from a full order of battle, an empty seat means
`CrewCondition::Absent` and nothing else, which is exactly what step 3 built.

Two things deliberately not done. `river_crossing` still carries the old ten
and its partial crews, because it is the determinism baseline and crewing it up
means regenerating the snapshot for a content change; it is in TODO. And the
mixed-nationality question this raised (tone.md's open question 1) is answered
descriptively rather than settled: the expansion followed the shipped pattern
and used the room to say something with it — Kuhlmann is a local school,
eighteen of twenty-four German, and the Valkyries recruit, eleven of
twenty-five.

## From the designer, 2026-08-24, and not yet acted on

Two notes, recorded verbatim in substance because both are about the same
thing — that the numbers in this engine are opinions and nobody has argued
with them yet.

### `threatened` ignores fire that cannot hurt her

> Threatened may still want to account for attacks that cannot damage, scaling
> with morale and discipline. Even if your IFV is immune to 50 cal from the
> front, getting hit by it is not a fun time.

This is right and it is load-bearing in more places than it looks.
`threatened()` requires `best_weapon_against(enemy → me)` to be `Some` — the
enemy must be able to *meaningfully hurt* her — which after the ballistics
rewrite means a medium tank taking machine-gun fire is, as far as every
planner in the game is concerned, standing in a quiet field. It gates the
battle drill, the danger term in `score_tile`, and (this is how it was found)
it made two mediums unable to threaten each other at all, which cost three
drafts of a test in step 1.

The shape of the fix is the one this note names: being shot at is a *morale*
event whether or not it is a *damage* event, so the threat should scale with
what the fire does to the crew's composure rather than to the plate — which
also gives suppression somewhere to live, and gives the machine gun on every
tank in the game a job it currently does not have. Not attempted here because
it changes AI behaviour everywhere at once and would move the determinism
baseline, which is a chunk of its own with its own before-and-after numbers.

### The magic numbers want iterating on

> We have a lot of magic numbers here, it might be worth iterating over
> different values to balance realism and player agency.

Half-answered by step 3: the casualty table is now `casualties` in
`mod.json`, so the fate rolls can be retuned without a recompile and a harsh
campaign is a mod rather than a patch. That is the pattern the rest should
follow. What is still bare Rust and shouldn't be, roughly in order of how much
a designer would want to touch it:

- `PLATEAU`, `MISSION_WEIGHT`, `contact_scale` and the `0.15` distance decay in
  `ai/eval.rs` — the numbers that decide what an order is *worth* against
  terrain, which is the exact knob the original complaint was about. **These
  are now the ones to do**, and they are one chunk: they are four terms in one
  sum and sweeping any of them alone says less than sweeping the shape.
- `AMBUSH_PATIENCE` in the battle layer.
- The interior-effect and brew-up constants not already in `balance`.

**Half-answered again on 2026-08-27**, and the answer is instructive. The five
AI constants became the `planner` block — `impatience`, `horizon_rounds`,
`boarding_rounds`, `devolved`, `exit_urgency` — deliberately *not* as fields on
`balance`, because `balance` is what is true on the battlefield and these are
only how well a side is played. The withdraw pull the list above named turned
out to be `exit_urgency` and went with them.

And the sweep the chunk was built for came back **null**: a horizon of one
round and a horizon of eight are the same game at 36 battles a variant, inside
a seed noise floor of ±3 wins. That is exactly the value this note asked for —
"the value of moving a number into data is only realised when somebody sweeps
it" — and what it bought was a negative result delivered in four seconds rather
than a belief held for another month. It also says something about the list
above: if the horizon is worth nothing on the shipped maps, the four terms in
`mission_value` are the more promising chunk, because those decide what an
*order* is worth and orders are what the complaint was about.

Worth doing as one chunk with the `balance --sim` tables run before and after,
rather than piecemeal: the value of moving a number into data is only realised
when somebody sweeps it, and a sweep needs an instrument.

## Open questions for the designer

- **Should a binding order be able to kill a crew that would otherwise have
  lived?** It must, or it means nothing — but that is the moment the game
  needs permadeath decided (TODO's first item), because the player is now
  authoring the loss.
- **Is `delegation` still wanted at all once the player's own orders bypass
  it?** It would then only describe AI-to-AI command, which is a much smaller
  job than the knob currently implies.
- **What is the per-unit twin of `Assault` called?** "Press on" reads well in
  a log line; the formation verb is already spoken for.

## Log

- **2026-08-24** — memo written. Diagnosis above, corrected mid-write once
  `tasking`/`detached` were read properly. Step 1 started on branch
  `feat/binding-orders`, off `feat/reactive-scripts` (for the `until`/`expect`
  script harness).
- **2026-08-24** — step 1 parts 1 and 2 landed; see above. Part 3 (mission
  latitude) deferred deliberately. Note for whoever picks up step 4: the
  screenshot half of the script harness cannot run from this shell — the window
  never gets a surface and every `shot` lands 1x1, including in the pre-existing
  `battle-tour.txt`, so it is the environment and not the tours.
- **2026-08-24** — that note is now wrong, and worth knowing why. Screenshots
  capture correctly from this shell after all; whatever the window was missing
  earlier in the session, it had by the end of it. Do not conclude from one
  degenerate capture that the harness is broken — run `battle-tour.txt` and
  check, which is what settled it in both directions.
- **2026-08-25** — step 5 is answered, and the answer is no. MCTS and the
  utility planner are at parity over 256 controlled battles (64–62), with
  same-brain control rows within two games of even, so the noise floor is
  small enough to trust it. Parked in writing. Worth keeping: the A/B only
  became answerable because the balance tables now fight across all cores —
  the run took four minutes where the first attempt at a *quarter* of the
  sample was framed as an overnight job.
- **2026-08-25** — the planner has goals, and the goal chooser is the seam a
  learned policy would replace: candidates are shared knowledge, choosing is
  judgement, and the executor keeps every rule it already knows. Two things
  to carry forward. A mission has to *replace* the candidate list rather than
  join it — letting it compete undid step 1 without anybody noticing until a
  test did — and that is precisely where subordinate initiative attaches, by
  widening the list on doctrine. And the layer removed difficulty entirely on
  its first run, which turned out to be the good news: moving the blur into
  the chooser means a worse commander goes to the wrong place instead of
  twitching, and it exposes that the chooser is too shallow for skill to have
  much to be better at yet. That is the next question, and it is a better one
  than the game had before.
- **2026-08-25** — the wander. Half fixed and half a negative result, and the
  negative result is the more useful of the two. Difficulty noise was drawn per
  candidate tile under an argmax, which is a selection bias rather than a
  handicap; one lean per unit per round fixed it and incidentally explained the
  side-B edge on the mirrored arena, which was never resolution order.
  Commitment — keeping a destination across rounds — was built as recommended
  and made things *worse*, because this planner only ever scores tiles it can
  reach this round, so there is no intention to carry: it fired 15 times in a
  battle, 12 of them to a tile one hex away, and pinned exactly the units that
  were already stuck. Anybody picking this up again should start from the goal
  layer, not from the commitment.
- **2026-08-25** — the defiance chunk landed: a broken crew now fights, runs
  or goes to ground by temperament, and rallies faster with her officer in
  sight. Two things worth carrying forward. The determinism baseline passed
  *unregenerated* again, and the reason is checkable rather than lucky — the
  one crew who breaks on `river_crossing` is temperamentally a fighter, and
  fighting differs from the old freeze only in ambush discipline. And the
  delegation tax hit its written target of zero as a side effect: a commanded
  force whose crews may break contact loses less to the command layer than one
  whose crews may only stand there and be shot.
- **2026-08-25** — step 4's first two taxes landed: a shell is priced against
  the plate it will strike, and a bounce no longer holds a decided battle
  open. Both were one line of arithmetic that never consulted the thing it was
  about. The determinism baseline was regenerated deliberately and the diff is
  the evidence for the change rather than a cost of it — across four seeds the
  only unit whose behaviour moved is the one firing the round whose price
  changed, every kill and every crew casualty is identical, and the snapshot
  shrank by 96 lines because battles stopped running on after they were
  decided. Also worth recording: the review's own diagnosis of tax 1 was
  wrong (the AI re-planned every round), and its proposed cure for tax 3 —
  more movement points — is backwards. Measuring the logs rather than reading
  them is what caught both.
- **2026-08-25** — step 1 is complete (part 3, mission latitude) and step 3's
  remainder landed (the academy roll). Determinism baseline passed
  unregenerated again; `SAVE_VERSION` is 3. What is left of the memo is steps
  4 and 5, and the designer's two notes.
- **2026-08-24** — the word is settled and the roll is full: *cadet*, and
  forty-nine of them. See *The word, and the school roll*. The determinism
  baseline passed the rename without regeneration, which is the whole of the
  evidence that a tree-wide substitution changed no rules.
- **2026-08-24** — the after-action page found a defect on the first run: a
  campaign stamped one cadet per *mention* of a character, so `frontier`'s ten
  characters became twenty-four cadets with six names between them. One cadet,
  one seat now; the content half is left for the designer. Worth noting how it
  was found — nobody would have read it out of the code, and the screen that
  exists to make consequences visible made this one visible in its first
  minute.
- **2026-08-24** — steps 2 and 3 landed; see above. The designer's two notes
  on `threatened` and on the magic numbers are recorded in their own section
  and deliberately not acted on, because both move AI behaviour and therefore
  the determinism baseline. The campaign map publishes `ScriptFacts` now
  (`turn`, `idle`, `waiting`, `log`), which it never did, so a campaign tour
  can wait on the game rather than on a stopwatch.
- **2026-08-27** — the last five tuning constants in `ai/` became the `planner`
  block, and the first sweep they made possible answered a standing question
  with a null result: the goal chooser's horizon is worth nothing measurable
  between one round and eight. Two things to carry forward. The block is
  separate from `balance` on purpose — nothing in `planner` reaches a rule, so
  a mod that rewrote all five leaves a human-versus-human battle bit-identical
  — and that test ("does this number decide what is *true*, or how well a side
  is *played*?") is the one to apply to the next number somebody moves. And
  the null result is an instrument result: it is the third measurement now
  saying the shipped maps and the mirrored arena have too little terrain for a
  better commander to be better *at*, after 5-over-3 barely discriminating and
  the road-reading terms moving nothing. Terrain-varied ground is the
  bottleneck, not the chooser's depth.
