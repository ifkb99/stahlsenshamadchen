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
  Training runs in Python and PyTorch, on the GPU.

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

## Not doing, now

- The visual novel, beyond what a battle needs to record for a recap.
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
- [ ] L0.2 Goal features: a fixed description of each candidate goal and
      of the chooser's context (her traits, her order, her state), pure Rust
      in `tactics_core::ai::features`. The utility chooser's own reading of
      each candidate (worth, road, exposure, who gets there first, the
      deviation charge) is part of the description, so the first network
      can reach the teacher by reading its arithmetic.
- [ ] L0.3 The imitation set: `examples/goal_dataset` fights battles under
      the utility AI and records every goal decision — context, candidates,
      the one chosen — as JSON lines.
- [ ] L0.4 `training/`, a uv project with PyTorch: a candidate-scoring
      network trained to imitate, reporting agreement with the teacher.
- [ ] L0.5 `LearnedChooser: GoalChooser` in Rust, reading the network's
      weights; a plain multilayer perceptron, no new dependency.
- [ ] L0.6 Bar: the imitating network fights the utility AI to parity on
      `ridge_arena`, seed-swept, inside the spread.

**L1 — the training ground.**

- [ ] L1.1 Platoon against platoon on small generated maps, many per seed.
- [ ] L1.2 Widen the candidates: `ground`'s vantages and firing positions,
      positions relative to the known enemy and to friends.
- [ ] L1.3 The rollout bridge: Rust battles on every core, decisions batched
      to a Python learner (PyO3, or a socket).
- [ ] L1.4 Rewards by order verb plus survival, and the personality weights.

**L2 — cadets.**

- [ ] L2.1 Goal-conditioned reinforcement learning on mixed orders, traits
      randomised.
- [ ] L2.2 Trust, as an input, moved by outcomes.
- [ ] L2.3 Bar: beats the utility AI seed-swept on `ridge_arena`; front-plate
      share down; a pinned advance recovers.
- [ ] L2.4 Playable: the player commands learned cadets on the world clock.

**L3 — the commander**, over frozen cadets. **L4 — alternation and a
league.** **L5 — across battles**: trait drift (1), learned plays (2), then
experience as input (3).
