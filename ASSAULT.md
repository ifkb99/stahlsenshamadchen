# The attack and the defence

The design memo for attack-and-defend battles, started 2026-10-16. A
defender holds a few spread-out zones and the attacker wants most or all of
them. On the attack, massing of forces and tactics are tested; on the
defence, coordination and counterattacks. It is the battle REIMAGINED.md's
L3 finding asked for (*The commander*): over the teacher's crews, on open
ground where nothing punishes an unco-ordinated fight, no fixed policy of
orders beat saying nothing, and a learned commander capped near 63% against
a coin. A battle that needs a plan is the test of whether the 90/10 gap the
designer is shooting for can exist at all.

## The designer's rulings (2026-10-16)

- **A classic attack and defence**: defenders hold a few somewhat spread-out
  zones; attackers want most or all of them.
- **No clock.** "A CO wouldn't call off a winning assault because a few hours
  had passed." A battle ends the way real ones do — a side gives up, breaks,
  or is wiped out — and the design was workshopped from there (below).
- **Prepared defences, mines and obstacles**, "if it's cheap".
- **Equal points to start.** Each side buys to the same budget; the designer
  expects this to fail somewhat by doctrine and wants the failure measured.
- **A mix of larger and smaller battles**, for variety in training.
- **Real battles to compare against**, modern enough to mesh with the
  equipment (late-war to 1973, no ATGMs yet).

## What the research said

Three reports, kept in the session record; the sources they cite are named
here so a claim can be followed up.

**How engagements end (Clausewitz; the quantitative record).**
- *On War* IV.4: "In the combat the loss of moral force has been the chief
  cause of the decision"; victory is the enemy's greater loss of physical
  and moral power **and "his open avowal of this by the relinquishment of
  his intentions"** — not his annihilation. IV.9: "The final decision is
  ruled mostly by the relative number of the fresh reserves remaining at the
  last"; lost ground and the want of fresh reserves are "usually the
  principal causes which determine a retreat".
- VI.1 and VI.5: defence is the stronger form, with a negative object, and
  "a sudden powerful transition to the offensive — the flashing sword of
  vengeance — is the greatest moment for the defense". VI.9: a fifth to a
  quarter of the whole kept back out of the battle, to open "a smaller
  offensive battle on his own part" once the assailant's plan is developed.
  VI.22: a cordon "can only have a very small degree of defensive stamina".
- **Units quit long before they are gone.** McQuie (1987, 80 engagements):
  "in most cases, a force has quit when its casualties reached less than ten
  percent per battle"; median attacker under 4%, defender under 8%. Clark
  (1954, 43 battalion breakpoints): attacks failed at 7–48% (average 26%),
  defences withdrew at 37–69% (average 52%), cumulative over days. HERO
  Report 117 (1988): attacker halts at 5.0% (regimental) or 16.6%
  (divisional), defender withdraws at 17.1% or 39.8% — and those are its
  *third and fourth* checks; the first is geometric (below).
  Every source agrees **the attacker gives up at a half to a third of the
  losses the defender takes before she withdraws**.
- **Manoeuvre, not casualties, ends most fights.** McQuie's 52 reasons a
  force broke off: enemy manoeuvre 64% (envelopment, encirclement or
  penetration 33; an adjacent unit withdrawing 13; surprise 8; key terrain
  lost 6; enemy reinforced 4), firepower 12%, no reserves left 12%, other
  12%. **Corrected** (*Breaking by manoeuvre*, below): the 64% is a
  grouping, and McQuie's own unpublished grouping of the same reasons gives
  less; the robust claim is that **losses were the stated reason about one
  time in ten**, and envelopment, encirclement or penetration about one in
  three. Clark's sample has no collapses because she dropped her five
  collapse cases; the Helmbold line quoted here before could not be
  checked.
- **Our battles are one to two orders of magnitude too lethal** for those
  numbers: a battle here is over in tens of minutes, a historical attack in
  hours. A break point tuned to history has to be read in the game's own
  loss rates (below), not copied.

