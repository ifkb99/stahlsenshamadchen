---
id: girls
title: Cores, Training and Traits
category: Reference
---

# Cores, Training and Traits

How a girl's abilities are decided, and why they are decided that way.

The short version: **what she can do is derived, never written down directly.**
A girl has a temperament she was born with, training she has been given, things
that have happened to her, and a condition she is in right now. Her gunnery is
the sum of those. Nothing in the data says "gunnery: 3".

That indirection is the whole point. If a stat *is* the ability, then a girl is
her competence and the roster is a spreadsheet. Separating who she is from what
she has been taught is what lets a gifted novice and a plodding veteran be
different people rather than different numbers.

## The four layers

**Cores** — temperament. Four values, slow to change, set at recruitment.

| core | what it is | what it feeds |
| --- | --- | --- |
| **Nerve** | steadiness under fire | morale; how much reactivity survives being shot at |
| **Wits** | quickness and perception | reactivity; spotting; how fast she reports contact |
| **Hands** | coordination | aiming and handling |
| **Presence** | authority | leadership; order clarity; steadying others |

**Training** — competence. Learned skills: gunnery, driving, signals, command.
This is the bulk of any ability.

**Traits** — character. Named, situational, and usually paired: a gift and a
cost together. See below.

**Condition** — right now. Wounded, suppressed, exhausted, out of contact.
Temporary, and the reason the same girl performs differently in two battles.

### Derived characteristics

Some abilities are not trained at all — they fall straight out of the cores,
the way GURPS derives Basic Speed, Will and Perception from its primaries.

| derived | from | governs |
| --- | --- | --- |
| **Reactivity** | Wits and Nerve | ticks of delay before acting on a new order or a new situation |
| **Perception** | Wits | spotting range and how quickly a contact resolves |
| **Will** | Nerve, steadied by a commander's Presence | morale checks, resisting suppression |

Reactivity is deliberately the analogue of GURPS Basic Speed, which decides who
acts first. Here it decides the same thing, measured in the ticks the round is
already divided into.

An ability is all four:

```text
aiming = gunnery training
       + Hands modifier          (small)
       + veterancy
       - suppression
       ± traits that apply here
```

### Cores do three jobs

They **nudge the result**, they **steer learning**, and — the piece GURPS
supplies — they **set the floor when there is no training at all**.

That third job is what makes the whole model hold together. In GURPS a skill
you have never studied is not zero; it *defaults* to an attribute at a penalty.
A girl who has never been trained as a gunner can still shoot, at Hands minus a
few. So:

```text
gunnery ability = trained level, if she has one
                  otherwise Hands - 4
```

The consequence is elegant and self-balancing: **cores matter most when
training is thin and matter less as training rises.** A raw recruit is almost
entirely her temperament. A veteran gunner is almost entirely her training,
with her Hands as the reason she got there faster and still has the edge in a
snap shot. That is exactly the relationship between talent and experience the
campaign wants, and it falls out of one subtraction.

It also means every girl can attempt every job badly, which matters the first
time a driver has to take over a gun.

## How training advances

Two paces, deliberately.

**In the field, by use.** She gets better at what she actually does. Gunners who
shoot improve at shooting. This ties progression to play and makes veterans
emerge from the campaign rather than from a menu.

**At the academy, by direction.** Between battles you decide what someone is
*studying*, which is how a girl with no combat time gets trained at all, and how
you deliberately turn a driver into a gunner. This is the academy layer's
reason to exist.

## Traits

The rule that separates a trait from a stat: **a stat changes how well a rule
applies; a trait changes whether or when it applies.** Numbers are competence;
conditions are personality.

Traits come from two places:

- **Innate** — a couple at recruitment, which is who she is when you meet her.
- **Acquired** — from what happens to her. A girl who bails out of a burning
  Panther can come back afraid of fire. This is the loop worth caring about:
  the simulation already records what destroyed a vehicle
  (`Unit.last_hit_by`), how survivable it was (`VehicleDef.safety`) and what
  became of her (`CrewFate`), so acquired traits fall out of machinery that
  exists. It is also what gives the visual-novel layer something to talk about.

Traits are usually **paired** — an upside with a matching downside — because a
trait that is only good is a stat with a name on it.

| trait | gift | cost |
| --- | --- | --- |
| Lead foot | faster on roads | bogs more easily off them |
| Hothead | takes opportunity shots readily | fires when told to hold, giving away position |
| Gun-shy | — | loses a reaction the first time she is fired on each battle |
| Steady | suppression does not slow her | — |
| Loner | better with nobody adjacent | worse in a pack |

## Soldiers and commanders

**One stat page, two tracks.** Everyone has the same four cores. A commander is
a girl with command training whose Presence is applied at formation scale
rather than to her own vehicle.

That matters for the story more than for the maths: promotion becomes something
that happens *to a girl you know*, and losing a commander is losing a specific
person rather than a game piece. A separate commander stat block would have
made them a different species.

## What the player sees

**Words by default, numbers on request.** "Anka: steady hands, quick, rattles
easily", with exact figures behind a toggle.

