---
id: command
title: Chain of Command
category: Reference
---

# Chain of Command

The plan for command as a system: who decides, how decisions travel, and what
happens when they do not arrive. This is the design and build order; the
gameplay wish list it satisfies lives in TODO.md under Chain of Command, and
the girl-model machinery it stands on is described in [girls](girls.md).

The short version: **orders become things that exist in the world.** Today an
order is a free action from an all-seeing side onto any of its units. After
this, an order is issued by a *person* with command training, travels at the
speed of her signals net, arrives late or not at all, and is carried out by
subordinates who have their own judgement about it. The game's point of view
shifts from "the side" to "the commander" — while, by the additivity rule,
remaining exactly today's game when a map declares none of it.

## Decisions taken

Settled with the designer before writing this, so the rest of the document can
build on them rather than hedge:

- **The player's side is hybrid.** Mission-type orders to formations are the
  default way to play (Combat Mission-shaped); dropping to per-unit orders
  remains possible for any unit in command contact. Out-of-contact units run
  on their standing mission and their leader's judgement. With the command
  systems off, every unit is in contact and per-unit control is exactly
  today's game.
- **Losing a commander degrades, with succession.** Her formation takes a
  morale hit and falls back on standing orders and initiative until the
  next-senior girl takes over, worse at it. A scenario *may* declare the
  command unit a loss condition in map data; the engine never hard-codes it.
- **Formations are data.** A map or army file declares them: members, a
  leader, optionally a doctrine of their own. A side that declares none is
  one flat pool and behaves as today.
- **Battle side first.** The balance harness can measure every step of the
  battle half; the campaign half lands on top once the vocabulary is proven.
- **Brains are interchangeable.** Different academies get different
  commanders, and future brains — a neural net, an LLM — must slot in without
  surgery. This is a hard requirement and it settles the architecture below.
- **Information degrades upward as well as downward.** Orders arriving late is
  half of command friction; the commander's *picture* being stale, secondhand
  and colored by whoever reported it is the other half, and probably the more
  beautiful one — it creates the commander's seat as an experience (deciding
  on old information) without taking the controls away. Core, not garnish;
  see the command picture below.
- **Delegation is the default posture, and pace is why.** A campaign day
  should play quickly, which means the player *relies on* the mission AI
  rather than fighting it. That sets a quality bar the build order has to
  verify: a formation run by missions must fight nearly as well as one
  micromanaged per unit, or delegation is a tax and the player will rightly
  refuse to pay it.
- **Permadeath is the default** (alongside delegation). `CasualtyRules`
  already models it; the default flips once the wound system has teeth —
  today nothing stops a wounded girl deploying, and a default-on permadeath
  before the muster screen shows the stakes would be silent cruelty. The
  gentle mod turning it off remains a first-class way to play.
- **The enemy AI is a character.** Doctrine already makes two sides fight
  differently; the brain should go further and be *somebody* — the enemy
  commander is a girl in the roster, and her doctrine, initiative and traits
  are the knobs her side's brain runs on. The theater rule: her preferences
  must be visible enough to learn ("Ravenna always leads with her recon"),
  because an opponent whose character shows in her maneuvers is worth ten
  points of invisible search depth.

## What already exists to build on

The seams were cut in advance, which is why this lands as extension rather
than rewrite:

- **WEGO.** Rounds are plan-then-resolve; orders are per-unit intents applied
  through one entry point (`BattleState::apply`), events come out the other
  side. A command layer is more of the same vocabulary, not a new phase.
- **`PlannerRegistry`** constructors already receive the registry precisely so
  a commander planner can build subordinate planners with their own doctrines.
- **`DoctrineDef.initiative` / `delegation`** are declared, defaulted, and
  deliberately unread. They are the doctrine half of "acting without orders"
  and "devolving decisions".
- **The girl model.** `command` (Presence, Intellect, Charm) is how an order
  lands; `signals` (Intellect, Perception) is how it travels; `discipline`
  (Will, Intellect) is whether it is followed under pressure. The `commander`
  and `radio` roles exist. Every check arrives at a place and time, which is
  exactly what an order in transit needs.
- **The morale ladder and `OrderRefused`.** Disobedience machinery that is
  already legible — stated causes, visible rungs. Command failures join that
  ladder rather than inventing a parallel one.
- **Reaction latency rules** (`reaction` block, `reactions` skill) exist in
  data and are deliberately unconsumed. The design settled that the delay
  belongs on *responses to new information* — and an order arriving mid-round
  is precisely new information, so chain of command is what finally spends
  this currency.
