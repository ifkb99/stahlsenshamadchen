# Reimagined: the layer of control

The design memo for rebuilding how cadets decide, and the live state of the
work it starts. Written 2026-10-14. [DIRECTION.md](DIRECTION.md) and
[PLANNING.md](PLANNING.md) are the arguments that came before it, and both
reached the same complaint from different ends; [WORLD.md](WORLD.md) is the
world this one fights on, and its clock stays. When a piece lands, its
durable half moves to [CLAUDE.md](CLAUDE.md) and its evidence to
[DONE.md](DONE.md), the way every arc before it did.

## The brief

From the designer, in full:

> **Extreme focus on the layer of control.** My main gripe with the game is
> that currently, the units and armies do not feel very alive, but more
> importantly they do not function well; they throw themselves at each
> other. I aim to fix this through starting at small company or even
> platoon level with the player as the leader, ironing out interactions for
> how units give and follow orders with reinforcement learning. Each
> individual cadet (or squad depending on hardware constraints) will have a
> head on their network to give their actions character, along with their
> stats.
>
> The conceit of this game is that you build a relationship with your
> fellow cadets, and they autonomously fight with you further building the
> relationship. There will be a split between game and visual novel — for
> now we focus on the game. The VN will mostly be flavouring and a way to
> recap battles.
>
> **Layers.** Most militaries already have a chain of command and order of
> battle we can structure ourselves around. The player should be able to
> insert themselves into a role, and perhaps be promoted upwards. Once
> orders are given, units act intelligently and autonomously to carry out
> said orders, giving and receiving updates along their communication
> network, potentially modifying said orders.
>
> **MVP.** A single operational-sized campaign with two companies facing
> off in open battle. The map should be auto-generated so we can more
> easily train the units.

The complaint is the one the instruments have been printing for two
months. DIRECTION.md: orders never work out the way they were meant.
PLANNING.md: two forces driving at each other, decisions that barely
decide, 81–85% of hits on front plate. The 2026-09-27 playtest: an advance
pinned in round six and lost 60–0 without firing at anything it could see.
Charging and freezing are the two ends of one missing thing, which is
coordination, and the hand-written AI has not found it.

## Decisions (2026-10-14)

1. **Learning sits at the goal level.** A learned policy replaces
   `GoalChooser` (`ai/goal.rs`) and nothing else. The executor keeps
   pathing, boarding, opportunity fire, the drill and defiance. The seam was
   built for exactly this — the module doc names it in the options
   framework's terms — and it buys **legibility**: every action is a named
   goal, so a cadet can say "moving to the treeline to cover you", and a VN
   about trusting the people beside you needs them to be readable. Accepted
   knowing that a new kind of goal means retraining (*Living with
   retraining*, below).
2. **Personality is traits as input and personality as reward.** One
   network, conditioned on the cadet's trait vector and trained across
   randomised traits, rather than a head per cadet: forty-nine heads would
   each need their own data, and heads sharing one reward converge on one
   behaviour. What she *cares about* — her own skin, her friends, the
   objective, glory — is a set of reward weights the network is trained
   across, so a brave cadet and a careful one are two settings of the same
   dial, and a cadet whose reward includes her friends' safety learns to
   cover them. **The test is whether you could name the cadet from watching
   her fight.**
3. **Functioning well is staying alive and accomplishing the goal**, defined
   per layer of the chain of command. The mission comes first; injury and
   death are a real penalty but a secondary one.
4. **The player's decision is her place in the chain of command.** Giving
   orders down and following orders from above *is* the loop, and what she
   decides changes as she is promoted.
5. **There is a mechanical relationship variable**: trust, below.
6. **Bit-exact determinism is no longer an invariant.** Reworking or
   retiring the tests that defend it is accepted. What is kept is seeded
   reproducibility on one build, which costs almost nothing and is needed
   for three things beyond the tests: a training run that fails has to be
   re-run to be debugged, a save has to restore the same future, and the VN
   recaps battles, which wants replays.
7. **The world clock stays.** Other campaign machinery — the drawn
   campaign, the campaign planner's headquarters rule, campaign endings
   beyond the one the MVP needs — can be left alone while this is built.

## What "functions well" means, layer by layer

Each layer's reward is the outcome of the order it was given, plus the
survival of the people under it, weighted by who she is.

- **A cadet** is rewarded for progress on her assigned goal and for her own
  survival, with personality weights on top (her friends' survival, the
  objective for its own sake, a kill).
- **A platoon or company commander** is rewarded for the mission she was
  given and for the survival of everybody under her.
- **Every verb in the order vocabulary needs a measurable "done"**, and the
  `Mission` vocabulary (`battle/command.rs`, `VOCABULARY`) is the checklist:
  *hold* is time held, *take* is reached and held, *recon* is contacts
  **reported** rather than merely seen, *support* is fire delivered for the
  element supported, *withdraw* is arriving with what she left with.

