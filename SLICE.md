# Slice: Three Days on the Weide

A design memo and the live plan. Written 2026-11-02, when the designer decided
the project had overextended and needed a smaller, finishable target. This
file replaces [DIRECTION.md](DIRECTION.md) as the live plan. DIRECTION.md's
argument still stands; this file is the cut that argument never made.

## Why this exists

The complaint is the one DIRECTION.md opened with in August: *the orders never
seem to work out how I would like them to … we may be layering too many things
on top of each other*. Since then the engine has gained a chunked world, a
generated campaign, plans by playout, a learned goal chooser and an assault
model. Each one is well built and measured. Most arcs ended with "wired,
measured, null result", and play is still neutral.

The diagnosis this time is about **legibility**, not depth. A player can't
form an expectation of how an order will play out. When it goes differently,
she can't see why. The fixes come from three games, each for one thing:

- **Advanced Squad Leader** for legibility. A unit's state is visible on the
  board. It changes only at named moments, by a roll you can see against a
  number you can see. That is what lets an outcome be expected, and lets a
  surprise be explained.
- **Fire Emblem** for attachment. A small cast of named people whose stats
  and choices come from who they are, and whom you can lose.
- **Combat Mission** for the shape. WEGO turns, a computer that keeps the
  books, and a short operation of linked battles where the same people carry
  their losses forward.

The computer's job is to keep the books, not to remove the abstractions. Every
time this project replaced an abstraction with "the real thing" (dispersion,
partial penetration, a hidden fear accumulator) it lost a little legibility.
The slice runs the other way.

### What "done" means

The slice is done when a stranger can play three linked battles with one
platoon and, afterwards:

1. name the cadets who were in it, and say something about each;
2. for any battle, point at the replay line that explains every surprise.

### The three-week test

Before maps 2 and 3, and before the interludes, this has to exist:

- map 1, hand-made;
- the cadet card (below) on every cadet in it;
- morale as checks (below);
- WEGO turns on the existing resolver;
- a replay that names every roll.

In that battle, one cadet has to do something *because of who she is*, the
player has to see it, and it has to matter next battle (a death, a wound, a
promotion). If three weeks pass and this doesn't exist, the problem isn't the
engine. Stop and talk about it before building more.

## The story

Working draft. Every name and place is replaceable; the shape matters more
than the names.

**The setting.** 1965, in a federation that came out of the war too poor to
keep a large standing army. Under the 1952 Territorial Act, each province
defends itself with its cadet academy: three years of schooling, drill and
gunnery, and a mobilisation clause nobody expected to be read. The cadets are
in between, as [tone.md](assets/wiki/reference/tone.md) says. They are not
career soldiers and not children playing at war. The kit is whatever the
province owns, and much of it fought in the last war.

**The quarrel.** The Weide valley carries the railway and the river that two
provinces both depend on. The dam and the railhead at Marienfeld sit just
inside Kuhlmann's province. The neighbouring province is richer, and its
academy, the Iron Valkyries, is privately endowed, recruits abroad and is
equipped like it. When the provinces fall out over the water, Halberg orders
its academy to "secure" Marienfeld before the federal mediators arrive. The
mediators are three days out.

**The three days.** Kuhlmann's 2nd Platoon is at summer field camp at
Hollerbach, the nearest unit to the road. Lt Sigrun Falk is ordered to delay
the Valkyries until the company comes up, then hold the Kreuzhöhe ridge until
the mediators arrive. The company does not come. What does come, the first
night, is the academy's only running medium tank: Captain Anka Weiss's
Panther, twenty years old and taken back from the parade ground.

**Why it works as a frame.** The story's clock is the campaign's length.
Three days means three battles and a natural end. Equipment age characterises
both sides before anyone speaks: Kuhlmann has old halftracks and one old tank,
the Valkyries have new fighting vehicles. Kuhlmann knows the ground; the
Valkyries know their doctrine.

## The force

**Mechanised infantry first. Tanks are rare, and each one is an event.** A
platoon of rifle squads in carriers is the player's army. Across the whole
operation there is exactly one friendly tank and, per map, at most one enemy
**gun vehicle** (a tank, tank destroyer or cannon-armed IFV). Everything else
is infantry, carriers and the odd scout. This is a count of *vehicles*, not
of weapons: every vehicle carries what it really would (the Wiesel has its
37 mm and a machine gun; a carrier its MG). We call it the **one-tank rule**. The point is that a tank's arrival changes the battle, and the player
should feel that in the forecast numbers before she feels it in losses.

### Kuhlmann, 2nd Platoon (the player)

All existing characters (`characters/kuhlmann.json`). Fifteen named cadets is
about a Fire Emblem chapter's worth, and is the ceiling.

The first draft cast Sandoval and Sorge as squad leaders from their bios.
Under the current 22 numbers both are tank commanders (no small arms, no
fieldcraft), and that draft lost every AI battle. Nothing short of reading all
22 numbers showed it, which is the card's case made by accident. The squads
went to the three Kuhlmann cadets the data says are infantry. Sandoval
commands Falk's carrier, so their thread is intact.

