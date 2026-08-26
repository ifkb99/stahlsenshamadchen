# The battlefields, and what each end of them is worth

*Written 2026-08-26, once `balance --only ground` could measure it. This is the
label on the maps rather than a plan to change them: a battlefield favouring
one end is strategy, and a scenario where one side holds the ridge is a
scenario about holding a ridge. What it must not be is unmeasured, because
`balance --sim` samples all three of these and every number it prints then
carries the term.*

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