**Doctrine for the attack and the defence.** FM 100-5 (1941) par. 651: "the
counterattack is the decisive element of defensive action"; par. 663: local
counterattacks "must be launched during the period of temporary confusion
… This period is relatively short." FM 3-90 (2001): area against mobile
defence, the reserve sized by uncertainty (a ninth in Soviet practice to a
third in American), engagement areas built in seven steps, obstacles that
disrupt, fix, turn or block and are always tied to fire. Concentration:
3:1 at the point of attack, 4–6:1 in Soviet breakthrough norms, with most
WW2 attacks in fact launched at 1.0–1.5:1 and won 63% of the time.
FM 20-32's minefield bands: disrupt 40–50% chance a crossing meets a mine,
fix 50–60, turn 75–85, block above 85. At Kursk most mine casualties were
broken tracks, repaired in days, crews unhurt. Dupuy's posture factors:
hasty defence 1.3, prepared 1.5, fortified 1.6.

## The ending, workshopped

The designer asked for the ending to be how it works in real life; the
proposal, built:

1. **A side breaks** past the share of its strength it can lose
   (`BreakPoint`), strength being substance aboard over the full complement
   the side brought. The attacker's is lower than the defender's.
2. **A side is gone**, or has driven off by its exits (an ordered
   withdrawal; an exited crew counts as out of the fight).
3. **The attack has stalled**: the stalemate clock, which already counted
   only accomplishment, also counts an assault's progress — ground changing
   hands, or an attacking crew getting closer than any had to ground she does
   not hold. An attack still closing in has not stalled; one that has stopped
   getting anywhere is called off, which is how real COs ended failing
   assaults and never winning ones.

Then the ground decides. **A defender who has broken has given up her
ground**, whatever she was standing on. Otherwise the attacker wins if she
holds `needs` of the zones — an attack called off keeps what it took — and
the defender wins if she does not. The running score still accrues and
decides nothing.