- **Objectives and exits** gave the evaluator reasons to move. The TODO
  already states the debt: *willingness* to pursue ground or take the exit is
  doctrine-wide today (`objective_value`, `withdraw_threshold` via
  `EXIT_URGENCY`) and belongs to a leader. This plan is where that ownership
  moves.

## The architecture

Six commitments, each chosen for a reason that will outlive the first
implementation.

### 1. Missions are orders, in the order stream

A mission — "take the bridge", "hold this ridge", "withdraw by the west road"
— is a new `Order` variant applied through `BattleState::apply` like every
other order, and recorded in battle state:

```text
Order::SetMission { formation: FormationId, mission: Mission }
```

This is the load-bearing decision, and interchangeability is why. Everything
that can command — the built-in utility commander, the human through the UI, a
future NN or LLM brain — speaks the *same serializable vocabulary through the
same entry point*. Consequences that fall out for free:

- **Saves round-trip missions** (they are state), so `tests/save.rs`'s
  fork-and-compare property extends to command with no new machinery.
- **Replays capture command decisions**, because the replay viewer records the
  order stream. A nondeterministic brain (an LLM) does not break replay: the
  sim stays deterministic given seed + orders, and the orders are recorded.
- **The determinism baseline is safe by construction**: a map with no
  formations never sees a `SetMission`, so the event stream cannot move.

The mission vocabulary starts small and is a serde enum, snake_case, so mods
and external brains read and write it as data:

```text
Mission::Advance  { to: Hex }         drive for ground and take it
Mission::Hold     { at: Option<Hex> } stand where told (default: where you are)
Mission::Recon    { toward: Hex }     find them; do not die finding them
Mission::Withdraw { via: String }     leave by the named exit objective
```

Attack-a-formation, screen, support-by-fire, and the side-level mission types
TODO wants (eliminate, harass, raid) extend the enum later; the executor
machinery below is what makes each addition a scoring context rather than a
new subsystem.

### 2. Formations are map data; command state lives on the battle

A unit placement gains two optional fields, mirroring how `side` already
works:

```json
{ "vehicle": "medium_tank", "side": 0, "at": [4, 7],
  "formation": "1st_platoon", "leads": true }
```

and a side (or the map) may declare the formations themselves, with an
optional doctrine each — two academies' platoons fight differently, and so
may two platoons of one academy:

```json
"formations": [
  { "id": "1st_platoon", "name": "1st Platoon", "side": 0,
    "doctrine": "massed_armor" }
]
```

`validate-mods` checks the references (a `formation` naming no declared block,
a formation with no members, two leaders). A placement with no `formation` is
in the side's flat pool exactly as today. `ArmyPlacement` on the overworld
maps naturally: an army is a formation by default, and a large army may
declare several.

`BattleState` gains a `CommandState`: each formation's members, current
leader, doctrine, standing mission, and the transit state of any order still
travelling (below). It is serialized, walked in formation-declaration order
(never hash order — determinism), and defaulted empty so old saves open as
what they were.

### 3. Brains are planners; the hierarchy hides behind the existing trait

The driver contract does not change: a side is driven by something
implementing `AiPlanner<BattleState, Order>`, called until it commits. What
changes is what stands behind it:

```text
SideCommand (implements AiPlanner)
├─ commander brain: AiPlanner emitting SetMission (+ orders for her own unit)
└─ per-formation executors: fill member unit intents in service of the
   formation's standing mission, each under its formation's doctrine
```

`SideCommand` sequences them: brain first (missions), then executors (unit
intents), then `Commit`. It is built by the `PlannerRegistry` — the seam that
was left for exactly this — and registered like any planner, so a mod or an
experiment swaps the brain without touching the executors, or vice versa.
Three shapes of the same structure cover every consumer:

- **AI side:** brain + executors, as above.
- **Human side (hybrid):** a null brain — the human's UI issues `SetMission`
  and per-unit orders directly — plus the same executors, which fill in any
  unplanned unit whose formation holds a mission before the human commits.
  The player commands through the identical vocabulary the AI uses, which is
  what keeps the two halves honest with each other.
- **External brain (NN/LLM, later):** an `AiPlanner` that serializes the
  determinized (fog-honest) view of the state, hands it to whatever is
  thinking, and returns the `Order`s it chose. The planning phase is untimed,
  so a slow brain costs wall clock, never sim correctness. Determinization
  already exists (`mcts::determinize`); the observation builder reuses it so
  an external brain cannot peek any more than MCTS can.

The executor is where doctrine keeps meaning what it means today: it is the
current utility planner given a *mission context* — the mission contributes
the objective-shaped gradient term instead of the map's whole objective list.
`Evaluator::score_tile` grows a context parameter; with no mission in force it
scores exactly as now, which is the additivity hinge and gets its own pinned
test.

