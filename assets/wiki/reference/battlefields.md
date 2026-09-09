# The battlefields, and what each end of them is worth

*Written 2026-08-26, once `balance --only ground` could measure it. This is the
label on the maps rather than a plan to change them: a battlefield favouring
one end is strategy, and a scenario where one side holds the ridge is a
scenario about holding a ridge. What it must not be is unmeasured, because
`balance --sim` samples all of these and every number it prints then carries
the term.*

*Extended 2026-09-09 with `battle_hills` and `battle_town`, so that a campaign
clash in the mountains or in a city is fought on its own ground. There are
five battlefields now, and the fought-out sample is a fifth of each rather
than a third.*

## How the numbers are made

Each map is fought both ways round: `games` battles as it ships, and `games`
more **with the two armies exchanged between the ends**. Placements stay
exactly where the map put them and only the vehicles standing on them swap.
Summed that way two independent things fall out of one sample:

- **ground** — which *end* won, with the armies cancelled.
- **force** — which *order of battle* won, with the ground cancelled.

Both sides get the same doctrine and the same difficulty, so nothing else is
in the number. Crews are anonymous, because a crew named for a medium tank
cannot follow it into a light one; that makes the casualty figures
incomparable with the fought-out pass, and the winner comparable.

Re-measure with:

```sh
cargo run --release -p tactics_core --example balance -- \
    --sim --games 36 --only ground --absolute --sweep seed=0,1000,2000,3000
```

## Measured, 8 seeds × 36 battles = 576 per map

| map | ground (west share) | | force (side 0's army) | |
| --- | --- | --- | --- | --- |
| `battle_forest` | 45.8% | −2.0 sd | 50.0% | ±0.0 sd |
| `battle_plains` | 55.6% | +2.7 sd | 50.0% | ±0.0 sd |
| `river_crossing` | 49.5% | −0.2 sd | **24.1%** | **−12.4 sd** |

The **force column of the first two maps is the table's own control**: those
maps ship *identical* orders of battle, so it has to read level whatever the
ground does, and it reads 288–288 exactly. A build where that column drifts
has a bug in the exchange rather than a finding about the content.

## The two generated battlefields, 4 seeds × 36 = 288 per map

*Measured 2026-09-09, after the currency work; read against the same-run rows
for the older maps rather than against the August table above, which predates
several changes to how the AI moves.*

| map | fought for | ground (west share) | | force |
| --- | --- | --- | --- | --- |
| `battle_hills` | `mountains` | 148–140, 51.4% | +0.5 sd | 144–144 |
| `battle_town` | `city`, `factory` | 156–132, 54.2% | +1.4 sd | 144–144 |
| `battle_plains` | `plains`, `highway` | 148–140, 51.4% | +0.5 sd | 143–143 |
| `battle_forest` | `deep_forest` | 82–206, 28.5% | −7.3 sd | 144–144 |
| `river_crossing` | — (the baseline) | 163–124, 56.6% | +2.2 sd | **63–224** |

**`battle_hills` and `battle_town` are the first shipped battlefields whose
ground is *exactly* mirror-symmetric** — every hex's terrain and elevation
equals its reflection's, and so does every placement. The hand-drawn maps are
not: `battle_plains` has 40 hexes that differ from their mirror image,
`battle_forest` 28 and `river_crossing` 960. That is what makes these two
rows worth reading twice. A perfectly symmetric map fought by two mirrored
forces has *nothing* in the ground for the west column to find, so whatever it
reads is the engine's own residual compass bias — the coordinate this project
allows to go last in a tie-break, which is a coin that always falls the same
way. At ±0.5 and ±1.4 sd there is not much of it left, which is the useful
result.

`tools/make_battle_maps.py` is what makes the symmetry a property rather than
an intention: every feature is written as a function of `a = |2·col − (40 −
row % 2)|`, the doubled distance from the map's vertical axis, and the script
asserts the whole grid against its own mirror before writing. **Regenerate;
do not edit the json.**

## What the ridge cost to get level

`battle_hills` took three drafts, all of them symmetric, and the win column
moved five standard deviations across them:

| draft | ground | |
| --- | --- | --- |
| two wide elevation bands, one straight crag bar across the approach | 186–102 | **+4.9 sd** west |
| a stepped ridge climbing to elevation 3, crags broken into knots | 98–190 | **−5.4 sd** east |
| the same ridge capped at elevation 2, smaller knots | 148–140 | +0.5 sd |

None of those three maps is unfair — all are exact mirrors — so the whole
swing is the engine's tie-breaks being resolved more or less often, and in
which direction, by how much ground the map leaves at exactly one value. The
first draft's wide bands are wide *plateaus of identical score*; the second
draft's steep narrow ridge funnels every crew through the same few hexes.
The lesson for the next generated map is the same one twice: **broken ground
is ground the engine has to decide about on its merits**, and a map that
leaves large expanses tied hands those decisions to a compass.

## What each row says

**`battle_forest` leans very slightly east** and **`battle_plains` very
slightly west**, both around two to three standard deviations — real, small,
and the sort of thing a wood on one flank does. Neither is worth changing.

**`river_crossing`'s ground is level and its armies are not.** Side 0 fields
artillery where side 1 fields a tank destroyer, and side 1 wins **436 of 576**
— a 24%/76% split that is nothing to do with the river. This is the one to
know about, for two reasons. It is the determinism baseline, so it is fought
in every snapshot; and the fought-out pass samples it alongside the other two,
so the doctrine table has been reading a three-map average in which one map
hands one side a large head start. It is not a bug — a scenario is allowed to
be uneven — but any conclusion drawn from `--sim` about *doctrines* is partly
a conclusion about that tank destroyer.

**`battle_hills` — the Rauhkamm.** A ridge running north to south, which is
the only arrangement in which a hill is a thing to be *taken* rather than a
thing one side was handed: it sits between the two ends and both have to
climb it. Two humps at elevation 2 with a saddle between them where the road
crosses — the crest, worth 3, is on the northern hump and the saddle, worth 2,
is the one lane over the ridge that does not climb. Spurs of elevation 1 reach
out to both flanks for a commander who would rather go round, and four knots
of `mountains` on the humps' outer rims are the map's only impassable ground:
foot-only, small, and easily driven round, which is the difference between a
channel and a smaller map. Level ground, 51.4% west.

**`battle_town` — Immenrode.** A market town round a crossroads, with a hamlet
up the northern road on a slight rise. The junction, worth 3, is *on the
roads* rather than in a building, because `town` blocks sight at two levels
and a solid block of it would be an objective nobody inside can see out of;
the lanes are the sight lines and holding the junction means holding them.
Fields out to nine hexes, meadow beyond, woodlots on the field boundaries.
The hamlet is worth 2. Level ground, 54.2% west, and the widest seed spread of
any map here — a town fight turns on who gets into which building.

## The figures this replaces, and why

An earlier version of this measurement (2026-08-26, same day, 72 battles a
map) reported `battle_forest` favouring the **west** 50–22 and
`battle_plains` favouring the **east** 29–43. Both were wrong, in the two
ways this project keeps finding:

1. They were measured before `movement::step_toward` and the utility planner's
   plateau argmax stopped breaking ties on a *coordinate*. Smallest x is west,
   so every crew in the game edged west wherever the real keys tied, which is
   forwards for a side attacking west and backwards for one attacking east.
   Some of what looked like ground was that.
2. 72 battles cannot resolve a tilt this size. A level pairing at 72 wanders
   28–44, and 50–22 is one draw of a biased build read against no band.

The lesson is the standing one: sweep the seed, and re-measure after any
change to how the AI moves.
