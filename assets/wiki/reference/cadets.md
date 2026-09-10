---
id: cadets
title: Cores, Training and Traits
category: Reference
---

# Cores, Training and Traits

How a cadet's abilities are decided, and why they are decided that way.

The short version: **what she can do is derived, never written down directly.**
A cadet has a temperament she was born with, training she has been given, things
that have happened to her, and a condition she is in right now. Her gunnery is
the sum of those. Nothing in the data says "gunnery: 3".

That indirection is the whole point. If a stat *is* the ability, then a cadet is
her competence and the roster is a spreadsheet. Separating who she is from what
she has been taught is what lets a gifted novice and a plodding veteran be
different people rather than different numbers.

## The four layers

**Cores** — temperament. Slow to change, set at recruitment, and **defined in
mod data** rather than in Rust: `mod.json` declares which cores exist, so a mod
— or another game built on this engine — can have a different set entirely.
They are resolved to indices when the registry loads, so a check costs an array
index rather than a string hash.

The base game ships the GURPS attribute set, plus two the genre needs.

| core | GURPS | what it is |
| --- | --- | --- |
| **Strength** | ST | ramming a shell home, heaving track links, carrying a load on foot |
| **Dexterity** | DX | laying a gun, handling a vehicle, steady hands |
| **Intellect** | IQ | reasoning and planning; understanding an order correctly |
| **Health** | HT | lasting a long day, and surviving what happens to the vehicle |
| **Will** | Will | holding when it would be sensible to run |
| **Perception** | Per | what she notices, and how soon |
| **Speed** | Basic Speed | how quickly she acts once something changes |
| **Presence** | — | whether an order lands and is obeyed |
| **Charm** | — | who she gets on with, and how morale travels |

Taking the whole set costs nothing at runtime — cores are a `Vec<i32>` resolved
to indices at load — and it buys depth that a smaller set was quietly throwing
away. The first sketch had four, and two of them were doing double duty:
"nerve" was Health *and* Will, so a cadet who was physically tough and a cadet
who was mentally unbreakable could not be told apart; "wits" was Perception
*and* Basic Speed, which conflated seeing a tank with reacting to it. Those are
exactly the distinctions that make a person recognisable.

GURPS treats Will, Perception and Basic Speed as secondary characteristics
derived from IQ and HT, but lets you buy them up separately. Here they are
simply stored, which is the same freedom with less arithmetic.

Presence and Charm are not GURPS attributes — in GURPS, Charisma is an
advantage. They earn a slot here because this is a game about command, and
because being *obeyed* and being *liked* need to come apart: a feared martinet
and a beloved section leader should play differently.

**The rule that keeps this honest: every core must be named by at least one
skill.** A core nothing reads is exactly the dead weight `morale` and
`leadership` were in the old `CrewStats` — declared for years, never once
consulted.

**Training** — competence. Learned skills, each naming the cores it draws on:

| skill | cores | what it does |
| --- | --- | --- |
| **gunnery** | Dexterity, Perception, Speed | laying and firing |
| **driving** | Dexterity, Health, Speed | handling, rough ground, not bogging |
| **loading** | Strength, Dexterity | rate of fire |
| **observation** | Perception, Intellect | spotting, and how soon |
| **signals** | Intellect, Perception | working the radio; relaying contact and orders |
| **maintenance** | Intellect, Dexterity, Strength | field repair |
| **first aid** | Intellect, Dexterity | treating a crewmate before she is a casualty |
| **command** | Presence, Intellect, Charm | orders that arrive and are followed |
| **discipline** | Will, Intellect | following an order under pressure; holding fire when told |
| **small arms** | Dexterity, Perception | personal weapons, for dismounts |
| **fieldcraft** | Perception, Intellect | cover, concealment, moving unseen on foot |
| **athletics** | Health, Strength | foot movement, endurance, obstacles |

Discipline is the one to watch. It draws on Will to stand the pressure and
Intellect to have understood what was actually asked, which makes obedience
*trainable* rather than pure temperament — and opens a real character axis:
steady-but-dim follows the letter of an order into a ditch, while
clever-but-undisciplined improvises constantly.

More command skills arrive with chain of command — tactics, reconnaissance and
supply are the obvious ones — so the skill list is data and adding to it should
never need a code change.

**Traits** — character. Named, situational, and usually paired: a gift and a
cost together. See below.

**Condition** — right now. Wounded, suppressed, exhausted, out of contact.
Temporary, and the reason the same cadet performs differently in two battles.

### Checks, not derived stats

There are no secondary stats. There is no stored "Reactivity" or "Perception"
number. **Every tool and every action declares which cores it draws on**, and
the answer is worked out where it is needed.