The brain, meanwhile, is where the enemy becomes a person. A commander brain
is constructed *for a girl* — the side names its commanding officer, and her
doctrine, her `initiative`, and eventually her traits are the parameters the
brain runs on. That is nearly free given the model (she is a
[girls](girls.md) instance like any other, and checks already arrive with
context) and it is what "the AI plays into the commander's character" means
concretely: swap the girl and the same brain fights differently, in ways a
player can learn and exploit. The theater half is a legibility duty, not an
algorithm: her signature moves have to be *visible* maneuvers.

The drive-planners-until-commit loop this hierarchy hides behind used to be
copied six times across the game, examples and tests; it now lives in one
place, `ai::AiDriver` (chunk 0, done), so the hierarchy is introduced behind
one loop.

### 4. Command is a check, and contact is a graph

The `command` block joins `scale`, `balance`, `reaction` and `morale` in
`mod.json` — data, replaceable wholesale by a later mod:

```json
"command": {
  "radius": 6,
  "radius_per_signals": 2,
  "relay": true,
  "latency_skill": "command",
  "base_ticks": 2,
  "ticks_off_per_level": 4
}
```

- **Contact.** A unit is in contact if it is within command radius of its
  leader, or of a friendly unit that is (when `relay` is on — this is what
  makes a radio vehicle worth fielding: a big radius multiplier is its
  character). Recomputed like fog is: deterministic, cached, and events on
  change — `OutOfContact` / `ContactRestored` — because a unit silently
  ignoring the player is indistinguishable from a bug.
- **Latency.** A mission does not take effect the tick it is issued. The
  issuer's `command` check and the receiver's `signals` check price a delay in
  ticks — the same currency as the reaction rules, spent at last. The order
  sits in `CommandState` in transit, with `MissionAssigned` when sent and
  `MissionReceived` when it lands, so the player watches the lag instead of
  suspecting it.
- **Out of contact.** Mission changes do not arrive. The formation continues
  its standing mission; whether its leader *deviates* — presses on, goes to
  ground, pulls back — is her doctrine's `initiative` weighted by her own
  command skill and nerve. This is where those reserved doctrine weights
  finally get read, and where "willingness belongs to the commander" lands:
  the evaluator's global exit gate becomes the *fallback* judgement of a
  leaderless or cut-off unit, while a formation in contact withdraws because
  it was told to.

Additivity: no `command` block (or zero coefficients) means infinite radius,
zero latency, everyone always in contact — today's game, with no `if` in Rust.
The gentle mod simply omits the block.

### 5. Information degrades upward: the command picture

The half of command friction most wargames skip, and the reason the
commander's seat will feel like a seat rather than a camera. Today a side's
fog is the union of its units' vision, live and perfect: every spot is known
to the whole side the tick it happens. Under command rules, what units see
stays exactly that — a unit fights with its own eyes, and the ambush and
opportunity-fire machinery is untouched — but what the *commander* works
from becomes the **command picture**: contacts as last reported, each with
an age and a reporter attached.

A sighting travels up the same wires orders travel down. The spotter's
`signals` check prices the delay; a unit out of contact cannot report at all,
which is the sharpest consequence in the system — **recon that cannot report
is recon wasted**, and suddenly the radio vehicle, the `signals` skill and
the relay chain earn their seats without a single new rule. What arrives is
her report, not the truth: a `ContactReported` event names who saw what,
where, *when*, and how she sounded — a rattled scout's report reaching the
player colored by her nerve is characterization and information warfare in
one line of log.

Consequences that fall out:

- **A contact nobody re-reports goes stale.** The picture renders it as a
  ghost at last known position. Ordering fire at a ghost is area fire at a
  hex — `FireIntent::Area` already exists and already carries the accuracy
  penalty that deserves.
- **The AI brain consumes the picture, not the side fog.** Symmetric honesty:
  the enemy commander is as behind events as you are, which replaces
  difficulty noise with something that produces humanlike mistakes for
  humanlike reasons. Executors act on their formation's own eyes, as real
  subordinates do.
- **The player's battle display reads the picture** when command rules are
  active. What your units *react* to and what you *know* come apart, and the
  gap is the tension the whole system exists to create.
- **Additive by construction.** With no `command` block the picture *is* the
  live fog — zero delay, nothing stale, today's display, today's brains.
- **Determinism holds**: reports are battle state advanced during ticks,
  walked in unit-id order, serialized like everything else.

This composes with, and does not replace, the detection rework on TODO:
detection decides whether a unit sees something at all; the picture decides
who else finds out, and when. The two are orthogonal and both make the recon
car worth its fuel.

