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
  Report 117 (1988): attacker halts at 4–17%, defender withdraws at 14–40%.
  Every source agrees **the attacker gives up at a half to a third of the
  losses the defender takes before she withdraws**.
- **Manoeuvre, not casualties, ends most fights.** McQuie's 52 reasons a
  force broke off: enemy manoeuvre 64% (envelopment, encirclement or
  penetration 33; an adjacent unit withdrawing 13; surprise 8; key terrain
  lost 6; enemy reinforced 4), firepower 12%, no reserves left 12%, other
  12%. Helmbold: none of Clark's breakpoints was a collapse, each "a
  rational decision … to substitute one course of action for another".
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

64 battles a row, `massed_armor` attacking `elastic_defense`:

| battle | commander | attacker won | ended by | attacker lost | defender lost |
| --- | --- | --- | --- | --- | --- |
| Noville | none | 66% | broken 64 | 36% | 63% |
| Noville | hand-written | 64% | broken 64 | 36% | 62% |
| Valley of Tears | none | 62% | broken 44, stalled 20 | 37% | 56% |
| Valley of Tears | hand-written | 53% | broken 47, stalled 17 | 38% | 57% |
| Arracourt | none | 17% | broken 64 | 51% | 35% |
| Arracourt | hand-written | 31% | broken 58, stalled 6 | 48% | 40% |

**Arracourt comes out as it went; Noville and the Valley of Tears do not.**
The narration says why: the defenders' crews leave their holes. A crew
choosing for herself has no notion that she is defending, drives out of a
prepared position toward the enemy or the next place worth standing on, and
loses the cover that was the whole of the defence's advantage; at Noville
the Shermans died in the open round the village they were meant to hold.
That is the coordination a commander is for — an order to hold is the
lever, and whether a learned commander finds it is the experiment. It is
**not** fixed here by a rule that tells crews to stay, which would be naming
the tactic (REIMAGINED.md, *Tactics emerge*).

## Open

- The learned commander (L3) trained on sectors, both roles, measured
  against orders by coin per role: the experiment this was built for.
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
