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
| 4 | Fix the fun taxes REVIEW.md found | not started |
| 5 | Park a layer (MCTS) | not started |

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

**Still open in step 1 — part 3.** A player-issued *mission* is still scaled by
`1.5 - doctrine.delegation` and still damped to a quarter on contact. Latitude
is per-unit only. Doing the same for missions means threading it through
`MissionChange`, `Formation` and the save, which is a chunk of its own; the
per-unit channel was the one the complaint actually named.

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

**Still open.** A roster screen the player can open whenever she likes. The
after-action page is deliberately not it: it is a thing to have read, with one
key and no choice on it, and anything the player wants to *do* about her
casualties belongs somewhere she is not being held behind a modal.

### 4. Fix the fun taxes

From REVIEW.md, in its priority order: artillery target fixation, broken units
that cannot withdraw, the 19-round mid-game creep. Note the first two are AI
defects rather than rules defects.

### 5. Park a layer

MCTS is ~3.3 s per order and ships unused; the scenario names `utility`. It is
maintenance surface with no role. Either it becomes the enemy brain or it goes
on ice, deliberately and in writing.

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

- `PLATEAU`, `MISSION_WEIGHT`, `contact_scale`, the `0.15` distance decay and
  the withdraw pull in `ai/eval.rs` — the numbers that decide what an order is
  *worth* against terrain, which is the exact knob the original complaint was
  about.
- `AMBUSH_PATIENCE` and `BOARDING_ROUNDS` in the battle layer.
- The interior-effect and brew-up constants not already in `balance`.

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