What this leaves for later, and the research says matters most: break
pressure from **manoeuvre** (a formation cut off, a neighbour withdrawing,
key terrain lost), the **reserves** rule (a side whose fresh force is weaker
than the enemy's known fresh force breaks sooner), and break by **rate**
rather than total (Clark's acceleration). Surrender, rare and contagious
(Lehmann and Zhukov), only under all of cut off, leaderless and broken.

## The rules, as built (2026-10-16)

Each additive; a scenario that declares none of it is the game before, and
the determinism snapshot passed unregenerated. Tests: `tests/assault.rs`,
mutation-checked.

| rule | data | shipped |
| --- | --- | --- |
| ground held at the start | `ObjectiveSpec::held` | — |
| an assault and its verdict | `Assault { attacker, needs }` | — |
| breaking | `BreakPoint { side, lost_percent }`, `EndReason::Broken` | sectors: attacker 50, defender 70 |
| an attack still closing in | the stalemate clock, `Battle::approach` | — |
| prepared positions | `VehicleDef::prepared_cover`, `Scenario::prepared` | infantry 60, tanks 40, others 30 |
| mines | `MineDef` (`mines/`), `Minefield` on the scenario | anti-tank, anti-personnel |
| the way round a known field | `balance.mine_route_cost` | 8 |
| obstacles | terrain `wire`, `tank_ditch` | foot 3; no hull crosses a ditch |
| fog | `VisibilityPhase { from_round, hexes }` | — |
| orders given before the battle | `StandingOrder { side, mission }` | sectors: the defender holds |
| relief past ten levels | elevation `a`-`z` (10-35) | — |

- **A prepared position is deeper than a scrape** and the round's digging
  does not fill it in, and like any hole it stays behind when she moves:
  that is what a counterattack costs a defender. A prepared side's riflemen
  start dismounted, not inside a hull one shell empties.
- **A mine is a shell from below.** A strike stops her, takes her running
  gear if the mine says it does, and spends its damage through the same
  effect rolls a penetration does; her own side never sets one off. The
  side, not the fog, knows where a field is, once one has gone off in it —
  and every road, step and goal reads only that, so no road gives away what
  nobody has found.
- **Fog is a cap on sight**, through one door (`BattleState::sight`), and
  the fog's vision cache is keyed on range, so it lifts the round the phase
  says.
- **A standing order is on the formation when the battle opens.** Every
  other mission travels the net and lands a round late, and in that round a
  defender's crews, choosing for themselves, scattered to every zone their
  side held — at Noville four Shermans and both M7s set off for Foy, two
  kilometres behind them. A defence's first order is given before the
  attack.

## The defended sector (`harness::sector`)

A generated sector for training: the defender's zones are the best key
ground — a village, a crossroads, a rise, a wood's edge, read off the tiles
— in lanes across a frontage wider than she can cover, so she must choose
between breadth and a reserve; the attacker starts kilometres off on the far
side. Three sizes:

| size | front | zones | standoff | budget each | vehicles each |
| --- | --- | --- | --- | --- | --- |
| small | 1.2 km | 2-3 | 1.4-2 km | 40-60 | ≤ 8 |
| medium | 2 km | 3-4 | 1.8-2.6 km | 70-100 | ≤ 14 |
| large | 3.2 km | 4-5 | 2.2-3.2 km | 120-170 | ≤ 26 |

Three fields in four are a prepared defence, dug in behind mines (densities
from FM 20-32's disrupt-to-turn bands), wire before a zone and, on the
larger sizes, a ditch across the open ground in front of the line with the
roads left as its crossings. The defender keeps a quarter of her platoons
back as a reserve. The attacker needs `n - n/3` zones. Side 0 attacks;
a rollout seats the learner on each side in turn.

`rollout --ground sector --sizes small,medium,large`;
`examples/assault <field> --size S` narrates one, `--survey N` stages N.

### Measured: the teacher on both sides

`rollout --net utility --deterministic`, 192 battles a row, the attacker's
wins (134 prepared, 58 hasty):

| size | commanders | all | prepared | hasty |
| --- | --- | --- | --- | --- |
| small | none | 34% | 35% | 33% |
| small | hand-written | 29% | 28% | 31% |
| medium | none | 35% | 28% | 53% |
| medium | hand-written | 27% | 20% | 43% |
| large | none | 29% | 24% | 40% |
| large | hand-written | 32% | 27% | 45% |

At equal points the defence wins about two battles in three, and more
behind a prepared position: Clausewitz's stronger form, with the attacker
not bringing the superiority doctrine asks for. 147-184 of every 192 end
with a side breaking. 192 large battles take 14 s.

## Real battles

`tools/make_historical_maps.py` draws them: the relief **measured** (EU-DEM
25 m for France and Belgium, produced using Copernicus data; SRTM 30 m for
the Golan; sampled at 250 m into `tools/history/`, read bilinearly at every
hex), everything else **drawn from the histories** rather than copied from a
modern map, because the ground of 1944 is not today's. The forces are
scaled and our chassis are not theirs, so what can be compared is the shape
of the fight and how it ended.

- **`history_noville`** (radius 20): Team Desobry in Noville, 19 December
  1944, the fog lifting at 10:30 (Cole, *The Ardennes*; Marshall,
  *Bastogne*). Noville in its dish, the ridges north-west and south-east,
  the soft low ground, no mines. 16 tanks, 4 grenadier platoons and 2
  batteries against 11 Shermans, 4 M18s, 2 M8s, 3 armoured-infantry
  platoons and a battery, dug in. *History: the attack of the 19th ebbed;
  Noville was given up by order on the 20th.*
- **`history_valley_of_tears`** (radius 30): 7th Armored Brigade's ramps
  between Hermonit and Booster, October 1973, the ditch and its mines in the
  valley. One assault wave of 30 tanks and 6 carriers against 15 tanks on
  the ramps. *History: the Syrians reached Booster's crest; the attack was
  called off.*
- **`history_arracourt`** (radius 32): Combat Command A against
  Panzer-Brigade 113 in the fog of 19 September 1944 (Cole, *The Lorraine
  Campaign*). 24 Panthers in three company jabs against 20 Shermans, 5
  Stuarts and 4 M18s, nobody dug in. *History: the brigade's attack broke up
  and fell back east.*

`examples/assault --map <id> --games N [--commander utility]`. Hill 112 is
researched (`tools/history/h112_elevation_250m.csv`) and waits for richer
infantry and artillery; it was an infantry-and-guns set piece.

### Measured: the teacher on the real battles

64 battles a row, `massed_armor` attacking `elastic_defense`, no commander,
the attacker's wins:

| battle | history | first cut | as shipped |
| --- | --- | --- | --- |
| Noville | the attack ebbed | 66% | **14%** (standing hold) |
| Arracourt | the attack broke up | 17% | **17%** (no standing order) |
| Valley of Tears | the attack was called off | 62% | **62%** (no standing order) |

The first cut gave neither defence an order before the battle, and the
narration of Noville showed the defenders out of their prepared positions in
round one, before any contact — 20 of 25. **The first diagnosis was that
they were leaving their holes, and the measurement says it was the
scattering.** Two rules that kept each dug-in crew in her own hole (the hold
order anchored on her hole, her hole counted by the terrain prior) were
built and measured and reverted: with both, Noville's attacker won 30%;
with neither, and a standing hold that closes each platoon on its leader in
the village, 14%. A concentrated defence beat one spread over its holes.

The standing order is the wrong one for the other two, and that is history
too: Combat Command A won Arracourt by manoeuvre, racing for the Bezange
ridge and swinging companies into the German flank (17% to 55% with a
standing hold), and the Valley of Tears was held from a line of ramps,
every tank on her own, which "hold where you are" — close on your leader —
pulls them off (62% to 86%). **The order the 7th Armored Brigade actually
had is one the vocabulary cannot give yet.** And even on their ramps the
Israeli tanks trade one for one with the attacker (in one battle: 6 lost to
tanks, 3 to artillery, for 6 Syrian tanks, and 12 mines struck); a prepared
hull-down position here is worth about one attacker, where in 1973 it was
worth several, which is a question for the cover and gunnery numbers.

### Measured: fixed orders over the teacher's crews

`rollout --ground sector`, all sizes, 576 battles, a fixed policy for the
learner's commander against the opponent mix, the learner's wins (96 a cell):

| policy | attacking vs none / coin / hand-written | defending vs none / coin / hand-written |
| --- | --- | --- |
| say nothing | 26 / 51 / 48% | 72 / 55 / 62% |
| hold the nearest own zone | 20 / 32 / 22% | 69 / 58 / 54% |
| hold where you stand | 2 / 10 / 8% | 68 / 62 / 60% |
| assault the nearest zone | 18 / 41 / 21% | 58 / 61 / 58% |
| mass on one zone | 18 / 31 / 21% | 44 / 50 / 51% |
| spread over the zones | 18 / 39 / 24% | 60 / 61 / 59% |

(Measured before standing orders.) The same finding as on open ground
(REIMAGINED.md, *The commander*): **over the teacher's crews, every order
costs the attacker**, massing included, and no order buys the defender
anything over choosing for herself. An order replaces a crew's candidates
with the commander's ground, and the crews carry it out worse than they
choose. With a standing hold for the defender the sector baseline barely
moves (the learner attacking 38-43%, defending 56-62%, 576 battles).

### The learned commander on sectors

`loop.py --train commander --ground sector`, all sizes, from a fresh network
standardised on 18,614 uniform decisions, 60 iterations of 1,536 battles,
teacher crews, against the mix of commanders (`target/rl/sector1`; run on
binaries frozen before standing orders). Her best choices on 576 battles no
rollout reached, every five iterations:

| iteration | vs coin | attacking vs coin | defending vs coin | vs none | vs hand-written |
| --- | --- | --- | --- | --- | --- |
| 0 | 45% | 28% | 62% | 37% | 41% |
| 15 | 56% | 42% | 71% | 52% | 56% |
| 30 | 52% | 41% | 64% | 46% | 48% |
| 55 | 54% | 39% | 70% | 47% | 51% |
| 60 | 47% | 29% | 66% | 49% | 48% |

Her rollouts went from 48% to 50% won. **She learned to stop hurting and
never learned to help**: a battle that needs a plan does not, by itself,
make orders worth giving, because the crews carry orders out worse than
they choose (the fixed-order table above). The 90/10 the designer is
shooting for is not reachable from this seat while that holds. What would
change it, in the order the measurements point:

1. **Crews who carry orders out well** (REIMAGINED.md L2.1): train the crews
   under orders, so that an order's ground is fought for as well as ground
   they chose. Every fixed policy here loses because the executor is worse
   at the commander's ground than at its own.
2. **Orders worth having**: the reserve as a thing a commander holds back
   and commits, and orders that name ground rather than a hex. **Ruled
   (2026-10-16): an order stays general** — "a general doesn't tell a squad
   where to stand, that's the NCO's job" — so "every crew in her own
   position" is not an order; it is something crews under "hold" must learn
   to do. `planner.order_ground` (below) is the first half.
3. **Breaking by manoeuvre** (above): while the only way a side breaks is
   its losses, the decisive thing in real engagements — being enveloped,
   cut off, outflanked — wins nothing a commander can plan for.

## Breaking by manoeuvre: the record checked (2026-10-16)

The designer's reading of the third recommendation: "needs to be explored
more but sounds directionally correct". A research pass over the primary
sources; nothing built.

**What the record says.**

- **McQuie (*ARMY*, Nov. 1987)**, 52 coded reasons for breaking off:
  envelopment, encirclement or penetration 33%, a neighbour withdrawing 13,
  no reserves left 12, casualties or equipment lost 10, surprise 8, key
  terrain lost 6. One reason per engagement, chosen from narratives —
  reasons people gave, not measured causes.
- **HERO Report 117, *Forced Changes of Combat Posture* (1988, for the
  Army's Concepts Analysis Agency; DTIC AD-A201405)**, 83 US engagements,
  built a breakpoint model for simulations that checks, in order: **how
  deep the attacker has got over the width of the front** (the defender
  withdraws at 2.0 regimental, 1.59 divisional), the force ratio, the
  attacker's losses, the defender's losses, the casualty ratio. About 80%
  matched, in sample. Its veterans' groups: enemy manoeuvre ("they were
  moving in behind us") present in 6 of 8 operations and **decisive in all
  6**; casualties decisive in 2. **An attacker with the reserve advantage
  forced the defender off in 14 of 14** divisional engagements; one with
  surprise, 10 of 10 regimental.
- **Dupuy's interim rule** (in HERO 117): three of four conditions change
  posture — causes combine; one fact moves a side toward breaking, it does
  not break it.
- **Clausewitz IV.4**: "the rear being endangered makes defeat more
  probable, and, at the same time, more decisive". **Lehmann and Zhukov
  (2019)**: surrender is contagious from battle to battle.
- Unchecked: Rowland's *Stress of Battle* figures; Helmbold R-772-PR beyond
  its abstract.

**What carries over.** The ratio (an attacker quits at about half the
losses a defender takes), not the level: our battles are minutes, history's
days. The mechanism is geometric — enveloped, penetrated, cut off; a
neighbour going; key ground lost; surprise; the reserve balance — and losses
are a minority cause. A side's will should read what her commander has been
told (`Knower::Commander`), a crew's nerve what she knows.

**Where the engine stands.** The crew level is mostly built (`cut_off`,
flank shock, `leader_lost`, `ally_destroyed`). Nothing of it reaches the
side: `strength_lost` counts substance and exits, so a crew cut off or
running is full strength to her side, and a platoon breaking is the AI's
private opinion (`beaten`), not a rule — "the unit beside us withdrew"
cannot happen.

**Options, smallest first**, each data with a neutral value:

- **A — what a side can count on.** A crew cut off, or running, counts as
  partly lost toward her side's break (`BreakPoint` gains two percents),
  as a crew who left by an exit already counts in full. The board-game
  rule: a unit with nowhere to go is lost to its side.
- **B — what a crew notices.** New pressure on the ladder: *bypassed* (a
  known enemy nearer her line of retreat than she is), *a neighbour
  withdrew*, *surprise* (a shot from a gun her side had not found),
  *isolated*, *a friend seen to run*.
- **C — a side's will.** The break point reads a meter: losses, plus A's
  shares, ground she held and lost, how much of her is bypassed, her
  commander lost, and the reserve balance (fresh crews against the enemy's
  known fresh crews); with a *why it broke* tally in `examples/assault` to
  set beside McQuie's shape.
- **D — platoons break**, as an engine event: a formation past its own
  point is withdrawn by the engine, which charges its neighbours, so a side
  breaks through its platoons. The largest, and it would be the engine
  issuing a binding order — a ruling.

**Recommendation**: A, then B's *bypassed*, *neighbour withdrew* and
*surprise*, measured on the three real battles (Noville first: Foy behind
the defence is a line-of-retreat case) and the sectors; C's ground and
reserve terms once A and B show a side breaking by position at all; D last.
Pre-registered: with teacher crews Arracourt should not move (the teacher
strikes from the front 78% of the time, so nobody goes round anybody), and
the real test is the learned commander retrained with them on.

**Built: A and B** (the designer agreed the recommendation; D is still
being thought about). Every number is data and every default is the game
before — the determinism snapshot passed unregenerated.

- **A**: `BreakPoint` gains `cut_off_percent` and `unnerved_percent`, and
  a side breaks on `BattleState::will_lost` — strength lost, with a crew
  cut off or unnerved counted as that share of her lost, the larger if
  both. Whether a crew is cut off is asked once a round
  (`Unit::cut_off`), so the tick's victory check reads a field.
  `SectorRules` and `rollout`/`assault --counts C,U` set it; the
  commander's break-margin features read the will, not the strength.
- **B**: `morale.surprise_percent` (a shot from a gun her side had not
  found when the tick opened, hit, bounce or miss; 100 neutral),
  `morale.bypassed` (a gun that can reach her stands nearer her side's
  rear: its exits, else the squared-distance centre of where it stood at
  the start, `BattleState::rear_distance`), `morale.neighbour_withdrew`
  and `neighbour_within` (a formation of her side within reach receives
  an order to withdraw). `examples/assault` takes `--set` and narrates
  each side's cut-off and unnerved crews and its will beside its losses.

*Measured, teacher crews:*

- **Unnerved cannot be counted as it stands.** The shipped ladder puts a
  crew on its non-obeying rung at 12 pressure, and under the shipped fear
  content 19 to 22 of Noville's ~30 attacking crews are there by round 2 —
  a crew going to ground, not a collapse. At `--counts 50,50` every one of
  64 Noville battles broke on the count, in 2.8 rounds. Before
  `unnerved_percent` is worth a value it wants to read flight, or the rung
  held for rounds.
- **Cut off decides almost nothing**: at `--counts 100,0` Noville is 9 of
  64 to the attacker either way (two battles broke on the count), and 576
  sectors read 290 and 291 learner wins with identical endings. The
  teacher strikes from the front and goes round nobody, so nobody is cut
  off: the third species of null, a rule nobody uses yet.
- **B**, Noville, 64 each, attacker wins: baseline 9; surprise 200% 7;
  bypassed 3 a round 10; neighbour 4 identical (no commander orders
  anybody out); everything at once 6. Sectors, 576, attacker 128 of 288
  and with everything 117 — toward the defender, the side that fires from
  where nobody has found her, and inside noise.

So the rules are in and the crews do not yet give them anything to read.
They are for the learner: crews and a commander trained with them on are
the experiment, and a "why it broke" tally beside McQuie's shape (option
C) the instrument.

Sources: McQuie's table as the Dupuy Institute reprints it
(dupuyinstitute.org, 2017-06-23); HERO 117 (generalstaff.org's DTIC scan);
Clark via HERO 117 App. C; Clausewitz, *On War* IV.4 and VI.4 (Graham);
Lehmann and Zhukov, *International Organization* 73(1), 2019.

## Open

- The learned commander (L3) trained on sectors, both roles, measured
  against orders by coin per role: the experiment this was built for
  (below, *The learned commander on sectors*).
- **Every crew in her own position** — a line of ramps, a belt of
  trenches — is not an order (ruled 2026-10-16); crews under a general hold
  must learn it (*Crews under orders*, below).
- Break pressure from manoeuvre and from reserves (above).
- Pre-registered fire: a defender's artillery lands on the ground she
  surveyed without adjusting (FM 3-90's target reference points; NTC found
  unregistered planned fires hit about a fifth of the time).
- Breaching: nothing but driving through clears a minefield, and nothing
  fills a ditch. The Syrians crossed on bridgelayers and causeways.
- Smoke is still fired by nobody; the breach drill (suppress, obscure,
  secure, reduce, assault) wants it.
- The hasty and prepared defences differ only in the work done; a prepared
  defender who also knew the ground (surveyed ranges) would be the rest of
  Clausewitz's "assistance of ground".
