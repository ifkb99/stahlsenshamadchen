# Playthrough Review

Played through the AI-vs-AI harness after reading the docs, the engine, and
the base mod content. Three battles, full narration:

| # | Map | Seed | Result |
| --- | --- | --- | --- |
| 1 | `river_crossing` (default) | 42 | 30 rounds, **Kuhlmann Academy wins** (Stalemate), score 28–0 |
| 2 | `battle_forest` | 7 | 15 rounds, **Kuhlmann Academy wins** (Stalemate), score 45–10 |
| 3 | `battle_plains` | 1234 | 20 rounds, **Kuhlmann Academy wins** (Stalemate), score 39–0 |

Both planners at difficulty 4: side 0 MCTS / `massed_armor`, side 1 utility /
`elastic_defense`. Reproduce with:

```sh
cargo run --release -q -p tactics_core --example playthrough <seed> [map]
```

Full logs from this pass were in `/tmp/stahlsensham/` (ephemeral — rerun the
commands above to regenerate them).

## Overall

A seriously built engine, and the identity lands: named cadets as crews,
morale rungs, a chain of command with radio latency, facing-dependent plate,
a penetration gate that makes a bounce a real thing. The narration is the
best part — "cadet #0 is OUT", "ammo rack destroyed", "crew abandons",
"command passes from X to Y". Combat reads like a story with legible cause
and effect. Most wargames tell you a tank died; this one tells you how,
through whom, and who is left to command.

## Fun — what works

- **The opening of game 1 is genuinely dramatic.** The howitzer kills 3 of 4
  enemy vehicles in the first 4 rounds. You instantly understand that
  artillery matters.
- **Nadja Orlov's tank destroyer in game 1 is a great antagonist.** She sits
  with her front on the gun, everything bounces, she shreds the one medium
  tank that dares close (opportunity fire, one shot), and survives the whole
  war at 57%. A character you would remember.
- **Morale shows up in the log** and is the system that gives the losing side
  a narrative (Wavering → Breaking → refusing to advance).

## Fun — what kills it (priority order)

**Re-measured 2026-08-25 before acting on any of it.** Game 1 reproduces
exactly, so nothing below has gone stale — but three of the five diagnoses
turned out to be wrong once the logs were counted rather than read, and the
annotations say where. Items 1 and 2's chain reaction are fixed; see DONE.md.

