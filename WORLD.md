# One world

The design memo and checklist for moving from a campaign board that spawns
closed battle maps to **one continuous world at two resolutions**. Written
2026-09-26, before any of it is built. [PLANNING.md](PLANNING.md) is the
companion memo for the AI's planning layer, whose requirement 2 ("nothing
here may assume a small, closed, fully known map") this arc cashes in;
[multiscale-sim-design.md](multiscale-sim-design.md) is the vocabulary
(restriction `R`, prolongation `P`, hysteresis) the bubble below is built
from. When an item lands, strike it through and say what closed it, the way
STRUCTURE.md does.

## The designer's rulings (2026-09-26)

1. **One world clock; a bubble decides the resolution.** There is no
   campaign turn and no battle-as-event. Everything shares one clock. Where
   there is contact, a *bubble* runs the ground at tile resolution — the
   battle engine, ticks and rounds — and everywhere else armies move at
   campaign resolution along the same clock. An army marching while another
   fights is the ordinary case, not an exception.
2. **Bottom up, for consistency.** The tiles are the truth and a campaign
   hex is a summary of the tiles inside it. Generation is a deterministic
   function, so only campaign hexes are resident until somebody needs the
   tiles under one, and generating them again gives the same tiles.
   Campaign movement is reworked to match: the best route between two far
   hexes runs at an angle through the ground, not through the centre of
   every hex on the way.
3. **The player is a cadet in the chain of command.** She orders one level
   down. When a unit within her radio range is attacked the game pauses and
   she can order it; she can call anybody within radio range at any time
   (reinforcements and the like).
4. **All of it, now, in chunks**, each landing on its own branch off
   `develop` with its measurements, in the order below.

### The second round of rulings (2026-09-26)

5. **Campaign pace: both.** The world grows *and* a column marches well
   below a single vehicle's dash. Halts are part of the march — and a halt
   is a moment for a leader to send a smaller element out to look, which
   makes the march itself a place decisions are made rather than a
   progress bar.
6. **Everything is generated**, skeleton included: rivers, roads, towns and
   objectives come out of the generator, not a drawn map. `frontier`
   becomes a seed and a set of parameters.
7. **Reaching down is allowed, at a cost.** The player rides in a
   **command vehicle with an HQ platoon** around her. She is not meant to
   run everything herself — there is far too much — and the AI below her
   is the thing that will be improved to carry it.
8. **Her death ends the campaign**, for now. **A wound is not a death.**
9. **She sits at the top of the radio net**: the net is rooted at her, so
   "anybody within radio range" and "anybody on her nets, relayed" are the
   same set.
10. **Real time with speed controls.** The clock runs; the player pauses it,
    and sets how fast it runs.

### What "bijective" can and cannot mean

A campaign hex is 1261 tiles; no function maps one to the other and back.
What the generator can guarantee — and what the tests will pin — is the
memo's consistency pair:

- **Determinism**: `tiles(seed, hex)` is a pure function. Generate a chunk
  in any order, alone or beside its neighbours, today or after a reload: the
  same tiles.
- **Consistency**: `R(P(c)) == c`. The summary of generated tiles is the
  campaign hex that asked for them. `P(R(x)) != x` is expected — the
  summary is lossy by design.

## The shape

### The world clock and the bubble

- The clock counts ticks (5 s). A bubble advances tick by tick; coarse
  armies are stepped at a coarse cadence, but their position is a function
  of the clock, so any tick can ask where a marching army is.
- **Refinement** (coarse → tiles): an army is lifted into a bubble when it
  comes within *contact reach* of an enemy — the longest vision, weapon or
  artillery reach on either side — **plus the distance it can cover in one
  coarse step**, so a marching column can never step clean over a bubble's
  edge between two checks. A column arriving at a fight joins it as a
  reinforcement on the road it came in by.
- **Coarsening** (tiles → coarse): at a lower threshold than refinement —
  no contact for some rounds and the sides out of reach — or bubbles
  thrash at the boundary.