```json
{
  "id": "lay_gun",
  "skill": "gunnery",
  "cores": { "hands": 2, "wits": 1 },
  "untrained_penalty": 4
}
```

A weapon names a check, or defines its own — a howitzer firing indirect can
lean on Wits where a tank gun leans on Hands, and that is a content decision
rather than a code change. Spotting, order latency, relaying a message and
holding under fire are all checks with different core mixes.

This is closer to how GURPS actually works than a secondary-characteristic
model is. In GURPS every skill names a *controlling attribute* — Guns is
DX-based, Electronics Operation is IQ-based — and a skill level is written
relative to it. The attribute is attached to the action, not to an intermediate
number.

Two reasons it is the right shape here:

**A check happens at a place and time; a stat does not.** Suppression, terrain,
being buttoned up, having just been fired on, and a trait that only applies in
woods are all modifiers arriving at the point of use. With a stored derived
stat they have to be bolted on, and the "real" value ends up recomputed at
every use anyway. It also means no cached value can go stale.

**It is the moddable shape.** Adding a weapon that rewards a steady hand, a
vehicle that punishes a nervous driver, or a whole new kind of order is data.
The rule is: **Rust decides *when* a check happens; data decides what it is
made of.**

An ability is all four:

```text
check = trained skill, or (controlling cores - untrained penalty)
      + weighted core contribution
      + veterancy
      - condition (suppression, wounds, being out of contact)
      ± traits that apply to this check, here, now
```

### Cores do three jobs

They **nudge the result**, they **steer learning**, and — the piece GURPS
supplies — they **set the floor when there is no training at all**.

That third job is what makes the whole model hold together. In GURPS a skill
you have never studied is not zero; it *defaults* to an attribute at a penalty.
A cadet who has never been trained as a gunner can still shoot, at Hands minus a
few. So:

```text
gunnery = trained level, if she has one
          otherwise Hands - 4      (so an average 10 shoots at 6: badly, but
                                    not helplessly)
```

The consequence is elegant and self-balancing: **cores matter most when
training is thin and matter less as training rises.** A raw recruit is almost
entirely her temperament. A veteran gunner is almost entirely her training,
with her Hands as the reason she got there faster and still has the edge in a
snap shot. That is exactly the relationship between talent and experience the
campaign wants, and it falls out of one subtraction.

It also means every cadet can attempt every job badly, which matters the first
time a driver has to take over a gun.

## How training advances

Two paces, deliberately.

**In the field, by use.** She gets better at what she actually does. Gunners who
shoot improve at shooting. This ties progression to play and makes veterans
emerge from the campaign rather than from a menu.

**At the academy, by direction.** Between battles you decide what someone is
*studying*, which is how a cadet with no combat time gets trained at all, and how
you deliberately turn a driver into a gunner. This is the academy layer's
reason to exist.

## Traits

The rule that separates a trait from a stat: **a stat changes how well a rule
applies; a trait changes whether or when it applies.** Numbers are competence;
conditions are personality.

Traits come from two places:

- **Innate** — a couple at recruitment, which is who she is when you meet her.
- **Acquired** — from what happens to her. A cadet who bails out of a burning
  Panther can come back afraid of fire. This is the loop worth caring about:
  the simulation already records what destroyed a vehicle
  (`Unit.last_hit_by`), how survivable it was (`VehicleDef.safety`) and what
  became of her (`CrewFate`), so acquired traits fall out of machinery that
  exists. It is also what gives the visual-novel layer something to talk about.

Traits are usually **paired** — an upside with a matching downside — because a
trait that is only good is a stat with a name on it.

### How a trait is written — implemented

Most traits are declarative: a condition and a modifier, in json, which keeps
them validatable and lets `validate-mods` report a typo instead of a crash.

```json
{
  "id": "lead_foot",
  "name": "Lead foot",
  "effects": [
    { "check": "drive", "when": { "terrain": "road" },  "modifier":  2 },
    { "check": "drive", "when": { "terrain_not": "road" }, "modifier": -2 }
  ]
}
```

Conditions are a small closed vocabulary — `always`, `terrain`, `not_terrain`,
`vehicle_class`, `alone`, `crewed` — so `validate-mods` can tell a modder that
`terrian` is not a condition, rather than the trait silently doing nothing for
the rest of the project.

Traits that change *behaviour* rather than a number — a hothead firing when
told to hold — get a **Lua hook** instead. The campaign host already vendors
`mlua`, so the scripting surface exists and costs nothing new; it is the escape
hatch for the handful of traits a declarative vocabulary cannot reach, not the
normal way to write one. Not yet built: nothing needs it until cadets can
disobey, which is slice 5.

