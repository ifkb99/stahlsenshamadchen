# TODO
## Design Decisions to Lock Early
Things that shape everything below them. Deciding late means rework; ordered by cost of delay.
- IGOUGO vs WEGO: chain of command ("everything happens at once", Combat Mission style) replaces the whole turn model — active side, moved/acted flags, counterfire, both AI planners, most of the battle UI. every battle feature built on the current alternating-turn model gets rebuilt when switching. if WEGO is the game (and "Wargame Red Dragon but anime" says it is), decide now and do the rework early, before the battle layer accretes features. the Order/Event boundary makes it tractable: orders become queued intents, resolution becomes a tick loop emitting the same events
- girls need to be instances, not just definitions: CharacterDef is static data and Unit.crew is id strings. wounds, xp, morale history, reserve membership all need a mutable per-girl campaign object that persists across battles. small refactor now, painful after the campaign layer grows
- what happens to crew when a vehicle dies? right now the unit is just marked dead. bail out / wounded pool / permadeath is a core identity decision (GuP is famously non-lethal) that affects morale, the wound system, and how much players care about the girls
- battle victory conditions: engine only knows eliminated and stalemate. campaigns and scenario variety want per-map objectives in map json ("hold the bridge 10 turns", "exit the map edge"). decide the shape before battle-flow assumptions (muster prompt, apply_battle_result) harden around "battle over = someone died"
- one-unit-per-hex is assumed in a few places (unit_at returns first match). apc passengers will bend this — decide whether cargo lives "inside" the carrying unit (cleaner, keeps the invariant) before writing the apc
- ammo selection changes the Attack order signature and adds per-unit inventory. if doing the WEGO rework, design the new order format with an ammo field from the start
- need to find better name for "girls". cadets kind of works but feels out of theme. should be something like soldier but cute
## Bugs
- check out warning: `WARN bevy_render::view::window: Couldn't get swap chain texture after configuring. Cause: 'Outdated'`
- warning: `WARN winit::platform_impl::linux::x11::xdisplay: error setting XSETTINGS; Xft options won't reload automatically`
## Immediate Goals
### Misc
- save/load: derive serde across BattleState/OverworldState/roster while the sim is still small (ChaCha8Rng supports serde). every field added from here on either serializes or becomes a migration problem
- allow better control of units. choose/see pathing, reverse movement (penalized), and a face command (uses movement)
- multiple girls in a vehicle, as it makes sense. can be wounded from hits to remove their bonuses (engine already supports multi-crew via crew_slots/crew_best; this is the roster-instance refactor + a wound model + UI)
- ability to place units in a starting zone in battle prep phase; if ambushed spawn in a column
- improve line of sight system, should be easier to hide while seeing enemy
### Menus
- ability to move girls around between tanks/reserve, and see stats
- ability to move units between companies on campaign map
- overall start menu to pick gamemode, settings menu, choose campaign submenu, activate mods, etc
### Units
- apc/ifv, can carry infantry that can dismount
## Mid Term Goals
- separate engine from game if needed. I want to use this for a roguelike in the future. make a clear deliniation for what is game vs engine in future
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
(depends on the IGOUGO vs WEGO decision above; if WEGO, this moves near the front of the queue)
- change movement/orders to only take place at end of turn. everything should "happen at once", including enemy moves. this is a rather large change, should be done in isolation
    - different order types ie: direct fire, move,
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
- late 1970s tech, with a cutesy anime vibe. going for "Wargame Red Dragon but anime"
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
- replay viewer: save the seed + order stream and re-watch. nearly free with the deterministic sim, great for debugging WEGO, doubles as a balance tool
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