- **Conservation at the boundary.** Vehicles, cadets, damage, wounds and
  ammunition survive both directions. `P` *reconciles against the cached
  fine state* (where each vehicle was, what was broken) rather than
  re-drawing it; a new bubble lifts an army into a march column along its
  road.
- **Bubbles merge and split.** Two that touch become one engagement; one
  whose halves lose contact becomes two. Unit identity is world-stable, so
  neither needs renumbering.
- **Each bubble draws its own dice**, seeded from the world seed and the
  engagement's key, so a fight is not a function of what else is happening
  or what is paged in.

### The ground

- **A skeleton, realised per tile.** Rivers, roads, towns and objectives are
  graph features a per-tile function cannot invent consistently, so they are
  a skeleton generated once at world creation (rivers down a coarse
  heightfield, towns where the ground suits them, roads as least-cost paths
  between towns, objectives on what the towns hold); every
  other tile is a pure function of `(seed, skeleton, hex)`. A chunk boundary
  does not exist inside the generator, which is what makes a seam
  impossible rather than merely unlikely.
- **The campaign hex is `R` of its tiles**: terrain class, elevation, the
  movement portals below, what is capturable. Computed once per hex,
  cached, and small enough to save.
- **Edits are an overlay.** A save is the seed, the skeleton and the tiles
  that changed (a blown bridge, an engineer's road), never 160k tiles.
- **Resident tiles are derived from unit positions** — every tile any unit
  can see or shoot, plus a margin — so residency is the same on every
  machine and a ridge in an unloaded chunk never stops blocking a sight
  line.

### Campaign movement

- An army always has a real tile position, even at campaign resolution.
- **A column is not a tank.** Its march rate is data (content, not Rust):
  spacing, halts, night, and later fuel. A halt is an event on the clock
  at which her commander may detach a small element — a recon section sent
  ahead — which becomes a coarse entity of its own until it rejoins.
- Routes are **hierarchical A\*** (HPA\*): portals on chunk borders,
  crossing costs between portals from the `MoveGrid`, a search on that
  graph, refined to tiles near the army. A route cuts corners because the
  graph it searches is made of the real ground.
- Capture and the campaign endings read tiles (the factory's tiles), not an
  army standing on a hex.

### Command, with the player in it

- The player is a cadet holding a rank, in a **command vehicle** with an
  **HQ platoon**; what she can order is derived from where she stands in
  `operational_commands`, the same derivation the AI's commanders use. The
  command vehicle is the one TODO.md has been waiting on since the
  headquarters *army* landed (2026-09-09), and `senior_army` is still the
  one function to change when it arrives.
- **The net is rooted at her.** Anybody her orders can reach — directly or
  relayed — she can call; anybody it cannot, she cannot.
- **Reaching down costs.** An order two levels down lands, but it goes
  round her subordinate: that commander's plan is disturbed and her own
  orders to the unit are overridden. The cost is the subject of W4.2.
- **Her death ends the campaign; her wound does not.** Decapitation already
  reads the headquarters flag; the player's cadet dying (not being carried
  out) is the loss.
- She orders **one level down**; below that her subordinates' AI carries it
  out under the existing latitude rules.
- **Real time with speed controls.** The simulation still advances in
  whole ticks; the speed setting is only how many ticks a wall-clock second
  runs, and an order is stamped with the tick it was given on. So the clock
  running faster or slower, or a pause held for an hour, changes nothing a
  replay could see — determinism rests on it.
- The pause is **news, not events**: the game stops when *her* picture
  (`Knower::Commander`) learns a unit within radio range is under fire. A
  pause keyed to the engine's own knowledge would tell her about an enemy
  nobody has reported, which is the fog leaking through the clock.
- **Talking is not commanding.** She can call anybody the net reaches;
  reports flow every way (the 2026-09-23 rank ruling); orders flow one
  level down.

## Checklist

Each chunk is behaviour-neutral unless it says otherwise, and neutral means
the determinism snapshot passes **unregenerated**.

### W0 — seams, before anything moves (neutral)

- [x] ~~**W0.1 Split `HexMap` into terrain and scenario.**~~ **Done
      2026-09-26.** `HexMap` is tiles only; objectives, `victory_score`,
      formations and `loss_conditions` are `map::Scenario`, held by the
      battle as `scenario: Arc<Scenario>` beside `map`. Both setup paths and
      `field::deploy` take a `Battlefield` (terrain + scenario, as one map
      file describes them); `SAVE_VERSION` 6 → 7, because a version-6 battle
      keeps its scenario inside `map` and would open about nothing.
      Behaviour-neutral: the determinism snapshot passed unregenerated, 550
      tests across 26 binaries.
- [x] ~~**W0.2 Intern the terrain id** on `Tile`.~~ **Done 2026-09-26.**
      Per map, not per registry: a `HexMap` carries a palette of the ids
      it uses and each tile an index into it, so a save stays
      self-describing and needs no registry to read (the blocker
      STRUCTURE.md item 8 names). A stored tile is 8 bytes and a map entry
      16, where it was 40 plus a heap-allocated name per tile; `Tile<'_>` is
      now a `Copy` view whose `terrain` borrows the palette, and
      `HexMap::insert` is the one way ground gets in — the door a world
      will fold chunks through. `SAVE_VERSION` 7 → 8. Neutral: snapshot
      unregenerated, perf flat.
- [x] ~~**W0.3 One owner for tiles and both grids.**~~ **Done
      2026-09-26.** `world::World` is the tiles, the `SightGrid` and the
      `MoveGrid`, and `World::insert` is the one door a tile comes in by —
      it writes the tile and resolves it into both grids in one call.
      `BattleState` holds `world: Arc<World>` in place of `map`, `sight` and
      `moves`, and answers `Ground` by asking it. It is `ground::Patch`
      promoted: the reader's tests stitch maps with it as before. Saved as
      its tiles under the old key `map`, so the file did not change and
      `SAVE_VERSION` did not move; `World::rebuilt` destructures by name
      the way `rehydrate` does. The two grids stayed two objects.
- [x] ~~**W0.4 Unloaded is not absent.**~~ **Done 2026-09-26**, with
      W1.1. `World::presence` answers `Known`, `Outside` or `Unloaded`; a
      world is held `Whole` (every battle today: a missing tile is outside)
      or a chunk at a time (`World::chunked`, `load_chunk`, `unload_chunk`:
      a hex in a resident chunk with no tile is outside, one in a chunk not
      resident is unloaded). The engine asks every sight question through
      `World::sight_clear`, which in a debug build of a chunked world panics
      on a line crossing unloaded ground — the grid would read it as open
      sky. A chunked world refuses to serialise until W1.6 gives it a
      format.
- [x] ~~**W0.5 World-stable unit ids.**~~ **Done 2026-09-26.** An id is a
      name, not a position: `units` is kept in id order, and
      `BattleState::lookup` / `lookup_mut` / `slot_of` find a unit by a
      look at the position her id names (a hit in every battle numbered
      from zero) and then a binary search. Ids can be said rather than
      counted — `from_map_numbered`, `from_muster` with a `Muster`,
      `spawn_unit_as` — and must rise in placement order, so id order and
      placement order stay one order. The fog's vision cache and the field
      battle's `origins` are keyed by id rather than indexed by it.
      `a_battle_fought_under_other_names_is_the_same_battle` fights the
      determinism baseline with every unit renamed `1000 + 7i` and requires
      the same transcript, mapped back, on all four seeds.
- [x] ~~**W0.6 Dice per engagement.**~~ **Done 2026-09-26.**
      `world::engagement_seed(world_seed, EngagementKey { when, at,
      attacker, defender })`, SplitMix64 written out and pinned to the bit.
      `OverworldState::seed` keeps the world's seed (saved, `serde(default)`
      0); `Clash::muster` derives `Clash::seed`, and the harness and the game
      stage with it and seed their planners from it. Before, the harness
      seeded the n-th battle by n and the game by the wall clock, so no two
      runs of a campaign — and no loaded save — met the same fight. A
      scenario battle is unchanged: the determinism snapshot passed
      unregenerated. The campaign table moved as dice do (23–9 over 52
      battles → 23–8 and one mutual decapitation over 36); the finding,
      day-two decapitation, did not.