Acquired traits are also still to come. The data they need already exists —
`Unit.last_hit_by`, `VehicleDef.safety`, `CrewFate` — so a cadet who bails out
of a burning Panther can come back afraid of fire. That is the loop worth
having, and it feeds the visual-novel layer for free.

### Unspecified crew means ordinary, not untrained

A placement that names no crew gets a vehicle that performs exactly as its data
says. The alternative — treating unspecified as untrained — made every such
vehicle quietly slower and blinder than its own definition, which broke a
pathing test in a way that took a while to read: the mover simply could not
reach a tile the comment said cost "exactly one round's fuel". Paper stats
should mean what they say; named cadets modify from there, in both directions.

| trait | gift | cost |
| --- | --- | --- |
| Lead foot | faster on roads | bogs more easily off them |
| Hothead | takes opportunity shots readily | fires when told to hold, giving away position |
| Gun-shy | — | loses a reaction the first time she is fired on each battle |
| Steady | suppression does not slow her | — |
| Loner | better with nobody adjacent | worse in a pack |

## Soldiers and commanders

**One stat page, two tracks.** Everyone has the same four cores. A commander is
a cadet with command training whose Presence is applied at formation scale
rather than to her own vehicle.

That matters for the story more than for the maths: promotion becomes something
that happens *to a cadet you know*, and losing a commander is losing a specific
person rather than a game piece. A separate commander stat block would have
made them a different species.

## What the player sees

**Words by default, numbers on request.** "Anka: steady hands, quick, rattles
easily", with exact figures behind a toggle.

This is not decoration. Cadets read as people when described and as units when
tabulated, and the game is asking the player to care about them. The numbers
stay available because a player who wants to optimise should be able to, and
because the developer needs them.

## Orders, and the licence to fail them

Orders are given, not executed. What a cadet was *told* and what she *does* are
different things, and the gap between them is where character lives.

The engine's round structure already has the right shape for this: a round is
twelve ticks, orders are fixed at the start, and the world changes during
resolution. A machine drives the ordered path into an ambush because that was
the plan. A person notices at tick four and does something about it at tick
six — or at tick nine, or not at all. **Reactivity is measured in ticks**, and
that is the single most important derived ability.

Cadets have **full latitude**: under enough pressure, or with the right traits, a
cadet can do something other than what she was told. She can break off, refuse to
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

## The scale

**Centred on 10, GURPS-style.** 8-12 is the ordinary human range, 14 and above
is remarkable, and 6 is a real weakness rather than a rounding error.

Two things this buys that the old 0-5 scale could not. **"Average" becomes a
place on the scale** — a cadet at 10 Hands is unremarkable rather than
mid-table, which is what lets descriptions be written honestly. And there is
**room to subtract from**: an untrained penalty of 4 is meaningful against a 10
and meaningless against a 3, so skill defaults only work at all on a scale like
this.

## What this replaces

`CrewStats` — five flat fields, of which `morale` and `leadership` were never
read — becomes cores plus training plus traits plus condition.

Two consequences worth stating in advance:

- ~~**Best-of-crew has to go.**~~ Done. Seats are positional: cadet *i* fills the
  vehicle's *i*th `crew_slots` entry, and a `roles` block in mod data says which
  skills each seat answers for. The gunner's gunnery lays the gun.
- **Substitution is the exception now, and it is the wound model that causes
  it.** It did not start that way: the school had ten cadets and its tanks four
  seats each, so an empty seat was ordinary and substitution was the normal
  case. The base mod now ships forty-nine characters and `frontier` fills every
  seat in both academies, so a gap in a crew means somebody is in the
  infirmary — `CrewCondition::Absent` — rather than that nobody was ever
  written for it. The mechanism is unchanged: the best remaining crew member
  covers the seat at `balance.substitution_penalty`, because a commander can
  lay a gun — she is simply not the gunner.

  This is also why filling the seats was worth doing as content rather than
  leaving it. Substance is counted per person aboard, so a medium tank crewed
  by two named cadets died about twice as fast as the same tank crewed by four
  anonymous ones: naming your characters was a straight penalty, and the
  penalty was invisible.

### Two things roles exposed immediately

Neither was caused by the change; both were hidden by best-of, where a second
crew member barely mattered.

**The demo scenario was lopsided.** Kuhlmann fielded six cadets to the
Valkyries' four, across four vehicles each. Under best-of that was nearly
invisible. With seats it decided battles: the first run after the change went
5-0. `river_crossing` still fields those ten, because it is the determinism
baseline; the campaign map no longer does.

