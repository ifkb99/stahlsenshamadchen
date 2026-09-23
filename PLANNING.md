# Plans, not moves

Design memo for the AI planning layer, written 2026-09-23 after a structural
review and a round of harness runs. [DIRECTION.md](DIRECTION.md) is the
design argument that came before it; this is the next one. Nothing here is
built yet. When a piece lands, its durable half moves to
[CLAUDE.md](CLAUDE.md) and its evidence to [DONE.md](DONE.md), the way every
arc before it did.

## Why

Battles are two forces driving at each other and optimising terrain once in
contact. The instruments say the same thing in numbers:

- **Decisions barely decide.** On the ridge arena, built to punish a wrong
  choice of ground, difficulty 5 against difficulty 1 is inside the seed
  spread (24–12 one seating, 17–19 the other, spread up to 11 of 36).
- **Doctrine does not change who wins.** A round robin of seven hand-built
  60-point teams gave near-identical standings under massed armour, elastic
  defence and bounding overwatch. Hardware decides.
- **Nobody manoeuvres.** 87–90% of hits struck front plate. Part of that was
  a rule — firing turned the whole hull, free, so a flank was worth one shot
  — fixed on 2026-09-23 (`VehicleDef::turret`, `balance.pivot_ticks`); front
  hits fell to 81–85%. The rest is that nothing in the AI goes looking for a
  flank.

The commander's whole strategic decision today is `SideCommand::mission_review`:
sort the objectives by value, hand each formation the next one (or Support,
or Withdraw when beaten), and let every crew pick her best tile greedily.
There is no **plan** — no relationship between formations over time. That is
why a longer planning horizon was a null result (looking further along a
greedy objective does not create coordination) and why MCTS was parked (it
searched *moves* with a weak evaluator at the leaves). What is missing is
vocabulary, not depth.

## The designer's requirements (2026-09-23)

1. **Command has three levels, and they are fluid, not designated.**
   Strategic at campaign level (the general), operational at battle level
   (the highest-ranking officer involved — possibly several, with different
   coordinating plans), tactical at unit level (a sergeant leading a
   group). Who plans at which level follows who is present, who is senior
   and who can reach whom — including when somebody dies.
2. **The map will eventually be one continuous world**, battle tiles flowing
   into each other. Not built now; nothing here may assume a small, closed,
   fully known map.
3. **Plans change.** A plan is a hypothesis, not a script.

## The shape

### A plan is a template matched onto terrain

**Terrain reading.** Cached, a pure function of the ground (like `SightGrid`
and `MoveGrid`), reflection-safe because every key is geometric:

- key terrain — objectives, crests by how much they see;
- avenues of approach — corridors between impassable or blinding ground
  (`roads` already has the Dijkstra);
- covered approaches — routes a given position cannot see into;
- firing positions — hull-down, hedgerow, long sight over an area;
- chokepoints.

**Templates, as mod data.** Roles, slot requirements, phases and triggers:

| template | roles | the idea |
| --- | --- | --- |
| fix and flank | fix, manoeuvre | fix from a firing position overlooking the enemy; manoeuvre along a covered approach that ends off his frontal arc; go when fix is in place, or on contact |
| reverse-slope defence | hold, reserve | sit in the dead ground behind a crest; engage as they come over |
| screen and reserve | screen, main body | eyes forward, body central, commit on contact |
| bounding overwatch | two elements | exists today at formation level |
| delay | rear guard | successive positions, break contact before being fixed |
| recon pull | scouts, main body | scouts find the gap, the body follows least resistance |

**Matching is the planning.** About five templates × a handful of role
assignments × the top three terrain slots per role is on the order of a
hundred candidates. Each is scored with what already exists — fire along
the route (`roads` + `incoming`), arrival time (`roads`), which face of the
enemy it arrives on (`shot_profile` prices the struck face; since the hull
fix that finally means something), objective worth — weighted by doctrine.
A hundred evaluations every few rounds, not thousands of move sequences.

**Execution already mostly exists.** A plan compiles to missions: `Support`
for a fixing element; queued `Advance` → `Assault` for a manoeuvre element
(mission queues and "a plan advances when its first leg is done" work
today). New: an advance constrained to a route (waypoints), and a phase
trigger ("go when element X is in place").

### Requirement 1 — command is who is there, not who was named

Levels are **roles a cadet holds because of where she is**, not titles
written on the map.

- **Rank.** Cadets get a rank (or a seniority order the academy keeps).
  Nothing has one today: seniority inside a formation is arrival order, and
  the side's "commander" is the leader of the first-declared formation — a
  field nothing reads.
