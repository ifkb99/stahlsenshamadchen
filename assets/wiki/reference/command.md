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
- Command passes to the lowest-id living member — placement order, which is
  the seniority the map author wrote down — announced with `CommandPassed`.
  The successor is worse at it: longer latencies, a smaller net, because the
  checks are hers now and her numbers are real. As built, that costs no extra
  mechanism at all: latency and radius were already priced on whoever the
  leader *is*.
- A scenario that wants decapitation stakes declares it: `loss_conditions` in
  map data, each naming a side, one of its formations, and whether the trigger
  is `leader_lost` (the girl who opened the battle in command is dead — dead,
  not withdrawn) or `wiped` (every member dead or gone, and at least one of
  them dead, so a clean withdrawal is not a decapitation). `check_victory`
  reads them before the score, because a battle whose command is destroyed is
  over whatever the points say. Engine machinery reads the declaration; no
  battle carries the rule unless its map wrote it down.

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
warmth — deliberately comes after it. Three boundary decisions, so nobody
re-litigates them mid-chunk: the first campaign ships **without the
requisition loop** — you fight with what you start with, and the interest
comes from the map, the missions and the commanders, not the economy; **every
girl is a placeholder** for now, character writing waits for the systems to
be worth writing into; and **campaign length is deliberately unnumbered** —
gameplay gets solidified first, then campaigns are sized around the session
time that gameplay turns out to want, not the other way round. That deferral is safe under exactly one
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

Each chunk carries a difficulty (1–5), which is really a judgment-density
rating: how much of the work is following this document versus making calls
this document cannot make for you. Low-rated chunks are well suited to a
cheaper model or a quicker pass — the determinism baseline, the additivity
tests and the harness are the safety net that makes that delegation cheap to
verify, which is the same argument the game itself makes about delegation.
High-rated chunks change what the game *is* and deserve the careful pass.

**0. One driver loop.** ✅ Done. The drive-until-commit loop that existed six
times now lives in `ai::AiDriver`; verified as a pure refactor (committed
determinism baseline passes unregenerated, playthrough and `balance --sim`
output byte-identical, battle-fight tour plays on screen).

**1. Formations as data.** ✅ Done (`4a131c7`): format fields, validation,
inert `CommandState`, save round-trip; baseline unregenerated, river_crossing
declares four formations and the stream did not move.

**2. Missions and executors.** ✅ Done (`333b0cd` plumbing, `74f5bad`
consumption): missions in the order stream, mission-aware evaluator (read
from state, so every planner sees them and none-mission paths are
bit-identical), `SideCommand` with the first brain and per-formation
executors, delegation-tax table in `balance --sim`. Judgment call recorded
in the commit: a Withdraw mission damps offensive appetites rather than
out-shouting them.

**3. Willingness moves to the commander.** ✅ Done (`79be5b6`): the brain
orders beaten formations out (never rescinded, never devolved), doctrine
colours missions (Advance vs Hold), initiative gates retargeting, delegation
read twice — as mission strictness in the evaluator and as the **directive
command** rule in the brain: delegation ≥ 0.6 assigns no ground at all,
because pinning elastic defence to anchors measurably cost it 16 of 24 wins.
The residual elastic tax (~5 in 36) is beaten formations leaving with
survivors, which a wins-only table cannot credit — the campaign is where
that trade pays.

**4. Contact and order latency.** ✅ Done (`947d049`, machinery only — the
base mod flip waited for chunk 5 so the baseline regenerated once):
`CommandRules` block, leader-anchored contact graph with relay, mission
transit priced on the leader's `command` check. The chunk's flagged conflict
— dead leaders versus additivity — was resolved in chunk 5 by the
standing-orders model below.

**5. The command picture.** ✅ Done (`6c10014`), with one model amendment
worth knowing: **a cut-off unit soldiers on the orders she was carrying**
(snapshotted at the moment the wire dies) rather than reverting to her own
judgment — the design's "falls back on standing orders", forced by
measurement when leaders proved to die on first contact on every seed. The
picture is per-side contacts-as-reported (seeing is not reporting; ghosts go
stale, never vanish; `ContactReported` names the reporter), and the base mod
now declares the block — the one deliberate baseline regeneration, and its
diff was 62 pure insertions of wire events with zero lines changed: words,
not deeds. Deferred from the original scope, both to chunk 7 where their
consumers live: the *display* reading the picture (ghost markers), and any
brain reading of enemy contacts (today's brain reads no enemy information
at all, so there was nothing to switch).

