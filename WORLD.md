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
- [x] ~~**W2.4 Halts, and what a halt is for**~~ (done as W4.6b, from the
      chain of command): a commander may detach an
      element to look ahead, which travels as its own coarse entity. The
      halt itself runs on the clock since W3.1 (the last minutes of every
      marching hour); what a commander does with one is still to build.
- [x] ~~**W2.5 Capture and endings read tiles.**~~ **Done 2026-09-26**
      with W3.4: a town fought over goes to whoever holds its tiles when
      the fight ends.

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
      battle is sent anywhere. `open_engagement` (now `enter_fighting`, W3.8) lifts both armies' vehicles
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
      in contact, which is the hysteresis. `close_engagement` (now `leave_fighting`) applies
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
      edge; the defender's ground is the fight's objective. **Done
      2026-09-26:** every town within `towns.contested_within` (30) tiles of
      the fight is an objective on its own tiles, worth `towns.worth` (2) or
      `factory_worth` (4), and when the fight ends each goes to whoever holds
      its tiles. Found doing it: survivors of both sides often end in the
      town they fought over, the contest cancelling — two hostile armies in
      one campaign hex — so one falls back a hex: the loser, or with no
      winner the attacker (tested; not moving it fails). The holder's
      capture is exercised only when somebody holds the town at the end,
      which the test's fight did not produce. `frontier_world` moved from
      Valkyries 14–2 to 10–6.
- [x] ~~**W3.5 Joining.**~~ **Done 2026-09-26** (merging and splitting
      dissolved by W3.8: there is one fight). A column whose next step enters the hex
      of an army already fighting — friend's or foe's — halts on its border
      and joins that fight (now `enter_fighting`, `BattleState::reinforce`,
      `CommandState::add_formation`): its vehicles lifted onto free tiles
      round it, named by the world, arriving as a formation of their own
      (last in seniority), assaulting the contested ground if it attacks
      and moving up onto it if not. A column moves a tile a minute and a
      fight lasts about nine, so a relief has to be close to arrive in time.
- [ ] **W3.6 Work scales with the fight.** *Reopened by W3.8.* It was
      true by construction while every engagement was its own
      `BattleState`; with one front for the world's fighting, the fog,
      `known_enemies`, `incoming` and a playout's clone walk every crew
      fighting anywhere, so two fights far apart pay for each other. Not a
      problem at the campaign's size (16 generated campaigns in 3.7 s, about
      what they took as separate engagements); the cure when it is one is a
      spatial index under those walks, not separate battles.
- [x] ~~**W3.7 `field.rs` replaced.**~~ **Done for generated campaigns**
      (2026-09-26): a clocked campaign never stages a `Clash`; the harness
      and `examples/campaign` run it on the clock and record each engagement
      as a battle. `field.rs` stays for the drawn campaign, which keeps its
      battles-as-events until it is retired.

- [x] ~~**W3.8 One front.**~~ **Done 2026-09-26**, the fourth round's
      ruling (below): there are no engagements, only the fighting.
      `engagement::Front` is one `BattleState` on a window onto the world
      holding every army in contact anywhere, each a formation. Contact, or
      a column reaching the fighting, **enters** it; an army **leaves** when
      none of its crews has had an enemy within reach for the battle's
      stalemate patience, taking its own people's losses with it, while any
      other fight goes on; the front is disbanded when it is empty. Merging
      and splitting (left open by W3.5) are therefore not operations at all.
      The towns fought over are those within `towns.contested_within` of
      some crew in it, kept every round as crews move — with every town in
      the world an objective, a crew was pulled at a factory a day off.
      Events are `ArmyEngaged` / `ArmyDisengaged` per army and
      `FightingOver` for the front. Three tests, each mutation-checked: two
      fights far apart are one battle (and a fresh battle per contact fails
      it); one can finish and its army go while the other goes on (never
      leaving on staleness fails it); only nearby towns are fought over
      (every town fails it). Determinism snapshot unregenerated; drawn
      `frontier` unchanged at 24–7 over 32; `frontier_world` 5–11 over 16
      (6–10 before, one fixed world, inside its noise).

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

- [x] ~~**W4.6a One chain of command, stored.**~~ **Done 2026-09-26.** The
      campaign's armies are gone as stored state: `OverworldState::elements`
      is each side's tree (root, companies, vehicles), an element with a
      `Place` is what stands on the map, and `Column` — what `army(id)`
      returns — is derived from the tree on every read. Neutral: the full
      event traces of 32 drawn and 16 generated campaigns were
      byte-identical before and after, and so was the determinism snapshot.
      Survivors of a fight are matched back to the vehicles they were, so a
      vehicle keeps her node (tested, mutation-checked). Save version 9.