- **Tactical.** Every formation's leader runs her formation's part of a
  plan: bounds, overwatch, where exactly to stop. The executor and the
  drill are already this level; what it gains is a phase to execute and
  latitude to adapt within it.
- **Operational.** Computed each review, not declared: take the command
  net (the walk `recompute_contact` already does), and in every connected
  component of formations the **senior cadet present** plans for that
  component. One connected army, one plan. A column cut off behind a river,
  or a second company arriving by another road, gets its own operational
  commander and its own plan — the "several officers with different
  coordinating plans" case — and when the net joins them again the senior
  absorbs the junior's formations *and her plan as a starting point*.
- **Strategic.** The campaign planner, on the overworld, with armies as its
  formations. Today `SimpleOverworldPlanner`; eventually the same template
  machinery one echelon up (an army is a formation; an overworld hex is
  ground).
- **Death, and anything else that removes a commander**, is the existing
  succession (`pass_command`) generalised: a formation's leader passes by
  seniority as it does now; an operational commander's role simply falls to
  whoever is now senior in her component at the next review, who inherits
  the plan in flight. Loss of a commander costs a review's worth of
  hesitation, which is the cost the design doc always meant it to have.
- **Coordination between operational commanders** is deliberately weak: two
  plans in two components know about each other only through what the net
  reports. That is the realism, and it is also what keeps the search small.

### Requirement 2 — built for a world that does not end at the map edge

- **Terrain reading is local and lazy.** Computed per region, cached per
  region, invalidated per region. No whole-map pass is ever on the
  decision path.
- **A commander plans inside an area of interest**: her formations, the
  objective she was given, what she knows of the enemy, and a margin. A
  plan's candidate slots come from that area only, so the cost of planning
  scales with the fight, not with the world.
- **Plans name ground by feature, not by map.** "The crest north of the
  ford" survives a camera that has scrolled onto the next tile; "hex (12, 7)
  of `river_crossing`" does not.
- **Echelons are the same machinery at different scales**, so a battle that
  spills onto the neighbouring tile is still one operational plan.

### Requirement 3 — plans change

A plan carries what it depends on, so it can tell when it has stopped being
true:

- **Assumptions**: the enemy is roughly where the plan thinks; the covered
  approach is still covered; the fixing element still exists; the
  manoeuvre element can still get there before dark.
- **Review**: at the pulse and on every interrupt (`SideCommand::interrupted`
  already fires on command passing, a formation beaten, a fresh contact),
  the current plan is re-scored *under current knowledge* alongside fresh
  candidates.
- **Hysteresis**: an alternative replaces the current plan only if it beats
  it by a margin — a commitment cost, per doctrine (a flexible doctrine
  switches readily, a stubborn one holds its course) and per commander (her
  `command` skill and traits). Without it the AI dithers; with too much of
  it, it is the script this memo is trying to get rid of.
