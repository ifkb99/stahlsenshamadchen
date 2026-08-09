---
id: controls
title: Controls
category: Field Manual
---

# Controls

Input differs between the overworld campaign map and a tactical battle.
Camera controls are the same in both.

In battle you give orders during the planning half of a
[round](../battle/the-round.md); the round then plays out for both sides at
once. Orders can be changed as often as you like until you commit.

| Input | Overworld | Battle |
| --- | --- | --- |
| LMB | Select army / issue move | Select unit / order a move / engage an enemy |
| RMB / Esc | Deselect | Deselect / cancel |
| A | — | Engage the hovered enemy |
| B | — | Area fire at a tile (accuracy penalty) |
| V | — | Hold: stay put and shoot at whatever appears |
| C | — | Clear the selected unit's orders |
| Enter / T | End day (or confirm a muster) | Commit the round |
| 1–9 / M / N | Toggle / add all / drop all reinforcements | — |
| Q / E | Rotate the view (6 perspectives) | Same |
| WASD / arrows / MMB drag | Pan camera | Same |
| Wheel | Zoom (whole-pixel steps, 1×–4×) | Same |

Selecting an army tints every tile it can reach this day and marks enemies
it could engage in red. Moving onto an enemy army starts a battle;
survivors and casualties persist back to the campaign.

## Saving

On the campaign map, **F5** saves and **F9** loads. There is one slot, at
`saves/campaign.json`.

A save restores the campaign exactly, including the random number stream, so a
reloaded game plays out the way the unsaved one would have — the same battles
roll the same dice. Girls keep their history: battles fought, experience, and
any wound with the days still left on it.
