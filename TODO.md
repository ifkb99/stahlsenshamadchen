# TODO
## Design Decisions to Lock Early
Things that shape everything below them. Deciding late means rework; ordered by cost of delay.
- girls need to be instances, not just definitions: CharacterDef is static data and Unit.crew is id strings. wounds, xp, morale history, reserve membership all need a mutable per-girl campaign object that persists across battles. small refactor now, painful after the campaign layer grows
- what happens to crew when a vehicle dies? right now the unit is just marked dead. bail out / wounded pool / permadeath is a core identity decision (GuP is famously non-lethal) that affects morale, the wound system, and how much players care about the girls
- battle victory conditions: engine only knows eliminated and stalemate. campaigns and scenario variety want per-map objectives in map json ("hold the bridge 10 turns", "exit the map edge"). decide the shape before battle-flow assumptions (muster prompt, apply_battle_result) harden around "battle over = someone died"
- one-unit-per-hex is assumed in a few places (unit_at returns first match). apc passengers will bend this — decide whether cargo lives "inside" the carrying unit (cleaner, keeps the invariant) before writing the apc
- ammo selection adds an ammo field to FireIntent and a per-unit inventory. the intent already carries a weapon index, so this is a one-field change plus the reload/consumption bookkeeping
- need to find better name for "girls". cadets kind of works but feels out of theme. should be something like soldier but cute
- ~~determine scale of a tile and a unit~~ DECIDED: **1 battle hex = 100 m, 1 round = 60 s, 1 tick = 5 s, 1 elevation level = 10 m, 1 overworld hex = 4 km = one battle map**. movement points were already calibrated for this (5 MP tracked on grass = 30 km/h; 7 MP wheeled on road = 42 km/h) — only ranges, vision and reloads had to move. the AW-style 10 HP model is what implicitly sets round length, so committing to a 60 s round commits to the ballistics rewrite: lethality has to come from penetration rolls, not HP attrition. see Realistic Ballistics
- ~~scale should live in data, not rust~~ DONE: `scale` block in mod.json (`hex_meters`, `round_seconds`, `ticks_per_round`, `elevation_meters`, plus `overworld_hex_meters` and `overworld_turn_hours`), typed as `data::Scale`. `TICKS_PER_ROUND` is gone as a const, so round resolution and cooldowns take a registry; `reload_ticks` had to become `Option<u32>` because a serde default fn cannot see the mod being loaded, read it via `weapon.reload(&scale)`. panels now lead with "1.6 km" / "30 km/h" / "elev 30 m" and keep the hex and MP counts in parens. a sibling `balance` block does the same for crew stats — the `awareness / 4` and `driving / 5` divisors are now +5% of the vehicle's base per point, so a gifted crew is worth about a quarter of what they are sitting in. `validate-mods` prints the whole roster through the scale, which is what makes a wrong number visible
- ~~battle maps are 32 hexes wide but an overworld hex is 40~~ DONE: a battle map is now a *hexagon* of one overworld tile, since that is how the overworld draws it. radius derives from the scale (4 km / 100 m = 41 across, 1261 tiles), `MapKind::Battle` implies `MapShape::Tile`, and validation rejects anything that is not that hexagon (`"shape": "free"` opts out for scenario maps). needed no map-format change — `HexMap` was already a sparse hash and a space already meant "no tile". `river_crossing` was rebuilt as a river valley: meandering river, road bridge on the centre row, two mud fords, woods, a town on the east bank, elevation falling from 3 at the rim to 0 along the water. cost: 1261 tiles against 768, and the engine test suite went 9.5 s -> 26 s, which is the argument for the fog batching fix
- overworld elevation shares `elevation_meters` with the battle scale, so frontier's mountains are 20 m tall. wants its own vertical scale once the strategic layer cares about height
## Bugs
- check out warning: `WARN bevy_render::view::window: Couldn't get swap chain texture after configuring. Cause: 'Outdated'`
- warning: `WARN winit::platform_impl::linux::x11::xdisplay: error setting XSETTINGS; Xft options won't reload automatically`
- units cannot move through friendlies on campaign map
- **mcts is too slow to use at the new scale**: one unit's order takes ~35 s on river_crossing (utility takes 77 us for a whole battle's worth). `examples/playthrough.rs` asks for mcts difficulty 4 and no longer produces a single round; the shipped scenario names `utility` for side 1, so actual gameplay is unaffected. this is not new code being slow, it is old code running for the first time: `MctsPlanner::next_order` falls through to the cheap fallback whenever `visible_enemies` is empty, and at 3-hex vision on a 12x9 map that was nearly always. now that deployments start in contact, the search actually runs. the cost is structural — each of 900 iterations rolls out to depth 20, and roughly every fifth rollout step is a Commit, which runs the enemy's whole planning pass and then resolves a full 12-tick round. removing the per-call Vec in `los_clear` was measured and did essentially nothing (13.3 -> 12.8 ms per 160 vision computations), so the fix is one of: cut `iterations`/`rollout_depth` for the new round cost, stop rolling out through full round resolutions, or make `fog::recompute` incremental instead of recomputing every side from scratch after every single shot
- every unit spawns facing east: `battle/mod.rs` hardcodes `facing: EdgeDirection::POINTY_EAST` in spawn_unit, and facing only updates on move or fire. on maps where the sides deploy east/west this hands the eastern side's rear armour (2) to the enemy instead of its front (5) for the whole first exchange. fix is either to face each unit at the enemy centroid on spawn or to add an optional `facing` to UnitPlacement
- terrain cover is applied twice: hit_chance_inner subtracts `cover / 2` from accuracy and raw_damage then multiplies by `(100 - cover) / 100`. town at 40 is -20 to hit *and* -40% damage, ~52% total. may well be intended, but the number in a mod file reads much weaker than it plays
## Immediate Goals
### Misc
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