This is not decoration. Girls read as people when described and as units when
tabulated, and the game is asking the player to care about them. The numbers
stay available because a player who wants to optimise should be able to, and
because the developer needs them.

## Orders, and the licence to fail them

Orders are given, not executed. What a girl was *told* and what she *does* are
different things, and the gap between them is where character lives.

The engine's round structure already has the right shape for this: a round is
twelve ticks, orders are fixed at the start, and the world changes during
resolution. A machine drives the ordered path into an ambush because that was
the plan. A person notices at tick four and does something about it at tick
six — or at tick nine, or not at all. **Reactivity is measured in ticks**, and
that is the single most important derived ability.

Girls have **full latitude**: under enough pressure, or with the right traits, a
girl can do something other than what she was told. She can break off, refuse to
advance, or shoot when ordered to hold.

That is a deliberate choice and it carries a debt. Deviation is only fair if it
is **legible, attributable, and predictable in advance**:

- a visible morale ladder, so the player can see a crew is wavering *before* it
  matters;
- a stated cause in the log — "Petra is rattled and stops short of the ridge",
  not a unit that silently ignores you;
- traits surfaced when giving orders, so a hothead's disobedience is a thing
  you knew about and accepted.

Without those three, full latitude reads as the game being broken. With them,
it is the best thing in it.

## Open: the scale

Cores are currently 0-5 integers inherited from `CrewStats`. GURPS centres on
10, with 8-12 as ordinary human range and 14+ as remarkable, which buys two
things a 0-5 scale cannot: **"average" is a real place on the scale**, and
there is room for skill defaults to subtract from without hitting the floor.

A 0-5 scale makes `Hands - 4` almost meaningless for anyone below 4. That
argues for moving to a 10-centred scale, which is a content change across
`crew.json` and the `balance` block rather than a hard one — but it should be
decided before any of this is written, not after.

## What this replaces

`CrewStats` — five flat fields, of which `morale` and `leadership` were never
read — becomes cores plus training plus traits plus condition.

Two consequences worth stating in advance:

- **Best-of-crew has to go.** `Roster::best` takes the highest value across a
  crew, which means an extra crew member can only help and nobody is ever a
  liability. Once abilities derive from training, roles decide *which* training
  applies: the gunner's gunnery, the driver's driving. `VehicleDef.crew_slots`
  already names those roles in every vehicle's json and has never been read.
- **Crews get weaker**, because the best member no longer covers for everyone.
  The whole roster needs rebalancing, which is what
  `cargo run --release -p tactics_core --example balance` is for.

## What to take from GURPS, and what to leave

GURPS is the closest fit to what this system is trying to be, and three of its
ideas are worth taking almost directly.

**Skill defaults.** Described above. The single most useful mechanic here: it
makes untrained competence meaningful and gives cores a job that decays
gracefully as training grows.

**Derived secondary characteristics.** Reactivity, Perception and Will are
computed, not stored. Fewer numbers to author, and they move automatically when
a core does.

**Advantages and disadvantages with point costs.** In GURPS a disadvantage
refunds points that buy advantages, which is precisely the paired-trait economy
this design wants — a flaw is not a punishment, it is what pays for a gift. The
natural use here is *recruit generation*: give the generator a budget, let it
spend into cores, training and traits, and a girl with a serious flaw arrives
unusually gifted somewhere else. That is how you get people worth remembering
instead of a smooth talent curve.

Three things not to take:

- **Its breadth.** GURPS has hundreds of skills and a modifier for everything.
  Four cores and four or five trained skills is the right size for a game where
  the player commands a company, not a character.
- **Point-buy in the player's hands.** Building a girl point by point is a
  different game. Use the budget internally, show the result.
- **3d6 for everything.** Combat already resolves through a tuned percentage
  model and there is no reason to tear it out. Where GURPS's shape *is* worth
  copying is stat checks — reaction contests, morale, spotting — because a
  centred bell curve makes competence reliable and upsets rare but real. Flat
  rolls make good crews feel arbitrary, which is fatal when girls have licence
  to disobey.

## Inspirations

Worth reading before building any of this.

- **Battle Brothers** — the closest existing thing: core attributes with hidden
  per-character variance, backgrounds granting traits with real costs,
  permanent injuries, and a four-rung morale ladder that is extremely legible.
  Steal the ladder almost directly; it is what makes disobedience readable.
- **X-COM (1994) and Long War** — reaction fire as a contest, and stats that
  grow through use.
- **Combat Mission** — orders that take time to arrive and are executed
  imperfectly; experience, motivation and leadership as separate axes. Proof
  that units going to ground under fire reads as realism rather than as a bug.
- **Darkest Dungeon** — quirks acquired from what happens to a character. The
  direct model for acquired traits.
- **Jagged Alliance 2** — personalities with genuine downsides, and constant
  voice barks as the cheapest characterisation in the genre.
- **RimWorld** — the skill-and-passion split, where passion is a growth rate.
  This is where "cores steer learning" comes from.
- **GURPS** — skill defaults, derived characteristics, and the
  advantage/disadvantage point economy. The backbone of the model above.
- **Fire Emblem** — already a stated inspiration: growth rates as per-character
  tendency, and supports as the payoff for caring.