1. **Artillery target fixation.** *(Fixed. The diagnosis below is wrong in one
   important way: the AI re-evaluated every single round. `round_worth` priced
   blast as a flat fraction of its rating and never looked at the plate it was
   landing on, while `overpressure` always has, so there was nothing in the
   arithmetic that could change its mind. Twenty-nine of the thirty-six shells
   were fired after the tracks and the antenna — everything a burst reaches
   from outside that glacis — were already destroyed, at which point the shot
   was provably worth nothing and was still priced at 1.8. Bounces also no
   longer reset the stalemate clock, which is the "two bad consequences" half.)* In game 1 the howitzer fires all 40 of its
   rounds at one hex — every single one bounces off the TD's front plate
   (pen 3 vs plate 6), zero structural damage. The AI never once
   re-evaluates. And there is a chain reaction: a bounce counts as
   "contact" (`orders.rs` ~line 1078, "the guns are still trying"), so the
   stalemate clock (`STALEMATE_ROUNDS = 8`, `battle/mod.rs`) resets every
   round, and the battle limps on for 8 extra rounds of pure wandering just
   because the gun kept lobbing at a plate it can't touch. One AI flaw, two
   bad consequences.

   Note the barrage was not *purely* wasted — overpressure from the bounces
   is what destroyed the TD's tracks and radio (`combat.rs::overpressure`).
   The AI just has no model of that either: it neither exploits it (softened
   target) nor stops for it (plate can't be beaten head-on).

2. **Nobody ever retreats.** *(Fixed, and not by making the exit reachable.
   Two mechanisms, neither of them the one named. `resolve_movement` refuses **all** path movement from a crew who
   will not obey — advancing and retreating alike — under a comment saying
   "they simply will not advance". And the exit is invisible to the evaluator
   until condition falls below `1 - withdraw_threshold`, which for massed
   armour means below 15%. Also: `battle_forest` declares no `exit` objective,
   so game 2 could not have produced a withdrawal under any AI. What shipped
   is the designer's model rather than a patch to either: a broken crew defies
   — fight, flight or freeze by temperament — and rallies out of it, and
   flight means away from contact rather than toward a lane. Foot units
   surviving to the bell went 66 of 96 to 73 of 96.)* Zero
   `UnitExited` events across all three
   battles. Withdrawal is a big designed feature (whole CLAUDE.md section)
   that never fires in actual play. In game 3 the heavy tank goes "Breaking
   and will not advance", then "refuses to advance" for seven straight
   rounds, and then gets destroyed where it stands. A broken unit that
   cannot retreat is free kills for the enemy — the losing side's story right
   now is "everyone stands still until shot". That is the worst possible
   shape for a morale system: it should buy survival, not just narrate the
   death spiral.

3. **Mid-battle dead time.** *(Real, and the proposed cure is backwards —
   see change 4 below. Counted over game 1: the medium tank drove 53 hexes in
   19 rounds to finish 10 hexes closer; the recon car drove 59 and ended three
   hexes further from the bridge than she deployed; the howitzer spent
   twenty-eight rounds oscillating between two adjacent hexes. That is a
   random walk, not a creep, and more movement points would widen it. The
   cause is `noisy_score` drawing difficulty noise per candidate tile under an
   argmax: the max of eighty draws beats the objective gradient every round,
   and the more tiles a vehicle can reach the worse it gets.)* Units move 1–3
   hexes/round and the AI creeps.
   Game 1: the medium tank takes 19 rounds to close a ~20-hex gap, then gets
   one-shot by opportunity fire on arrival. The recon car then patrols the
   map's edge for 25 rounds with nothing to do. If a human plays those 19
   rounds, that is 19 rounds of "nudge the tank a hex closer". Battles need
   to compress: faster movement, or missions that force commitment instead
   of creeping.

4. **Kuhlmann won 3/3, including two ~30-0s.** The armies are roughly mirror
   images (verified: each side fields a heavy, a TD, artillery, recon,
   infantry in the forest/plains scenarios), so this is not a scenario
   artifact. It smells like a planner/difficulty asymmetry that the
   mirrored-arena skill-gap test cannot see (that one fights equal vehicles).
   Cheap to confirm: swap which side gets MCTS and rerun the three maps.

5. **No battle ended by elimination or victory_score.** All three ended
   Stalemate-on-score, and `river_crossing`'s `victory_score` of 60 is
   unreachable (max scored: 28). A score target that can never trip is dead
   data — it should be reachable in a contested fight.

## Complexity

The rule set is deep but each action is simple (move, shoot), and the scale
contract (hex = 100 m, round = 60 s, reload = practical rate of fire) is what
makes it feel like a fight rather than an abstraction. The risk is on the
player-model side, not the rules: facing × plate, the pen gate, overpressure,
splash, crew, morale, ammo, fog, concealment, command latency, objectives,
exits — no human holds all of that at once, and the AI can score every tile
but the player cannot. The game will feel either "brilliantly deep" or
"random" depending entirely on whether the UI surfaces the load-bearing
numbers: plate facing the gun vs its pen chance, ammo remaining, morale rung,
who still has a radio. The balance example's "worth a look" section is the
template for exactly that.

## Changes I'd want

1. *(Done, differently.)* **Teach the evaluator about facing.** A shot at a target whose plate
   toward you exceeds your pen should be discounted to near zero. The P(pen)
   the balance tool already computes through `preview_attack` — the AI can
   afford to call it per candidate target. Kills the 40-round barrage.
2. *(Done.)* **Don't let bounces reset the stalemate clock.** A barrage against a plate
   is not contact; it is the battle already being over.
3. *(Done, differently — she runs from contact, not to a lane.)* **Let Broken units actually withdraw** — or gate the AI's retreat on
   morale rung, not just damage vs `withdraw_threshold`. "Refusing to
   advance" should mean "falling back", or it is a death sentence wearing a
   morale label.
4. **Compress the mid-game.** ~~More MP per tick~~ *(No: measured, the
   vehicles are already fast — a medium tank makes 5 hexes a round, 30 km/h —
   and they spend that speed wandering. Raising it multiplies the wander. The
   thing to fix is the per-candidate noise draw under the argmax; see item 3.)*
   Or give the AI advance missions that make it commit. The current creep is
   the biggest fun tax per round.
5. **Run the side-swap A/B** *(and note it is confounded three ways as
   written: `playthrough.rs` hands side 0 both MCTS and `massed_armor`, on a
   map whose sides field different vehicles. Swap them independently.)* to settle whether MCTS is actually better than
   utility, or side 0 is just lucky. If it is real, the difficulty system has
   a hole the parity test missed.
6. **Narrator QoL:** print plate vs pen on a bounce
   ("BOUNCES off Nadja Orlov (front 6 vs pen 3)"). One extra token makes the
   log self-explanatory to a human reading it — without it I had to open the
   data files to understand why 40 shells did nothing.
7. **Small:** the default playthrough map is the 4v4 river scenario, but the
   forest/plains armies with infantry, APCs and heavies are where the
   interesting battles are. Consider defaulting the narrator to a bigger
   scenario.

## Short version

The rules and the storytelling layer are ahead of most wargames I have seen;
the AI's tactical layer is holding the fun back. Fix target fixation, give
the losing side a way out, and speed up the mid-game, and these battles go
from "interesting log" to "I want to be on one of those crews".
