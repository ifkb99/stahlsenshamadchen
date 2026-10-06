---
id: combat
title: Combat
category: Battle
---

# Combat

Shots resolve through accuracy, then penetration against armour.

## Hit chance

Hit chance comes from:

- The weapon's base accuracy and range falloff
- The crew's gunnery
- Elevation advantage
- Target cover

Nothing is certain: the engine clamps hit chance between 5% and 95%.

## Damage and armour

Damage compares weapon penetration to the armour facing struck — front,
side, or rear, decided by the attacker's direction relative to how the
target is facing. Terrain cover and elevation also reshape the result.

## Fire modes

| Mode | Requirement |
| --- | --- |
| Direct fire | Line of sight to the target |
| Indirect fire | A friendly spotter who can see the target |
| Blind fire | Always available; fires at a tile with an accuracy penalty |

Blind fire (`B`) is how you shell a suspicious forest without waiting for
a spotter — or when you are the only eyes left.

## Rate of fire

Every weapon reloads on its own clock, counted in ticks of the
[round](the-round.md). A machine gun chatters through a round; an 88
manages a couple of aimed shots; a howitzer gets one. A unit with several
weapons reloads them independently, so a tank can keep its coaxial gun
talking while the main gun is being loaded.

## Opportunity fire

Return fire is not a special case. Any crew with a loaded weapon, a target
in range, and line of sight will shoot during the same tick — including a
crew that spent the round driving, and a crew you gave no orders at all.
Being shot at is simply the most common reason to find a target.

Because a tick resolves every shot before counting the dead, two crews can
kill each other. Nobody shoots first.