| Element | Unit | Cadets |
| --- | --- | --- |
| 1st Squad | `rifle_squad` | **Lt Sigrun Falk** (platoon leader, leads from 1st Squad) |
| 1st Carrier | `halftrack` | **Sgt Vera Sandoval** (commander, MG), Nele Brauer (driver) |
| 2nd Squad | `rifle_squad` | **Sgt Traudl Kessler** |
| 2nd Carrier | `halftrack` | **Sgt Wilma Sorge** (commander), Yuki Tachibana (driver) |
| 3rd Squad | `rifle_squad` | **Marlen Ostwald** |
| 3rd Carrier | `halftrack` | Ruth Adler (commander), Lotte Vogel (driver, first-year) |
| Scouts | `scout_section` | **Elsa Brandt**, Ilse Radtke |
| *From day 2* | `medium_tank` | **Capt Anka Weiss** (commander), Mina Falkenrath (gunner), Juno Akiyama (driver), Rosa Steiner (loader, first-year) |

The infantry squads have one named cadet each, the section leader. The rest of
the section is the `troops` module and stays unnamed. This is ASL's split
between squads and leaders, and it keeps the cast small enough to care about.

### The Iron Valkyries (the enemy)

Also existing characters (`characters/valkyries.json`). Enemy commanders are
named, and the AI should play to their bios through doctrine and temper.

| Map | Force | Named |
| --- | --- | --- |
| 1 | Recon: scout section, 2 rifle squads in 2 `halftrack`, **one `light_tank`** | Greta Müller, Runa Eide (scouts), Sgt Sofia Ravenna (Wiesel commander), Kira Belova, Rea Marchetti; Sanna Lindholm, Marta Kovač (squads); Franka Mohr, Malin Roth, Cecile Aubry, Selin Aydın (carriers) |
| 2 | Lt Zofia Nowak's mechanised platoon: 3 rifle squads in carriers (the draft's 3 `ifv` would be three more gun vehicles, breaking the one-tank rule; see the playtest log), **one `heavy_tank`** | Nowak, Sgt Erika Thal, Sgt Britta Ehlers; Lt Hedda Ziegler (Löwe), Carmen Solano (gunner) |
| 3 | Lt Ingrid Stahl's holding force: 2 dug-in squads, **one `tank_destroyer`**, and the Löwe if it survived day 2 | Stahl, Sgt Alma Reiter; Nadja Orlov (Marder gunner) |

### Character threads

These are the seeds for the interludes. Each pairs a bio already in the data
with something the battles can produce.

- **Falk and Sandoval.** Vera argues with every plan and then carries it out
  exactly. Their thread is trust, and it moves when Sandoval holds a position
  Falk ordered her to hold.
- **Lotte Vogel.** A first-year who "has hit the target twice, and talks about
  both", driving a full carrier. She is the cadet the player will most fear for.
  That's the point, and the story doesn't protect her.
- **Mina Falkenrath and Nadja Orlov.** The academy's gunnery prodigy against
  the Valkyries' tank-destroyer ace, in the same town on day 3.
- **Ruth Adler.** "The first to notice the wire." Day 2's road is mined. Her
  Eyes and her trait should be what finds it.
- **Hedda Ziegler's Löwe.** Sixty tonnes at walking pace, frontally
  invulnerable to everything the platoon owns on day 2. If the player
  immobilises or kills it, it is absent on day 3, and she should be told so.

## The three battles

All three are hand-made `"shape": "free"` maps, about 24 × 18 hexes (2.4 ×
1.8 km at 100 m a hex). That's Combat Mission's scale for a platoon, written
in the existing ASCII row format. Maps are content: nothing about them should
need a code change.

### Day 1: Hollerbach (meeting engagement)

**Situation.** The platoon reaches the crossroads village just as the
Valkyrie recon arrives from the south. Neither side has dug in.

**Ground.** A village of five or six hexes at a crossroads. Orchards and
hedges to the west, open fields to the south, a sunken lane the scouts can use
unseen.

**Ending.** Two objectives, the crossroads (2 a round) and the church (1), and
a victory score of 24. Holding both ends it in eight rounds. There is no
clock in the engine and the slice shouldn't add one; the score race is the
clock. This is the one battle in the slice decided by score, and the one
place it teaches what the score is for.