- [x] ~~**W4.6b Detachment as a place in the tree.**~~ **Done 2026-09-26**
      (W2.4, rulings 8, 10, 11). The move order given to a vehicle's node
      sends her out: she takes a place of her own and a standing `Hold`,
      and is still her company's node. `Recall` takes her orders back; a
      node standing on its own with no orders of its own goes home, and its
      place is dropped when it stands with its company. No new army, no
      rejoin order, and no flag saying either. Neutral where nobody is sent
      out: both campaigns' event traces byte-identical. Four tests
      mutation-checked; tour `send-a-vehicle-out`. *Not yet:* sending out a
      platoon (a node with vehicles under it) — the rule is written for any
      node, but the content has no platoons in a company; and a
      detachment's value to a halt (W2.4's "a halt is a moment to send
      someone to look") is the player's to use, since the AI does not.

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
- [x] ~~**W5.1 Chunked rendering.**~~ **First version done 2026-09-26.**
      Zoomed in, the campaign screen draws the ground a chunk at a time —
      the chunk under the camera and its ring, ~9k tiles — straight from
      the generator, and streams chunks in and out as the camera pans
      (`stream_ground`). Still an entity a tile, which is fine at this
      count; merged meshes would let the ring grow.
- [x] ~~**W5.2 Two zoom levels, one camera.**~~ **First version done
      2026-09-26.** Z on a generated campaign zooms from the campaign map
      onto the ground under the selected company (or the hovered hex), with
      every army standing on its own tile, and Z zooms back out. The view
      pivots on the tile it opened at. Zoomed in, the map is for looking —
      orders are given on the campaign map — and the campaign's highlights
      and owner dots are hidden. *Not yet:* a continuous zoom, the campaign
      hexes drawn at their true size and angle over the tiles, and the
      side panel's movement line (still the drawn campaign's hexes a day).
- [x] ~~**W5.3 Tours.**~~ **Done 2026-09-26.** `generated-campaign` runs
      and stops the clock, zooms in and out, and skips to dawn;
      `fight-on-the-ground` plays a fight on the ground from contact to the
      end and back to the map.

### W6 — a world made to order, and made like country

The designer's rulings (2026-09-27, after a playtest): **the world
generation should be more realistic**, and **it should be variable — the
player sets various settings, like Civilization's map setup.**

What the playtest measured (seed 1, all 417k tiles dumped): the layers of
the generator ignore one another — forest is 27–31% at *every* elevation
level and 29% on slopes against 28% on the flat; not one of 17,732 mud
tiles lies within 300 m of water; hedgerow is the fringe of the wood noise,
so every wood wears a hedge halo instead of fields having hedges. Nothing
in it is larger than a campaign hex (the coarsest relief lattice is 48
tiles; a chunk is 41 across), so the campaign map it adds up to is nearly
salt and pepper: 42% of neighbouring campaign hexes share a class against
34% for the same hexes shuffled. Three rivers of ~10 km rise near the rim
and leave by it; the interior has none. Fourteen identical towns of 700 m.
The display then makes it look worse: campaign elevation is drawn at the
battle's 14 px a level on a hex 4 km wide (about 90× exaggeration, so
every mountain hex is a tower), no summary rule names a river so the
campaign map never shows one, and a hex any road touches is "highway".

The shape: **a setting is data, and it is a list of `worldgen` fields to
set.** The generator's numbers already live in the `worldgen` block, so a
choice like "woodland: heavy" is nothing more than `cover.wood_percent: 45`
— written in `mod.json` as options on a setting, addressed by the same json
paths `balance --set` uses, and applied through the same patch. A mod adds
a setting, or an option to one, without a line of Rust. **The default
option of every setting changes nothing**, so a world made with every
setting at its default is the mod's world, exactly — the additivity rule.
A world's rules still travel in its save (W1.6), so what the player chose
is what a loaded campaign stands on.

- [x] ~~**W6.1 Settings as data.**~~ **Done 2026-09-27.**
      `world_settings` in `mod.json` (`WorldSetting`, `WorldOption`): the
      base mod ships six — map size, terrain, woodland, field boundaries,
      rainfall, settlement — of two to four options each, every default
      setting nothing. `WorldGen::with_settings` applies a choice through
      `data::patch`, the walk `balance --set` uses, moved out of the
      harness because the game now uses it too. Towns and rivers are
      densities (`every`), which kept seed 1's world byte-identical (14
      towns, 3 rivers, the same 15 roads) and bumped `SAVE_VERSION` to 10.
      `validate-mods` validates all 324 combinations.
      `OverworldState::from_map_setup` takes the choices and a seed.