**The mission vastly outweighs everything else, and it weighs more the
higher in the chain of command it belongs** (the designer's ruling,
2026-10-14): war > strategic objective > operational objective > tactical
objective, with losses and survival below them all. `training/ppo.py` writes
it as tiers an order of magnitude apart — `war` 1000, `strategic` 100,
`operational` 10 (the battle's outcome, ±1, and its objective margin as a
share of the score that wins it outright), `tactical` 1 (her order carried
out), `attrition` 0.1, `self` 0.05. A battle-level rollout reaches only the
operational tier and below; the higher tiers are there so a campaign trainer
adds terms rather than a new scale. The first runs weighted the objective at
0.05 a point and the mission did not come first — two learned sides then
drew every battle without firing, each waiting for the other.

Two rules about the reward that are easy to break:

- **The survival weight is the dial between the playtest's two
  pathologies.** Too high and a force is pinned and passive; too low and it
  throws itself at the enemy. That is why personality-as-reward fits: the
  dial is a person.
- **A subordinate is not charged for her commander's plan.** A cadet
  punished every time her order was impossible learns to ignore orders.
  Grade her on how well she executed against how hard the order was; the
  commander answers for the order.

And one rule about measuring it: the reward is what training optimises, not
what tells us it worked. The instruments do that — the share of hits on
front plate (flanking moves it), difficulty 5 against 1 on `ridge_arena`,
`examples/mirror`, a learned side against the utility side seed-swept, and
whether a pinned advance recovers. Each milestone below names its bar.

## Personality, trust, and learning across battles

**Trust** is one more input in the trait vector, held per pair: what this
cadet thinks of this commander. Orders that got her hurt lower it; missions
that went well raise it. It decides how much latitude she takes when an
order looks bad, how readily she obeys a binding order, and how fast she
rallies near the one who gave it. It is the single number that joins the
battle to the VN.

**A cadet learns across battles without the network learning at all.**
Gradient updates during a campaign are the wrong tool: a network needs
thousands of episodes to learn anything and a campaign has twenty battles,
and learning that fast online makes behaviour erratic or forgets what it
knew. Three mechanisms instead, in order of when they are built:

1. **Experience moves her traits.** The network already takes any trait
   vector, so a battle shifts hers: caution up after she was hit, trust up
   when the orders brought her home, aggression up after a win. No
   retraining, the change is readable, and it is what the VN says out loud
   — *she has been careful since the ridge*. This is the RPG's levelling
   model with a learned policy reading the levels.
2. **She learns a play.** PLANNING.md already holds the slot: a cadet's
   repertoire is what her academy taught plus what she picked up
   (`CharacterDef::templates`, carried on the `Cadet`), and "a campaign can
   let a cadet who survived a flank learn it". Gaining a play is a discrete
   thing she learned and can be told about, and a commander who knows more
   plays has more to choose between.
3. **Later: experience as input.** A short record of what has happened to
   her — flanked twice by tank destroyers, lost a friend at a river — fed to
   the network, which is trained across randomised histories and so learns
   what a cadet with that history does. Her past then changes her in a
   learned way rather than a written one. Built only once 1 and 2 have been
   played.

In-context learning (a recurrent policy that adapts within play) is
research-grade and is not on this plan.

## Where the learning sits

- **The policy scores candidates; it does not pick from a menu.** It is
  given each candidate goal as a description — where it is, how exposed the
  road there is, what she could shoot from it, who could shoot her, which
  order verb it serves — and scores each, the way `score_tile` does now. A
  new *kind* of goal is a new kind of description, so it is a fine-tune of
  the network rather than a new one.
- **The world is described as a list of things, not a fixed grid.** Units,
  contacts and ground each arrive as feature vectors, so a new chassis or a
  new terrain is new data and not new architecture. The trait vector keeps
  spare slots for traits not yet invented.
- **The candidate list has to grow**, and that is the first real design work.
  Today `candidates` offers the objectives' hexes, the ground her orders
  name, the tile this round's sweep picked, and holding — which is why the
  chooser cannot flank: no flank is on the list. `tactics_core::ground`
  already reads vantages, dead ground, firing positions and covered routes
  for a planning layer that never arrived; it is the obvious supplier, with
  positions relative to the known enemy (his flank, his rear, a hull-down
  spot on his approach) and relative to friends (overwatch of the element
  moving, closing on the leader).
- **The utility AI is kept, three times over**: as the **teacher** the
  network first imitates, as the **sparring partner** it is measured
  against, and as the **fallback** when no network is loaded. A mod with no
  network plays the game as it is today, which is the additivity rule this
  project has kept from the start.
- **Inference in the shipped game is on the CPU, in plain Rust.** A scoring
  network for a handful of candidates is a small multilayer perceptron, a
  few dozen lines of Rust with weights read from a file — no dependency in
  `tactics_core`, microseconds a decision, and the GPU stays free to draw.
  Training runs in Python and JAX, on the GPU.

## Living with retraining

Every change to what a cadet can do or see will want the networks retrained.
That is accepted, and made cheap:

- **Scoring candidates** (above) means a new kind of goal is a fine-tune.
- **Warm starts.** A new network starts from the old one's weights, plus a
  short period imitating the old one on what did not change.
- **Training is a build step.** A scripted job, a network versioned beside
  the mod content it was trained on, and features batched so that several
  land before each retrain. Retraining then costs compute and wall clock,
  not anybody's attention.
- **The teacher is always there.** Imitating the utility AI takes a network
  from random to competent in hours, so no run starts from nothing.

## The machine

Two RTX A4500s (20 GB each), an i9-14900K with 32 threads, 46 GB of memory.

- **The simulation stays on the CPU.** It is branchy, per-unit logic, and
  the 32 threads run battles in parallel; round resolution is about 2 ms on
  `river_crossing`. Moving the simulation itself onto the GPU means
  rewriting the engine as array programs (the JAX-style vectorised
  environment) — that is the real rebuild, and it is not worth it.
- **The GPU does all of the learning**: imitation, the gradient updates of
  reinforcement learning, and the batched inference during training, when
  the decisions from many parallel battles are gathered into one batch.
  Goal decisions are rare — one when a goal finishes, not one a tick — so
  **the CPU simulation will be the bottleneck and the GPUs will be
  under-used.** Two of them means one learner and one evaluating, or two
  experiments at once.
- **The CPU is unstable under sustained parallel load** on its current
  microcode (CLAUDE.md, the tactics-dev skill: safe Rust segfaults, one
  physical core takes most of the crashes). Training is hours of exactly
  that load. Update the microcode before the first long run, or pin the
  workers off CPUs 10 and 11.

## Curriculum

The designer proposed: a commander over cadets who always listen, then
cadets learning to follow orders, then the commander again over the live
cadets. The plan adopted turns the first two round:

1. **Imitate the utility AI** — the warm start.
2. **Cadets first, on a mix of orders**: scripted, random, and deliberately
   bad. The bad ones teach when following blindly gets her killed, which is
   exactly where personality and trust should decide. Traits are randomised
   across training.
3. **A commander over frozen live cadets.**
4. **Alternate 2 and 3** a few times, keeping a pool of past versions as
   opponents so that nobody over-fits to the current one.

Why cadets first: a commander trained over obedient cadets learns plans
that only work with obedient cadets, and is retrained anyway; what a
commander can order depends on what cadets can actually do, so the bottom
layer is learned first; and **it is playable sooner** — after step 2 the
player is the commander, ordering learned cadets on the world clock, with
the enemy under the existing `SideCommand`. That is the core loop, in hand,
before any learned commander exists. The always-obedient cadet the designer
described already exists — the utility executor under a binding order — and
is the teacher in step 1.

## MVP

Two companies in open battle on a generated map, on the world clock, with
the player as one company's commander giving orders to learned cadets.
Platoon against platoon is the training ground before that.

## Tactics emerge; they are not named (2026-10-14)

The designer's ruling: the networks should learn tactics without the
tactics having names, so they can develop and adapt freely. Two places a
name would creep in, and both are kept out:

- **The candidate list is places, not tactics.** No candidate is a "flank"
  or an "overwatch position"; `planner.goal_places` offers spread-out hexes
  she can reach, each the best place to stand in its patch, unlabelled.
- **The features are facts, not flags.** What makes a place a flank is the
  aspect it shows the enemy's hull, who can see it, and which friend is
  shooting the same man from where — and those are what the network sees,
  as distances, counts and cosines of angles between differences (never a
  signed angle, which a reflection turns round; the twin test enforces it).

Coordination needs three more things, built in later stages: she sees her
friends' intentions (their goals are in the features already), the reward is
partly shared (L1.4), and eventually the network relates candidates,
friends and enemies to each other directly (a small attention model over the
sets, once the hand-made summaries are learned from). Names exist only for
the player and the instruments: "going round their left" is read off the
geometry of a choice after it is made, for the radio and the VN, and the
same post-hoc labels are what measure whether tactics emerged — the share of
hits on side and rear plate, the crossfire rate, a halted friend covering a
moving one. **If a tactic never emerges, ask whether the rules reward it**
before asking anything of the network.

## A model of the enemy

The designer's question (2026-10-14): combatants usually have some idea of
how their enemy is equipped and how he operates; do the networks?

**Today, only of the enemies she can see, and only of their hardware.**
Every feature about the enemy comes from `known_enemies` with her own
knowledge — fog-honest — and the arithmetic behind `incoming` and `worth`
reads each known enemy's real guns and armour, so she knows exactly what a
*seen* tank can do to her. She knows nothing of the enemy she has not seen,
nothing of where a lost contact has gone, and nothing of how he fights.

Three layers would give her one, in order of cost:

1. **What he fields.** An intelligence prior: the order of battle she was
   briefed on (what the enemy academy fields, how many), less what she has
   seen. Features: expected unseen strength by kind of threat (armour,
   anti-tank, artillery, infantry) and a per-candidate "fire from the
   unseen" estimate over ground nobody has looked at. In a campaign the prior
   is earned — scouting and earlier battles fill it in — which makes
   reconnaissance worth something to a learned chooser for the first time.
