# Context gaps

Temporary working file. Not meant to be committed long-term — the durable half
of each item has already moved into `CLAUDE.md` (defects, invariants, commands)
or `assets/wiki/` (tone). What is left here is **the questions**, which is the
part only Ian can close. Delete the file once they are answered.

All six gaps are implemented and every question has been answered. This file
is now a record of the session rather than a live todo — safe to delete, with
the exception noted under item 5 (four tone questions live on in the tone page).

---

## 1. The presentation layer is unverifiable — **DONE**

`crates/game/src/devtools.rs`, plus `scripts/dev/{battle,overworld}-tour.txt`.

```sh
STAHL_DEBUG=1 STAHL_BATTLE=river_crossing \
  STAHL_SCRIPT=scripts/dev/battle-tour.txt cargo run -p stahlsenshamädchen
```

Correction to my first pass: `STAHL_BATTLE=<map_id>` already existed
(`mods.rs:57`) — I missed it. Only scripted input and multi-shot capture were
actually missing.

Verified end to end on both screens: hovering a hex, selecting a unit, ordering
a route, and rotating the view all produce the right screenshots. It caught a
bug in itself on the first run — the click silently selected nothing, because
Bevy clears `just_pressed` at the top of `PreUpdate` and I was injecting before
that. `run_script` now runs `.after(InputSystems)`.

Decisions taken, matching what I proposed:

- Injected presses go through the real `ButtonInput`, so every `just_pressed`
  branch in the real handlers runs.
- The cursor does **not** go through the window, because `bevy_winit` warps the
  real OS pointer when `Window::cursor_position` changes — it would fight you
  for your mouse and fail on Wayland. `map_render::View` consults a
  `ScriptedCursor` resource instead.
- Scripts address a **hex**, so they survive camera, zoom and rotation changes.
  `pixel` exists for UI chrome.
- Dev-only, behind `STAHL_DEBUG`; a normal run registers nothing.

Bonus: this grew into `map_render::View` during the clippy work (item 4), which
absorbed rotation and centre as well and removed parameters from nine systems.

### Questions

- **Should this stay `STAHL_DEBUG`-gated, or become a proper cargo feature?**
  Right now a release build still contains the code, just never registers the
  plugin. A feature flag would remove it entirely. Only worth doing if you ever
  ship a binary you care about hardening.
- **Do you want these scripts as CI fixtures eventually?** They are
  deterministic and hex-addressed, so screenshot comparison is feasible — but it
  needs a GPU-capable runner and a tolerance policy for driver differences. I
  would not do it yet; flagging that the door is open.

## 2. No perf harness — **DONE**

`cargo run --release -p tactics_core --example perf` (add `--mcts` for the slow
ones). Went with the plain example rather than Criterion, as proposed.

Measured, and now in `CLAUDE.md`:

| | |
| --- | --- |
| round resolution | 1.66 ms (0.96–2.54 across seeds) |
| `reachable()` per call | 32.8 µs |
| `unit_vision` per unit, cold | 86.0 µs |
| utility order | 0.03 ms |
| mcts order, difficulty 3 / 4 | 1.84 s / 4.24 s |

Two notes on how these relate to the numbers that were in `CLAUDE.md`:

- The stale **~39 µs `reachable()` on the 768-tile map** is gone — that map
  stopped existing when battle maps became the radius-20 hexagon. It is 32.8 µs
  on the 1261-tile map now. I corrected it in place.
- **Round resolution reads 1.66 ms against the 1.02 ms** the fog commit quoted.
  Not a regression: I measure the first eight rounds, when all eight units are
  alive and moving, which is the expensive end. The per-seed low is 0.96 ms.
- I measure `unit_vision` rather than `fog::recompute`, because a recompute in
  a real battle is mostly the cache saying "nobody moved" — that is the whole
  point of the fog work, and timing it would measure the wrong thing. One cold
  look is the number shadowcasting FOV would actually move.

### Questions

- **Should perf run in CI?** Still my recommendation: no, or advisory-only.
  GitHub runners are too noisy for thresholds, and a flaky red is worse than no
  signal. I left it out of the workflow.
- **Is `--mcts` at 4 s/order worth keeping in the default seed set?** It is
  opt-in precisely because it dominates run time. If you want a pre-commit
  habit, the default (no `--mcts`) run takes a couple of seconds.

## 3. Determinism not pinned — **DONE**