**6. Commander loss.** ✅ Done: `morale.leader_lost`, succession by seniority,
`CommandPassed`, and optional `loss_conditions` in map data. Three things are
worth carrying forward. **Succession is formation machinery, not wire
machinery** — it runs whether or not a mod declares a `command` block, because
who leads a platoon is a fact about the platoon; only the radius and the
latency belong to the radio. **The successor is worse at it for free**: every
price the chain charges is already read off the *current* leader's crew, so
promoting a weaker girl lengthens her formation's latencies and shrinks its
net with no second mechanism. And the founding leader is kept beside the
current one, because a scenario's `leader_lost` condition asks about the girl
the map named, not about whoever holds the job now. This was the one
deliberate baseline regeneration of the chunk: succession replaced the
`OutOfContact` cascade a dead leader used to cause with a `CommandPassed` and,
often, a `ContactRestored` — recon that could not report can report again.

**7. The human hybrid.** ✅ Done. `F` walks the player's formations and the
side panel becomes a formation panel (leader, standing orders, anything still
in the air, members tagged where they are out of contact); `G`/`H`/`R`/`W` on
the hovered hex issue `Order::SetMission` through the same `apply` the enemy
commander speaks through, so the log carries her orders and his in one
vocabulary. Three things are worth carrying forward:

- **Delegation fills in the *how*, never the *whether*.** Committing drives a
  new `SideCommand::executor_only` — the executors and no brain — over the
  player's side, so a unit she left unplanned inside a formation *under a
  mission* is planned by that formation's executor. A unit in no formation, or
  in one nobody has ordered, gets a bare hold-fire instead of the fallback
  planner's own judgment: filling that gap would be inventing an order she
  never gave. The rule is enforced inside `executor_only` rather than only in
  the UI, so the two cannot disagree.
- **The display reads the picture, and so does targeting.** One function,
  `shown_to`, answers where a unit is drawn and how solidly: fresh contacts
  solid, stale ones as dimmed ghosts on the hex they were last reported from
  (no health bar — a report knows where, not how hurt), unreported enemies not
  at all. `pump_events` asks it before handing a sprite to the move animator,
  or a ghost would walk the enemy's real route across the screen.
- **A refusal must not leak what the picture withheld.** Firing at a ghost is
  refused in the log, naming her and the report's age, because the marker is
  drawn and silence would read as a broken click. An enemy that a cut-off unit
  can see but nobody has *reported* is treated exactly as an empty tile — the
  click becomes a move and the ambush machinery handles it — since any message
  at all would announce her. That is the deliberate reading of "refuse a
  direct order to an out-of-contact unit with a stated reason": the reason is
  stated about the player's own crew (`set_intent` names her and says she is
  following her last orders), never about the enemy's.

**8. Campaign missions.** ✅ Done. `ArmyMission` (advance / hold / withdraw) in
the overworld order stream, a radio net in overworld hexes with relay, and a
battle that inherits what its armies were doing. Four things are worth
carrying forward:

- **Standing orders execute as delegation, through the same door.** An army
  that was not moved by hand this turn acts on its mission when the side ends
  its turn, by calling `apply_move` internally — so a mission move captures,
  triggers battles and stops short of enemies exactly as a hand-ordered one
  does, and the game layer's `BattleTriggered` handling needed no changes at
  all. An army that cannot comply today keeps its orders and tries again
  tomorrow; forgetting a standing order because of one blocked road would be
  the system deciding the player did not mean it.
- **An army is a formation.** This was the missing half of "battle
  inheritance": field battles deployed as flat pools, so there was nobody for a
  campaign order to be *given* to. `deploy` now fills the terrain map's
  declared formations, one army per formation in declaration order, first
  vehicle leading — the case `CommandState::from_placements` had already
  documented itself against. A side whose map declares no formations is the
  flat pool it always was.