- [x] ~~**W6.2 The instruments take a world.**~~ **Done 2026-09-27.**
      `examples/worldgen` and `examples/campaign` take `--world
      size=large,woodland=heavy`; `examples/worldgen --settings` makes one
      world per option and measures it (the W6 diagnosis's columns: clump,
      wood high and low, wet by water). On seed 1 every option moves what
      it names and nothing else — woodland 12/28/45%, hedge 2/9/22%, mean
      level 0.40/1.44/2.31/3.80, towns 8/14/24 — and the realism columns
      read the same bad news on every row (clump 0.05–0.11, wood the same
      share high and low, 0–2% of wet ground by water), which is W6.4–W6.6.
- [x] ~~**W6.3 The setup screen.**~~ **Done 2026-09-27.**
      `crates/game/src/setup.rs`, `AppState::WorldSetup`: before a fresh
      generated campaign, every setting with its option (Up/Down, Left/
      Right), the seed (N for a new one, drawn from the clock — the one
      place the game does, and it never reaches the simulation, which is
      handed the number), Backspace for the mod's own world, and a picture
      of the world at a pixel a tile, hillshaded, with the rivers, roads,
      towns, factories and both sides' companies drawn over it. The
      picture is the real campaign: each choice runs `from_map_setup` on a
      thread of its own (a run of key presses costs one world, after the
      last), and Enter hands *that* campaign to the map
      (`PreparedCampaign`), so what she saw is what she gets. A drawn
      campaign skips the screen; `STAHL_WORLD=default` or
      `size=large,…,seed=4` skips it with a choice, which the three tours
      that are about something else now declare. `world-setup` is the
      screen's own tour.
