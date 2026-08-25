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
| R | Open / close the academy roll | Reconnoitre (see below) |
| 1–9 / M / N | Toggle / add all / drop all reinforcements | — |
| Q / E | Rotate the view (6 perspectives) | Same |
| WASD / arrows / MMB drag | Pan camera | Same |
| Wheel | Zoom (whole-pixel steps, 1×–4×) | Same |

Selecting an army tints every tile it can reach this day and marks enemies
it could engage in red. Moving onto an enemy army starts a battle;
survivors and casualties persist back to the campaign.

**The academy roll** (`R`) lists your whole school in order of battle —
vehicle by vehicle, seat by seat — with each cadet's availability and how many
battles she has behind her. Anyone whose vehicle did not come home is on it
too, under *Without a vehicle*. It is a page to look at rather than a prompt
to answer: `R` or `Esc` closes it and nothing happens on the map while it is
open.

## Giving a formation its orders

Press `F` in battle to pick a formation, then a verb on the hovered hex. Each
key says what it commits the platoon to, in the panel, before you press it.

| Key | Order |
| --- | --- |
| G | Advance — take it, halting to fight what shoots |
| X | Assault — take it through fire, and pay for it |
| H | Hold — stand fast and hold what you have |
| R | Reconnoitre — find them without getting pinned |
| W | Withdraw — break contact and leave the field |

Two modifiers qualify the order rather than changing it, and they compose:

- **Shift** queues the order behind the last one instead of replacing it —
  "…and then this". Nothing may be queued behind a hold or a withdrawal.
- **Ctrl** means it. Without it, how closely a formation holds to the order is
  its own doctrine's business, and a doctrine that devolves reads an order
  loosely on purpose. With it, she holds to the letter of it whatever her
  doctrine prefers.

Ctrl is a different question from the verb, and both are worth asking.
*Assault* says she presses on through fire; *Ctrl* says her doctrine may not
discount the order in the first place. A crew has the same distinction one at
a time: select her and press `X` on a hex to send her there through fire.

## Saving

On the campaign map, **F5** saves and **F9** loads. There is one slot, at
`saves/campaign.json`.

A save restores the campaign exactly, including the random number stream, so a
reloaded game plays out the way the unsaved one would have — the same battles
roll the same dice. Cadets keep their history: battles fought, experience, and
any wound with the days still left on it.
