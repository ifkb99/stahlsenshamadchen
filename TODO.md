# TODO

What is left. Finished work and the reasoning behind it moved to
[DONE.md](DONE.md) — check there before undoing a decision that looks
arbitrary. Engineering defects live in [CLAUDE.md](CLAUDE.md); this file is
gameplay and design.

## Design Decisions to Lock Early
Things that shape everything below them. Deciding late means rework; ordered by cost of delay.
- **what happens to crew when a vehicle dies** — decided in shape, open in tuning. permadeath is an *option* (`CasualtyRules::permadeath`, off by default in code), and what befalls a cadet comes from what hit the vehicle (`Unit.last_hit_by`) and how survivable it is (`VehicleDef.safety`, 0-5 — the recon car is 4 because everyone is a hand's reach from a hatch, the tank destroyer is 2 because a casemate has no turret roof to climb out of). `resolve_crew_fate` turns those into Unharmed / Wounded / Lost / Killed, and `Lost` is explicitly not death: she bailed out, could not reach her own side before the shooting stopped, and walks back over a few days.
  **decided 2026-08: permadeath is the intended campaign *default*** (see `assets/wiki/reference/command.md`, Decisions taken) — the code default flips once the wound system has teeth, i.e. the item below, and after a tuning pass on `resolve_crew_fate`'s first-draft numbers. the gentle mod turning it off stays first-class.
  open: the numbers in `resolve_crew_fate` are a first pass, and the option needs hooking to a settings menu.
  also open, and a hole left by the same work: **nothing stops a wounded cadet from deploying.** the game crate never checks `CadetStatus::is_ready`, so a wounded or Lost cadet still rides into the next battle and merely contributes no bonus. either an army should refuse to field her, or the muster screen should show it and let the player choose — leaving it implicit means the wound system has no teeth
- **land objectives are not a requirement and must stay that way.** a map that declares none is fought to the death exactly as every map was before they existed, down to the evaluator — `a_map_that_names_no_objectives_is_fought_exactly_as_it_was_before` requires two doctrines with wildly different `objective_value` to score every tile identically, and the determinism baseline was untouched by the exit work for the same reason. the missions that are *not* about ground — eliminate, harass, ambush, raid and get out — are the point of keeping it that way, and want a **mission type on the side** rather than a fake objective dropped in the middle of the map
- **willingness to pursue an objective belongs to the commander, not the hex.** right now the appetite is doctrine-wide (`objective_value`, and `withdraw_threshold` gating exits via `EXIT_URGENCY`) which is the correct placeholder but the wrong owner: whether a formation presses an attack, holds, or takes the exit is a decision a *leader* makes out of her nerve, her training and what she has been ordered to do. this is a chain-of-command dependency, not an evaluator tweak — the planner registry and the unused `initiative`/`delegation` weights are the seams it lands on, and `Evaluator::objective_value` is where the number currently comes from
- objectives, still open: **per-objective ownership at setup**, so a map can say who starts holding what; and no shipped scenario exercises `exit` beyond `river_crossing`'s two retreat lanes
- **elastic_defense beats massed_armor 16-7** over 24 sim battles. this is not new, it is newly *visible*: before objectives two thirds of battles drew, so the doctrines never resolved against each other. cover_value 2.2 and elevation_value 1.8 against massed_armor's 0.5/0.8 look like the reason, and `concentration: 1.8` makes the attacker clump into artillery bait. worth a pass now that the harness can see it
- **the recon car is pure fodder**: 1 kill against 48 losses over 24 battles. it is doing its job — driving forward to spot — and dying for it every time. wants either a reason to survive (the detection rework, so spotting does not mean being seen) or a doctrine that does not spend it like ammunition
- one-unit-per-hex is assumed in a few places (unit_at returns first match). apc passengers will bend this — decide whether cargo lives "inside" the carrying unit (cleaner, keeps the invariant) before writing the apc
- ammo selection adds an ammo field to FireIntent and a per-unit inventory. the intent already carries a weapon index, so this is a one-field change plus the reload/consumption bookkeeping
- ~~need to find better name for "girls"~~ — settled 2026-08-24: they are **cadets**, in the types, the UI and the prose. The "cadet is a low rank and we span the whole chain of command" objection is answered in `assets/wiki/reference/tone.md` under *The word: cadet*: at an academy cadet is an enrolment status rather than a rung, so appointments layer on top of it. The warmth the old note was reaching for ("soldier but cute") belongs in the names, not in the collective noun. Still open there: the German, and whether the VN layer coins a nickname on top.
- overworld elevation shares `elevation_meters` with the battle scale, so frontier's mountains are 20 m tall. wants its own vertical scale once the strategic layer cares about height
- **difficulty as mods, not as a setting.** somebody who does not want bailouts, disobedience or permadeath — who wants this to behave like Fire Emblem — should be able to load a mod that turns those off, rather than the engine carrying a `if difficulty == Gentle` branch through every system. the mechanism exists: `scale`, `balance` and `casualties` are blocks a later mod replaces wholesale, and mods have dependencies and load order.
  **the constraint this puts on development:** every harsh system has to be an *additive rule whose absence is the gentle game*. reaction latency collapses to zero ticks when its coefficients are zero; the morale ladder reduces to a single rung where nobody wavers; casualties reduce to everyone walking away; a map with no objectives is the old game. if any of those needs an `if` in Rust to be switched off, it was built wrong.
  still needs: **selecting which mods are active at runtime** (see Menus)

## Bugs
- ~~three of the ten dev tours are red~~ — **mostly my own bad invocation, fixed 2026-08-26.** Two of the three (`press-on`, `orders-explained`) were only ever run without the map they need and passed the moment they got it. The third, `infantry-tour`, was a real regression: since stacking landed, a platoon dismounts onto her *carrier's own hex*, and click-to-select goes through `unit_at`, which answers with whoever comes first in id order — so **the second crew on a hex could not be selected at all** and the tour could never re-mount her. Clicking a hex now cycles through its occupants. The tour was the only thing in the project that ever tried to re-mount a platoon, which is the argument for the runner below.
- ~~units cannot move through friendlies on campaign map~~ fixed 2026-08-26,
  and it was three places, not one: `reachable`'s expansion, the `retain` that
  says what may be *stopped* on, and — the one that actually moves the army —
  the A* cost function in `move_army`, which treated a friend as impassable.
  Same conflation of "cannot stop here" with "cannot cross here" that a hex
  holding one crew was in battle. `an_army_drives_past_a_friend_and_stops_beyond_her`
  pins the budget to exactly the straight-line cost so a detour will not fit,
  and it was checked against all three sites individually — an earlier draft
  asserted only that the far tile was reachable and passed against the old
  rule, because the army just drove around.
- ~~terrain cover is applied twice~~ — stale, closed 2026-08-25. The second
  half died with `raw_damage` in the ballistics rewrite's B1 chunk: a round
  that is through the plate is through, and the tree the shell passed did not
  make the inside of the tank bigger (see `ShotProfile::damage`). Cover now
  reaches a shot in exactly one place, `hit_chance_inner`, and it is data
  rather than a divisor — `balance.cover_to_hit_percent`, 50 by default,
  which is the `/ 2` it replaced. The one other `.cover` read in `combat.rs`
  is artillery splash halving itself against dug-in neighbours, which is a
  different rule and deliberate.

## Immediate Goals
### Misc
- **traits should be gained from gameplay, and should mean more than a skill modifier.** `reckless`, `craven` and `stolid` exist and reach the fight/flight question through `TraitEffect.defiance`; what is missing is the rest — traits earned from what a cadet lived through, and the character work that makes 49 cadets feel like people rather than cores. The designer's note: "much can be postponed until we dive into making characters feel real"
- **`river_crossing` and `battle_plains` are still anonymously crewed**, so every crew on them freezes when she breaks — correct by design (nobody wrote her a temperament) and invisible to the instrument. `battle_forest` now names 23 seats a side. Crewing `river_crossing` means regenerating the determinism baseline, and would break the coincidence that let the defiance chunk land without touching it
- ~~**MCTS: delete or keep?**~~ settled — kept, and the reasoning is in [PARKED.md](PARKED.md) rather than here, so it stops costing attention. it is not maintenance surface anybody owes anything to: no change elsewhere owes it a performance budget
- ~~**the goal chooser is shallow, and that is where difficulty has to come from.**~~ **deepened 2026-08-26.** it now prices the *road* (real terrain cost through `battle::roads`, one Dijkstra for the whole candidate list), the fire along it (`DoctrineDef::route_caution` against the share of the march a spotted gun can see), and who gets there first (`contest_aversion`). difficulty gained a second axis to go with the blur — `difficulty_foresight`, how much of that reasoning she actually does — because more terms make a blur matter *less*, so deepening without it would have made difficulty mean less rather than more. see CLAUDE.md, "The road, not the crow flight"
  what is left of the item:
  - **the arena cannot measure it.** 5-over-1 moved 62.8% → 64.2% and 5-over-3 54.3% → 54.8% at 8 seeds × 36 battles, both inside the noise: the mirrored arena is a radius-10 hexagon with a few forest hexes and no objectives worth arguing about, so there is nothing for a road-reader to be better at. a terrain-varied arena — or a skill table fought on the shipped maps with matched forces — is the instrument this question now wants
  - **the road is priced, not chosen.** `roads` minimises terrain cost, so a crew notices that the cheapest road is exposed but never actually routes *around* the gun. a danger-weighted Dijkstra would, at the cost of the road no longer being measured in rounds
  - **still nothing about what the enemy does next** beyond where she is standing now, nothing about what her own side is doing elsewhere, and no memory
  - `HORIZON` in `ai/goal.rs` joins `IMPATIENCE` on the list of evaluator numbers that want to be data. it is 4 rounds, cut from 6 on 2026-08-26 after measuring what six actually covered (the whole map — see CLAUDE.md's performance section for the 61/106/155/219 µs curve) — so a higher-difficulty commander has very little to be better at. this is now the whole of the "higher difficulty should be better for intelligence reasons" question, and it is tractable in a way it was not when difficulty was jitter on a tile sweep. `ai/goal.rs` is the file; `GoalChooser` is the trait
- **subordinate initiative**: a mission currently *replaces* the candidate list, so an ordered crew has exactly one goal open to her. `DoctrineDef::initiative` (0.3 massed / 0.7 elastic / 0.9 recon) is still unread and this is where it attaches — a high-initiative doctrine admits her own candidates alongside the ordered one and lets her weigh them. nothing else has to move. the designer's eventual shape is a hierarchy of policies (commander / section / battle drill) with initiative as a policy parameter, NOT as an rl epsilon — epsilon is exploration noise and anneals away, which would make a high-initiative crew act randomly rather than independently
- **`IMPATIENCE` and `HORIZON` in `ai/goal.rs`** join `PLATEAU`, `MISSION_WEIGHT`, `contact_scale`, the 0.15 decay, `AMBUSH_PATIENCE` and `BOARDING_ROUNDS` on the list of evaluator numbers that want to be `mod.json` data and want sweeping with `balance --sim`. note two of that family *did* become data on 2026-08-26 — `DoctrineDef::route_caution` and `contest_aversion` — which is the shape the rest want
- **`battle_forest` declares no `exit` objective**, so no crew on it can withdraw whatever the AI decides. content, and it is why one of the review's three battles could not have shown a retreat
- think about retreating. how does it work IRL?
  - the battle half exists: an `exit` objective is those tiles, leaving by one keeps the crew, and `river_crossing` has a retreat lane at each road vertex. what is missing is the **campaign half** — an army that withdrew should arrive somewhere, not merely stop existing on the battle map
  - IRL there are no tiles. maybe allow enemy to attempt to pursue?
  - the lanes are three hexes at the map's west and east road vertices. widening them to the whole edge would make retreat easier to reach from the flanks; keeping them narrow makes the road matter. undecided — **and hex streaming eventually settles it by deleting the question**: the overworld and the battlefield become one continuous world at two zoom levels, crossing what is today a map edge is an ordinary drive, and there is nothing left to declare an `exit` objective *for*. Withdrawal becomes "drive that way until nobody is shooting at you", and the campaign half (where does an army that withdrew arrive) is then the only half left
- overlays currently reproject via HexOverlay + reposition_map on view rotate. alternative: parent each overlay to its MapTile entity and let Bevy transform propagation carry them (more robust as overlay kinds grow; needs a Hex→Entity index when spawning highlights)
- allow better control of units. planned routes are drawn now, but they cannot be shaped: waypoints, reverse movement (penalized), and a face command (uses movement)
- multiple cadets in a vehicle, as it makes sense. can be wounded from hits to remove their bonuses (engine already supports multi-crew via crew_slots/crew_best; this is a wound model + UI)
- ability to place units in a starting zone in battle prep phase; if ambushed spawn in a column
- ~~improve line of sight system, should be easier to hide while seeing enemy~~ — **half done 2026-08-26**, and the half that is done is the detection roll: finding somebody inside your own field of view now costs a die, once per tick, against the ground's `concealment`, the outer band of the spotter's reach, and what the target has driven this round. Firing still bypasses it. The record, the shipped table and the measurements are in [detection.md](assets/wiki/reference/detection.md); the rules that will bite an editor are in CLAUDE.md under "Looking is not seeing".
  what is left is the **FOV algorithm** — shadowcasting instead of a raycast per tile — which changes *which* tiles are visible rather than how long they take to resolve, and which was deliberately kept out of the same stretch so that what moved could be attributed. Two findings from the detection work point straight at it: contact on these maps is made at 11 hexes mean against vision ranges of 8–20, so **line of sight already binds harder than vision range does** — the ground is doing most of the hiding — and round resolution is now 1.87 ms with the raycasting the bulk of it.
  and the third finding is not about sight at all: **nothing in the evaluator wants to be unseen**, so every fought-out column stays inside the seed noise floor. Same shape as stacking. See the goal-chooser item above
- **hex streaming**: the overworld and the battle map become one continuous world at two zoom levels, with no gameplay difference between map tiles and no seam to cross. planned for later. what is already settled and already built for it:
  - **both derived grids are stream-shaped** as of 2026-08-26 — `MoveGrid` and `SightGrid` are keyed on tiles rather than on a map's identity, fold a region in through `extend`/`insert`, and have `build` defined as `extend` onto an empty grid so the streaming path is the one exercised every day. adding a per-tile fact to either is one field on `TileMove`/`Heights` and one line in its `of`
  - **the grids live with the map and load with it** (designer's call). they should be *derived on load*, never written into map assets: a shipped grid is a second copy of `move_cost` and `vision_block` that a retuned mod silently contradicts, and deriving is free anyway — 85 µs + 25 µs for a whole 1261-tile battle map. the refactor is to give `HexMap` and its two grids one owner, so `BattleState`'s `map` / `sight` / `moves` become one `Arc<_>` that a chunk load replaces or extends. the two grids probably become one object at the same time; they are the same structure
  - **the loading rule is not "where units are".** `sight_line_clear` treats a tile it cannot find as *transparent* — right when the map is the whole world, a wrong answer when it is a window, because a ridge in an unloaded chunk would stop blocking and whether a crew can see a tank would depend on what was paged in. the resolved region has to cover everything any unit can **see or shoot** (max vision, max weapon range, artillery reach) plus a margin, and be derived from unit positions alone so it is the same on every machine. determinism is load-bearing here: replays, the search AI and the committed event stream all rest on it
  - **`exit` objectives stop existing**; see the retreating item above
- overworld elevation currently reads at the battle scale (see the correctness item near the top) — one more thing that has to be settled when the two views become one map
- occupancy index: `unit_at` is a linear scan over all units, and it is called from `passable()` inside the dijkstra inner loop and from `claimed_by_friend` (another full scan) once per candidate hex in reachable's final retain. so `reachable()` is O(hexes x units) twice over, and MCTS calls it constantly. a `HashMap<Hex, UnitId>` kept up to date on move fixes both, and it is the same refactor the apc needs: "who is in this hex" becomes a real query instead of a first-match
- `accuracy_falloff` is an integer per hex, which was fine when the longest band was 5 hexes and is coarse now that the 88 reaches 16. consider "accuracy lost per 10 hexes", or a float. **now the last term in `hit_chance_inner` that is neither data nor scale-aware** — the resolver-depth arc moved the other four into `balance` and added three more, so this one stands out
- **artillery still does not lead a moving target**, and the shell-flight
  table has been printing the size of the lead it would need since B3. The
  gun aims at the hex the target is standing on right now; dispersion (R2)
  made the shot honest about the *gun's* error but nothing models the
  gunner's guess about where she will be. Wants the target-track memory the
  chain-of-command work has been circling — last seen hex and tick, on the
  side's own picture — which is the same feature the ROE/target-arc item
  below wants, and which cannot be faked by reading the target's `intent`
  without leaking fog
- **smoke** is the one round type the ammunition schema has always been able
  to carry and nothing has written: the data is trivial and the fog
  interaction is not. Deliberately after the detection-roll work below,
  since both change what can be seen and doing them together would leave
  neither attributable
- `max_climb` means something physical now: at 10 m per elevation level, `max_climb: 1` over a 100 m hex is a 10% grade. that is conservative for tracked vehicles (real limit is nearer 30% sustained). left at 1 for now, but 2 for tracked is defensible and would open up the hills
### Menus
- ability to move cadets around between tanks/reserve, ~~and see stats~~ — the
  seeing half landed 2026-08-25: `R` on the campaign map opens the academy
  roll (`roster_page` in `game/src/overworld.rs`), which lists the school in
  order of battle, vehicle by vehicle and seat by seat, with each cadet's
  availability and battles behind her. What is left is *doing* something about
  it: reassigning seats, and a reserve to hold anyone with no vehicle. Note
  seats are positional (crew *i* fills `crew_slots[i]`), so a reassignment UI
  is also the thing that finally answers cadets.md's "seats are positional"
  limitation
- ability to move units between companies on campaign map
- overall start menu to pick gamemode, settings menu, choose campaign submenu, activate mods, etc
### Units
- apc/ifv, can carry infantry that can dismount
- ~~**more than one unit to a hex** (MVP)~~ built 2026-08-26.
  `VehicleDef.footprint` against `TerrainDef.capacity`, opt-in per terrain so
  clearing the capacities in data reproduces the old game exactly. The rules
  and the traps are in CLAUDE.md under "Two crews on one hex"; the designer's
  shot-at-a-stack question was answered as **the gunner aims and only a miss
  is a lottery** (`balance.stray_percent`, weighted by `presence` = 100 +
  profile).
- **stacking has a mechanism and no reason.** Crews share ground in 69 of
  ~460 rounds, the deepest stack anywhere is 2, and strays therefore fire 5
  times in 1055 misses — the rule is live and almost never met, which is what
  `balance.blind_penalty` turned out to be and is worth fixing before more is
  built on it. **Nothing in the evaluator values sharing ground**: a hex with
  room in it scores exactly what an empty one does, so the only reason anybody
  ever stacks is a platoon dismounting where her carrier stands. Two candidate
  levers, both in `Evaluator::score_tile` and both data-shaped rather than
  Rust-shaped: cover is worth more to two crews than to one (a wood that hides
  a platoon *and* its taxi is better ground than one that hides either), and
  mutual support — a friend on the same hex is a friend who cannot be flanked
  away from you. Measure with `--only sim` and read `crews shared ground in N
  round(s)` and the stray count; both are printed for exactly this reason.
  Note the interaction already measured: infantry survival fell 79 -> 65 of 96
  when stacking landed, because they dismount more and stand where the
  shooting is, and **that is not the strays** (64 survive with
  `stray_percent: 0`). Anything that makes the AI stack *more* will push that
  further, so re-read the infantry pricing item below at the same time.
- **storming the same building as the enemy** (designer's example, 2026-08-26).
  Stacking deliberately does not allow it: a spotted enemy blocks a
  destination whatever the capacity says, and the movement tick ends the
  advance on contact. Wanting it is reasonable and it is *not* a capacity
  change — it is close assault, and it needs four rules stated rather than
  fallen into. What a shot at range zero means (the whole to-hit gradient is
  built on range and the minimum is 1 hex). Who counts as being *in* the
  building for cover, since both sides would be. What a gunner outside can
  see and shoot into a contested hex without picking her own side's crew off.
  And how a shell that lands there sorts friend from enemy — `shell_lands`
  currently resolves against every occupant, which is right for a stack of
  friends and would be friendly fire here. Worth doing after the MVP, and
  worth doing as its own arc.
- **capacity has no per-hex override and no vision or spotting term.** A wood
  full of infantry is currently exactly as easy to find as a wood with one
  section in it, because `concealment` scales a spotter's range per *target*
  and knows nothing about how many are there. A stack should be easier to
  spot and easier to shell — the shell half already works, since
  `shell_lands` resolves against every occupant.

## Mid Term Goals
- separate engine from game if needed. I want to use this for a roguelike in the future. (mostly already true: tactics_core has no bevy dependency, the rng is seeded ChaCha8, BattleState is Clone for search branching, and the boundary really is intents-in/events-out. what is left is that VehicleDef/ArmorSpec/MovementSpec are tank-shaped — and those live behind the registry in data/defs.rs, so the seam is where it should be)
### Combat Sim
- morale system, route/retreat when morale too low. affected by flanking and ambushes
- ability to retreat from a battle, with lowered morale. maybe other penalties too
- indirect fire option for artillery. rethink how this operates once implementing chain of command
### Realistic Ballistics
- **the design and build order live in `assets/wiki/reference/ballistics.md`** — decisions taken 2026-08-13 (no HP, CM-style outcomes; named-cadet crew hits by station; modules as content; ammo loadouts as data with editing stubbed), the pipeline, and chunks B0–B5 with routing. read it before touching combat.
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
- **rules of engagement as assignable states** (noted 2026-08-14, designer's
  ask): ambush discipline currently ships as one engine constant applied to
  any unseen unit — the designer wants leaders able to *assign* postures
  (hold fire / fire at will / ambush-until-decisive, maybe target arcs) so
  units act differently by order. Belongs in the mission/order vocabulary so
  saves, replays and external brains carry it; pairs with the target-track
  memory item (observed headings enabling wait-for-the-flank and artillery
  lead).
- **per-unit tasking, so a taxi can be told to deliver and get out** (noted
  2026-08-14, from the infantry-employment pass; narrowed 2026-08-14 after
  taxi runs landed): a mission belongs to a *formation*, so the carrier in a
  grenadier section holds the ground her passengers were sent to hold, and
  21 of 24 taxis die there whether the side fights flat or under command.
  "Unload here and withdraw" is a sentence about one unit and there is no way
  to say it. `Unit.detached` is the existing seam — it already excuses a unit
  from her formation's standing mission — so the work is a brain that
  detaches an emptied carrier and something for her to do next. Same
  vocabulary as the postures above and probably the same pass.
  The AI now *mounts* and runs pickups, so a surviving carrier has a job to
  go back to; what is missing is only her permission to leave. Two cheaper
  substitutes were tried and measured and neither worked: fractional risk in
  the evaluator (inert — threat only reaches six hexes) and dismounting short
  of the objective (lost two more platoons, because the taxi is usually
  killed by something the side never spotted).
(WEGO landed: rounds are plan-then-resolve, orders are per-unit intents, and AI is split into planner + doctrine + difficulty. the seams left for this are the planner registry, which can build child planners, and the unused `initiative`/`delegation` doctrine weights)
- **the design and build order live in `assets/wiki/reference/command.md`** — formations as data, missions as orders in the order stream (so saves/replays/NN/LLM brains all speak one vocabulary), comms as checks. read it before touching anything below. chunk 0 (the shared `AiDriver` planning loop) is done
- planners are held per side; command wants them per formation, with a commander planner owning subordinates that have their own doctrine
- mission-type orders: a commander sets its subordinates' intents instead of a human doing it, and only within radio range
- in battle, works similar to Combat Mission
- on campaign map, can order units to conduct different types of missions. must be in radio range or have another unit relay instructions to modify their mission — engine side done (chunk 8: `OverworldOrder::SetMission`, advance/hold/withdraw, `overworld_radius` with relay, standing orders carried out on end-turn, withdrawal inherited into the battle). what is left is the campaign *UI* to give them, and a wider mission vocabulary (raid, screen) once the map has more to do
#### Units
- command unit. if you lose this unit you lose the battle
  - need to workshop this. should be able to send out smaller units that may not have a higher level command unit. maybe certain tier of commander can only handle so many units? should having higher tiers give some sort of bonus?
  - different levels of AI to complete the given mission depending on commander type sounds interesting, but I need to balance this with giving the player agency
- radio unit. not able to fight well, but extends comms range on map. maybe can find some way to give bonus in battle
- engineer unit to build roads/buildings, repair other units (or self). needs resources to repair, can carry a moderate amount
- logistics unit to carry resources
### Campaign
- **one continuous world, two zoom levels** (noted 2026-08-13, deliberately deferred until most other MVP work is sorted): vehicles "exist" on the battle layer at all times and the overworld becomes a zoomed-out view of the same space rather than a separate board that spawns battles. no map tiles to retreat from — a withdrawal is driving away on real ground, so pursuit and luring become real maneuvers instead of tile transitions. this dissolves the `from_placements` seam, `choose_battle_map`, and battle-as-event; it is a large structural change and everything above it should settle first
- cadet progression: xp, leveling, skills. fire emblem is a stated inspiration and this is the emotional engine of the genre. sketch the shape early since it lives on the cadet-instance model
- basic requisition flow: vehicle costs, side funds, and income all exist but nothing spends money until academy mode. a minimal buy/reinforce loop shouldn't wait for the 4x layer
### Balance
- ~~**side B wins 56.6% of equal-skill battles on the mirrored arena**~~ fixed
  2026-08-26. Three faults pointing the same way, none of them the resolution
  order this note kept naming: the arena was sheared by the offset conversion
  and was never a mirror, and two tiebreaks read `(x, y)` — a *compass* — so
  every crew in the game edged west wherever the real keys tied. 42.9% -> 49.2%
  of 2304 equal-skill battles, and round one on the symmetric arena at equal
  skill is now mirrored 8 of 8. The general rule is in CLAUDE.md's invariants;
  the account is under "Difficulty is inverted in practice".
- ~~**two of the three battle maps favour an end**~~ re-measured 2026-08-26
  after the coordinate tiebreaks were fixed, and the answer changed: at 8
  seeds x 36 battles the ground is nearly level everywhere.
  `battle_forest` 45.8% west (-2.0 sd), `battle_plains` 55.6% west (+2.7 sd),
  `river_crossing` 49.5% (-0.2 sd). The first reading — forest west 50-22,
  plains east 29-43 at 72 battles — was one draw of a build in which every
  crew edged west. Recorded and accepted in
  `assets/wiki/reference/battlefields.md`, per the designer: ground advantage
  is strategy, it just has to be labelled.
- **`river_crossing`'s two orders of battle are wildly uneven**: side 1 wins
  **436 of 576** (24.1% / 75.9%, -12.4 sd) with the ground cancelled, because
  it fields a tank destroyer where side 0 fields artillery. Nothing to do with
  the river — the ground there measures level. Two reasons it matters more
  than a scenario being uneven usually would. It is the determinism baseline,
  so it is fought in every snapshot; and it is one of the three maps
  `balance --sim` samples, so **every doctrine conclusion the fought-out pass
  prints is partly a conclusion about that tank destroyer**. Decide whether
  the scenario means it (a river crossing with the defender better armed is a
  perfectly good scenario) and if so consider dropping it from the `--sim`
  sample or fielding a fourth, even map alongside it. Re-measure with
  `--only ground`.
- **difficulty 5 over difficulty 3 barely discriminates**: 51.9% of 2304
  battles (+1.8 sd) against 61.1% for 5-over-1. Not a bias — the bias is gone
  — but it is the measurement behind "the goal chooser is shallow" under
  Immediate Goals, and it is the number that item should be judged against
  when somebody deepens the chooser.
- **infantry lose badly at their asking price** (measured 2026-08-14, first
  run of the mustered-forces table): given 60 points, elastic defence buys
  seven mixed units — two rifle platoons, two scout sections, their rides, a
  howitzer — and loses 34–1 to massed armour's heavy tank, medium and tank
  destroyer, which pay 2.4 points a battle for the privilege. Recon pull's
  spread beats elastic 33–3 and fights massed to 19–17. Read it as a question
  about `cost` *and* about employment, in that order: the AI cannot mount
  anybody, cannot plan a taxi run, and holds infantry short-ranged in cover,
  so a platoon is currently paying five points for very little. Re-run the
  table after per-unit tasking lands before touching a single price.
- **`balance.blind_penalty` is inert in every measured battle** (found
  2026-08-26 by the first sweep that could ask). At 36 games, `--sweep
  balance.blind_penalty=40,0,95` prints three byte-identical rows: outcome,
  length, gunnery, artillery, crew cost and every chassis's kills and losses
  all agree exactly. It is not broken — nothing in `ai/` ever sets
  `ShotFired.blind`, so blind fire is a player-only path and an AI-vs-AI
  sample can never move with it. Two consequences. The number cannot be tuned
  by this harness at all, so tuning it needs either a scripted battle or an AI
  that shells ground it cannot see; and **shelling unseen ground is a tactic
  the AI does not have**, which is worth deciding about on its own — see
  artillery lead under Chain of Command, which wants the same target-track
  memory. Do not "fix" this by giving the sweep a special case; the sweep is
  right and it is reporting something true.
### Content gaps
- **a random map generator** (noted 2026-08-14, deliberately not yet): real
  balance work needs terrain the numbers were not tuned on, and the designer
  wants one eventually. Until then the answer is more hand-made maps and a
  harness that samples them — `balance --sim` draws from the whole battle-map
  roster, so every map added is free balance signal.
- **every overworld battle is fought on `river_crossing`.** `choose_battle_map` looks for a map named `battle_<terrain>` and otherwise returns the first battle map in the registry — and the base mod ships exactly one. wants a `battle_plains`, `battle_forest`, `battle_city` and so on, each the radius-20 hexagon; `validate-mods` rejects any that are not, so the sizing cannot drift
- **`river_crossing` still fields partial crews** (noted 2026-08-24). The
  campaign map now names a cadet in every seat of every vehicle; the scenario
  map still carries the original ten across eight vehicles, so half its tanks
  are two-thirds crewed and correspondingly fragile. It was left alone because
  it is the determinism baseline: crewing it up changes the fight and means
  regenerating `tests/snapshots/event_stream.txt`, which is a thing to do
  deliberately and on its own rather than inside a content change. Do it with
  a `balance --sim` before and after.
### Game Theme
- late 1960s tech, with a cutesy anime vibe. going for "Wargame Red Dragon but anime"
- stylize entire game. very "programmer graphics" at the moment
### Sprites
- worth contacting an actual artist and paying. but who?
- everything pixelated, except for the cadets and maybe some other important aspects
- cute sprites for cadets (this is vital)
- side identity is blue vs red, which is the classic colour-blind failure. now is the cheap moment: there are two sides and one marking system, and the magenta team-key mechanism could swap a *pattern* as easily as a colour. much more expensive once there are eight academies and hand-painted art
- individual vehicle sprites, with themes for different schools. STARTED: `VehicleDef.sprite` is loaded and `medium_tank` and `recon_car` have art, drawn by `tools/make_vehicle_sprites.py` — a script rather than hand-painted files so the roster stays consistent in palette, light direction and proportion while it is placeholder-grade. remaining four vehicles fall back to the generated blob.
  three constraints an artist needs to be told: **three isometric frames per vehicle — east, north-east, south-east, 56x40 each, left to right** (the other three directions are those mirrored, and `sync_units` picks a frame and sets `flip_x` rather than rotating, since rotating a drawn isometric vehicle tips it over); **height is deliberately exaggerated** against the map's `ELEV_PX`, the same licence the terrain prisms take, because an honest 3 m tank against 100 m hexes is four pixels tall; and **magenta `#FF00FF` is a key colour** replaced at load with the academy's colour, which is what keeps the world drab and the sides legible
- animations for movement, idle, attacking, destruction, etc
- tile sprites
### Audio
- music, engine sounds, gun reports. even placeholder sfx changes game feel enormously
- cadet voice barks — cheap characterization for the cute side of the identity
### Tooling
- **run the dev tours in CI.** `scripts/dev/run-tours.sh` runs all ten with the boot environment each declares on its own `#!env` line, and is now step 5 of the change loop. `STAHL_HEADLESS=1` runs them with no display and no GPU, under `xvfb-run` with Mesa's lavapipe — **tried, and it works**: `battle-tour` and `after-action` both pass that way on a machine that has no screen of its own, so `ubuntu-latest` is not the obstacle.
  what is left is the budget. it is **roughly an order of magnitude slower**, because everything a tour waits on is paced by animation, animation is paced by frames, and llvmpipe draws this scene at a few frames a second — the full set is minutes on a GPU and looked like half an hour without one. so: a nightly or a `workflow_dispatch` job rather than one on every push, or a fast subset on push and the rest nightly. the presentation layer is the half of this project nothing gates, which is exactly how a selection bug lived through a whole stacking arc
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
- Academy: recruit, train, and manage cadets. if you lose this you lose the game
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