- [x] ~~**W6.4 Relief with a shape.**~~ **Done 2026-09-27.** Simplex
      noise in place of lattice value noise (no more grid-straight edges;
      octaves turned by the exact 3-4-5 rotation), and two layers under
      the local hills, weighted by data: the landform (uplands and
      lowlands, `relief.landform_percent` 40 at scale 300 in the base mod)
      and ridged ranges (`relief.ridge_percent` 35 at 160) raised where the
      landform is high. Neighbouring campaign hexes' mean heights now
      correlate 0.69 on seed 1 (0.64–0.73 across every setting's options;
      about 0.35 under the old noise, −0.12 for the hills alone). The
      terrain setting's options move the layers as well as the level
      shares. Cover's scale went 18 → 40 because simplex woods come out
      finer than value-noise woods at one scale, and at 18 no campaign hex
      reached deep forest. `GENERATOR_VERSION` (2) travels in a world's
      save and another is refused. `examples/worldgen --picture` writes the
      world as the setup screen draws it (`worldgen::picture`), and the
      `--settings` table gained `h-corr`. The wars on the new ground
      (`examples/campaign`, seed 1's world): 6–2 to the Valkyries as
      before, but 9 battles in 8 campaigns against 16, and two won by
      holding the factories without a fight — the factories now sit in
      lowlands one side reaches first.
- [x] ~~**W6.5 Water that drains.**~~ **Done 2026-09-27.** A priority
      flood from the rim drains every tile; catchment decides what the
      water is — a stream from 3 campaign hexes of it (a new fordable
      terrain, so the network does not cut the land into islands), a river
      from 12, broad (three tiles) from 60 — and a river's valley floor is
      wet meadow. Seed 1: 36 watercourses, 11 of them rivers, 1,474 tiles
      of river against 3 rivers and 290 tiles before; water 0.64% of the
      land; wet ground within 300 m of a river 10% against 0–2%. Rainfall
      now moves the network (dry: 18 courses, 3 rivers; wet: 51 and 25)
      rather than a count. `GENERATOR_VERSION` 3. Creation is 710 ms for the standard
      world (617 at W6.4, 290 before it: the three relief layers are most
      of the cost), 1.3 s for the large one, all on the setup screen's
      thread. The wars (`examples/campaign`): every seed ends, 5–3 to
      Kuhlmann on the default world (6–2 to the Valkyries before). Found
      on the way: on mountainous ground the Valkyries win five of eight by
      holding the factories with little or no fighting — where the
      factories fall relative to each side's start is not yet anybody's
      rule (W6.7). Tours now find a company by name (`hex "1st Company"`,
      from a new `places` fact) instead of a pair of coordinates the
      ground moves.
- [x] ~~**W6.6 Cover that reads the ground.**~~ **Done 2026-09-27.** The
      wood score is the cover noise leaned toward high ground and steep
      ground (`cover.wood_on_high` 25, `wood_on_slope` 20; zero is the old
      rule), still calibrated to `wood_percent`: woods are 51% of the upper
      half of the land and 14% of the lower (28/27 before), 48% of steep
      ground and 22% of flat (29/27). Hedges are the boundaries of fields —
      Worley F2 − F1 on a jittered lattice `field_size` (5) tiles apart —
      laid in districts of hedgerow country (a broad noise, calibrated to
      `hedge_percent`), no longer the fringe of the wood noise: 13% of
      hedge tiles touch a wood. The generic lowland wet ground went from
      20% of level 0 to 8%, because the floodplain carries most of the wet
      now (wet ground within 300 m of a river: 23%, from 10 at W6.5). The
      campaign map reads as regions (clump 0.08 → 0.14). `GENERATOR_VERSION`
      4. Wars: 4–4 over eight seeds.
- [x] ~~**W6.7 Settlements in a hierarchy.**~~ **Done 2026-09-27.** The
      best `towns.cities` (3) sites are cities of `city_radius` (5), and the
      factories go to them. Towns want a river within four tiles, not any
      water: since W6.5 a stream is within reach of nearly anywhere, so
      "by water" had stopped telling sites apart. Villages
      (`villages.per_ten_hexes` 10, radius 1, spacing 12): the best of a
      sample of every ninth tile, low, level and by running water, greedily
      spaced and clear of the towns — town terrain on the ground, but not
      objectives, not campaign hexes' names, not road nodes. Seed 1: 331
      villages, all on the lower half of the land; 41% within 300 m of
      running water against 4% of the land (12% without the pull), towns
      on rivers. Settlement moves villages too (sparse 132, dense 662).
      Roads still join towns only, straight across the valleys rather
      than down them; that and village lanes are left. `GENERATOR_VERSION`
      5. Wars: 4–4 over eight seeds, 14 battles.
- [x] ~~**W6.8 The campaign map drawn at its own scale.**~~ **Done
      2026-09-27.** `scale.overworld_elevation_meters` (30 in the base mod,
      the battle's 10 by default) is the campaign's own vertical scale: a
      generated campaign hex is raised by its land's mean height in those
      units (`Summary::mean_tenths`), so mountains are ranges a level or two
      high rather than towers. It also closes CLAUDE.md's "overworld
      elevation is priced at the battle scale", which no rule reads on a
      generated world. Rivers are drawn over the campaign map as lines
      (`draw_rivers`): no campaign hex is named for one, so the map showed
      none, though a river is the one thing on it a column cannot drive
      across. The chunk lattice is linear, so each river tile's position in
      campaign hexes is solved exactly and projected like a hex. The
      "highway" summary is left as it is: it names only a hex that is not
      a town, mountains or forest, so it draws the road net across the
      open country, which reads as roads.

Found along the way and not yet anybody's rule: on seed 1 from W6.5 on,
the campaign AI does not come for a player who waits (no contact in 16
minutes of fast clock, against about one on W6.4's ground), and on
mountainous ground the Valkyries win five of eight by holding the
factories with little fighting. Where the factories fall relative to each
side's start, and what makes the AI attack, want their own look.

W6.4 onward change what every generated campaign stands on. Saves carry a
world as its seed and rules, and regenerate it with the generator of the
day, so each one either keeps worlds bit-identical or refuses an old save.

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

The fourth round (2026-09-26, after W4):

7. **There is no real separation between engagements** — only different
   things happening on different parts of the map at once, or eventually
   the same part. So there is one front, not fights to merge and split
   (W3.8).
8. **Detaching an element is the player's order only, for now** (W2.4);
   the AI does not detach.
9. **The generated campaign is the default**: the game opens on
   `frontier_world`, and the drawn `frontier` is reached by
   `STAHL_CAMPAIGN=frontier`. **Done 2026-09-26**: `campaign` in `mod.json`
   names it, validated; the game and `examples/campaign` both read it.
10. **There is no separate state for a detachment; it all flows from the
    chain of command.** Asked how far: **one tree a side, and no `Army`
    type** — the side, its companies, their vehicles, and what the map
    shows is a view of the tree (W4.6).
11. **A detached element holds where she was sent until recalled**, as a
    crew's march in battle becomes a hold at the same latitude.