### 6. Losing the commander degrades the formation

When the leader's vehicle is destroyed or she is out of the fight:

- Her formation takes a pressure hit (`morale` block gains a `leader_lost`
  entry beside `hit` and `ally_destroyed` — same ladder, no parallel system).
- Command passes to the next `leads`-ranked member, else the best `command`
  skill aboard the formation, announced with `CommandPassed`. The successor
  is worse at it: longer latencies, weaker initiative, because the checks are
  hers now and her numbers are real.
- A scenario that wants decapitation stakes declares it: a side-level
  `loss_condition` in map data naming a unit or formation whose loss ends the
  battle. Engine machinery reads the declaration; no battle carries the rule
  unless its map wrote it down.

## What the player sees (game crate)

The battle UI grows a formation panel: formations listed with their standing
mission, contact state, and leader; clicking one issues a mission (type +
target hex) through the same `SetMission` order the AI uses. Per-unit orders
work exactly as today for units in contact; a direct order to an out-of-contact
unit is refused with a stated reason, the same bargain `OrderRefused` already
makes. The morale ladder taught the rule: deviation and delay are only fair
when they are **legible, attributable, and predictable in advance** — every
command event says who, why, and how long.

Enemy contacts render from the command picture: live where a report is fresh,
a ghost marker at last known position where it is not, with the reporting
girl and the report's age a hover away. The log carries the reports
themselves — "Kesselring reports armor at the bridge, two minutes ago" — which
is where the battlefield's serious register and the girls being people meet
in one line.

A devtools script (`scripts/dev/command-tour.txt`) exercises the panel so the
UI is reviewable in screenshots, per the working guide.

## The campaign half (after the battle half proves out)

- `OverworldOrder::SetMission` for armies: the same vocabulary at operational
  scale (advance to, hold, raid toward, withdraw to).
- Radio range in overworld hexes, relayed through friendly armies (and later
  the comms-tower building). An army out of range continues its standing
  mission — which finally makes the radio and logistics units worth their
  place in the TODO's unit list.
- A battle inherits its sides' army missions as the commander brains' goal
  input, so what you ordered on the map is what the formations try to do on
  the field. The withdrawal case closes the loop the TODO's retreat item
  wants: an army whose battle-half withdrew *arrives somewhere* instead of
  ceasing to exist.

## Sequencing against the MVP

The MVP is core gameplay solidified to the point of a complete campaign or
two; the life layer — post-battle reports, sfx, barks, the academy screens'
warmth — deliberately comes after it. That deferral is safe under exactly one
condition, so it is stated here as a rule rather than a hope: **every event
must keep carrying the story** — who, what, why, in the event itself, never
reconstructed after the fact. `OrderRefused` names the girl and her rung;
`ContactReported` names the reporter and her state; `CommandPassed` names who
took over. As long as that discipline holds, the recap screen and the barks
are *formatting* deferred, not archaeology deferred — the `playthrough`
narrator is the standing proof, because any event stream it can already tell
as a readable battle is one a recap screen can tell with portraits.

## Build order

Staged so every chunk is verifiable with the instruments before the next
starts, per the working guide. Chunks 1–2 are the heart; nothing after them
starts until `balance --sim` says the formations fight sanely.

**0. One driver loop.** ✅ Done. The drive-until-commit loop that existed six
times now lives in `ai::AiDriver`; verified as a pure refactor (committed
determinism baseline passes unregenerated, playthrough and `balance --sim`
output byte-identical, battle-fight tour plays on screen).

**1. Formations as data.** Map/army format fields, validation in
`validate-mods`, `CommandState` on `BattleState` (populated, inert), save
round-trip. No behaviour change anywhere. *Verify:* baseline byte-identical;
new save test covers command state; validation tests for the referential
mistakes.

**2. Missions and executors.** `Order::SetMission`, mission context threaded
through `Evaluator`, `SideCommand` with a first commander brain (assign
missions from objectives + doctrine, reusing the objective-scoring logic that
exists) and per-formation executors. The brain is constructed for the side's
commanding girl from the start, even while it only reads her doctrine — the
seam costs nothing now and is what her traits and the theater plug into.
`river_crossing` gains formations so the shipped scenario exercises it.
*Verify:* the additivity test — a map with no formations produces a
byte-identical event stream; `balance --sim` before and after, with the
numbers in the commit; new engine tests
(`a_formation_advances_on_the_ground_its_mission_names`, etc.). This chunk
also stands up the **delegation-tax measurement**: the same army fought as a
flat per-unit pool versus as mission-run formations, over the 24-battle
harness. The tax will not be zero at first; the point is to *see* it, and to
drive it toward zero by chunk 6, because delegation is the default posture
and a delegation the player pays for is one they will refuse.