2. **Where he has been.** Lost contacts kept with their age (the picture
   already holds contacts; the feature is how stale each one is), so "he
   was in that wood two rounds ago" is something she can act on.
3. **How he fights.** Two halves. *Implicitly*, a policy trained in a league
   of opponents — every shipped doctrine, past versions of itself — learns
   what enemies tend to do, and conditioned on which academy she faces
   (an embedding of the enemy's doctrine, as her own traits are) she can
   learn that Kuhlmann's crews hold and the Valkyries' push. *Explicitly*,
   an auxiliary head that predicts where each known enemy will be in a few
   rounds, trained supervised from the rollouts' own futures (the labels are
   free). It sharpens what the network learns and it is legible: "she
   expects them at the ford" is a readout, not a guess.

And it joins learning across battles: a cadet who has fought the Valkyries
before can carry what she learned of them in her experience record, so
knowing your enemy is something a campaign teaches.

## An OODA skeleton with learned stages (ruled 2026-10-14)

A known structure rather than everything learned end to end. **Observe**
stays the engine's (the fog, detection, the net, `known_enemies`) and so
does **Act** (the executor: pathing, fire, the drill); **Orient** (the
features, the ground reading, later the enemy model and her experience) and
**Decide** (the goal chooser) are learned — which is the shape the code
already has. Why: the fog stays honest by construction, because observe is
the only door to the state; every stage has a readout, which is how a cadet
explains herself and how we debug her (the twin test found its compasses
because the stages are apart); it constrains *where* decisions sit and not
*which* tactics emerge; and Boyd's two real ideas land on things being
built — orientation is where culture and experience shape perception (her
traits, doctrine, experience record), and tempo is the decision rate. The
cost is that the boundaries are hand-placed; if a policy ever needs to
direct its own attention, observe gains a learned *where to look* input
rather than being replaced.

## Decision tempo, from the record (2026-10-14)

How often decisions are made against execution, researched for the
designer. The answer: **decisions are event-driven, rare against execution,
and each echelon decides on a slower clock than the one below it.**

- **Each echelon hands most of its time down.** The one-third/two-thirds
  rule (FM 3-20.15): a platoon leader takes about a third of the available
  time to plan and issue the order and leaves two-thirds to her vehicle
  commanders; modern standing procedures push it to a fifth.
- **At the bottom a decision is a choice of drill.** Battle drills exist for
  situations "requiring instantaneous response" and are executed by
  instinct (FM 7-8); after reacting to contact the squad leader decides
  between flanking, breaking contact and a base of fire — a short typed
  choice an event triggers, which is the goal layer's shape.
- **Most decisions are recognition, and the errors are of orientation.**
  Klein's group (ARI RN 90-51, armoured platoon leader exercises) probed 57
  decision points: options deliberated in about half, analogues used in
  about half, and errors came less from missing cues than from
  *misinterpreting their importance* — orient is where commanders fail.
- **Units fail at execution more than decision.** RAND's NTC study (MR846):
  "companies plan better than they execute" and "maneuver better than they
  control direct fires".
- **Soviet doctrine pre-computed decisions** (norms and nomograms, so a
  commander did not re-solve recurring problems under fire) where mission
  command pushes them down — a ready-made academy axis: how often her crews
  decide for themselves.
- **Decisive fights are short**: 73 Easting, 23 minutes from first contact
  to the halt; a ridge battle here is about seven rounds of a minute.

So the clocks, roughly (the seconds-to-hours bands are general knowledge,
not a figure from these sources): crew drill and gunnery in seconds (the
executor, per tick); a crew's manoeuvre in a minute or two, *on events*
(the goal layer); a platoon leader in minutes; a company commander in tens of
minutes (plans, playouts); battalion planning in hours (the campaign day).
What it changed here: a cadet used to re-decide only when her goal
finished, about twice a battle. **`planner.goal_events`** ends a goal on
the events crews re-decide on — contact, a hit, a friend lost, being pinned,
new orders — each a difference from a `GoalWitness` kept with the goal, and
puts the interrupted goal back on her list so thinking again can mean
carrying on. It is L2.5's first, hand-written half. Off by default.

**On constructed scenarios**: as a teacher to distil, little — imitation
anchors a network to its teacher's ceiling, as measured; written TTPs
(bounding overwatch, fix-and-flank) earn a place only as demonstrations to
seed exploration toward coordination that random search finds rarely, and
only if it never emerges. As the training distribution, essential and
*generated* (L1.1). As an **exam suite**, worth building from the history: a
covering-force delay, a meeting engagement, a hasty river crossing, a
defence from battle positions, an ambush at a defile — vignettes where we
know what good looks like and score whether that kind of behaviour emerged,
without ever naming it to the network. L2.6.

## Where to spend thinking (2026-10-14)

