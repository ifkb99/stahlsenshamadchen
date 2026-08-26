---
name: tactics-dev
description: How to work on Senshamädchen — the instruments, the change loop, the verification traps, and the design rules that constrain code. Load at the start of any session doing real work on this repo, before touching the simulation, the mod data, or the Bevy layer.
---

# Working on Senshamädchen

`CLAUDE.md` says what the code *is*. This says how to work on it: what to reach
for, how to tell whether a change did what you meant, and the specific ways
this repo has already fooled people.

Read `CLAUDE.md` for architecture and invariants, `TODO.md` for what is left,
`DONE.md` for why the built things are shaped as they are, and
`assets/wiki/reference/` for design — especially `cadets.md`, which is the model
everything about crews hangs off.

## The four instruments

This project can answer most questions about itself. Reach for these before
reasoning from first principles, and **quote their numbers in commit
messages** — that is the house style and it is why claims here are checkable.

| question | instrument |
| --- | --- |
| What did that data change do? | `cargo run --release -p tactics_core --example balance` — instant analytic tables. `-- --sim` fights whole battles. |
| What does that number do *that the old one did not*? | `balance -- --sim --games 36 --sweep <path>=<a,b,c>` — every value at once, with a table of differences. `--set` for a single run, `--help` for the paths. |
| How much of that difference was the dice? | `balance -- --sim --games 36 --sweep seed=1000,2000,3000` — the same game, sampled. Read a difference against this before believing it. |
| Did behaviour change, and did I mean it? | `cargo test -p tactics_core --test determinism` |
| Is it still fast? | `cargo run --release -p tactics_core --example perf` |
| What does it look like? | `STAHL_PRESENT=immediate STAHL_DEBUG=1 STAHL_BATTLE=river_crossing STAHL_SCRIPT=scripts/dev/battle-tour.txt cargo run -p stahlsenshamädchen` |
| Is the renderer broken, or is it the driver? | `cargo run -p stahlsenshamädchen --example minimal_window` |

`--release` is not optional for `perf` and `balance`; the dev profile builds at
`opt-level = 1` and the numbers are meaningless.

## The change loop

1. Make the change.
2. **Run the whole suite and look at every binary** (see the trap below).
3. Read the determinism diff. A broad diff means you changed balance or rules
   on purpose — regenerate with
   `UPDATE_SNAPSHOTS=1 cargo test -p tactics_core --test determinism` and say so
   in the commit. A *small* diff about the **order** of otherwise identical
   events means you introduced an iteration-order bug. Do not regenerate.
4. If it touched combat, movement or content: run `balance --sim` and compare.
   If the change *is* a number, sweep it instead of running twice by hand —
   `--sweep` never touches `mod.json`, so there is no revert to forget, and
   it puts the baseline on the row above the difference. Sweep the seed in
   the same session: this project has quoted differences smaller than its own
   noise floor before.
5. If it touched the Bevy layer: take a screenshot. The sim being right and the
   UI being silently dead is a real failure mode here — it has happened.
6. Gates: `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo fmt --check`, `cargo run --bin validate-mods`.
7. Commit with the numbers in the message.

To verify a *revert* is complete, restore the committed baseline
(`git checkout develop -- crates/tactics_core/tests/snapshots/event_stream.txt`)
and require it to pass. Regenerating would hide any residue.

## Traps this repo has already sprung

Every one of these cost real time.

**A test binary can fail invisibly.** `cargo test --workspace` prints one
`test result:` line per binary. Summing the `ok` ones with `awk` reports green
while a binary is red. Always list them:

```sh
cargo test --workspace 2>&1 | grep -E "Running|test result" | paste - - | sed 's/.*deps\///'
```

**Never pipe a build to `/dev/null` without checking the exit code.** A
compile error means the *old binary* runs, and every conclusion drawn from it
is worthless. This produced an hour of debugging a feature that was not in the
binary being tested.

**In-game messages go to the HUD log, not stdout.** Grepping the process output
for "Loaded." proves nothing. Screenshot the log panel instead.

**`cargo fmt` silently defeats string-based edits.** A patch matching
`foo(a, b)` will not match after fmt wraps it across lines, and the edit
reports success having changed nothing. For anything already formatted, use
line-based edits or re-read the file and confirm.

**`undefined hidden symbol: anon.*.llvm.*`** at link time means corrupt
incremental artifacts, usually after a rustc crash. `rm -rf
target/debug/incremental`. Symptoms include bisects that give contradictory
answers.

**This machine needs `STAHL_PRESENT=immediate`.** Without it the Vulkan FIFO
present path loses the GPU a few seconds in. It is an NVIDIA driver bug, proved
with `examples/minimal_window.rs`; see TODO under Bugs.

## Design rules that constrain code

Breaking one of these is a design regression even if it compiles and passes.

**Content is data, not Rust.** Vehicles, weapons, terrain, cores, skills,
roles, traits, morale and reaction rules all live in `assets/mods/`. Adding a
tuning constant to Rust that a modder would want to change is a smell.

**Difficulty is a mod.** Every harsh system must be an additive rule whose
absence *is* the gentle game — reaction latency collapses to zero ticks, the
morale ladder reduces to one rung, casualties reduce to everyone walking away.
**If switching one off needs an `if` in Rust, it was built wrong.**

**Determinism is load-bearing.** Replays, search AI and the baseline all rest
on it. Never let `HashMap`/`HashSet` iteration order reach an event stream or
an AI decision — sort first, or walk `state.units` in id order. This has been
violated once already and stayed invisible for months.

**Abilities are derived, never stored.** A cadet has cores, trained skills and
traits; what she can *do* is computed where it is needed, so terrain,
suppression and traits arrive as arguments rather than as corrections to a
cached number. See `data/cores.rs`.

**Every core must be named by at least one skill.** A core nothing reads is
dead weight — exactly what `morale` and `leadership` were in the old
`CrewStats`, declared for years and never once consulted.

**`tactics_core` must not depend on Bevy.** Presentation lives in
`crates/game`.

## House style

Comments explain *why*, in prose, and are often several lines. Tests are named
as full sentences stating the rule they defend
(`unspotted_enemies_still_ambush`). Errors are `thiserror` enums. Commit
messages explain the reasoning and quote measurements; they are the project's
memory and are worth the length.

Work on a branch off `develop`, conventional commits, and do not push unless
asked.

## When a test fails after a deliberate change

Ask whether the test encodes an assumption the change invalidated, or whether
the change is wrong. The tell: if the test fails at *any* setting of a new
knob — not just an aggressive one — the model is wrong, not the tuning. That
distinction killed one implementation of reaction latency and was the right
call.
