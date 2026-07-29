---
id: commanders
title: Commanders
category: Battle
---

# Commanders

Every side can be human-controlled or driven by an AI planner. Planners
only see what their side's fog allows — they do not cheat.

## Included planners

| Planner | Behaviour |
| --- | --- |
| Utility | Scores candidate orders; difficulty adds noise to the score |
| MCTS | Searches over a cloned, determinized copy of the battle |

Assign a planner per side in map JSON:

```json
"ai": { "planner": "mcts", "difficulty": 4 }
```

Difficulty is planner-specific. On the utility planner it mainly controls
how noisy scoring gets; higher values make play less optimal and more
erratic.