- **Branches before switches**: a template can name its own fallback ("flank
  blocked → hold the fix and bypass"), which is cheaper than a full
  re-plan and reads as foresight.
- **Changing a plan costs what giving an order costs**: new missions travel
  the net with its latency, so a commander who changes her mind every round
  pays for it in the time her orders spend in the air. That is already
  modelled.
- **Below the plan, latitude still applies**: a subordinate adapts *how*
  she carries out her phase (initiative, the drill), never *whether* — the
  existing line between delegation and defiance.

## What the levels buy, beyond the AI

- **Difficulty gets a real axis**: a weak commander considers fewer
  candidates, reads the terrain worse (the existing *foresight* axis), and
  re-plans late. A strong one finds the flank.
- **Doctrine gets teeth**: which templates it knows, how much it likes each,
  how readily it switches.
- **The player gets the same vocabulary**: "fix with 1st, flank with 2nd, go
  on contact" as an order is also the fix for the clunky controls.
- **Cadets matter as commanders**: a command skill and a rank that decide
  who plans, how well and how stubbornly — which is on-theme for a game
  about named cadets.

## Order of work

1. ~~Hull and turret facing~~ (2026-09-23).
2. ~~**The terrain reader**~~ (2026-09-23): `tactics_core::ground`. It finds
   the ridge's crest and both spurs exactly, all 42 reverse-slope wood hexes
   as dead ground from the crest, and a covered route to the far flank that
   is 27 movement points with 8 exposed hexes against the plain route's 19
   with 17. Built for many tiles: a `Ground` trait, lazy per-region readings
   on a tiling of the whole plane, areas of interest, and a test that two
   maps side by side read as one piece of ground. Still to decide when a
   world exists: whether readings are owned per planner (today, simple, no
   save burden) or shared world-wide (cheaper once there are many planners).
3. ~~**Rank and dynamic operational command**~~ (2026-09-23): a rank ladder
   as mod data, rank on characters and cadets, succession and field-battle
   leadership by rank, and `operational_commands` deriving each group's
   senior from a new leader-to-leader command net. Behaviour-neutral: the
   base mod declares no ladder yet, and nothing plans from the commands.
   **The designer owes the content**: which ranks exist and who holds them.
   Also open: `SideCommand` still speaks for the whole side; step 4 gives
   each operational command its own planner.
4. **One template, fix and flank**, end to end, with waypoint advances and
   one phase trigger; the plan object with assumptions and hysteresis.
   Measured on the ridge with *different* doctrines on each side — a mirror
   match hides everything (the symmetric null).
5. The library, doctrine preferences as data, and the player's orders in the
   same vocabulary.

**What success looks like**: side and rear hits up; template choice varying
by map; difficulty 5 beating 1 on the ridge by more than the seed spread;
doctrine moving the round-robin standings.

## The designer's rulings (2026-09-23, second round)

- **Templates belong to cadets.** A cadet knows the plays she knows; the
  commander who plans is the one whose repertoire is searched. Doctrine
  decides which templates an academy *teaches* — it no longer chooses
  anything at battle time.
- **Doctrines need a rethink** (below is a strawman, not a ruling).
- **Cadets have explicit ranks.** Rank governs **orders, which flow
  downward**: who may command whom, who takes over when a commander falls,
  whose plan wins when two operational commanders' nets join.
- **Information flows every way.** Lower ranks can and should share what
  they see — with each other and upward — regardless of rank. Reports are
  not orders; the net carries both, and only orders care about rank.

What that means for the design:

- `CharacterDef` gains a `rank` (content), and the campaign's roster keeps
  it (promotion is then a campaign event, not a map edit). Seniority inside
  a rank is the tiebreak — battles fought, then the character's order in
  the academy's list — so succession never falls to a coin.
- **Two graphs on one net.** The contact walk already computes who can hear
  whom. *Orders* travel it only downward in rank from the operational
  commander; *reports* travel it in any direction between any two crews
  who can talk. So two components cut off from one another in command can
  still share a picture if any crew links them for reports — which is the
  case the single side-wide picture could not express, and the reason
  `Knower` will want a third answer: what *this group* has been told.
- **The operational commander of a component is its highest rank present**
  (seniority breaking ties), not the leader of the first-declared
  formation. A junior who is in contact does not command a senior who is
  cut off; the senior commands her own component and the junior hers.

## A strawman for doctrine

Today's `DoctrineDef` mixes three different kinds of thing, which may be
why it decides so little:

| today's field | what it really is | where it would live |
| --- | --- | --- |
| `aggression`, `initiative`, `withdraw_threshold`, `delegation` | a commander's temperament | the cadet: cores and traits, read when *she* plans |
| `cover_value`, `elevation_value`, `concentration`, `route_caution`, `contest_aversion`, `scouting`, `screening`, `indirect_appetite` | habits drilled into every crew | the academy's **standing operating procedures**: what a crew trained there does when nobody has told her anything |
| `objective_value` | what this battle is for | the **orders**: the mission, the operational plan |

A doctrine then becomes an academy's **curriculum**:

- **Templates taught**, each with the proficiency a graduate starts at
  (a command-type skill per template, or one `command` skill scaled by how
  much the curriculum stresses the template).
- **Standing operating procedures**: the tactical-level weights above,
  which is what makes a Kuhlmann crew and a Valkyrie crew behave
  differently when both are left alone.
- **A culture of command**: the default latitude orders are given with and
  the default commitment cost (how stubbornly its graduates hold a plan) —
  which a cadet's own traits then move.

A cadet's repertoire is what her academy taught her, plus what she has
picked up — a campaign can let a cadet who survived a flank learn it, or a
cadet from another academy bring her own plays. That is where "who
commands this battle" stops being a formality: two cadets of the same rank
from the same academy can still fight differently.

## Open questions for the designer

- **The doctrine strawman above** — right split? Is anything in today's
  `DoctrineDef` a thing the *battle* should decide rather than a person or
  an academy?
- **What ranks exist**, and does rank carry anything besides order flow
  (a bigger command radius, more formations one commander can hold)?
- **When two operational commanders' nets join mid-plan**, does the senior
  absorb immediately, or finish the current phase first?
- **Hysteresis per doctrine or per cadet** — or doctrine sets the base and
  the cadet's traits move it?
- **Is a template a thing the player can see the AI choose** (a readout of
  "they are trying to flank your left") as a difficulty-dependent tell, or
  strictly hidden?
