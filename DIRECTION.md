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
  that nothing stops a wounded girl deploying, and the game crate never checks
  `GirlStatus::is_ready`.
- No XP, no progression, no roster screen, no post-battle "here is who came
  back".
- Requisition exists; nothing spends money.

Fire Emblem's battles are not mechanically richer than these. They are
*consequential*, because the player knows the people. [tone.md] already says
the emotional half of this game lives at the academy, and the academy does not
exist. The battle layer cannot carry the spark alone and no quantity of
sprites will make it.

Meanwhile the best writing in the project is the combat narration — "command
passes from X to Y", "ammo rack destroyed", "girl #0 is OUT" — and it is
currently visible only to whoever runs the `playthrough` example.

## The plan

Ordered by spark per hour, not by dependency.

| # | step | state |
| --- | --- | --- |
| 1 | Orders mean what they say | **parts 1–2 done**, part 3 open |
| 2 | Say what a verb means before it is pressed | not started |
| 3 | Close the consequence loop | not started |
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

### 3. Close the consequence loop

Battle → named girls wounded and lost → a roster screen the player looks at →
next battle. Wounds with teeth (`GirlStatus::is_ready` actually checked), one
post-battle screen. Small next to what is already built, and it is the whole
difference between a wargame and *this* wargame.

### 4. Fix the fun taxes

From REVIEW.md, in its priority order: artillery target fixation, broken units
that cannot withdraw, the 19-round mid-game creep. Note the first two are AI
defects rather than rules defects.

### 5. Park a layer

MCTS is ~3.3 s per order and ships unused; the scenario names `utility`. It is
maintenance surface with no role. Either it becomes the enemy brain or it goes
on ice, deliberately and in writing.

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