`crates/tactics_core/tests/determinism.rs` +
`tests/snapshots/event_stream.txt` (1435 lines). Four seeds, twelve rounds,
orders and events and final board state all recorded.

Went with the middle option I proposed: pin the full stream via `Debug`, make
regeneration one command. `Debug` rather than a formatter written for the test,
because a hand-written one can only print the fields someone remembered — and
the forgotten field is exactly the one that regresses unnoticed.

```sh
UPDATE_SNAPSHOTS=1 cargo test -p tactics_core --test determinism
```

Verified it actually fails: bumping `gun_75` damage by one point turned it red
and pointed at the exact line (`damage: 3` → `damage: 4`). A snapshot test that
cannot fail is worthless, so this mattered more than it sounds.

There is a second, cheaper test alongside it that runs the same seeds twice in
one process. If both fail, the sim is inconsistent with itself; if only the
baseline test fails, the baseline is stale or something reordered.

**ANSWERED:** stays default-on and as-is for now; the shape of this and the
rest of the tests may change later.

## 4. CI below the repo's bar — **DONE**

`.github/workflows/rust.yml` rewritten. Now runs on `develop` as well as `main`
(it effectively never ran before, since you work on `develop`), installs the
ALSA/udev packages bevy needs, caches the dependency tree, and adds
`validate-mods` as a **gating** step — that was the real hole, since a broken
mod file was mergeable and no Rust test reads content the way a modder does.

All steps verified locally: build, test, and `validate-mods` all pass.

**ANSWERED: clippy and rustc warnings gate.** Done — all 32 warnings cleared
and the workflow runs `cargo clippy --workspace --all-targets -- -D warnings`.
That single step covers rustc's lints too (verified: an unused import fails it),
so there is no separate `RUSTFLAGS` step — using `RUSTFLAGS` would apply to
every dependency, bust the build cache, and fail on warnings that are not ours.

What the cleanup actually involved, since it was more than `--fix`:

- `map_render::View` — rotation, centre, window, camera and the scripted cursor
  were five things threaded separately through nine systems, and they are all
  "how are we looking at the world". Systems now take one `view` and call
  `view.hovered(&map)` / `view.face_at(&map, hex)`.
- `TextSlot`, `MarkerQuery`, `BattleHud` — names for the query types whose
  `Without` filters exist only to prove two `&mut` queries cannot alias. That
  is all 11 `type_complexity` warnings.
- `PlacementCheck` in `map.rs`, replacing an eight-argument nested fn.
- Two `#[allow]`s left, on `pump_events` and `enter_overworld`, each with a
  comment: a Bevy system's parameters are its dependency list, and those two do
  not decompose into a smaller noun.

**Correction worth flagging:** CLAUDE.md said the worst `too_many_arguments`
offenders were the combat functions and proposed a `Shot` struct. That was
wrong — `combat.rs` never tripped the lint; every offender was a Bevy system in
the game crate. A `Shot` struct may still be worth doing when penetration adds
parameters, but as a readability choice, not a lint fix. I corrected the note.

Verified behaviour-preserving three ways: the determinism snapshot is
unchanged, the full suite passes, and both dev-script tours produce screenshots
identical to the pre-refactor ones.

**ANSWERED: rustfmt adopted and gated.** `cargo fmt` applied — 129 hunks
across 25 files, all whitespace and import ordering — and the workflow now runs
`cargo fmt --check` as a gating step.

Added `rustfmt.toml` pinning `style_edition = "2024"`. Without it the gate is
hostage to the toolchain: rustfmt's defaults are what changed the import
ordering in the first place, and a future default change would turn CI red with
no commit to blame. `edition` comes from Cargo.toml, but `style_edition` is the
knob that actually decides formatting, so it is stated explicitly.

Verified the reformat changed nothing but whitespace: the determinism snapshot
is byte-identical (sha256 before and after), all 63 tests pass, clippy is still
clean at `-D warnings`, and the battle tour screenshots match.

**Every question in this file is now closed.** The file can be deleted; the
durable content lives in `CLAUDE.md` and `assets/wiki/reference/tone.md`, except
for the four tone questions, which live in the tone page itself.

## 5. No tone or art bible — **STUBBED, mostly yours**

`assets/wiki/reference/tone.md`, linked from the wiki index.

