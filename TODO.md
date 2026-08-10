# TODO

What is left. Finished work and the reasoning behind it moved to
[DONE.md](DONE.md) — check there before undoing a decision that looks
arbitrary. Engineering defects live in [CLAUDE.md](CLAUDE.md); this file is
gameplay and design.

## Design Decisions to Lock Early
Things that shape everything below them. Deciding late means rework; ordered by cost of delay.
- **what happens to crew when a vehicle dies** — decided in shape, open in tuning. permadeath is an *option* (`CasualtyRules::permadeath`, off by default), and what befalls a girl comes from what hit the vehicle (`Unit.last_hit_by`) and how survivable it is (`VehicleDef.safety`, 0-5 — the recon car is 4 because everyone is a hand's reach from a hatch, the tank destroyer is 2 because a casemate has no turret roof to climb out of). `resolve_crew_fate` turns those into Unharmed / Wounded / Lost / Killed, and `Lost` is explicitly not death: she bailed out, could not reach her own side before the shooting stopped, and walks back over a few days.
  open: the numbers in `resolve_crew_fate` are a first pass, and the option needs hooking to a settings menu.
  also open, and a hole left by the same work: **nothing stops a wounded girl from deploying.** the game crate never checks `GirlStatus::is_ready`, so a wounded or Lost girl still rides into the next battle and merely contributes no bonus. either an army should refuse to field her, or the muster screen should show it and let the player choose — leaving it implicit means the wound system has no teeth
- **land objectives are not a requirement and must stay that way.** a map that declares none is fought to the death exactly as every map was before they existed, down to the evaluator — `a_map_that_names_no_objectives_is_fought_exactly_as_it_was_before` requires two doctrines with wildly different `objective_value` to score every tile identically, and the determinism baseline was untouched by the exit work for the same reason. the missions that are *not* about ground — eliminate, harass, ambush, raid and get out — are the point of keeping it that way, and want a **mission type on the side** rather than a fake objective dropped in the middle of the map
- **willingness to pursue an objective belongs to the commander, not the hex.** right now the appetite is doctrine-wide (`objective_value`, and `withdraw_threshold` gating exits via `EXIT_URGENCY`) which is the correct placeholder but the wrong owner: whether a formation presses an attack, holds, or takes the exit is a decision a *leader* makes out of her nerve, her training and what she has been ordered to do. this is a chain-of-command dependency, not an evaluator tweak — the planner registry and the unused `initiative`/`delegation` weights are the seams it lands on, and `Evaluator::objective_value` is where the number currently comes from
- objectives, still open: **per-objective ownership at setup**, so a map can say who starts holding what; and no shipped scenario exercises `exit` beyond `river_crossing`'s two retreat lanes
- **elastic_defense beats massed_armor 16-7** over 24 sim battles. this is not new, it is newly *visible*: before objectives two thirds of battles drew, so the doctrines never resolved against each other. cover_value 2.2 and elevation_value 1.8 against massed_armor's 0.5/0.8 look like the reason, and `concentration: 1.8` makes the attacker clump into artillery bait. worth a pass now that the harness can see it
- **the recon car is pure fodder**: 1 kill against 48 losses over 24 battles. it is doing its job — driving forward to spot — and dying for it every time. wants either a reason to survive (the detection rework, so spotting does not mean being seen) or a doctrine that does not spend it like ammunition
- one-unit-per-hex is assumed in a few places (unit_at returns first match). apc passengers will bend this — decide whether cargo lives "inside" the carrying unit (cleaner, keeps the invariant) before writing the apc
- ammo selection adds an ammo field to FireIntent and a per-unit inventory. the intent already carries a weapon index, so this is a one-field change plus the reload/consumption bookkeeping
- need to find better name for "girls". cadets kind of works but feels out of theme. should be something like soldier but cute
- overworld elevation shares `elevation_meters` with the battle scale, so frontier's mountains are 20 m tall. wants its own vertical scale once the strategic layer cares about height
- **difficulty as mods, not as a setting.** somebody who does not want bailouts, disobedience or permadeath — who wants this to behave like Fire Emblem — should be able to load a mod that turns those off, rather than the engine carrying a `if difficulty == Gentle` branch through every system. the mechanism exists: `scale`, `balance` and `casualties` are blocks a later mod replaces wholesale, and mods have dependencies and load order.
  **the constraint this puts on development:** every harsh system has to be an *additive rule whose absence is the gentle game*. reaction latency collapses to zero ticks when its coefficients are zero; the morale ladder reduces to a single rung where nobody wavers; casualties reduce to everyone walking away; a map with no objectives is the old game. if any of those needs an `if` in Rust to be switched off, it was built wrong.
  still needs: **selecting which mods are active at runtime** (see Menus)

## Bugs
- `WARN bevy_render::view::window: Couldn't get swap chain texture after configuring. Cause: 'Outdated'`
- `WARN winit::platform_impl::linux::x11::xdisplay: error setting XSETTINGS; Xft options won't reload automatically`
- units cannot move through friendlies on campaign map
- terrain cover is applied twice: hit_chance_inner subtracts `cover / 2` from accuracy and raw_damage then multiplies by `(100 - cover) / 100`. town at 40 is -20 to hit *and* -40% damage, ~52% total. may well be intended, but the number in a mod file reads much weaker than it plays

## Immediate Goals
### Misc
- think about retreating. how does it work IRL?
  - the battle half exists: an `exit` objective is those tiles, leaving by one keeps the crew, and `river_crossing` has a retreat lane at each road vertex. what is missing is the **campaign half** — an army that withdrew should arrive somewhere, not merely stop existing on the battle map
  - IRL there are no tiles. maybe allow enemy to attempt to pursue?
  - the lanes are three hexes at the map's west and east road vertices. widening them to the whole edge would make retreat easier to reach from the flanks; keeping them narrow makes the road matter. undecided
- overlays currently reproject via HexOverlay + reposition_map on view rotate. alternative: parent each overlay to its MapTile entity and let Bevy transform propagation carry them (more robust as overlay kinds grow; needs a Hex→Entity index when spawning highlights)
- allow better control of units. planned routes are drawn now, but they cannot be shaped: waypoints, reverse movement (penalized), and a face command (uses movement)
- multiple girls in a vehicle, as it makes sense. can be wounded from hits to remove their bonuses (engine already supports multi-crew via crew_slots/crew_best; this is a wound model + UI)
- ability to place units in a starting zone in battle prep phase; if ambushed spawn in a column
- improve line of sight system, should be easier to hide while seeing enemy. now urgent rather than nice-to-have: `vision_range` is doing detection's job, not eyesight's. at 100 m hexes a commander really can see kilometres, so a hard range cutoff is the only thing keeping anything hidden, and it gets less believable the more the ranges grow. wants a detection roll against terrain concealment plus modifiers for moving and for having just fired (`revealed` already covers the last one).
  **do the detection roll before touching the FOV algorithm.** a detection roll is additive and cheap; shadowcasting changes *which* tiles are visible, which invalidates the determinism baseline and every balance intuition at once — and doing that in the same stretch as the ballistics rewrite would leave two big changes with no way to attribute what moved
- occupancy index: `unit_at` is a linear scan over all units, and it is called from `passable()` inside the dijkstra inner loop and from `claimed_by_friend` (another full scan) once per candidate hex in reachable's final retain. so `reachable()` is O(hexes x units) twice over, and MCTS calls it constantly. a `HashMap<Hex, UnitId>` kept up to date on move fixes both, and it is the same refactor the apc needs: "who is in this hex" becomes a real query instead of a first-match
- `accuracy_falloff` is an integer per hex, which was fine when the longest band was 5 hexes and is coarse now that the 88 reaches 16. consider "accuracy lost per 10 hexes", or a float
- `max_climb` means something physical now: at 10 m per elevation level, `max_climb: 1` over a 100 m hex is a 10% grade. that is conservative for tracked vehicles (real limit is nearer 30% sustained). left at 1 for now, but 2 for tracked is defensible and would open up the hills
### Menus
- ability to move girls around between tanks/reserve, and see stats
- ability to move units between companies on campaign map
- overall start menu to pick gamemode, settings menu, choose campaign submenu, activate mods, etc
### Units
- apc/ifv, can carry infantry that can dismount

## Mid Term Goals
- separate engine from game if needed. I want to use this for a roguelike in the future. (mostly already true: tactics_core has no bevy dependency, the rng is seeded ChaCha8, BattleState is Clone for search branching, and the boundary really is intents-in/events-out. what is left is that VehicleDef/ArmorSpec/MovementSpec are tank-shaped — and those live behind the registry in data/defs.rs, so the seam is where it should be)
### Combat Sim
- morale system, route/retreat when morale too low. affected by flanking and ambushes
- ability to retreat from a battle, with lowered morale. maybe other penalties too
- indirect fire option for artillery. rethink how this operates once implementing chain of command
### Realistic Ballistics
The balance harness prerequisite is met, and battles now resolve rather than
stalemate, so there is finally a baseline to measure a rewrite against.
- **remove the `.max(1)` damage floor** (`combat.rs`). any weapon that hits does at least 1 damage regardless of armour, so an MG firing 6 bursts a round grinds down a heavy tank. the single most consequential thing in the combat model, and the period decision makes it worse — a roster spanning twenty years of armour development is exactly where a damage floor breaks, since the point of a 1943 gun meeting 1960s armour is that it *cannot* get through
- some sort of simulation for penetration, both for if penetrates and fragmentation once it does
- depends on where vehicle was hit
- (well contained: combat.rs is isolated and hit_breakdown extends naturally to a penetration breakdown)
### Ammo Types
- start with AP and HE, limited amounts
- easily moddable ammo types
### Chain of Command
(WEGO landed: rounds are plan-then-resolve, orders are per-unit intents, and AI is split into planner + doctrine + difficulty. the seams left for this are the planner registry, which can build child planners, and the unused `initiative`/`delegation` doctrine weights)
- **the design and build order live in `assets/wiki/reference/command.md`** — formations as data, missions as orders in the order stream (so saves/replays/NN/LLM brains all speak one vocabulary), comms as checks. read it before touching anything below. chunk 0 (the shared `AiDriver` planning loop) is done
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
### Content gaps
- **every overworld battle is fought on `river_crossing`.** `choose_battle_map` looks for a map named `battle_<terrain>` and otherwise returns the first battle map in the registry — and the base mod ships exactly one. wants a `battle_plains`, `battle_forest`, `battle_city` and so on, each the radius-20 hexagon; `validate-mods` rejects any that are not, so the sizing cannot drift
### Game Theme
- late 1960s tech, with a cutesy anime vibe. going for "Wargame Red Dragon but anime"
- stylize entire game. very "programmer graphics" at the moment
### Sprites
- worth contacting an actual artist and paying. but who?
- everything pixelated, except for the girls and maybe some other important aspects
- cute sprites for girls (this is vital)
- side identity is blue vs red, which is the classic colour-blind failure. now is the cheap moment: there are two sides and one marking system, and the magenta team-key mechanism could swap a *pattern* as easily as a colour. much more expensive once there are eight academies and hand-painted art
- individual vehicle sprites, with themes for different schools. STARTED: `VehicleDef.sprite` is loaded and `medium_tank` and `recon_car` have art, drawn by `tools/make_vehicle_sprites.py` — a script rather than hand-painted files so the roster stays consistent in palette, light direction and proportion while it is placeholder-grade. remaining four vehicles fall back to the generated blob.
  three constraints an artist needs to be told: **three isometric frames per vehicle — east, north-east, south-east, 56x40 each, left to right** (the other three directions are those mirrored, and `sync_units` picks a frame and sets `flip_x` rather than rotating, since rotating a drawn isometric vehicle tips it over); **height is deliberately exaggerated** against the map's `ELEV_PX`, the same licence the terrain prisms take, because an honest 3 m tank against 100 m hexes is four pixels tall; and **magenta `#FF00FF` is a key colour** replaced at load with the academy's colour, which is what keeps the world drab and the sides legible
- animations for movement, idle, attacking, destruction, etc
- tile sprites
### Audio
- music, engine sounds, gun reports. even placeholder sfx changes game feel enormously
- girl voice barks — cheap characterization for the cute side of the identity
### Tooling
- replay viewer: save the seed + order stream and re-watch. nearly free with the deterministic sim (the same intents replay tick for tick), doubles as a balance tool
- the game crate is barely tested: 120 tests, 5 of them in `crates/game`, and four of those are devtools/iso unit tests. `nobody_deploys_onto_their_own_way_off_the_map` is the first real one and it caught a battle-ending bug on its first run, which is the argument for more. `finish_battle`'s survivor accounting and the `apply_battle_result` wiring still have no coverage, and that is the seam where campaign state can corrupt silently. a headless test that runs a field battle end to end and checks the roster afterwards would cover most of it

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