**Seat order in a map file is now meaningful, and `river_crossing` had it
backwards.** Juno drives like the brakes are a suggestion (driving 15) and was
sitting in the commander's seat, while Mina — gunnery 15 — was driving. Swapping
those two cadets moved the crew hit rate from 58% to 65% and the result from 5-0
to a single win in twelve. That is the system working: who sits where is now a
decision with a measurable price.

### Known limitation: seats are positional

A placement lists crew in seat order, so a crew of two fills seats one and two
and cannot skip to the gunner's seat. That is why Mina commands a light tank
she ought to be gunning. Explicit seat assignment is the fix, and it belongs
with the "move cadets between tanks" screen on the roadmap rather than being
bolted onto the map format now.

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
spend into cores, training and traits, and a cadet with a serious flaw arrives
unusually gifted somewhere else. That is how you get people worth remembering
instead of a smooth talent curve.

Three things not to take:

- **Its breadth.** GURPS has hundreds of skills and a modifier for everything.
  Four cores and four or five trained skills is the right size for a game where
  the player commands a company, not a character.
- **Point-buy in the player's hands.** Building a cadet point by point is a
  different game. Use the budget internally, show the result.
- **3d6 for everything.** Combat already resolves through a tuned percentage
  model and there is no reason to tear it out. Where GURPS's shape *is* worth
  copying is stat checks — reaction contests, morale, spotting — because a
  centred bell curve makes competence reliable and upsets rare but real. Flat
  rolls make good crews feel arbitrary, which is fatal when cadets have licence
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

## Everything harsh must be switchable off in data

Difficulty is a mod, not a setting. A player who wants none of this — no
bailouts, no disobedience, orders that simply happen — should be able to load a
`gentle` mod and get a Fire Emblem-shaped game, without the engine carrying a
difficulty branch through every system.

That is a constraint on how the rest of this is written, not a feature to add
afterwards. **Each harsh system has to be an additive rule whose absence is the
gentle game:**

- reaction latency collapses to zero ticks when its coefficients are zero, and
  orders execute the instant they are given;
- the morale ladder reduces to a single rung, and nobody ever wavers;
- casualty resolution reduces to "everyone walks away", which is what
  `CasualtyRules::permadeath` being off does — and the base mod's
  `casualties.permadeath: true` is the additive half, declared by content, so
  the campaign without the rule is the campaign that says nothing.

If switching one of them off needs an `if` in Rust, it was built wrong.

## Build order

Staged so each slice is verifiable on its own. The determinism baseline and
`examples/balance.rs` are the instruments: every slice should be able to say
what it moved.

1. **Cores and checks as data.** `cores` and `skills` blocks in `mod.json`, a
   `checks` definition, `CharacterDef`/`Cadet` restructured, `crew.json` rewritten
   on the 10-centred scale. Reroute the three existing consumers — aiming,
   driving, spotting — through checks. Behaviour should land close to today's;
   the balance harness says how close.
2. **Roles instead of best-of.** `VehicleDef.crew_slots` already names them and
   has never been read. This is where crews get weaker and the roster needs
   retuning.
3. **Traits.** Declarative condition-and-modifier, with the Lua hook for the
   behavioural ones. Innate first, acquired-from-events second — the casualty
   data needed for the latter already exists.
4. **Reaction latency.** Half done, and the half that is missing is the
   interesting half. The rules exist — a `reaction` block, a `reactions` skill
   on Speed and Will, and `ReactionRules::delay` turning a skill level into
   ticks — and nothing consumes them yet.

   The first attempt gated *planned execution*: a unit could not move or fire
   until its delay had passed. That was wrong twice over. Gating fire
   double-models rate of fire, which weapon cooldowns already do, and it taxes
   a crew every round for orders they were given before it started — measured,
   it cut shots across a battle by 42% and doubled how long fights ran. Gating
   movement broke the meaning of a vehicle's stated speed, and broke three
   invariant tests that any non-zero delay would have broken, which is the tell
   that it was not a tuning problem.

   Re-reading the design settled it: *"a person notices at tick four and does
   something about it at tick six"* is about reacting to **new information**,
   not about being slow to start a move already ordered. So the delay belongs
   on responses to things the cadet was not told about — a tank appearing
   mid-round, an ambush, an order that arrives late — which is the same trigger
   deviation needs, and therefore belongs with slice 5 rather than before it.
5. **Morale ladder and full latitude.** Steady / wavering / breaking, visible
   before it matters, with a stated cause in the log whenever a cadet does
   something other than what she was told.

Chain of command lands on top of 4 and 5: an order that takes time to arrive
(signals, Presence) and time to act on (reaction latency) is the same currency
twice, so the two systems compose rather than collide.