- [x] ~~**W0.7 A closed map is a world with hard edges.**~~ **Done
      2026-09-26**, by construction: every battle stands on a `World` held
      `Whole`, where a missing tile is outside. The proof W0 moved no rule:
      the determinism snapshot passed unregenerated through W0.1–W0.6; the
      ridge arena's `skill` and `ground` tables (`balance --sim --games 36
      --only skill,ground --absolute --arena ridge_arena`, 288 battles) are
      **byte-identical** at `5511560`, before W0, and after it; and
      `examples/mirror --arena ridge_arena` returns four roots, all the
      documented equal-key coin, none a different key.

### W1 — the ground, bottom up

- [x] ~~**W1.1 Chunk geometry.**~~ **Done 2026-09-26.** A campaign hex is
      a chunk of radius `Scale::battle_map_radius()` (20: the 1261-tile
      hexagon a battle map already is) on `hexx`'s hexagons-of-hexagons
      tiling, the one `ground::region_of` reads terrain in.
      `world::chunk_of` is that tiling in integer arithmetic (hexx divides
      in `f32`), held to hexx's answer over radius 300 at four chunk sizes;
      `chunk_hexes` walks a chunk centre-out. The chunk lattice is turned
      about 29° against the tile grid — chunk `(1, 0)` is centred on tile
      `(41, -20)` — so a chunk coordinate is a coordinate, and goes last in
      any tiebreak like every other.
- [x] ~~**W1.2 The skeleton, generated.**~~ **Done 2026-09-26.**
      `worldgen::GeneratedWorld` makes it once: rivers are the
      least-climbing path from a high source to the rim (or into an earlier
      river), towns sit on flat low ground near water, spaced, the best
      sites getting the factories, and roads join every town (Prim, plus
      `extra_links`) by A* over the natural ground. Every number is the
      `worldgen` block of `mod.json` (`data::WorldGen`), optional like
      `command`; `validate-mods` checks it.
- [x] ~~**W1.3 The per-tile generator.**~~ **Done 2026-09-26.**
      `GeneratedWorld::tile(hex)` is a pure function of seed, rules,
      skeleton and hex: value noise hashed from integers and blended with
      `+` and `*` only; shares calibrated as quantiles of the world's own
      noise (a guessed threshold gave 7.6% wood for 28% asked). Ties go to
      a seeded lot, never the compass — coordinates first lined every
      town up on the western edge. `chunk_tiles` feeds
      `World::load_chunk`. The base mod's world is radius 6 (127 campaign
      hexes, 160,147 tiles), made in ~110 ms, every tile generated and
      summarised in ~20 ms. `examples/worldgen` draws it.
- [x] ~~**W1.4 `R`**, the campaign hex summary.~~ **Done 2026-09-26.**
      `GeneratedWorld::summary(chunk)`: the mod's `summary` rules, first
      that holds (a skeleton feature, a mean elevation, a terrain's share),
      and the land's mean level as its elevation — so the campaign's
      vertical scale *is* the battle's, which retires "overworld elevation
      is priced at the battle scale" once the campaign stands on a
      generated world. Since the tiles are the truth there is no given
      coarse state for `R(P(c)) == c` to check; what is pinned instead is
      that the summary is read off the tiles and nothing else, and that
      any order or subset of chunks gives the same tiles.
- [x] ~~**W1.5 Residency.**~~ **Done 2026-09-26.** `world::needed_chunks`
      is a pure function of `(position, reach)` pairs, conservative by a
      chunk's radius. A unit's `reach` is the greatest of her sight, her
      longest gun and her movement over `planner.horizon_rounds + 1`, plus
      `worldgen.residency_margin` (4). `World::window_onto(generated)` is a
      chunked world with a source; `World::settle` loads what is needed and
      forgets the rest, and a battle calls it at setup and after every
      tick's movement (`from_muster_on` puts a battle on such a world).
      `a_battle_on_a_window_of_the_world_is_the_battle_on_the_whole_of_it`
      fights `river_crossing`'s armies and scenario for 12 rounds on a
      generated world twice — held whole, and as a window that held at most
      12 of 61 chunks and paged during play — and requires the same
      transcript. Loading only each unit's own chunk trips the sight guard;
      shrinking reach to a third of sight is *not* caught, because the
      chunk-radius padding (20 tiles) dominates it — the reach term is slack
      today and will matter if chunks shrink. `forget_around` is not wired:
      nothing in a battle holds a `TerrainReader` across ticks yet.
- [x] ~~**W1.6 Saves.**~~ **Done 2026-09-26.** A `GeneratedWorld` saves as
      its seed, its rules (carried in the file, so a retuned mod cannot give
      a loaded campaign different ground) and its chunk radius — under 4 kB
      for 160,000 tiles — and regenerates on load. A window saves as that,
      its resident chunks and an **edit overlay** (`World::edit`), which is
      laid over a chunk every time it loads; `World::rebuilt` reloads the
      resident chunks. A whole world still saves as its tiles, so no
      existing save changed shape and `SAVE_VERSION` did not need to move.
      A battle on a window forks through a real save file mid-fight and
      fights on identically.

### W2 — campaign movement on real ground

- [x] ~~**W2.0 A campaign on a generated world.**~~ **Done 2026-09-26**
      (added: the rest of W2 needs a campaign to move on). A campaign map
      file may say `"world": { "seed": 1 }` (and optionally its own
      `rules`) instead of drawing rows; its campaign map is then
      `GeneratedWorld::campaign_map()` — the chunks' summaries at the
      chunks' own coordinates — and each army says
      `"place": { "feature": "town", "toward": "west", "rank": 0 }` instead
      of `at`, resolved by `GeneratedWorld::place_armies` (the `rank`-th
      town furthest that way on the plane, the nearest free campaign hex if
      taken). `OverworldState::world` keeps the world and saves as how to
      make it. `frontier_world` ships beside `frontier` — every rule the
      campaign has (missions, the net, the endings, the planner) runs on it
      unchanged — and is played with `STAHL_CAMPAIGN=frontier_world` or
      `examples/campaign -- --map frontier_world`; the
      `generated-campaign` tour drives it. Its battles are still fought on
      the shipped battle maps by terrain (W3 moves them onto the ground).
      The unused `map::MapGenerator` trait is retired: a world is not a
      map file.
- [x] ~~**W2.1 Armies have a tile position.**~~ **Done 2026-09-26.**
      `Army::tile` on a generated world (`pos` is always its chunk);
      `GeneratedWorld::stand_tile` is where a column stands on a campaign
      hex (a town's square, else the passable tile nearest the centre), and
      placement, marches and `place_army` keep the two in step.
- [x] ~~**W2.2 Hierarchical routing.**~~ **Done 2026-09-26**, as two levels
      rather than portals. The coarse graph is campaign hexes, each edge the
      tile-level passage between neighbours' standing tiles over those two
      hexes, priced lazily and cached (`OverworldState::passages`, keyed by
      the column's kind). `reachable` is Dijkstra on it within a day's
      march; a march is a coarse A* to the goal, then a tile A* to the first
      coarse hex beyond the day, confined to a corridor of the coarse route
      and its neighbours — so a route cuts across hexes, and what
      `reachable` offers the march delivers (tested; halving the march's
      budget breaks it). Measured on the 417k-tile world: a march across
      it planned whole at the tile level 208 ms; the corridor march 9 ms
      warm, ~1 s the first time (it prices ~1,700 passages at 0.65 ms —
      the coarse heuristic is admissible and so loose for a column that
      pays 2–4 a tile in cover; a weighted one would break the reach
      guarantee). `examples/perf` carries the rows.
- [x] ~~**W2.3 A column's march rate.**~~ **Done 2026-09-26.** The `march`
      block: `column_percent` (20) of the slowest carrier's speed,
      `hours_per_day` (4), `halt_minutes_per_hour` (10) — about 200
      movement points, 20 km of grass, a day for a tank column. A column
      goes where all of its classes can and pays the dearest; infantry ride.
      With it the base mod's world grew to radius 10 (331 campaign hexes,
      417k tiles; 14 towns, 3 rivers): `frontier_world` now runs 3–12 days
      where every seed had ended on day one at radius 6 and the old pace.
      **Finding:** three seeds in sixteen never end — a headquarters reduced
      to one hidden crew is attacked every day in a battle that stalemates
      without contact, the loser keeps its hex, and nothing changes. Latent
      in battle-as-event on any map; W3 dissolves it (a fight is a bubble
      the attacker keeps searching) rather than a rule being added now.
- [ ] **W2.4 Halts, and what a halt is for**: a commander may detach an
      element to look ahead, which travels as its own coarse entity. The
      halt itself runs on the clock since W3.1 (the last minutes of every
      marching hour); what a commander does with one is still to build.
- [ ] **W2.5 Capture and endings read tiles.** Folded into W3.4:
      objectives come from the skeleton when fights are on the ground.

### W3 — the world clock and the bubble

- [x] ~~**W3.1 The clock.**~~ **Done 2026-09-26**, for generated
      campaigns (a drawn one keeps its turns). `OverworldState::clock` counts
      5-second ticks from the first dawn; `turn` is the day it falls in. An
      order sets a `MarchOrder` (the target, whether to go round an enemy or
      into him, the leg of tiles, movement banked in integer hundredths of
      a tick's point) and the clock walks it: at dawn the sides give their
      orders in turn, and when the last has, the day runs for everybody at
      once — halting for the last `halt_minutes_per_hour` of every marching
      hour, bivouacking once `hours_per_day` are marched, and stopping at
      the first tick a column meets an enemy (it halts on the border and
      the fight is there), after which the same end-of-turn runs the rest of
      the day. Dawn heals, restores marching hours, walks every side's net,
      sends held orders and checks the ground held. `advance_clock(ticks)`
      is the entry real-time play will drive: speed is only how many ticks a
      second asks for. The existing loops (the game's, the harness's, the
      planner's `EndTurn`) drive it unchanged. `frontier_world` now runs
      5–13 days. *Still to do:* orders at any tick from any side (W4.4) —
      today they are given in the dawn phases.
- [x] ~~**W3.2 Refinement.**~~ **Done 2026-09-26**, at the contact rule the
      march already has (a column halts on the border of an enemy's hex): no
      battle is sent anywhere. `open_engagement` lifts both armies' vehicles
      onto the tiles round where each stands, gives the world its next unit
      ids, and fights an `engagement::Engagement` — a `BattleState` on a
      window onto the world — one battle tick per clock tick while every
      other column marches. Its AI is planned inside the engine each round
      by planners seeded from the fight's dice and the round, so nothing
      between rounds lives outside the battle and a save mid-fight fights on
      the same (tested tick by tick). *Not yet:* the one-coarse-step margin
      and reinforcements arriving by road (W3.5); an army that reaches a
      fight in progress waits on the border.
- [x] ~~**W3.3 Coarsening.**~~ **Done 2026-09-26**, by the battle's own end
      rules — elimination, or its stalemate clock after rounds with nobody
      in contact, which is the hysteresis. `close_engagement` applies
      casualties and survivors through the same code a battle event uses
      (`apply_losses_and_survivors`, split out of `apply_battle_result`) and
      stands each army where its first surviving vehicle is (tested: a
      survivor stands beside where its vehicles last were; not moving it
      fails that). **It dissolved the hidden-headquarters loop**: all 16
      seeds of `frontier_world` now end (6–26 days), where 3 ran past 60.
      Found on the way: with no ground to fight for, the attacker was sent
      at the one tile the defender occupied, which no crew can choose, and
      the sides sat nine hexes apart until the stalemate clock ran out — so
      an engagement's scenario holds **the defender's ground** as an
      objective (radius 2, value 3) and the attacker assaults its nearest
      free tile.
      **Balance finding:** the Valkyries win 14 of 16 on `frontier_world`'s
      one fixed world (seed 1, where their headquarters starts beside both
      factories); giving Kuhlmann the same doctrine in fights moves it only
      to 12 of 16. A world per campaign (`"seed"` omitted) would measure the
      rules rather than the map.
- [ ] **W3.4 Engagements, not battles.** *Partly done 2026-09-26:* an
      engagement's `over`, `score`, stalemate clock and `check_victory`
      *are* its battle's, used unchanged; there are no exits on the ground
      and armies arrive where they stand rather than being deployed from an
      edge; the defender's ground is the fight's objective. *Still to do:*
      towns and factories inside a fight as objectives of their own, from
      the skeleton (and capture read from their tiles, W2.5).
- [x] ~~**W3.5 Joining.**~~ **Done 2026-09-26** (merging two fights, and
      splitting one, still to do). A column whose next step enters the hex
      of an army already fighting — friend's or foe's — halts on its border
      and joins that fight (`join_engagement`, `BattleState::reinforce`,
      `CommandState::add_formation`): its vehicles lifted onto free tiles
      round it, named by the world, arriving as a formation of their own
      (last in seniority), assaulting the contested ground if it attacks
      and moving up onto it if not. A column moves a tile a minute and a
      fight lasts about nine, so a relief has to be close to arrive in time.
- [x] ~~**W3.6 Work scales with the fight.**~~ **True by construction**
      (2026-09-26): every engagement is its own `BattleState` holding only
      its own units on its own window onto the world, so the fog,
      `known_enemies`, `incoming` and a playout's clone never see beyond it.
- [x] ~~**W3.7 `field.rs` replaced.**~~ **Done for generated campaigns**
      (2026-09-26): a clocked campaign never stages a `Clash`; the harness
      and `examples/campaign` run it on the clock and record each engagement
      as a battle. `field.rs` stays for the drawn campaign, which keeps its
      battles-as-events until it is retired.

### W4 — the player in the chain of command

- [x] ~~**W4.1 The player is a cadet.**~~ **Done 2026-09-26.** A campaign
      map flags a vehicle `command` (the ruling: a flag on an existing
      chassis); its senior cadet, by rank then enlistment, is
      `OverworldSide::commander`, fixed when the campaign begins — after
      that her command vehicle is whichever she rides in. `frontier` and
      `frontier_world` flag the headquarters company's Panther (Anka Weiss;
      Irma for the Valkyries). The net already roots at the headquarters
      army she rides with.
- [x] ~~**W4.2 One level down, and reaching further at a cost.**~~ **Done
      2026-09-26** (core). `BattleState::commanders` names the sides a
      person commands and the vehicle she rides in; on those sides an order
      to a crew outside her company `reaches_down` — it is held at the radio
      even when the crew can hear, lands at the top of the next round, and
      detaches the crew from her company's plan when it does (the ruling:
      latency and a disrupted plan). Her companies are one level down: a
      mission to one is an ordinary order. Empty on every existing battle
      and every AI side.
- [ ] **W4.3 Calling anybody in reach**: reports and requests, distinct
      from orders. *Largely moot at the root of the net*: she has nobody to
      request from, and orders reach any company on her net on any tick.
      What is left is presentation — a way to call a company that is not
      selected (W5).
- [x] ~~**W4.4 The pause on news.**~~ **Done 2026-09-26** (core). With
      `OverworldState::human_command` on, a fight in which a side a person
      commands has a company on her net stops the clock at its planning
      phase (`EngagementAwaitsOrders`, `awaiting_orders`) until she commits
      through `order_in_fight`; the engine plans every other side meanwhile,
      and a company she cannot reach fights under its own leader without
      the clock waiting. More than one fight waiting is one pause, answered
      fight by fight. Orders on any tick: on a clocked campaign an order is
      taken from any side at any time, and a newer one replaces the march
      in progress. `human_command` is off until the game can show a fight
      (W5), so the harness and today's game are unchanged.
- [x] ~~**W4.5 Her death ends the campaign; a wound does not.**~~ **Done
      2026-09-26.** `victory.commander` (both campaigns declare it) makes a
      dead commander a defeat, `CampaignEnd::CommanderKilled`;
      `acting_commander` is her while she is fit and otherwise the most
      senior fit cadet riding with the side, until she is back. It ends 3
      of 32 `frontier` campaigns and 2 of 16 `frontier_world` ones
      (`frontier` 24–7 over 35 battles, from 23–8 over 36).

### W5 — presentation

- [x] ~~**W5.0 The clock in real time.**~~ **Done 2026-09-26** (added). On a
      generated campaign the campaign screen runs the world clock every
      frame at the chosen speed — a game minute, ten minutes, an hour or
      six hours a second (`[` / `]`) — and Space stops and starts it; it
      opens stopped. Enter runs it to the next dawn. The AI sides give the
      day's orders at dawn (`overworld::plan_day`, which asks a planner
      until it is done without ending anybody's turn), standing orders
      set out at dawn for every side, and the player orders any company
      at any time. The banner reads the time of day and the speed. A fight
      waiting for her orders stops the clock. The `generated-campaign` tour
      runs, stops and skips it.
- [x] ~~**W5.0b The fight screen.**~~ **Done 2026-09-26** (added). The
      game turns `human_command` on for a clocked campaign, so W4 is in
      play: when one of her companies makes contact within her reach, the
      clock stops and the battle screen opens as a *view onto the
      campaign's own fight* (`PendingBattle::Engagement`) — it mirrors the
      fight's battle, sends every order she gives (and her staff's, at
      commit) to the fight itself, and resolves it by running the world
      clock a tick at a time, taking the mirror again after each and
      playing the fight's own events (`Engagement::recent`). So the fight
      is resolved in lockstep with every march on the map, and nothing is
      staged. It hands back to the campaign when the fight is over, or when
      another of her fights needs her. On the clock, the campaign screen no
      longer queues a column's every hex crossing as a beat of animation —
      the sprites follow the state, and at six hours a second the beats
      held the clock to the speed of the animation. The
      `fight-on-the-ground` tour runs the campaign until a fight opens,
      commits until it is over, and is back on the map.
- [ ] **W5.1 Chunked rendering.** `map_render.rs` spawns an entity per
      tile, fine at 1261 and not at 160k.
- [ ] **W5.2 Two zoom levels**, one camera.
- [ ] **W5.3 Tours** for the clock, the pause and the zoom.

## Open questions

All answered 2026-09-26. The first round (pace, skeleton, reaching down,
the player's death, the net, the clock) is folded into the rulings above;
the second:

1. **An order lands on the tick it is given**, then travels the net with its
   usual latency. The 60-second round survives as the AI's planning pulse and
   the unit every price is quoted in.
2. **A wounded player hands over.** While her cadet is in the infirmary the
   next senior takes the field by the existing succession rule, and the
   player commands through *that* cadet until hers is back.
3. **A generated campaign names features.** The scenario says "the town
   nearest each side's edge", "every factory", and the generator resolves
   those against the world it made. A campaign is a seed, the generator's
   parameters and a scenario.

The third round (2026-09-26, before W4):

4. **The command vehicle is a flag on an existing chassis** — the HQ
   platoon's lead vehicle — not new content, until a proper command variant
   is designed.
5. **Reaching down costs latency and a disrupted plan**: an order past her
   companies travels the net and lands a round late, and the bypassed
   company's plan for that crew is dropped.
6. **One level down, in a fight, is her companies**: an army is one
   formation in an engagement and she orders it; a crew is reaching down.
