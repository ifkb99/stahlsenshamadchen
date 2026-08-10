# DONE

Finished work, kept for its reasoning rather than its status. [TODO.md](TODO.md)
is what is left; this is why the built things are the shape they are — the
measurements that settled an argument, the guesses that turned out wrong, and
the traps that are only obvious once you have fallen into them.

Nothing here needs doing. Read it when a decision looks arbitrary, or before
undoing one.

## Scale and maps

**A tile and a unit have a size.** 1 battle hex = 100 m, 1 round = 60 s,
1 tick = 5 s, 1 elevation level = 10 m, 1 overworld hex = 4 km = one battle
map. Movement points were already calibrated for this (5 MP tracked on grass =
30 km/h; 7 MP wheeled on road = 42 km/h) — only ranges, vision and reloads had
to move. The AW-style 10 HP model is what implicitly sets round length, so
committing to a 60 s round commits to the ballistics rewrite: lethality has to
come from penetration rolls, not HP attrition.

**Scale lives in data, not Rust.** A `scale` block in mod.json (`hex_meters`,
`round_seconds`, `ticks_per_round`, `elevation_meters`, plus
`overworld_hex_meters` and `overworld_turn_hours`), typed as `data::Scale`.
`TICKS_PER_ROUND` is gone as a const, so round resolution and cooldowns take a
registry; `reload_ticks` had to become `Option<u32>` because a serde default fn
cannot see the mod being loaded — read it via `weapon.reload(&scale)`. Panels
lead with "1.6 km" / "30 km/h" / "elev 30 m" and keep hexes and MP in parens. A
sibling `balance` block does the same for crew stats: the old `awareness / 4`
and `driving / 5` divisors became +5% of the vehicle's base per point, so a
gifted crew is worth about a quarter of what they are sitting in.
`validate-mods` prints the whole roster through the scale, which is what makes
a wrong number visible.

**A battle map is a hexagon of one overworld tile**, since that is how the
overworld draws it. Radius derives from the scale (4 km / 100 m = 41 across,
1261 tiles), `MapKind::Battle` implies `MapShape::Tile`, and validation rejects
anything that is not that hexagon (`"shape": "free"` opts out for scenario
maps). It needed no map-format change — `HexMap` was already a sparse hash and
a space already meant "no tile". `river_crossing` was rebuilt as a river
valley: meandering river, road bridge on the centre row, two mud fords, woods,
a town on the east bank, elevation falling from 3 at the rim to 0 along the
water. Cost 1261 tiles against 768, which took the engine suite from 9.5 s to
26 s — since paid back several times over by the fog work.

## Girls, crews and rules

**Girls are instances, not definitions.** `tactics_core::roster` owns `Girl`
(id, def, name, owner, stats, xp, status, battles) and `Roster`; `Unit.crew`
and `ArmyUnit.crew` are `GirlId`s, and crew bonuses read the instance — so a
wounded gunner already costs her vehicle its gunnery with no special case. The
overworld owns the roster and armies carry handles into it, so the same girls
come out of a battle as went in.

**One roster for the whole world**, with `Girl.owner` naming her academy —
not one per side. That keeps `GirlId` unambiguous everywhere and makes girls
changing hands (recruited, poached, captured) a field change rather than a
renumbering, which is what an academy-scale mode will want. `Army.units` is
`ArmyUnit` rather than `UnitPlacement` for the same reason: a placement is
map-file data carrying a coordinate a unit inside an army has no use for, and
conflating the two produced the `at: [0, 0]` survivor hack.

**Cores, skills and traits are built**, and the scale question they were
blocked on is settled: cores are the full nine GURPS attributes centred on 10
(8–12 ordinary), not the old 0–5, which is what makes skill defaults like
`Hands - 4` mean anything. Thirteen skills, five seat roles, and traits that
are declarative and conditional. Abilities are *derived*, never stored — see
`data/cores.rs` and [the wiki](assets/wiki/reference/girls.md).

**Every core must be named by at least one skill.** A core nothing reads is
dead weight, which is exactly what `morale` and `leadership` were in the old
`CrewStats`: declared for years and never once consulted.

## Battles

**Battle victory conditions.** A map declares `objectives` (id, name, `at`
hexes, `value`, `kind`) and optionally a `victory_score`. Ground of kind
`hold` pays its value each round to whoever holds it; an `exit` is ground
worth *leaving* by, paid once, and the vehicle drives off the map. Reaching
`victory_score` wins outright (`EndReason::Objectives`); otherwise the points
decide a battle that would have been a draw, so **`EndReason::Stalemate` no
longer implies `winner: None`**.

The reason it jumped the queue was never the feature. `balance --sim` reported
**67% of battles ending in stalemate**, and the cause was not the standing
guess that "the sides lose each other" — measured, survivors were parked on
their own start rims 20+ hexes apart and *drifting further apart*. The tell was
an inversion: the stalemate rate **rose** as the AI got better (difficulty 5,
zero noise: 11/12; difficulty 1, ±6 noise: 6/12), because random noise was the
only thing causing contact. With elimination as the only win condition, sitting
in the best cover on the map is optimal play, and two sides doing that never
meet. After: **1 draw in 24, 652 shots against 302, 9.3 rounds against 16.5**.