- **Contact roots at the senior army, and that is a placeholder** wearing a
  sign: the first-declared living army stands in for a headquarters until the
  command unit on TODO exists, at which point `senior_army` is the only thing
  that changes. `overworld_radius: 4` is its own number rather than a scaling
  of the battle radius, because forty battle hexes to the overworld hex means
  one figure would be deaf on the map or omniscient on the field. Consequence
  worth knowing: `frontier` starts each side's second company six hexes out,
  i.e. off the net on day one — the map predates the rule, and it is a map
  question, not a code one.
- **The wire is a display secret.** The engine emits contact and mission
  events for every side; the campaign log only narrates the player's own,
  because what an enemy army has been told and whether it can still be
  reached are the operational counterpart of the command picture. Captures
  and destructions stay public — a flag changing colour is something you can
  see.

Deferred deliberately: the campaign UI issues no missions yet (the panel
*shows* orders and contact, but there is no key that gives them), and
`Advance`/`Hold` do not map onto battle missions — the battle brain already
advances on the ground the map declares worth holding, and overruling it with
a hex chosen four kilometres away would be worse than saying nothing.

**Later, enabled but not built:** external brains (NN/LLM) via the
observation/order adapter; side-level mission types (eliminate, harass, raid)
as brain goal inputs; commander traits feeding the brain (the theater's full
form — the seam exists from chunk 2); the engineer/logistics units the comms
rules make meaningful.

## Playtest follow-ups (chunk 9)

The first real play session (2026-08-10) found the wire working and found the
three things it lacks: orders die instead of waiting, the net is invisible,
and the radio is a rule rather than a thing a vehicle carries. Decisions
taken with the designer; the sub-chunks land in this order because 9a changes
the net's shape and everything after draws it or rides on it.

### 9a. The net is two media (difficulty 3/5)

Contact stops being one flat radius and becomes the union of two edge kinds,
walked by the same deterministic BFS from the leader:

- **Radio, within the chain of command.** `VehicleDef.radio: Option<u32>` —
  transmit range in hexes, hardware, the seam that damage and interception
  later attach to. A vehicle without the field uses the command block's
  `radius` exactly as today, so mods written before the field keep their
  game. The base mod declares it on every vehicle at the uniform current
  value (8) — deliberately no differentiation yet, so this pass changes
  shape, not balance; a tuning pass differentiates later. Radio hops stay
  *inside the formation* (leader to members, member relaying to member —
  dissemination up and down the squad); cross-formation coordination goes up
  to the side and back down, which today is free because the side root is
  the player or the brain, and becomes real when an HQ unit exists. Each
  hop transmits at the *transmitter's* effective range (her vehicle's radio
  plus `radius_per_signals` on her crew).
- **Visual, between any friendlies.** A new `visual_range` in the command
  block (default 3 — a flag or a hand at 100 m hexes carries much shorter
  than an eye spots a tank) links ANY two friendly units within it that
  have a clear sight line (`state.sight.clear`, the cheap cached check).
  Formation membership is irrelevant to seeing a signal flag, which is what
  keeps a small formation near its neighbours on the net without a radio
  vehicle existing yet.

The baseline regenerates deliberately (net shape moves the wire events);
read the diff — combat drift beyond what contact changes explain is a bug.
Campaign contact keeps its current senior-army radio model; 4 km hexes have
no visual signalling to speak of.

*Future, recorded not built:* radios as damageable components (waits on the
ballistics/component rework); transmissions as detectable events — an enemy
with the `signals` skill learning that *somebody* transmitted nearby, then
decoding with time — is the electronic-warfare layer, and it falls out of
transmissions being events once somebody wants it.

### 9b. Orders wait instead of dying (difficulty 3/5)

Deliver-on-contact, both layers. A direct order to an out-of-contact unit is
*accepted* and waits at the radio: the battle stores the intent (the
destination, never the computed path — she re-paths from wherever she is
when it reaches her) and delivers it at the first planning phase she is in
contact for, announced with its own event. `ClearIntent` clears a waiting
order without needing contact — not sending is free. On the campaign, a
mission to an out-of-range army queues at the senior army and transmits at
a turn start that finds the army in range. The formation panel and army
panel both show "orders waiting", because an order silently parked is as
illegible as one silently dropped.

Delivery is at the planning phase, not mid-round: WEGO's bargain is that
resolution plays out what was planned, and a queued order landing mid-tick
belongs to the reaction-to-new-information machinery that arrives with
traits, not here.

### 9c. Mission sequences (difficulty 4/5)

