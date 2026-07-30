---
id: commanders
title: Commanders
category: Battle
---

# Commanders

Every side can be human-controlled or driven by an AI planner. Like you, a
planner writes orders during the planning half of a
[round](the-round.md) and then lives with them. Planners only see what
their side's fog allows — they do not cheat, and that includes your orders:
a planner that searches ahead has to guess what you will do, exactly as you
have to guess about it.

An opponent is described by three independent things:

| Axis | Question it answers | Where it lives |
| --- | --- | --- |
| Planner | *How* does it think? | Engine code, chosen by name |
| Doctrine | *What* does it value? | Mod data, `doctrines/*.json` |
| Difficulty | *How well* does it execute? | A number, 1–5 |

Keeping them apart is what lets two academies feel like different armies
rather than the same army at different skill levels. A clumsy massed-armour
opponent still comes at you; it just does it badly.

## Planners

| Planner | Behaviour |
| --- | --- |
| Utility | Scores every tile a unit could hold and takes the best. Cheap and predictable. |
| MCTS | Searches ahead over a cloned, fog-honest copy of the battle, standing in for the enemy with a utility planner. |

## Doctrines

A doctrine is a set of weights describing a fighting style: how much
aggression, how much cover and high ground are worth holding, whether to
mass or spread out, how readily to advance into unscouted ground, how
freely to spend artillery, and how much damage to take before pulling back.

The base game ships three:

| Doctrine | Character |
| --- | --- |
| `massed_armor` | Advances on a broad front and trades fire. Ground is taken by weight of steel. |
| `elastic_defense` | Cedes ground, fights from cover and high ground, would rather shell an advance than meet it. |
| `recon_pull` | Leads with fast, cheap eyes and commits weight only where they find a way through. |

## Assigning one

Per side, in map JSON:

```json
"ai": { "planner": "mcts", "difficulty": 4, "doctrine": "massed_armor" }
```

`doctrine` may be omitted for a balanced default. Adding your own is a
matter of dropping a file in `doctrines/` — see
[Modding](../reference/modding.md).

Difficulty is competence only: on the utility planner it adds noise to
scoring, and on MCTS it sets how long the search thinks. It never changes
what a side is trying to do.
