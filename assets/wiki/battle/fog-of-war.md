---
id: fog-of-war
title: Fog of War
category: Battle
---

# Fog of War

Every battle tile sits in one of three knowledge states for your side:

| State | Meaning |
| --- | --- |
| Unseen | Never observed; the map is blank here. |
| Explored | Once seen; terrain remains, but enemy units may have moved. |
| Visible | Currently in line of sight; enemy presence is up to date. |

Line of sight respects elevation and terrain height. Forests and towns
block sight; high ground sees over them.

Firing reveals a unit until it moves. Stay quiet in cover if you need to
vanish again.

Scouting is updated every tick of a [round](the-round.md), so what your
side knows changes while the round is still playing out. A unit that drives
into the unknown reports back as it goes, and the crews behind it act on
what it finds.

## Running into people

One unit per hex. An enemy in the way ends an advance: the unit stops on
the tile before them and gives up the rest of its route for the round.

Whether that counts as an ambush depends on who saw whom. Crews look
between ticks rather than between hexes, so a unit moving at a walk usually
spots the enemy a hex out and simply halts. A unit covering ground faster
than its own reconnaissance can drive straight into somebody — that is a
genuine ambush, and it is the price of pushing hard through fog.

The move overlay will not warn you. Tiles holding hidden enemies still look
like ordinary destinations, because a refused order would tell you exactly
where the enemy is standing. Scouting is the only honest way to know.

A friendly unit in the way is only traffic: it will probably have moved on
by the next tick, so the unit behind waits and follows.
