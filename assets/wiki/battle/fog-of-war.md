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

## Ambushes

You may drive straight through your own units, but an enemy you have not
spotted stops the advance: the unit halts on the tile before them and its
turn ends.

The move overlay will not warn you. Tiles holding hidden enemies still look
like ordinary destinations, because a refused order would tell you exactly
where the enemy is standing. Scouting is the only honest way to know.