Distilled from a look at "semantic if" (TypeSafe's Jev and the open SemIf):
a cheap, typed decision whose job is to say whether something expensive is
worth doing. The language-model half of it is ruled out — a 4B model will
not run on every player's machine, so story features do not lean on one —
but the pattern is already the shape of the goal layer (options declared at
runtime, scored in one pass, no generation), and its other half is the
question of **when to think at all**: rational metareasoning, the value of a
computation. Two places it lands:

- **When a cadet re-decides.** Today a goal ends by a fixed list
  (`Goal::finished`). A learned *termination* — the options framework's
  other half, learned with a small **deliberation cost** per re-decision
  (Harb et al., "When Waiting Is Not an Option") — lets her reconsider when
  the world has actually changed (a new contact, a friend down) rather than
  on fixed triggers, and tells us where the decisions are worth their cost.
  L2.5.
- **When the commander plays out.** The plan review plays out every
  nomination, 4 samples of 5 real rounds, on every pulse and interrupt. A
  gate that plays out only when the model's top options are close (their
  margin under what a playout could plausibly change) spends the cycles
  where the choice is in doubt. **Deferred until a shipped doctrine teaches a
  play**: today no shipped battle plans, so a gate would save nothing a
  player meets; `examples/plans` is its instrument when it does.

## Not doing, now

- The visual novel, beyond what a battle needs to record for a recap.
- A language model anywhere a player's machine has to run it (story,
  dialogue, order parsing): not every device can.
- The drawn campaign (`frontier`), the campaign planner and its
  headquarters rule; they keep working and are not developed.
- Cross-machine bit-exact determinism.
- Moving the simulation onto the GPU.
- A head per cadet.
- Online gradient learning inside a campaign, and in-context learning.
- A change of grid. Hexes are not why units throw themselves at each
  other, and they suit learning (a small discrete action space, cheap
  observations, all of `ground`). What may change is their **size**: at
  platoon scale 100 m is coarse, the designer has already ruled that the
  content's resolution should rise, and `hex_meters` is data. Decided after
  L2, if the policy turns out to be limited by the grid.

## Infantry: composition and grain (proposed 2026-10-14)

The designer's question: is the weight of infantry against vehicles right
for the early Cold War? On the record, no, twice over.

- **Composition.** A 1969 CIA estimate gives a Soviet motor rifle division
  ~186 tanks and ~200 APCs (about three tanks per rifle platoon) and a tank
  division ~314 tanks and ~80 APCs (about twelve); a US ROAD armored
  division ran six tank battalions to five mechanized (about seven per
  platoon, from general knowledge, not verified here). Fights were fought
  by company teams: a US tank company with a mech platoon attached or the
  reverse, a Soviet motor rifle company (nine squads in nine BTRs) with a
  tank platoon of three. The shipped battle maps field ~7 armoured vehicles
  a side to one rifle platoon and one scout section.
- **Grain, the bigger one.** A `rifle_platoon` is one unit — thirty
  soldiers, one hex, one carrier — beside tanks that are one unit each. A
  rifle platoon was three or four squads in three or four carriers, the
  same grain as a tank platoon's three to five tanks. So a platoon decides
  once to a tank platoon's five, one carrier hit takes the whole platoon,
  and a learner barely meets infantry at all.