**3. Willingness moves to the commander.** `initiative`/`delegation` read;
withdrawal becomes a mission a leader issues, with the evaluator's global exit
gate demoted to the fallback for leaderless/cut-off units. *Verify:* `balance
--sim` on doctrine matchups (elastic_defense's 16-7 dominance is the
baseline to watch, not to fix — see the open question on balance targets);
exits still taken, never on round one.

**4. Contact and order latency.** `command` block, contact graph + events,
mission transit delay priced by `command`/`signals` checks. *Verify:*
additivity (no block = today, pinned test at any coefficient — the
reaction-latency post-mortem's tell); scripted battle showing
`MissionAssigned` → `MissionReceived` lag in the log.

**5. The command picture.** Reports travel up: `ContactReported` events,
per-side picture state (contact, position, age, reporter), stale contacts,
the brain switched from side fog to picture, the display switched with it,
ghost markers in the game crate. The biggest chunk after 2 and the one that
changes how the game *feels* the most. *Verify:* additivity (no command
block: picture ≡ fog, display and baseline unchanged); an engine test where
a cut-off scout's sighting provably never reaches the brain
(`a_scout_out_of_contact_reports_nothing`); `balance --sim` before/after,
since the brains now fight on degraded information.

**6. Commander loss.** Pressure entry, succession, `CommandPassed`, optional
`loss_condition` in map data. *Verify:* engine tests
(`command_passes_to_the_next_girl_and_she_is_worse_at_it`); a scenario with
the loss condition ends when it should.

**7. The human hybrid.** Formation panel, mission issuing, contact-gated
direct orders, ghost-contact hovers, devtools tour script. *Verify:*
screenshots; the game-crate end-to-end test TODO wants grows a command case.
This chunk is also where the delegation-tax number faces its real judge: a
play session where relying on the missions feels better than overriding
them.

**8. Campaign missions.** Overworld vocabulary, radio range + relay, battle
inheritance, withdrawal arrival. *Verify:* a campaign test that orders a
mission out of radio range and watches it not arrive.

**Later, enabled but not built:** external brains (NN/LLM) via the
observation/order adapter; side-level mission types (eliminate, harass, raid)
as brain goal inputs; commander traits feeding the brain (the theater's full
form — the seam exists from chunk 2); the engineer/logistics units the comms
rules make meaningful.

## Open questions

Carried deliberately, none blocking chunks 1–2:

- **What a campaign day costs in minutes.** Delegation exists to keep a day
  quick, but "quick" needs a number before chunk 7 tunes anything against it
  — one battle plus campaign moves in twenty minutes is a different game from
  the same in forty-five. Wants a stake in the ground, then the resolution
  animation pace, auto-resolve options and battle count per day get designed
  against it rather than discovered.
- **When to flip the permadeath default in code.** The decision is made
  (default on); the flip waits for the wound system to have teeth — an army
  that refuses to field a wounded girl, or a muster screen that shows the
  choice — plus a pass over `resolve_crew_fate`'s first-draft numbers, since
  those probabilities were tuned for a game where death was opt-in.
- **How a ghost contact renders.** Last-known-position markers are the
  convention; the open part is age — fade, timestamp, or the reporter's name
  on hover. Art-direction question as much as UI, and it lands with chunk 7.
- **Mission granularity for the brain.** Does the first commander brain
  reassign missions every round or hold them until conditions change? Holding
  is more legible and cheaper; start there.
- **Contact pricing for the human's direct orders.** Refuse out-of-contact
  direct orders outright (chosen above for legibility), or allow them at a
  long latency? Revisit after playing chunk 7.
- **Where the command radius lives.** Per-vehicle (a radio set is hardware),
  per-mod block (chosen for now), or both with the vehicle as a multiplier.
  The radio unit will force this decision in chunk 7.
- **`leads` succession order.** Declaration order vs. best command skill.
  Start with declaration order (authorable), fall back to skill.
- **What the balance target is.** After chunk 3 the doctrines fight through
  commanders; the 24-battle matchup table is the instrument, but what "good"
  looks like (decision rate, shot volume, exit usage) needs a stake in the
  ground once the first numbers exist. Note the current elastic_defense 16-7
  dominance is *not* a defect to fix on the way: doctrines were historically
  designed around the quality and quantity of a force's equipment, its war
  objectives and its military tradition, so doctrines being unequal on a
  symmetric map is expected. A doctrine overhaul may come later; nothing in
  this plan should quietly attempt it.
