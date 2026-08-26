# PARKED

Work that is deliberately on ice: built, measured, and then set down on
purpose rather than abandoned or forgotten. Nothing here is a bug and nothing
here is a plan — [TODO.md](TODO.md) is what is left to do and
[DONE.md](DONE.md) is what was finished. This file answers a narrower
question: *why is this still in the tree if nothing uses it?*

It exists so that answer does not have to live in [CLAUDE.md](CLAUDE.md).
That file is read at the start of every session, so every paragraph in it is
paid for on every task; a thing nobody is working on should cost nothing to
not work on. Read this one when you find code with no callers and want to
know whether it is dead or resting.

**The rule for adding an entry.** Park something when you have measured it and
the measurement says it does not earn its place — not when you merely suspect
it. Say what it cost, what it bought, what would have to change for it to be
worth reviving, and what keeping it costs in the meantime. An entry without a
number is a preference, and preferences belong in TODO.

---

## MCTS (`crates/tactics_core/src/ai/mcts.rs`)

**Parked 2026-08-25. It is not better than the planner it costs thirty
thousand times as much as.**

`MctsPlanner` is a full Monte Carlo tree search: 900 iterations at difficulty
4, rolling out to depth 20, every fifth step a `Commit` that runs the enemy's
whole planning pass. It works, it is tested, and it has never shipped — every
scenario names `utility`.

The question was whether it *should* ship, and it stayed open for a long time
because it was never asked properly. REVIEW.md read "Kuhlmann won 3/3" as
evidence for MCTS, and it could not be: `playthrough` handed side 0 both MCTS
*and* `massed_armor`, on a map whose two sides field different vehicles. Three
candidate explanations, one observation.

`balance --brains` separates them — the mirrored arena, same forces, same
doctrine, same difficulty, both orientations, only the brain differs:

| pairing (A vs B) | A won | B won | draws |
| --- | --- | --- | --- |
| utility vs utility | 33 | 31 | 0 |
| mcts vs mcts | 31 | 33 | 0 |
| mcts vs utility | 29 | 34 | 1 |
| utility vs mcts | 28 | 35 | 1 |

**MCTS 64 wins, utility 62, out of 128 mixed battles.** Parity, at 64 battles
a pairing.

The control rows are what make that trustworthy rather than merely a number.
Both same-brain pairings land within two games of even, so the noise floor is
about ±3 — and a real difference would have had to clear it in *both* mixed
rows. Neither does: both favour whichever side happens to be B, by a combined
69 of 128, which is inside one standard deviation of a coin.

**Why it fails, which is the part that generalises.** MCTS's rollout policy
**is** the utility planner. It searches a tree whose leaves are evaluated by
the very thing it is trying to beat, so 900 iterations buy almost nothing over
asking that evaluator once. A search is only as good as what it searches
toward. The same conclusion arrived independently from the other end the same
day: difficulty stopped meaning much once the goal chooser existed, because
the chooser prices being *there* and the drive and nothing about the route,
the risk on the way, or the enemy.

**What would justify reviving it.** A materially better evaluator — one that
prices a route, an exchange, or what the enemy does next. Re-run
`balance --brains` against that, and if MCTS still cannot clear the control
rows in both orientations, the search is not the missing piece. Do not revive
it on the strength of a narrated battle: that is exactly the observation that
kept this question open for months.

**What keeping it costs.** ~400 lines that compile and whose tests run, and a
`PlannerRegistry` entry so a mod can still name `"mcts"`. It is not on any hot
path, and no planner change owes it a performance budget — if a change to the
utility planner or the evaluator makes MCTS slower, that is not a reason to
reject the change. If it ever starts *blocking* work rather than sitting
still, delete it; the history has it, and this file explains what you would be
deleting.