**What it teaches.** Dismounting; that a halftrack is a taxi, not a tank;
concealment markers; morale checks. Above all, **what a single light tank does
to infantry in the open.** The Wiesel's 37 mm gun is a small one, and the
forecast should still show it winning every exchange with a squad at range
(if today's numbers don't, that's a content finding for milestone 2). The
answer is to kill it from cover at close range, with one of a squad's two
anti-tank rounds.

**The moment it is built for.** The first broken squad, and its temper
showing in what it does.

### Day 2: the Kreuzhöhe (defence, no clock)

**Situation.** The company never arrives. The platoon holds the ridge above
the river crossing with what is left of it, dug in overnight (`prepared`).
Anka's Panther arrived in the dark and starts **in reserve** behind the ridge.

**Ground.** A ridge with a reverse slope, a road climbing it through a
cutting, woods on one flank and a minefield on the road, sown by the
Valkyries' engineers in the night.

**Ending.** An assault (`assault`, `breaking`): no clock. It ends when the
Valkyries break on their will, lose the strength to go on, or take the ridge.

**What it teaches.** That a heavy tank cannot hold ground alone, and that
infantry assaults break on will before they run out of strength. The real
decision of the day is **when to commit the only tank you have.** The reserve
mechanism already gives this to the player: any order commits it. The Löwe
outguns the Panther frontally, so committing early is a mistake. The
forecast must show that.

**The moment it is built for.** The Panther coming over the crest and
knocking out two IFVs in a round, the first time the player has had that
power.

### Day 3: Marienfeld (counter-attack, no clock)

**Situation.** The Valkyries have taken the railhead below the ridge, and the
mediators arrive at dusk. Whoever holds the station then is negotiating from
it.

**Ground.** A small town: rail yard, station, a water tower with long sight
lines (a natural position for the Marder), and streets too narrow for the
Panther to manoeuvre in safely.

**Ending.** An assault with the player attacking.

**What it teaches.** Combined arms. The Panther is needed to crack the
strongpoints, and the infantry are needed to find and kill the Marder that
will kill the Panther. Each arm is helpless without the other, and the
forecast numbers say so.

**The branch.** This is the only one, and it's deliberately simple: what
day 2 did to the Löwe, and whether the ridge was held. If the ridge held, day
3 starts from it. If it fell, the platoon starts further back and the
Valkyries have the high ground. If the Löwe survived, it is in the town. Every
branch is told to the player in the briefing, by name.

### Between battles: the interludes

Two interludes, after days 1 and 2. This is the academy register: warm,
character-first, Fire Emblem's support conversations. Each one:

- reads the after-action report: who fought, who was hurt, who did something
  named;
- reads the losses aloud, by name;
- plays **one** authored conversation between two cadets who fought together,
  chosen from those still alive. If one of a pair died, the other gets the
  conversation about it instead.

Text and placeholder portraits only. No new screen beyond a page over the map.

### Reinforcements

**Built (2026-11-03), at the designer's suggestion.** A unit that arrives during a battle
instead of starting on the map. The story wants it: being beaten back and the
tank arriving in time is a moment the slice should have. The heat map below
wants it too: a tank that starts on the map drives straight into the
village, and one that arrives later has to cross ground where its power can
be seen.

- **Day 2: Anka's Panther arrives** over the reverse slope during the
  assault rather than starting there, and arrives *into reserve*, so the
  decision of the day (when to commit it) stays the player's.
- **Day 1: the Wiesel arrives** up the south road around round 4 (±1),
  after the Valkyrie recon, across the open valley floor, to a village the
  player may already hold. Shipped on `slice_hollerbach`; see playtest 2.

**How it stays legible (ASL's convention).** The entry is announced in the
briefing ("Weiss expects to be with you around round 6") and is a named roll:
a fixed round, ±1 on a die, shown in the replay. Not a trigger on the player
losing: a rescue that comes because you're losing reads as the game cheating,
and the enemy commander has to be able to plan around it the same way.

**How it is built.** Scenario data: `"arrivals": [{"formation":
"valk_armour", "round": 4, "spread": 1}]` (`map::ArrivalSpec`). The
arriving formation's units are written in `units` at the hexes they enter
on. **A unit that arrives does not exist until it does**: set aside at
setup with its crew, the round rolled once from the battle's dice, then
brought on at the top of its round through the engine's existing
`reinforce` (built for the campaign's columns). That answered the open
question without a fourth `Fate`. Nothing can see, shoot, count or lose a
unit that isn't there, so fog, strength and the rest are untouched.

The rules it needed:
- a side with help still on the way isn't eliminated;
- the arrival takes its side's standing order, and arrives in reserve if
  the map says so;
- if an enemy stands on its entry hex, it enters on the nearest free one;
- `Event::Arrived` is heard by its own side; the enemy has to see it.

The seat shows the window ("expected round 4 give or take 1"), never the
roll. Defended by `tests/arrivals.rs`; the determinism snapshot passes
unregenerated.

## The cadet card: fewer numbers

**The problem in figures.** A cadet carries 9 cores (strength, dexterity,
intellect, health, will, perception, speed, presence, charm) and 13 skills
(gunnery, driving, loading, observation, signals, maintenance, first aid,
command, discipline, small arms, fieldcraft, athletics, reactions). That's 22
numbers, plus up to 8 traits. An ASL squad has three numbers and an ASL leader
two. A Fire Emblem unit has about eight, each with one obvious effect. A player
can't predict an outcome from 22 numbers, so nothing a cadet does under fire
can feel like *her*.

**The rule.** Every number on the card names the roll it changes, in one
sentence on the card. A number that can't be described that way isn't on the
card.

### The card

```text
SGT VERA SANDOVAL                          2nd Squad · Veteran
Nerve  8   holds when a check is called: 2d6, 8 or less (72%)
Aim   +1   added to her unit's to-hit and fire rolls
Eyes   0   finding the enemy and reacting to him
Drive -1   handling a vehicle; she isn't trained
Lead  +1   added to the Nerve of every unit within 2 hexes of her
Temper     Steady: when she breaks, she goes to ground
Traits     Contrarian (argues with an order; carries it out exactly)
Condition  Good order
```

Five numbers, one word for temper, at most two traits, and a condition. Every
cadet has every number, so a dead gunner's place can be taken by somebody
worse at it, and the card says how much worse.

| Card | What it does | Replaces |
| --- | --- | --- |
| **Nerve** | Target number for morale and pin checks (2d6, roll this or less) | discipline; will |
| **Aim** | Modifier to her unit's to-hit roll (vehicle) or fire-table roll (squad) | gunnery, small arms |
| **Eyes** | Modifier to spotting, and to how soon she reacts with opportunity fire | observation, fieldcraft, reactions |
| **Drive** | Modifier to bog and rough-ground checks, and to how fast she reverses out | driving |
| **Lead** | Added to the Nerve of units she leads, and to rally attempts; zero for anyone unranked | command |
| **Temper** | How she breaks: Steady (ground), Skittish (flight), Fiery (charge) | the defiance scores over speed and will |
| **Experience** | Green / Trained / Veteran / Elite; promotion raises a number | veterancy |

**Where the rest goes.** Nothing in the data is lost. Each piece either
becomes a trait (a rule that applies or doesn't) or goes off the card.

| Old | Goes to | Why |
| --- | --- | --- |
| loading | the vehicle's rate of fire, with the loader's seat filled or not | a loader's skill is invisible from outside; whether the seat is filled is not |
| maintenance | trait **Mechanic** (field repair is possible) | it's a yes/no in play anyway |
| first aid | trait **Medic** (a wounded crewmate in her unit is not lost) | same |
| signals | cut; the radio is equipment | nobody forecasts a relay roll |
| athletics | cut; foot speed is the unit's | ASL doesn't model it per man either |
| health | trait **Tough** (survives a wound that would kill) | one rule, at one moment |
| the 9 cores | off the card: **growth**. Stored as now, never rolled, they decide which number rises on promotion. Charm decides how fast supports grow. | "who she is" decides how she grows; "what she was taught" decides what she does today |

Traits get the same audit. The existing eight (`lead_foot`, `deliberate`,
`eagle_eyed`, `eager`, `loner`, `reckless`, `craven`, `stolid`) are kept only
where each one changes *whether or when* a rule applies, per
[cadets.md](assets/wiki/reference/cadets.md)'s own definition. A trait that's
really a number bonus is folded into the card.

### How it's built

The engine reads skill ids by name in about a dozen places
(`battle/mod.rs`, `roster.rs`, `data/morale.rs`, `battle/command.rs`, …).
The first job is to move those into data: a `checks` block in `mod.json`
naming the card number each engine rule rolls against. After that the card
is content. That fixes a "content is data" smell on the way, and it is the
only Rust the card needs.

The 49 shipped characters are converted by a script, from their current cores
and skills into card numbers. The slice uses about 30 of them; the rest stay valid
content.

## Legibility: what's taken from ASL

In order of payoff. Each says what changes and where.

### 1. Morale is states and checks, not hidden pressure

**Now.** Pressure accumulates invisibly: a hit is 3, being targeted 1, flank
fire 150%, a near miss 50%, an open top 75%, being cut off 3. A crew pins at 6
and breaks at 12. The player can't predict either.

**Slice.** Three visible states: **Good order → Pinned → Broken** (and back, by
rallying). A state changes only on a named check:

- a **pin check** when fire lands near her (fail: pinned);
- a **morale check** when her unit takes a casualty or a penetration, a friend
  dies in sight, or her leader is lost (fail: broken).

Each check is 2d6 against Nerve, plus Lead if a leader is near, plus cover.
The current pressure inputs aren't wasted; they become **what calls a check
and what modifies it**. Flanked: −1. Cut off: −1. Open top: −1. The forecast
shows the target number before the shot, and the replay shows the roll after.

**Cost.** The AI prices fear through the same arithmetic
(`MoraleRules::pressure_for`). It has to price the *chance of failing a
check* instead. The one-currency rule makes that one place to change, not
many. The determinism snapshot will move broadly, which is expected.

### 2. One roll per question, with named modifiers

`combat::hit_breakdown` already builds hit chance from additive, named
modifiers: ASL's modifier list in all but name. Extend the pattern to every
roll, and move the whole game to **2d6, roll this or less**. Players learn its
bell curve quickly, and it gives the extremes below their names.

| 2d6 ≤ | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 | 11 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| chance | 3% | 8% | 17% | 28% | 42% | 58% | 72% | 83% | 92% | 97% |

The shot follows ASL's vehicle sequence: **to hit, then to kill**, each one
roll. The result is one of a few named categories: *no effect*, *shocked*,
*immobilised*, *gun out*, *knocked out*. Fire at infantry goes through a fire
table instead: firepower column × 2d6 → *no effect / pin check / morale check /
casualty*. The partial-penetration share and continuous damage are the
fidelity to cut; dispersion stays where it explains a miss.

This is the largest rule change. It comes **after** the three-week test,
and only if play shows the shot is where legibility is lost (milestone 6).

### 3. The extremes have names

A 2 on any check always passes and is a **moment**: on a morale check, she
steels herself and holds, and her neighbours take +1 Nerve for the round. A 12
always fails and is also named. These are 3% each, rare enough to remember.
They earn experience, and they're what the interludes talk about.

When a cadet breaks, her **Temper** decides what she does. This already exists
in the engine: the rung decides *that* she defies, the temperament decides
*how*. The change is that temper is now one visible word on her card, so a
Fiery cadet charging when she breaks is expected and still dramatic.

### 4. Leaders are modifiers

Lead is added to the checks of units within two hexes of her. Supports are the
Fire Emblem half of the same rule: two cadets with a support are each other's
+1 when adjacent. Relationships appear in the forecast and the replay as
numbers with names: "Falk is with her: +1".

### 5. Concealment is a marker

What the commander has been told (`Knower::Commander`) is drawn as **?**
markers on the board: something is there, not yet what. The detection rolls
stay as they are; the change is that the player can see *where she doesn't
know*.

### 6. Named movement modes

Three: **Advance** (slow, can fire, harder to hit), **Move**, **Rush** (fast,
can't fire, exposed). Each maps onto the existing per-order route pricing
(`balance.route_exposure`) and speed. The trade-off is on the button.

## WEGO, made readable

Combat Mission's weakness is that it never tells you *why*. The computer's
advantage over cardboard is that it can show every roll an ASL player makes by
hand.

- **Before committing:** the shot forecast in 2d6 terms with its modifiers
  listed, the danger along a planned path (`fire_on`, already drawn), and the
  checks a unit is likely to face there.
- **In the replay:** every state change cites its roll. *"Morale check: 8 or
  less, rolled 10. Sorge's squad breaks. She's Skittish: falls back."*
- **After the battle:** an after-action report that names each cadet and what
  happened to her. The interludes are built on it.

The rule worth testing: **no state change without a roll in the event that
caused it.** Wanted as a test, and later as a CLAUDE.md invariant.

## The AI: learned, not scripted

**Ruling (2026-11-02).** Combat Mission's AI is mostly scripted, scenario by
scenario. This game's isn't. Three seats, and nobody in them follows a
script:

| Seat | Who decides | What she decides |
| --- | --- | --- |
| The player's commander | the player | a job and ground for each element; when to commit the reserve |
| Every crew, both sides | one learned crew policy, behind `GoalChooser` | where to stand, whom to shoot, when to go to ground, within her order |
| The enemy commander | a learned commander policy | the same menu the player orders from |

The utility AI isn't scripted either. It stays as the teacher, the warm
start, and the floor a learned policy has to beat before it ships.

### What the record says

From [REIMAGINED.md](REIMAGINED.md) and [ASSAULT.md](ASSAULT.md). These
numbers decide what is realistic:

- **Crews work.** A learned crew policy reaches parity with the utility
  teacher from scratch in about 15 iterations (minutes), and the big runs
  beat it by 6–7 points on worlds no rollout touched. It wins by score and
  holds ground under fire. No manoeuvre has emerged: it flanks no more than
  the teacher.
- **Commanders are the open problem.** Over free crews in a meeting
  engagement, nothing beat saying nothing. Once a battle had a reserve only
  the commander could release, the first learned commander beat the coin
  attacking (117 vs 83 of 288, about 3 sd). Defending stayed within noise,
  and a later checkpoint slid back.
- **Crews trained under orders** (L2.1) execute orders without the order
  costing the side, but they score a little below the utility teacher's crews
  in the same seat (44.8% vs 48.4% on fresh seeds).

**Why the slice is a good place for the commander.** The finding was that
the battles didn't need a plan. The slice's battles are built around one: a
reserve on both sides on day 2, a tank destroyer in overwatch on day 3, few
elements, and a small fixed menu. And because the enemy commander's seat is
the same as the player's, self-play puts a learned commander in the player's
seat too. The enemy learns against something that orders the way a player
does, through the same door.

### Learned crews on the player's side

Keep them. This is the 2026-10-16 ruling (*orders are general*: the commander
names a job and ground, and the crew picks where to stand). It is also the
same policy on both sides, so the player never fights crews smarter than her
own.

It is also the biggest threat to the slice's legibility. August's complaint
was that orders didn't land, and a learned crew is a new way for an order not
to land. So, three conditions:

1. **Her intent is shown before the turn is committed.** In WEGO a crew
   decides at the start of the round. The planning screen runs the policy on
   the planning state and draws where each crew means to go under the order
   being drafted. If she changes her mind mid-round (`planner.goal_events`),
   the replay says so and says why. A deviation must announce itself; that
   rule already exists.
2. **Crews are trained under orders**, with the order's intent in the reward,
   so a player's order means something. Measured alongside the win rate: the
   share of orders a crew keeps.
3. **Personality goes through the card.** The network sees the card (Nerve,
   Aim, Eyes, Drive, Lead, Temper, traits), and personality is in the reward
   weights (ruled 2026-10-14). Lotte and Vera should do different things in
   the same hex, and the difference should match their cards. One policy
   that ignores who she is makes every cadet the same person, which undoes the
   Fire Emblem half.

### The enemy, made readable

Fog hides the enemy commander's intent during a battle, as it should. The
after-action report explains it afterwards: her decisions, with the round
each was made ("round 6: Nowak commits the Löwe"). Enemy commanders get
personality the same way crews do, through reward weights: Erika Thal pushes,
Ingrid Stahl waits. Their bios should become readable from how they fight.

### Difficulty

A **checkpoint ladder**: difficulty is how long she trained. Earlier
checkpoints are weaker, and each rung is measured against the teacher on
fresh seeds. One caveat: no win rate against the teacher says anything about a
human. Only playtests measure that. Part of Fire Emblem's fun is an enemy you
can outthink, and the top of the ladder may need to be optional.

### Overfitting

Trained on three maps, a policy will know those maps. That's acceptable *about
the ground*: it's what Combat Mission encodes by hand in a script, learned
instead of written. It is not acceptable *about the opponent*. A policy trained
against one opponent learns that opponent's habits, and a human won't share
them. So training is a league: the player's seat is filled by the teacher, the
coin and past checkpoints. Generated sectors are added to the training mix for
variety; they are training data, not slice content.

### Sequencing and cost

- **Every feature change retires every network** (`load_goal_net` refuses a
  mismatch). The card and morale milestones change the features, so **nothing
  is trained before milestone 4**. Until then the utility AI plays every seat.
  Afterwards, retraining is hours, not weeks: the pipeline exists.
- The utility teacher prices fear through `pressure_for`. It needs
  morale-as-checks pricing (milestone 3) before it is a decent teacher.
- **Ship gate:** in each seat, on each map, a learned policy ships only if it
  beats the utility AI in that seat on fresh seeds (`balance --brains`, seeds
  no rollout used). Otherwise the utility AI plays that seat and training
  continues. Neither one is a script, so the gate never forces one.
- Not in the slice: new network architectures, features beyond what the card
  needs, new reward research, learning across battles, the campaign-level
  commander.

## Playtest log

### Playtest 1: Hollerbach, 2026-11-03

**What was fought.** The map is `slice_hollerbach`, drawn by
`tools/make_slice_maps.py` (shapes, not hand-typed rows). Two kinds of
battle:

- **AI against AI**, through `examples/playthrough` (utility planner both
  sides, 16 seeds, doctrines as shipped and swapped). These measure the
  ground and the kit.
- **By hand**, through `examples/command`, a new headless seat: Claude as
  Kuhlmann's commander, giving section missions and radio orders from a
  situation report that shows only what side 0 knows (`heard_by`,
  `known_enemies`, and the commander's lost contacts). The crews were the
  utility planner, inside those orders. Two games on seed 1. **Both lost**:
  eliminated in round 10 (3 to 22) and round 8 (4 to 14), having killed one
  enemy vehicle in total.

**The ground and the kit** (AI against AI, Kuhlmann wins of 16, shipped /
swapped doctrines):

| Valkyries field | Kuhlmann wins |
| --- | --- |
| a Schütze IFV, a Kasten APC and the Wiesel (the draft) | 0 / 1 |
| two Kasten APCs and the Wiesel | 2 / 1 |
| two halftracks and the Wiesel (shipped) | 3 / 7 |
| two halftracks, no Wiesel | 15 / 15 |

The ground favours Kuhlmann heavily, and one light tank takes it back. That
is the slice's premise working. The draft's IFV broke the one-tank rule (a
20 mm autocannon makes it a second gun vehicle): its cannon, not the Wiesel,
did the killing (73 penetrations to 43).
The Kasten's armour 2 was the other half. Nothing Kuhlmann carries goes
through it except six rockets, while its MG goes through a halftrack (73
penetrations to 14). Day 2's three IFVs need the same treatment.

**What the hand games found.** In order of how much each one cost:

1. **Orders don't land, and the screen can't show it in time.**
   - Under "advance to the house", two of three squads dismounted three hexes
     short on their own judgement, in both games, and walked the rest under
     fire at one hex a round.
   - Under "hold (12,9)", a carrier's crew decided to "take the crossroads"
     and drove into rocket range.
   - A binding radio order to the hex a carrier already stood on was ignored:
     there is no order for "stay here" short of a hold mission, and the crew AI
     planned it fresh, onto the crossroads. All three carriers died there in
     one round.
   - Missions travel by radio and arrive *mid-round*, after the crews have
     planned without them.

   Every deviation was printed as an intention, but only after the round was
   committed. This is the case for the AI section's first condition (intent
   shown before committing). It needs the seat to split planning from
   resolution, and probably a "stay" order for one unit.
2. **Morale is invisible, and it ends battles.** Three carriers destroyed in
   sight put every squad on the platoon at Breaking in the same tick
   (`ally_destroyed` 4 each, Breaking at 12). Squads then sat at Breaking for
   rounds without rallying, and each was eventually removed by the bail-out
   check (`combat.rs`). That check treats a rifle squad like a tank crew
   abandoning a hull: a breaking squad that fails discipline is
   **destroyed**. None of Kuhlmann's leaders has the discipline skill; four
   Valkyries do. The narration says `Abandoned` with no roll, target or
   cause.

   Exempting infantry from the bail-out (a temporary experiment) changed
   nothing AI-against-AI: 1 of 16 against 2, with all 61 infantry bail-outs
   gone and the squads dying of other causes. So the bail-out is *how* they
   die, not *why* the Valkyries win. It is still the wrong rule for the slice:
   Broken should rout and be able to rally (milestone 3).
3. **The shot is illegible.** Rockets went through and did nothing a player
   could predict: 9 points into a halftrack's front (the Valkyrie one kept
   going), 4 into a light tank's side at close range. Kuhlmann's rockets made
   31 penetrations and no kills over 16 battles, while one Valkyrie rocket
   brewed up a halftrack through its ammunition rack. "Pts of effect" isn't a
   number a player can use. This is evidence that milestone 8 (the shot) may
   need to come earlier than planned.
4. **The 22 numbers hid a bad cast.** See *The force*.
5. **Walking is the hidden decision.** Foot units move one hex a round and
   carriers five or six. How far from the fight a squad dismounts decides
   whether she arrives at all, and nothing on the screen prices it.
6. **The narrator.** The AI-against-AI narrator prints `cadet #8` and axial
   coordinates. The seat prints names and map coordinates, but still as the
   events' debug text.

**What it means for the build order.** Finding 1 comes before the card and
before morale: no rules change helps while orders don't land. Next, then: the
seat splits planning from resolution and shows each crew's intent before the
round is committed, with a way to overrule it.

### Playtest 1b: where the battle happened, 2026-11-03

A new instrument, `examples/heatmap`, counts per hex and per side where
units moved, stopped, fired from, were hit, died, were spotted and were
pinned, and draws each over the ground (an SVG). It reads AI battles it
fights itself (the same seats as the kit table, which it reproduces: 3 and
7 Kuhlmann wins of 16) or hand games from the seat's new `rounds.jsonl`.
Hand game 2 replays exactly from its orders files; game 1 predates the
recast and doesn't.

**What the map was drawn for.** The Valkyries forming up in the Buchwald,
Kuhlmann's flank in the Kreuzholz, the scouts down the sunken lane to the
church, the hedges west of the village as cover in strips, and the Wiesel
winning exchanges with infantry *in the open* while a squad with rockets
kills it *from cover at close range*.

**What was fought.** A brawl on five hexes.

- **Everything converges on the crossroads.** The busiest hexes for every
  layer, both sides, are the objective hexes and their neighbours. 70–77% of
  all shots are at one hex (100 m), and 91–93% within two, in both doctrine
  orientations. Hand game 2 is the same: 67% at one hex.
- **Most of the map is decoration.** None of the three woods is used. The
  valley floor and the plateau are only driven across. The sunken lane is used
  only by the scouts who start at its head.
- **The Wiesel fights in the village, and wins there.** It drives up the
  south road onto the crossroads and fires from the objective hexes: 14 shots
  a battle, 74% at one hex. It is lost in half the battles. It wins as a
  brawler in a village, the one place the slice says infantry with rockets
  should beat it. That ties finding 3 (31 rocket penetrations, no kills) to
  the map: the village is no refuge, so the lesson Day 1 was written for
  can't be learned on it.
- **Suppression barely acts.** About one `PinnedDown` per side per battle.

**Why, as far as measured.** Vision is not the limit: a squad sees 8 hexes
and the Wiesel 14, and weapons reach 4 to 8. The likely cause is the score race.
Five hexes are worth 3 points a round, and both columns, at 5–6 hexes a round,
can reach them in two rounds.
Not yet measured: the range at which each side first spots the other.

**What it suggests, not yet tried:** the Wiesel as a reinforcement (above),
so it arrives across the open floor to a village the player holds; and
rockets that can kill (milestone 8, earlier). Each is a heat-map
comparison: the same picture before and after.

### Playtest 2: Hollerbach with tools, a preview and a late Wiesel, 2026-11-03

**The instruments** (`examples/heatmap`), each of which changed what we
think is wrong:

- **Sight lines** (`--sight`). The map isn't the one I meant to draw. Each
  10 m level is a terrace whose edge hides the ground below it:
  - Kuhlmann's start sees 110 hexes, all of them its own plateau.
  - The Valkyries' start sees 75 hexes, all of them their own strip.
  - The crossroads, inside the town, sees 49.
  - Only the church knoll overlooks the field (204).
- **Contact.** Both sides first see each other in round 2, at 2–3 hexes,
  and 86–92% of all spots are within two hexes. With the sight map, that's
  why playtest 1b found a brawl: nobody sees anybody until they're touching.
  **The map needs relief that can be seen across (gentler, longer slopes,
  or real overlooks) before anything else about it is tuned.**
- **Orders kept, timelines and diffs.** These are now numbers and
  pictures: whether a plan keeps its order, whether the round ends where it
  was planned, each unit round by round, and B minus A for any change.

**The planning preview** (`command plan`). It shows each crew's plan under
the orders file before the round is committed, and `turn` then fights
exactly that. Replaying hand game 2 through it explains finding 1 before
each round:
- **Squads dismount short because of when dismounting happens.** A
  passenger gets out at the first tick, where the carrier *starts* the
  round. Her crew AI decides to (within two hexes of a held objective, or
  the ride under threat), and the carrier then drives on without her. The
  overrules are `stay_aboard` this round, then `dismount` next round at
  the destination.
- **A binding order to a crew's own hex was a no-op.** It was a march
  with no step to take, so she stayed unplanned and her crew AI drove her
  onto the objective. Fixed in the engine: it is now a hold, and a crew
  under her commander's hold stays put.
- **Missions cost a whole round.** A formation's mission reaches its crews
  10–15 s into the round, after they've planned, so under WEGO any delay
  above zero is a round late. A radio order to one crew is instant.
  **Ruled (designer, 2026-11-03): crews re-plan when orders arrive.** Built
  as `command.replan_on_arrival`: when a mission lands, each crew it reaches
  drops her own errand and re-routes for the order's ground for the rest of
  the round, saying so (`Replanned`). Her full judgement of where to stand
  returns at the next planning phase, and the drill can still take her
  into cover.

**The late Wiesel**, AI against AI, 16 seeds, before → after. One
confound: the recon is now led by Lindholm's squad, since the Wiesel left
the recon formation.

| | Kuhlmann wins (shipped / swapped) | Wiesel shots a battle | Wiesel lost |
| --- | --- | --- | --- |
| On the map from round 1 | 3 / 7 | 14 | half the battles |
| Arrives round 4 ± 1 | 4 / 10 | 21 | under a third |

It now crosses the valley in the open, and about one shot in ten is at
5–8 hexes, but 81% of its shots are still within two hexes. Arrival timing
doesn't cure the brawl; the dead ground does.

**Hand game 3**, Claude commanding with the preview: lost 1–12 in round 8,
but it was a different battle.
- **Orders kept:** 100% of 35 plans kept their order, where game 2's
  carriers deviated repeatedly.
- **Kills:** two enemy units, against one across both earlier games.
- **Range:** 40% of Kuhlmann's shots were at 4+ hexes, from carriers on
  the slope.
- **The moment the day was written for happened.** The Wiesel was seen
  coming across the valley, drove onto the church hex next to 2nd Squad,
  and Traudl Kessler's squad was ordered to fire both rockets at it at one
  hex. Both missed. Nothing on the screen said what the chance was. That's
  finding 3 at its sharpest, and the case for the forecast (milestone 4)
  and the shot (milestone 8) coming sooner.
- **The seat's new orders:** `stay`, `stay_aboard`, `fire`.

### The relief, redrawn (2026-11-03)

The fix playtest 2 asked for, made with the sight tool before any battle
was fought on it. `--across A B` measures how much of one band of rows can
be seen from another:

- **Before:** a plateau (rows 0–5), a village terrace one level up (rows
  6–10), then the valley.
- **After:** the plateau ends at a crest on row 5, a short slope (rows
  6–8) falls to a village at valley level, and the church knoll still
  stands over it.

| Sight within 8 hexes | Before | After |
| --- | --- | --- |
| Valley (rows 12–17) from the north slope (rows 4–9), pairs | 13% | 50% |
| North slope (rows 6–11) from the valley, pairs | 44% | 65% |
| Anything below the crest from Kuhlmann's start (rows 0–2) | 5% | 4% |

AI against AI, 16 seeds, doctrines as shipped / swapped, Wiesel arriving:

| | Before | After |
| --- | --- | --- |
| Kuhlmann wins | 4 / 10 | 7 / 6 |
| First contact (Kuhlmann) | round 2.0, 1.2 hexes | round 1.2, 7.1 hexes |
| Shots at one hex | 69–74% | 49–54% |
| Wiesel's shots at 5–8 hexes | 10% | 29% |
| Wiesel lost | under a third of battles | one in ten |

Kuhlmann wins 14 of 32 overall either way, so the balance hasn't moved
beyond the noise of 16 games. What moved is what kind of battle it is:
- The lines of fire now cross the valley: the Valkyries shoot up into the
  village from the south slope, Kuhlmann down into the valley.
- The Wiesel fights from the open floor at range. That is what Day 1 was
  written for.
- The crossroads is still where the fighting gathers, because it is the
  objective.
- Nobody uses the woods yet. That is the AI's appetite, not the ground's.

## What's parked

Parked means no new work, tests keep passing and CI keeps it compiling.
Nothing is deleted. See [PARKED.md](PARKED.md).

- the overworld and the generated campaign; worldgen; chunk residency beyond
  what a battle needs;
- RL beyond the slice: new architectures, learning across battles, the
  campaign-level commander;
- plans by playout, and the chain of command above platoon;
- every new balance-harness feature (the harness itself stays as an
  instrument).

## Build order

Each milestone is playable and ends with a playthrough, not a test count.

| # | Milestone | Done when |
| --- | --- | --- |
| 0 | This memo; branch `feat/slice` | the designer has read and changed it |
| 1 | **The card**: `checks` block in data; conversion script; card on the cadet screen | `every_engine_check_names_a_number_on_the_card`; the 26 slice cadets read on the card |
| 2 | **Map 1** authored and fought on today's rules | played by hand twice, with notes on what was illegible |
| 3 | **Morale as checks**: states, 2d6, temper, named extremes; AI prices check failure | `a_cadet_who_breaks_does_what_her_temper_says`; replay names every check |
| 4 | **Forecast and replay**: 2d6 forecast; narrated rolls; after-action report by name | `no_state_change_without_a_roll`; the predict-then-check playtest below |
| | *The three-week test* | |
| 5 | **Learned crews**: features from the card; trained under orders, in a league; intent drawn on the planning screen | beats the teacher in the crew seat on fresh seeds; order-kept share reported; `a_crews_intent_is_shown_before_the_turn_is_committed` |
| 6 | **The operation**: maps 2 and 3 authored; three maps linked, roster and vehicle fates carried, the day-3 branch, interludes | `an_operation_carries_its_dead`; the whole thing played start to finish |
| 7 | **The learned enemy commander**: trained on the three maps in self-play and a league; her decisions in the after-action report; the checkpoint ladder | beats the utility commander in her seat, per map, on fresh seeds; otherwise the utility commander ships that map |
| 8 | **The shot**: to-hit/to-kill and the fire table, *if* the playtests say the shot is still illegible | the same playtest, better |
| 9 | The authored conversations; the cadets' portraits brief | done means done (above) |

**The playtest protocol.** Before resolving a turn, write down what each unit
is expected to do. Afterwards, for every surprise, find the replay line that
explains it. Count the surprises with no explanation. That count is the
slice's legibility number, and it should go down with every milestone.

## Tripwires

Signs the slice is turning back into the old project. If one shows up, stop
and say so:

- a new resolver, a new rules memo, or a fidelity improvement before the
  three-week test passes;
- a number added to the card;
- a fourth map, a second branch, or a generated map in the slice (generated
  sectors as *training* data are fine);
- a network trained before milestone 4, while its features are still
  moving;
- a new network architecture, a feature the card doesn't need, or a reward
  term researched for its own sake;
- an RL result quoted from seeds a rollout or an evaluation used;
- a harness sweep run to answer a question a playtest could answer;
- any work on a parked system.

## Open decisions for the designer

1. **2d6 or percentages.** Recommended: 2d6, roll this or less, everywhere. The
   forecast can show both ("8 or less, 72%"). Percentages read better to Fire
   Emblem players; 2d6 gives named extremes and is what ASL players expect.
2. **Hex scale.** ASL's hex is 40 m; ours is 100 m. Recommended: keep 100 m for
   the slice. Changing scale ripples through every range and speed.
3. **The story premise.** A draft. The names of the valley, the provinces and
   the Act are placeholders.
4. **How a death is shown.** Permadeath is on. What the player sees when a
   cadet dies (the replay line, the after-action report, the interlude) is
   undecided, and the tone doc's "how lethal is it allowed to look" is still
   open.
5. **Card values as targets or modifiers.** The draft shows Nerve as a target
   and the rest as modifiers, ASL-style. One scale for all five may read
   better.
6. **Difficulty.** Recommended: a checkpoint ladder, measured against the
   teacher, with the strongest rung optional. The alternative is the existing
   difficulty noise and foresight on one strong policy.
7. **Enemy commanders' personalities.** Who sets each named commander's reward
   weights, and whether a bio has to be legible from her play before she
   ships.