Writing it turned up something worth knowing: **the content has decided more
than the roadmap admits.** Every vehicle is a real Wehrmacht designation (Luchs,
Wiesel, Panther, Löwe, Marder, Hummel); six of ten crew names are German but
the rest are Japanese, Italian and Russian — so the GuP-style mixed roster
inside a nationally-themed school is already the de facto pattern. And the
palette is a deliberate-looking split: drab olive/khaki world, saturated
primaries reserved for side identity. That last one is the one part of the
current look that reads as design rather than placeholder.

It also surfaced a real contradiction: **the roster is WWII, the brief says
late 1960s.** A Panther with a 75 mm gun is 1943. That decides what the
ballistics rewrite is modelling and whether the vehicle names survive, so it is
worth settling before that work starts.

**ANSWERED: in between — serious on the battlefield, lighthearted at the
academy, with VN/support-conversation features planned in the Fire Emblem
tradition.** Written up as a decided section at the top of the page.

That answer closed two of the six questions on its own: the HUD stays austere
(cuteness concentrates in the academy screens, portraits and barks, not sprayed
across the interface), and the cadets' relationship to the fighting is settled.

Two consequences worth pulling out, both recorded in the page:

- **The academy layer is now half the tone, not decoration.** The menus and
  roster screens already on TODO carry as much identity as the battle screen,
  so they should not be built as bare utility UI to be prettied later.
- **The cadet-instance refactor got more load-bearing.** Support conversations
  need persistent per-cadet state — who has fought alongside whom, how often —
  which is the same mutable object wounds and XP need. TODO already ranks it
  first under "Design Decisions to Lock Early"; it now serves two systems.
- **Crew death gets sharper, not softer.** The more the academy half invests
  you in a specific cadet, the more permadeath costs. Worth deciding before the
  VN work rather than after.

**ALSO ANSWERED: the 1960s is the setting, not the equipment list.** Older kit
stays in service, so a Panther on the field is a twenty-year-old tank somebody
still maintains rather than an anachronism. That closes the roster-versus-brief
contradiction in the more interesting direction, and it is true to life — WWII
armour served into the sixties and seventies in second-line and training roles,
which is exactly where a school's tanks would sit.

Two consequences recorded in the page: nothing in the roster needs renaming, and
this *strengthens* the case for the ballistics rewrite — a twenty-year spread of
armour is exactly where flat HP attrition breaks down, because the point of a
1943 gun meeting 1960s armour is that it cannot get through, and the `.max(1)`
damage floor currently says it can.

### Still open

Four, all in the page: is the mixed-nationality roster intended; what do we
actually call the cadets; is an artist near-term; and how lethal it is allowed to
look.

## 6. Two conventions — **still yours**

- **`CLAUDE.md` vs `TODO.md` ownership.** I added to `CLAUDE.md` this session
  (the harness, the perf table, the determinism note) and left `TODO.md` alone,
  on the theory that CLAUDE.md is for what is true about the code and TODO for
  what is planned. That is a guess. If you want them kept in sync instead, say
  so — but the duplication is real and they will drift.
- **Commits.** I have written nothing to git this session; everything is in the
  working tree for you to review. Do you want me committing by default? If so,
  conventional commits (`feat:`/`fix:`/`perf:`) on `develop`, matching your
  recent history.

---

## Files added or changed this session

```
CONTEXT-GAPS.md                                  (this file, temporary)
CLAUDE.md                                        harness docs, perf table, determinism note
.github/workflows/rust.yml                       rewritten
assets/wiki/INDEX.md                             + tone link
assets/wiki/reference/tone.md                    new
crates/game/src/devtools.rs                      new
crates/game/src/main.rs                          plugin wiring, dev_screenshot moved out
crates/game/src/map_render.rs                    ScriptedCursor, View, query aliases
crates/game/src/battle.rs                        View + BattleHud, clippy cleanup
crates/game/src/overworld.rs                     (same)
crates/tactics_core/examples/perf.rs             new
crates/tactics_core/tests/determinism.rs         new
crates/tactics_core/tests/snapshots/event_stream.txt   new baseline
rustfmt.toml                                     new, pins style_edition 2024
scripts/dev/battle-tour.txt                      new
scripts/dev/overworld-tour.txt                   new
```

Full suite green: 45 engine + 2 determinism + 14 unit + 2 devtools.
`validate-mods` exits 0. No formatting changes, no git history touched.