- **Price** (5 points to a medium tank's 16) should be set by the `mustered`
  instrument once the grain is right, not imported: the period's own firepower
  indices (WEI/WUV) were subjective and abandoned.

**Ruled 2026-10-14: adopted**, and with it a principle — **doctrine comes
from what a side has** (its training, its equipment, its manpower), and the
networks must not overfit to one method. Built:

- `rifle_squad`: about ten (troops size 8, toughness 2), one cadet seat, a
  squad's rifles (`squad_rifles`: the platoon's round at a third of its rate,
  so three squads are the platoon's fire and fear shot for shot) and its own
  RPG with two rounds. **Not for sale (`cost` 0)**: squads come in forces,
  aboard their carriers, until the army builder (`force::muster`) knows a
  mechanised platoon — at 2 points it bought squads as pocket change and
  elastic defence stopped buying its howitzer, because appetite is counted
  per chassis and the squad split infantry's.
- **Forces are data** (`forces/` in a mod, `ForceDef`, validated): a force
  names its doctrine beside its units. The base mod ships five company teams
  — a Western team (three tanks, three squads in three APCs, bounding
  overwatch), a Soviet motor rifle company (six squads in six open carriers,
  two tanks, massed armour), a Soviet tank company (four tanks, two squads),
  a Western armoured cavalry troop (recon pull) and a covering force
  (delaying action).
- `harness::forces::stage` puts two forces on any battlefield;
  `rollout --forces all` draws a matchup a world, both seatings, each side
  fighting by its own force's doctrine, and reports the learner's record
  *per force* so overfitting to one method shows as one force carrying the
  average. The network sees its doctrine (already) and both sides'
  composition (`own_armour`, `own_infantry`, `own_carriers`, `known_armour`,
  `known_infantry`).

Still to do: the shipped battle maps and the campaign's armies keep their
platoons — moving them to squads needs seats the academy roll does not have
(three squads and three carriers are nine cadets where a platoon and a
halftrack were four), which is a roster decision; and repricing through the
`mustered` instrument once the builder can buy a mechanised platoon whole.

## Open questions

- **"Operational-sized campaign with two companies"**: one battle, or
  several days of two companies fighting? The MVP above reads it as one
  campaign of days on the world clock.
- **What the player does at the bottom of the chain.** As a platoon leader
  she is a cadet with a vehicle of her own; does she drive it, or does her
  crew?
- **Promotion**: what earns it, and whether the player can decline it.
- **Who learns trust from whom** when the commander is an AI cadet rather
  than the player.

## Checklist

**L0 — the plumbing, behaviour-neutral.** Nothing the utility AI does
changes; the determinism snapshot passes unregenerated throughout.

- [x] L0.1 Branch `feat/reimagined`, and this memo.
- [x] L0.2 Goal features: a fixed description of each candidate goal and
      of the chooser's context (her traits, her order, her state), pure Rust
      in `tactics_core::ai::features`. The utility chooser's own reading of
      each candidate (worth, road, exposure, who gets there first, the
      deviation charge) is part of the description, so the first network
      can reach the teacher by reading its arithmetic.
- [x] L0.3 The imitation set: `examples/goal_dataset` fights battles under
      the utility AI and records every goal decision — context, candidates,
      the one chosen — as JSON lines.
      *Landed 2026-10-14.* `UtilityChooser::choose` split into
      `goal::read_candidates` (the facts) and the judgment, behaviour-neutral
      (snapshot unregenerated); `ai::features::describe` turns a decision into
      a context row (28 fixed + one flag per order verb) and a 21-wide row per
      candidate; `UtilityPlanner::tapped` / `SideCommand::tapped` record
      decisions without touching the rng. `tests/learning.rs`: a recorded
      battle is the battle unrecorded; every record is its schema's width; a
      crew and her mirror twin on `battle_hills` are described identically
      (mutation-checked with a coordinate slipped into a feature). 32 battles
      record 1,833 decisions in 0.3 s, **4.2 candidates each** — the list is
      short, which is L1.2's problem. Baselines to beat: "always the first
      candidate" agrees with the teacher 69%, "the highest `worth`" 91%; at
      difficulty 5 the teacher is a fixed function of features all in the
      record, so imitation should approach 100%.
- [x] L0.4 `training/`, a uv project on **JAX** (the designer's preference;
      jax 0.11.2 with the CUDA 13 plugin, optax 0.2.8, numpy 2.5.4, Python
      3.14): `imitate.py` trains a candidate scorer (an MLP over
      `concat(context, candidate)`, 64-64 ReLU, softmax over the list) and
      writes it as JSON. *Landed 2026-10-14*: 1,024 battles, 60,802
      decisions, a tenth of the battles held out at random — **98.7%
      held-out agreement** (always-first 65%, highest-worth 91%) after 7.7 s
      on one A4500. The first split held out `game % 10`, which the dataset
      maps to one battlefield under one planner; it read 97.1% and was wrong.
- [x] L0.5 The network is content: `data::GoalNet` (layers, standardisation,
      the feature names it was trained on, a probe), `DataRegistry::goal_net`,
      and `ai::learned::LearnedChooser`; a plain forward pass, no dependency.
      `ai::learned::GoalBrain` says who chooses a crew's goals — `Utility`,
      `Learned(net)`, or `Random` — and the planner names `learned`,
      `learned_command` and `random_goals` reach every instrument; a `learned`
      planner with no network is the utility planner, battle for battle.
      **Loading checks the names and the arithmetic**: a network trained on
      other features is refused, and so is one whose probe the engine scores
      differently. The probe caught a real fault on its first run — JAX's
      default matmul precision on an Ampere card is TF32, a part in a
      thousand off, so the trainer computes the probe in NumPy float32.
      `balance --brains --brain-vs <planner> --net <path>` measures it, and
      `--seed` now shifts the brains table like every other.
- [x] L0.6 Bar met: on `ridge_arena`, difficulty 5, 36 battles a seating,
      seeds 0/1000/2000/3000, the learned chooser wins **145 of 288 (50.3%)**
      against the utility chooser it imitates.

**What L0 found.** `random_goals` — a coin choosing among the same
candidates — loses to the utility chooser **115–173 (40%)** on the same
ridge sample, about 3.4 standard deviations: the goal decision is worth
some ten points of win rate. So the layer has headroom and reinforcement
learning has a signal, but it is modest, and two things cap it: the
executor carries much of a battle on its own (its sweep tile is always a
candidate, and the shot is not the chooser's), and a ridge battle is over in
about seven rounds with 4.2 candidates a decision. Widening the candidates
(L1.2) and longer platoon fights (L1.1) are what make the decision matter
more, and `random_goals` against `utility` is the instrument that says
whether they did.

**L1 — the training ground.**

- [~] L1.1 **Generated ground** (`harness::generated`): a seed is a small
      world and the battlefield its centre chunk — one battle map of relief,
      woods, hedges, streams, rivers and villages — with an arena's forces and
      deployment moved onto passable ground and the objectives read off it
      (the highest ground near the centre worth 3, the highest on each flank
      worth 2). Two games a world, one per seat. `rollout --ground generated`
      and `--deterministic`; `loop.py --ground generated` evaluates on worlds
      training never reaches. Generated battles run 7.6 rounds (3–16), none
      drawn, 70 of 256 won by the score. Platoon-scale forces are still to
      come (see *Infantry*, below).

  *What it said (2026-10-14, every event on, mission-first reward):*
  - **The ridge policy does not transfer**: the 58% mission-first network
    goes 124–132 (48%) on generated ground. It had learned the ridge.
  - **Trained on generated ground, it reaches parity and stops**: from
    scratch, 12.5% to ~50% within ten iterations and 47.6–52.8% thereafter;
    continued from the ridge policy, 45–54%. Every reading is inside ±8.5
    of 144 at 288 battles. It learns to play unseen ground as well as the
    hand-written chooser does, quickly, and not better in 40 iterations.
  - Candidates for the gap, cheapest first: a longer run with bigger batches
    (ground variety is variance, and the critic explains ~0.28 of returns);
    a wider network (64-64 was sized for one arena); features about the
    ground around her and not only about each candidate; and the forces
    themselves, which are four tanks a side.
- [x] L1.2 **Candidates are places; features are relational facts** (the
      designer's ruling below). `planner.goal_places` (0 shipped, so the game
      and the snapshot are unchanged) offers that many unlabelled places from
      her reachable roads; `ai::features` gained nine relational facts
      (`enemy_distance`, `closing`, `seen_by`, `targets`, `aspect_min`,
      `aspect_nearest`, `crossfire`, `friends_heading_near`,
      `friend_distance`). *Landed 2026-10-14*, at 16 places on the ridge,
      difficulty 5, four seeds × 72:
      - **21.3 candidates a decision**, against 4.2.
      - **Utility beats a coin 218–70 (75.7%)**, against 60% on the short
        list. Read with care: part of that is a coin having more bad places to
        pick, and what RL needs is places *better* than the teacher's choice,
        which only RL can show.
      - The imitator reaches **97.5% held-out** over 21 candidates (always-first
        59%, highest-worth 89%) and fights the teacher to **146 of 288
        (50.7%)**.
      - The twin test, run with places on and the sides staged in sight of
        each other, caught **two compasses** on its first runs: the sampler's
        own greedy spread (a tie group is now taken whole), and an old one in
        the evaluator's support term — the nearest friend chosen by
        `(distance, x, y)` and asked alone for a sight line. Fixed by asking
        every friend at that distance; snapshot unmoved. It is not the cause
        of the generated maps' lean (town 218–358 before and after, eight
        seeds).
      - `examples/mirror` gained `--explain` (both twins' candidates set side
        by side, feature by feature, and whether the board was still a
        mirror) and now checks positions *after* each round resolves. That
        corrected it: **every skill-arena "different key" root was a round-2
        split it could not see** — two light tanks driving for the centre hex,
        which is its own reflection, and the first to move in unit-id order
        taking it. A first-mover rule for a contested hex, not a compass, and
        a candidate rule change for later (who arrives first should not be
        who has the lower id).
- [x] L1.3 The loop, with no bridge at all: the network is microseconds on a
      CPU, so **Rust plays and infers** (`examples/rollout`, every core, the
      learner *sampling* through `GoalBrain::Explore` and recording each
      choice's log-probability) and writes finished battles with a per-round
      **ledger** of every crew's condition and both scores; **JAX learns**
      (`training/ppo.py`, PPO with a critic only Python runs); `loop.py`
      repeats it on fresh seeds and measures with `balance --brains` on seeds
      no rollout uses. About 13 s an iteration of 1,024 ridge battles.
      `Arena::stage` moved into the harness so the rollouts and the brains
      table fight the same battles (brains table byte-identical).
- [~] L1.4 Rewards, first cut: the engine has no opinion about them — the
      trainer reads the ledger. The mission first and shared by the side
      (enemy substance destroyed, own lost, the objective margin, ±1 for the
      battle), her own survival second and hers alone; a decision is credited
      with the rounds it governed (semi-Markov TD, GAE over each crew's
      decisions). **Still to do**: rewards by order verb, and the personality
      weights per cadet.

  *What the first runs said (2026-10-14, ridge, 16 places, difficulty 5):*
  - **Warm-started from the imitator, it does not move**: 30 iterations, eval
    flat at 50%. The imitator's softmax is peaked (entropy ~0.9 over 21
    candidates) and PPO's small steps stay in the teacher's basin.
  - **From scratch it learns the teacher's level**: a near-random network
    (12.5% deterministic) reached 53–56% on the loop's own eval seeds within
    10–15 iterations — about 15,000 battles, three and a half minutes — and
    held there for 40. **On eight seeds nothing had touched, it is parity**:
    301, 300 and 291 of 576 for three checkpoints (±12 is one sd). The 55%
    was choosing the best checkpoint on reused seeds.
  - So the loop learns, the reward is coherent, and the ceiling with these
    features on this arena is so far the hand-written chooser's. What to try
    against it: a warm start at a higher temperature (or entropy bonus) so the
    imitator explores; more decisions per battle (L2.5's learned re-decision,
    and longer platoon fights, L1.1 — two decisions a crew a battle is little
    to learn from); self-play rather than one fixed opponent; and varied
    ground, because one arena's dice are all it has seen.
- [ ] L1.5 The enemy model's first two layers: the briefed order of battle
      less what has been seen, and lost contacts with their age (*A model of
      the enemy*). The third — a league, an enemy-doctrine embedding, the
      prediction head — belongs with L2.

**L2 — cadets.**

- [ ] L2.1 Goal-conditioned reinforcement learning on mixed orders, traits
      randomised.
- [ ] L2.2 Trust, as an input, moved by outcomes.
- [ ] L2.3 Bar: beats the utility AI seed-swept on `ridge_arena`; front-plate
      share down; a pinned advance recovers.
- [ ] L2.4 Playable: the player commands learned cadets on the world clock.
- [ ] L2.5 Learned termination with a deliberation cost: when a cadet
      re-decides (*Where to spend thinking*). First half, hand-written event
      triggers (`planner.goal_events`), landed 2026-10-14 — see below.
- [ ] L2.6 An exam suite of Cold War vignettes (*Decision tempo*).

  *What event triggers did (2026-10-14, ridge, 16 places, difficulty 5, all
  five events on for every crew):*
  - **Decisions 2.4×** (27,822 → 66,372 over 512 battle-map battles; ~16 a
    battle on the ridge against ~7). Battles bloodier and a little shorter
    (6.3 rounds against 7.0); utility over a coin 203–85 (70.5%, from 75.7%
    — a coin that reconsiders on contact is less of a coin).
  - **RL from scratch now beats the teacher.** A fresh network, 40
    iterations of 1,024 battles (~13 minutes): on eight seeds nothing had
    touched, **422 of 576 (73.3%)** against the utility chooser (one sd is
    12). Checkpoints 15 and 25 read 407 and 412: most of it is learned by
    iteration 15.
  - **What it learned, by the instrument PLANNING.md has used since
    September** (1,024 battles each, near-deterministic): its strikes on
    front plate **71%** against the imitator's 78% (rear 5% → 9%); it is
    struck **4,524** times against 8,623 while striking more (8,955 against
    8,206), and what strikes it lands on its front plate 83% of the time
    against 74%. It holds 28% of its decisions (the teacher never does),
    moves in shorter bounds (3.4 hexes against 4.9) yet closes further
    (+2.06 against +1.44 a decision), onto ground fewer enemies can see
    (0.42 against 0.63). Getting round, keeping its front to the enemy, and
    being hit half as often — fire and movement, named to it nowhere.
  - **Against itself it does not fight**: 36 draws of 36, nobody lost, 9
    rounds each. The likely cause is the reward, not the network: the
    objective margin was weighted 0.05 a point, so the crest (3 a round) was
    worth ~0.15 a round against the losses of taking it — the mission was
    *not* first, against the designer's rule. Raising the mission's weight,
    and objectives that a waiting side forfeits, are the next run's
    question; a mirrored meeting engagement with no clock is a stalemate
    for any two players who have learned that the defender wins.

  *The mission first (2026-10-14, the tiered reward; two arms from scratch,
  40 iterations, every event on, measured on eight untouched seeds):*
  - **The stalemate broke**: self-play went from 288 draws in 288 to 0 —
    144–144 over ~7.2 rounds (arm A).
  - **Against the teacher, 335 of 576 (58.2%, ~3.9 sd)** with the utility AI
    as the only opponent (A), and 328 (56.9%) with a league of four past
    checkpoints added (B) — the league bought nothing measurable in 40
    iterations of its own lineage.
  - **So most of the old 73% was exploiting the teacher's aggression.** The
    old policy's final objective margin was **−7.0** (it conceded the ridge
    and won by killing the side that came for it); the mission-first
    policy's is **+6.2** (the teacher −0.4), it wins by reaching the score
    47 times in 1,024 against the teacher's 15, aims 97% of its decisions at
    objective ground, never sits still, and goes by ground fewer enemies can
    see (0.43 against 0.63), struck 7,102 times to the teacher's 8,623 while
    striking about as often. It flanks no more than the teacher (78% front):
    the job is the crest, not the tank.

**L3 — the commander**, over frozen cadets. **L4 — alternation and a
league.** **L5 — across battles**: trait drift (1), learned plays (2), then
experience as input (3).
