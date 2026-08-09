# TODO
## Design Decisions to Lock Early
Things that shape everything below them. Deciding late means rework; ordered by cost of delay.
- ~~girls need to be instances, not just definitions~~ DONE. `tactics_core::roster` owns `Girl` (id, def, name, owner, stats, xp, status, battles) and `Roster`; `Unit.crew` and `ArmyUnit.crew` are `GirlId`s, and crew bonuses read the instance — so a wounded gunner already costs her vehicle its gunnery with no special case. the overworld owns the roster and armies carry handles into it, so the same girls come out of a battle as went in: survivors get `battles += 1`, casualties get a status, and `advance_day` runs once per campaign day.
  **one roster for the whole world, with `Girl.owner` naming her academy** — not one per side. that keeps `GirlId` unambiguous everywhere and makes girls changing hands (recruited, poached, captured) a field change rather than a renumbering, which is what an academy-scale mode will want.
  `Army.units` is now `ArmyUnit` rather than `UnitPlacement`: a placement is map-file data carrying a coordinate a unit inside an army has no use for, and conflating the two is what produced the `at: [0, 0]` survivor hack
- what happens to crew when a vehicle dies? DECIDED IN SHAPE, open in tuning. permadeath is an *option* (`CasualtyRules::permadeath`, off by default), and what befalls a girl comes from what hit the vehicle (`Unit.last_hit_by`) and how survivable it is (`VehicleDef.safety`, 0-5, in vehicle json — the recon car is 4 because everyone is a hand's reach from a hatch, the tank destroyer is 2 because a casemate has no turret roof to climb out of). `resolve_crew_fate` turns those into Unharmed / Wounded / Lost / Killed.
  `Lost` is explicitly not death: she bailed out and could not reach her own side before the shooting stopped, and walks back over a few days. with permadeath off, a would-be fatal wound becomes a long recovery instead.
  what is still open is the tuning — the numbers in `resolve_crew_fate` are a first pass, and hooking the option to a settings menu
- battle victory conditions: engine only knows eliminated and stalemate. campaigns and scenario variety want per-map objectives in map json ("hold the bridge 10 turns", "exit the map edge"). decide the shape before battle-flow assumptions (muster prompt, apply_battle_result) harden around "battle over = someone died"
- one-unit-per-hex is assumed in a few places (unit_at returns first match). apc passengers will bend this — decide whether cargo lives "inside" the carrying unit (cleaner, keeps the invariant) before writing the apc
- ammo selection adds an ammo field to FireIntent and a per-unit inventory. the intent already carries a weapon index, so this is a one-field change plus the reload/consumption bookkeeping
- need to find better name for "girls". cadets kind of works but feels out of theme. should be something like soldier but cute
- ~~determine scale of a tile and a unit~~ DECIDED: **1 battle hex = 100 m, 1 round = 60 s, 1 tick = 5 s, 1 elevation level = 10 m, 1 overworld hex = 4 km = one battle map**. movement points were already calibrated for this (5 MP tracked on grass = 30 km/h; 7 MP wheeled on road = 42 km/h) — only ranges, vision and reloads had to move. the AW-style 10 HP model is what implicitly sets round length, so committing to a 60 s round commits to the ballistics rewrite: lethality has to come from penetration rolls, not HP attrition. see Realistic Ballistics
- ~~scale should live in data, not rust~~ DONE: `scale` block in mod.json (`hex_meters`, `round_seconds`, `ticks_per_round`, `elevation_meters`, plus `overworld_hex_meters` and `overworld_turn_hours`), typed as `data::Scale`. `TICKS_PER_ROUND` is gone as a const, so round resolution and cooldowns take a registry; `reload_ticks` had to become `Option<u32>` because a serde default fn cannot see the mod being loaded, read it via `weapon.reload(&scale)`. panels now lead with "1.6 km" / "30 km/h" / "elev 30 m" and keep the hex and MP counts in parens. a sibling `balance` block does the same for crew stats — the `awareness / 4` and `driving / 5` divisors are now +5% of the vehicle's base per point, so a gifted crew is worth about a quarter of what they are sitting in. `validate-mods` prints the whole roster through the scale, which is what makes a wrong number visible
- ~~battle maps are 32 hexes wide but an overworld hex is 40~~ DONE: a battle map is now a *hexagon* of one overworld tile, since that is how the overworld draws it. radius derives from the scale (4 km / 100 m = 41 across, 1261 tiles), `MapKind::Battle` implies `MapShape::Tile`, and validation rejects anything that is not that hexagon (`"shape": "free"` opts out for scenario maps). needed no map-format change — `HexMap` was already a sparse hash and a space already meant "no tile". `river_crossing` was rebuilt as a river valley: meandering river, road bridge on the centre row, two mud fords, woods, a town on the east bank, elevation falling from 3 at the rim to 0 along the water. cost: 1261 tiles against 768, which took the engine suite from 9.5 s to 26 s — since paid back and then some by the fog work below
- ~~fog::recompute rebuilds every side's vision after every shot~~ DONE, and without the batching trade-off: a firing unit is still revealed instantly. a `SightGrid` resolves tile sight-heights once (`los_clear` was doing a String-keyed terrain lookup per ray step, ~1M times a round); vision is cached per unit against `(pos, range)`, which is exact because the map cannot change mid-battle; and a side whose `(unit, pos, range)` list is unchanged skips the union entirely, which is what makes the per-shot recompute free. round resolution 14.05 -> 1.02 ms, engine suite 26 s -> 2.6 s, event stream and final fog state bit-identical across four seeds. the remaining LoS cost is that vision raycasts every tile in range independently; a shadowcasting FOV would be roughly another order of magnitude, but it changes *which* tiles are visible, so it needs its own design pass alongside the detection-roll rework
- overworld elevation shares `elevation_meters` with the battle scale, so frontier's mountains are 20 m tall. wants its own vertical scale once the strategic layer cares about height
## Bugs
- check out warning: `WARN bevy_render::view::window: Couldn't get swap chain texture after configuring. Cause: 'Outdated'`
- warning: `WARN winit::platform_impl::linux::x11::xdisplay: error setting XSETTINGS; Xft options won't reload automatically`
- ~~intermittent `DeviceLost`~~ DIAGNOSED, and it is not ours. `Caught DeviceLost error: Unknown Unexpected error variant (driver implementation is at fault)` a few seconds in, killing the app. Looked like an overworld rendering bug; it is the NVIDIA Vulkan FIFO present path on this box (2x RTX A4500, driver 580.126.20, X11).
  the evidence: `crates/game/examples/minimal_window.rs` is a stock bevy window with none of our systems, and it reproduces 3 runs out of 3. under `PRESENT=immediate|mailbox|nosync` it survives 3 out of 3; under `autovsync`/`fifo` it dies every time. so it is vsync-path specific and nothing to do with this game's code.
  workaround: `STAHL_PRESENT=immediate` (see `main.rs`). default stays vsync, because this is pixel art and tearing is the one artefact it cannot hide, so the knob is opt-in for the affected machine rather than a change for everyone.
  false trail worth remembering: `WGPU_BACKEND=gl` looks like a fix but only because the GL backend finds no device here and panics before rendering — it produces zero `DeviceLost` lines for the wrong reason.
  the long-standing `Couldn't get swap chain texture ... Cause: 'Outdated'` warning above appears on every run just before this, and is probably the same driver issue's first symptom.
- units cannot move through friendlies on campaign map
- ~~mcts is too slow to use at the new scale~~ MOSTLY FIXED by the fog work: ~35 s per unit order -> ~3.3 s, and `examples/playthrough.rs` now plays a full 32-round battle at mcts difficulty 4 in ~50 s instead of failing to produce a single round. still too slow to plan against a human in real time, so the shipped scenario keeps `utility`. what is left is structural, and unchanged by the fog fix: 900 iterations x depth 20, with roughly every fifth rollout step a Commit that runs the enemy's whole planning pass and resolves a full 12-tick round. cutting `iterations`/`rollout_depth`, or not rolling out through full round resolutions, are the remaining levers
- ~~every unit spawns facing east~~ FIXED, and both suggested fixes turned out to be the right pair: units now turn towards the centroid of their enemies once every placement is spawned (`face_units_at_enemies`), and `UnitPlacement` gained an optional `facing` so a scenario can still place someone looking the wrong way — which is what makes an ambush authorable rather than something the engine decides. worth recording that the note above overstated the damage: measured against the determinism baseline it was 6 rear hits out of 59, not the whole first exchange, because on the 41-hex hexagon units spend several rounds closing and facing updates on move. the bias was real and entirely one-sided though — all 6 fell on side 1 — and after the fix the same 4 seeds produce zero rear hits
- terrain cover is applied twice: hit_chance_inner subtracts `cover / 2` from accuracy and raw_damage then multiplies by `(100 - cover) / 100`. town at 40 is -20 to hit *and* -40% damage, ~52% total. may well be intended, but the number in a mod file reads much weaker than it plays
## Immediate Goals
### Misc
- think about retreating. how does it work IRL?
  - I want to allow the player to conduct an actual retreat; reaching some tiles to leave the map
  - that may be fine, but IRL there are no tiles. maybe allow enemy to attempt to pursue?
- overlays currently reproject via HexOverlay + reposition_map on view rotate. alternative: parent each overlay to its MapTile entity and let Bevy transform propagation carry them (more robust as overlay kinds grow; needs a Hex→Entity index when spawning highlights)
- save/load: derive serde across BattleState/OverworldState/roster while the sim is still small (ChaCha8Rng supports serde). every field added from here on either serializes or becomes a migration problem
- allow better control of units. planned routes are drawn now, but they cannot be shaped: waypoints, reverse movement (penalized), and a face command (uses movement)
- multiple girls in a vehicle, as it makes sense. can be wounded from hits to remove their bonuses (engine already supports multi-crew via crew_slots/crew_best; this is the roster-instance refactor + a wound model + UI)
- ability to place units in a starting zone in battle prep phase; if ambushed spawn in a column
- improve line of sight system, should be easier to hide while seeing enemy. now urgent rather than nice-to-have: `vision_range` is doing detection's job, not eyesight's. at 100 m hexes a commander really can see kilometres, so a hard range cutoff is the only thing keeping anything hidden, and it gets less believable the more the ranges grow. wants a detection roll against terrain concealment plus modifiers for moving and for having just fired (`revealed` already covers the last one)
- occupancy index: `unit_at` is a linear scan over all units, and it is called from `passable()` inside the dijkstra inner loop and from `claimed_by_friend` (another full scan) once per candidate hex in reachable's final retain. so `reachable()` is O(hexes x units) twice over, and MCTS calls it constantly — this is the thing that will hurt once maps are 30x25 instead of 12x9. a `HashMap<Hex, UnitId>` kept up to date on move fixes both, and it is the same refactor the apc needs: "who is in this hex" becomes a real query instead of a first-match
- `accuracy_falloff` is an integer per hex, which was fine when the longest band was 5 hexes and is coarse now that the 88 reaches 16. consider "accuracy lost per 10 hexes", or a float
- `max_climb` means something physical now: at 10 m per elevation level, `max_climb: 1` over a 100 m hex is a 10% grade. that is conservative for tracked vehicles (real limit is nearer 30% sustained). left at 1 for now, but 2 for tracked is defensible and would open up the hills
### Menus
- ability to move girls around between tanks/reserve, and see stats
- ability to move units between companies on campaign map
- overall start menu to pick gamemode, settings menu, choose campaign submenu, activate mods, etc
### Units
- apc/ifv, can carry infantry that can dismount
## Mid Term Goals
- separate engine from game if needed. I want to use this for a roguelike in the future. make a clear deliniation for what is game vs engine in future. (mostly already true: tactics_core has no bevy dependency, the rng is seeded ChaCha8, BattleState is Clone for search branching, and the boundary really is intents-in/events-out. what is left is that VehicleDef/ArmorSpec/MovementSpec are tank-shaped — and those live behind the registry in data/defs.rs, so the seam is where it should be)
### Combat Sim
- morale system, route/retreat when morale too low. affected by flanking and ambushes
- ability to retreat from a battle, with lowered morale. maybe other penalties too
- indirect fire option for artillery. rethink how this operates once implementing chain of command
### Realistic Ballistics
- some sort of simulation for penetration, both for if penetrates and fragmentation once it does
- depends on where vehicle was hit
- (well contained: combat.rs is isolated and hit_breakdown extends naturally to a penetration breakdown)
### Ammo Types
- start with AP and HE, limited amounts
- easily moddable ammo types
### Chain of Command
(WEGO landed: rounds are plan-then-resolve, orders are per-unit intents, and AI is split into planner + doctrine + difficulty. the seams left for this are the planner registry, which can build child planners, and the unused `initiative`/`delegation` doctrine weights)
- planners are held per side; command wants them per formation, with a commander planner owning subordinates that have their own doctrine
- mission-type orders: a commander sets its subordinates' intents instead of a human doing it, and only within radio range
- in battle, works similar to Combat Mission
- on campaign map, can order units to conduct different types of missions. must be in radio range or have another unit relay instructions to modify their mission
#### Units
- command unit. if you lose this unit you lose the battle
  - need to workshop this. should be able to send out smaller units that may not have a higher level command unit. maybe certain tier of commander can only handle so many units? should having higher tiers give some sort of bonus?
  - different levels of AI to complete the given mission depending on commander type sounds interesting, but I need to balance this with giving the player agency
- radio unit. not able to fight well, but extends comms range on map. maybe can find some way to give bonus in battle
- engineer unit to build roads/buildings, repair other units (or self). needs resources to repair, can carry a moderate amount
- logistics unit to carry resources
### Campaign
- girl progression: xp, leveling, skills. fire emblem is a stated inspiration and this is the emotional engine of the genre. sketch the shape early since it lives on the girl-instance model
- basic requisition flow: vehicle costs, side funds, and income all exist but nothing spends money until academy mode. a minimal buy/reinforce loop shouldn't wait for the 4x layer
- battle objectives beyond elimination (see design decisions)
### Game Theme
- late 1960s tech, with a cutesy anime vibe. going for "Wargame Red Dragon but anime"
- stylize entire game. very "programmer graphics" at the moment
### Sprites
- worth contacting an actual artist and paying. but who?
- everything pixelated, except for the girls and maybe some other important aspects
- cute sprites for girls (this is vital)
- individual vehicle sprites, with themes for different schools
- animations for movement, idle, attacking, destruction, etc
- tile sprites
### Audio
- music, engine sounds, gun reports. even placeholder sfx changes game feel enormously
- girl voice barks — cheap characterization for the cute side of the identity
### Tooling
- replay viewer: save the seed + order stream and re-watch. nearly free with the deterministic sim (the same intents replay tick for tick), doubles as a balance tool
- balance harness: batch AI-vs-AI runs with stat summaries, for tuning ammo/ballistics/morale without playing 200 games by hand. starting point exists at crates/tactics_core/examples/playthrough.rs
## Long Term Goals
### Academy Mode
- 4x territory capture mode
- defend your academy, capture others
- manage resources and money
- resources on map
- diplomatic interactions with other academies
### Buildings
- Academy: recruit, train, and manage girls. if you lose this you lose the game
- Factory: build vehicles
- Fort: map that is easily defendable. need to workshop this one
- Road: move faster across terrain
- Warehouse: used to store a limited amount of resources/vehicles, as well as repair
- Comms Tower: can be chained together to improve comms range
- various resource harvesting buildings
### Ronin Mode
- similar to academy mode, but you have no academy. you lose when your marshall dies
- large randomly generated map
- can pledge allegiance to an academy to recieve a stipend from them
- can build buildings and conquer territory
- figure out how to make this meaningfully different from academy mode. maybe make it post-apocalyptic, with very few academies. workshop this idea