"Advance to the ford, then hold it." `Formation` gains a plan — a queue of
missions behind the standing one. `Order::SetMission` replaces the whole
plan (current behaviour, unchanged); new `Order::QueueMission` appends. The
plan is transmitted once and executed locally by the formation: promotion
from one mission to the next needs no wire, because the leader has known
the whole plan since it arrived — which is the Auftragstaktik shape and
also the cheap one. Promotion happens in the sim at end of round when the
standing mission *completes*:

- `Advance { to }` completes when a member stands within 1 hex of `to`.
- `Recon { toward }` completes when the formation has eyes on it — the
  target tile visible to a member.
- `Hold` and `Withdraw` are terminal: nothing follows a stand-fast or a
  retreat, and queueing behind one is refused so the impossibility is said
  rather than silent.

Completion and promotion are announced (the log hears "1st Platoon reaches
its objective; moving to the next order"). Executors keep reading only the
standing mission — sequencing is entirely the formation's bookkeeping. UI:
holding Shift with G/H/R queues instead of replacing; the panel lists the
plan in order.

### 9d. Seeing the net (difficulty 2/5)

- Leaders get a marker on their sprite (team-coloured chevron), everywhere,
  always — who is in charge is not privileged information about your own
  side, and on the enemy it only shows for units the picture shows anyway.
- Selecting a formation draws the net: a range ring at the leader's
  effective radio range, members tinted by contact state, and — if it reads
  well in the screenshot loop, not otherwise — link lines showing who hears
  whom and by which medium.
- The panel names the numbers: "Net: radio 8 (+1, Elsa's signals), visual
  3" and, per member, in contact / out / orders waiting.
- Campaign mirror: a marker on the senior army, a radio-range ring on
  selection, and the existing out-of-contact tag kept.

## Open questions

Carried deliberately, none blocking chunks 1–2:

- **What a campaign day costs in minutes.** Deliberately left unnumbered for
  now: gameplay gets solidified first and campaigns are sized to fit after.
  Delegation still exists to keep a day quick — the delegation-tax metric is
  how "quick without being worse" stays honest while the target floats.
- **When to flip the permadeath default in code.** The decision is made
  (default on); the flip waits for the wound system to have teeth — an army
  that refuses to field a wounded girl, or a muster screen that shows the
  choice — plus a pass over `resolve_crew_fate`'s first-draft numbers, since
  those probabilities were tuned for a game where death was opt-in.
- ~~**How a ghost contact renders.**~~ Settled in chunk 7, and cheaply: the
  unit's own sprite at the last reported hex, dimmed to 45% and stripped of
  its health bar, with the reporter and the report's age in the panel on
  hover. Reusing the sprite rather than spawning a marker set is what keeps
  "the picture" a display rule instead of a parallel world; age is words
  rather than a fade because the log already speaks in rounds. An explicit
  staleness *fade* is still available later if ghosts need to age visibly.
- **Mission granularity for the brain.** Does the first commander brain
  reassign missions every round or hold them until conditions change? Holding
  is more legible and cheaper; start there.
- **Contact pricing for the human's direct orders.** Refuse out-of-contact
  direct orders outright (chosen above for legibility), or allow them at a
  long latency? Revisit after playing chunk 7.
- **Where the command radius lives.** Per-vehicle (a radio set is hardware),
  per-mod block (chosen for now), or both with the vehicle as a multiplier.
  The radio unit will force this decision in chunk 7.
- ~~**`leads` succession order.**~~ Settled in chunk 6: declaration order,
  full stop. It is authorable, it cannot depend on a hash, and "the senior
  girl takes over" is a rule a player can predict — which the skill-based
  fallback, chosen by numbers she cannot see, would not be. A scenario that
  wants a different heir writes her formation down in a different order.
- **What the balance target is.** After chunk 3 the doctrines fight through
  commanders; the 24-battle matchup table is the instrument, but what "good"
  looks like (decision rate, shot volume, exit usage) needs a stake in the
  ground once the first numbers exist. Note the current elastic_defense 16-7
  dominance is *not* a defect to fix on the way: doctrines were historically
  designed around the quality and quantity of a force's equipment, its war
  objectives and its military tradition, so doctrines being unequal on a
  symmetric map is expected. A doctrine overhaul may come later; nothing in
  this plan should quietly attempt it.