Three things about it are easy to break and were expensive to get right:

- Objectives live on `HexMap`, not `BattleState`, so both setup paths pick them
  up with no new plumbing. The battle carries only `objective_held` + `score`.
- **Leaving is not dying.** `alive` means "on the battlefield"; `exited` means
  she went home. Classify end-of-battle units with `surviving_units()` /
  `lost_units()`, never `!alive`.
- **`deploy` must skip exit hexes.** A side deploys at the shallowest tiles of
  its own edge, which is exactly where its retreat lane is, so without the
  filter the attacker's leading vehicles spawned on their own way out and drove
  off on the first tick. Caught by
  `nobody_deploys_onto_their_own_way_off_the_map`, which is the game crate's
  first real test.

**Units no longer all spawn facing east.** They turn towards the centroid of
their enemies once every placement exists (`face_units_at_enemies`), and
`UnitPlacement` gained an optional `facing` so a scenario can still place
someone looking the wrong way — which is what makes an ambush authorable rather
than something the engine decides. Worth recording that the original note
overstated the damage: measured against the determinism baseline it was 6 rear
hits out of 59, not the whole first exchange, because on the 41-hex hexagon
units spend several rounds closing and facing updates on move. The bias was
real and entirely one-sided though — all 6 fell on side 1 — and after the fix
the same four seeds produce zero rear hits.

**Saves record which mods were playing.** `SaveGame.mods` stamps id and
version; mismatched ids are refused (the rules genuinely differ), version drift
on the same set warns and loads (a content patch must not cost the player their
campaign), and saves written before the field existed are trusted rather than
rejected.

## Performance

**`fog::recompute` no longer rebuilds every side's vision after every shot** —
and without the batching trade-off the original note proposed, so a firing unit
is still revealed instantly. Three things did it: a `SightGrid` resolves tile
sight-heights once (`los_clear` was doing a String-keyed terrain lookup per ray
step, ~1M times a round); vision is cached per unit against `(pos, range)`,
which is exact because the map cannot change mid-battle; and a side whose
`(unit, pos, range)` list is unchanged skips the union entirely, which is what
makes the per-shot recompute free.

Round resolution 14.05 → 1.02 ms, engine suite 26 s → 2.6 s, event stream and
final fog state bit-identical across four seeds. The remaining LoS cost is that
vision raycasts every tile in range independently; shadowcasting would be
roughly another order of magnitude, but it changes *which* tiles are visible,
so it needs its own design pass alongside the detection-roll rework.

**MCTS went from impossible to merely expensive**: ~35 s per unit order → ~3.3
s, and `examples/playthrough.rs` plays a full 32-round battle at difficulty 4
in ~50 s instead of failing to produce a single round. Still too slow to plan
against a human, so the shipped scenario keeps `utility`. What is left is
structural: 900 iterations × depth 20, with roughly every fifth rollout step a
Commit that runs the enemy's whole planning pass and resolves a full 12-tick
round.

## Tooling

**The balance harness**, `examples/balance.rs`, and it is two harnesses on
purpose. The analytic pass is instant — it stands two vehicles on an empty
field and asks `preview_attack` what one shot does, so it is the loop to sit in
while editing json — and `--sim` fights whole battles for the check at the end.
Everything goes through the real combat code rather than reimplemented
formulas, so the report cannot drift from the game. Its first run said two
useful things: `mg kills heavy_tank in 3 rounds`, which is the `.max(1)` floor
stated as a number rather than a worry, and the 67% stalemate rate above.

## Bugs that turned out not to be ours

**Intermittent `DeviceLost`.** `Caught DeviceLost error: Unknown Unexpected
error variant (driver implementation is at fault)` a few seconds in, killing
the app. It looked like an overworld rendering bug; it is the NVIDIA Vulkan
FIFO present path on this box (2× RTX A4500, driver 580.126.20, X11).

The evidence: `crates/game/examples/minimal_window.rs` is a stock Bevy window
with none of our systems and it reproduces 3 runs out of 3. Under
`PRESENT=immediate|mailbox|nosync` it survives 3 out of 3; under
`autovsync`/`fifo` it dies every time. Workaround is `STAHL_PRESENT=immediate`
(see `main.rs`); the default stays vsync, because this is pixel art and tearing
is the one artefact it cannot hide, so the knob is opt-in for the affected
machine rather than a change for everyone.

False trail worth remembering: `WGPU_BACKEND=gl` looks like a fix but only
because the GL backend finds no device here and panics before rendering — it
produces zero `DeviceLost` lines for the wrong reason. The long-standing
`Couldn't get swap chain texture ... Cause: 'Outdated'` warning appears on
every run just before it, and is probably the same driver issue's first
symptom.
