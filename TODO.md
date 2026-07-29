# TODO
## Immediate Goals
### Misc
- units should be able to move through friendlies on battle map. currently they get ambushed
- allow better control of units. choose/see pathing, reverse movement (penalized), and a face command (uses movement)
- multiple girls in a vehicle, as it makes sense. can be wounded from hits to remove their bonuses
- ability to place units in a starting zone in battle prep phase; if ambushed spawn in a column
- improve line of sight system, should be easier to hide while seeing enemy
- check out warning: `WARN bevy_render::view::window: Couldn't get swap chain texture after configuring. Cause: 'Outdated'`
- review the rest of this list, are there any structural changes that should be made? easier now than later
### Menus
- ability to move girls around between tanks/reserve, and see stats
- ability to move units between companies on campaign map
- overall start menu to pick gamemode, settings menu, choose campaign submenu, activate mods, etc
### Units
- apc/ifv, can carry infantry that can dismount
## Mid Term Goals
### Misc
- morale system, route/retreat when morale too low. affected by flanking and ambushes
- ability to retreat from a battle, with lowered morale. maybe other penalties too
- change overall theme to late 1970s tech
- indirect fire option for artillery. rethink how this operates once implementing chain of command
### Game Theme
- late 1970s tech, with a cutesy anime vibe. going for "Wargame Red Dragon but anime"
- stylize entire game. very "programmer graphics" at the moment
- need to find better name for "girls". cadets kind of works but feels out of theme. should be something like soldier but cute
### Sprites
- worth contacting an actual artist and paying. but who?
- everything pixelated, except for the girls and maybe some other important aspects
- cute sprites for girls (this is vital)
- individual vehicle sprites, with themes for different schools
- animations for movement, idle, attacking, destruction, etc
- tile sprites
### Realistic Ballistics
- some sort of simulation for penetration, both for if penetrates and fragmentation once it does
- depends on where vehicle was hit
### Ammo Types
- start with AP and HE, limited amounts
- easily moddable ammo types
### Chain of Command
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