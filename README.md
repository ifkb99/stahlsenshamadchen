# Stahlsenshamädchen

A hex-based tactics roguelike engine in Rust + Bevy, inspired by Girls und
Panzer, Fire Emblem, and Advance Wars: girls in tanks, fog of war, and an
overworld campaign feeding tactical hex battles.

## Running

```sh
cargo run -p stahlsenshamädchen     # the game (starts on the overworld)
cargo run --bin validate-mods       # modder tool: validate assets/mods
cargo test -p tactics_core          # headless engine tests
```

### Controls

| Input | Overworld | Battle |
| --- | --- | --- |
| LMB | select army / issue move | select unit / move / attack enemy |
| RMB / Esc | deselect | deselect / cancel |
| A | — | attack the hovered enemy |
| B | — | blind fire at a tile (accuracy penalty) |
| V | — | selected unit waits |
| Enter / T | end day | end turn |
| Q / E | rotate the view (6 perspectives) | same |
| WASD / arrows / MMB drag | pan camera | same |
| Wheel | zoom (whole-pixel steps, 1x–4x) | same |

Moving an overworld army onto an enemy army starts a battle; survivors and
casualties persist back to the campaign.

## Architecture

Two crates:

- `crates/tactics_core` — the pure simulation. No Bevy dependency: battle
  state is plain data, cloned and stepped headlessly. Orders go in, events
  come out (`BattleState::apply(Order) -> Vec<Event>`). This is what makes
  search-based AI (MCTS), deterministic replays, and fast tests possible.
  Contains the data/mod registry, map format, movement, fog of war,
  line-of-sight, combat, the overworld sim, and the AI planners.
- `crates/game` — the Bevy presentation: isometric hex rendering with
  elevation prisms, six-step view rotation, tile picking, battle/overworld
  UI, and the Lua campaign host (`mlua`, vendored Lua 5.4).

### Key mechanics

- **Fog of war**: three knowledge states per tile (unseen / explored /
  visible). Line of sight respects elevation and terrain height — forests
  and towns block sight, high ground sees over them. Firing reveals a unit
  until it moves.
- **Combat**: hit chance from weapon accuracy, range falloff, crew gunnery,
  elevation advantage, and target cover. Damage from penetration vs the
  struck armor facing (front/side/rear from attack direction), terrain
  cover, and elevation. Direct fire needs line of sight; indirect weapons
  need a friendly spotter; anything can blind-fire at a tile.
- **AI**: swappable `AiPlanner` trait. Planners only see what their side's
  fog allows (no cheating). Included: a utility planner (difficulty =
  scoring noise) and an MCTS planner that searches over the cloned,
  determinized sim. Assign per side in map JSON:
  `"ai": {"planner": "mcts", "difficulty": 4}`.
- **Overworld**: armies, capturable objectives with income, softer fog
  (concealing terrain only), battles triggered by contact.
- **Pixel-perfect rendering**: sprites are drawn at native size, so one texel
  is one world unit. Hex faces are whole-texel sized (64x40, a 64x30 tiling
  pitch), sprite and camera positions snap to the pixel grid, and zoom is
  quantised to a whole number of physical pixels per texel. Without that
  last part a fractional desktop scale factor (1.25, 1.5, ...) feeds
  straight into the projection and smears the art.

## Modding

Everything the engine reads lives in `assets/mods/<mod>/`; the base game is
itself a mod. Mods declare dependencies in `mod.json` and load in dependency
order; later mods override earlier definitions by re-declaring an id.

```
assets/mods/base/
  mod.json           id, name, version, dependencies
  characters/*.json  people: portrait, crew stats (gunnery, driving, ...)
  vehicles/*.json    chassis: armor facings, movement, vision, weapons[]
  weapons/*.json     damage, penetration, range, accuracy, indirect
  terrain/*.json     move costs per class, cover, vision block, income
  maps/*.json        palette + ASCII rows + elevation digits (+ scenario)
  campaigns/*.lua    campaign hooks (on_start, on_turn, on_battle_end)
```

Map files are CDDA-style: a palette maps glyphs to terrain, `rows` paint the
map, and a parallel `elevation` grid of digits raises tiles:

```json
{
  "id": "river_crossing",
  "palette": { "g": "grass", "f": "forest", "w": "water" },
  "rows":      ["ggggffg", "ggwwggg"],
  "elevation": ["0011220", "0001100"]
}
```

Run `cargo run --bin validate-mods` to check every reference (weapons,
terrain, palettes, placements) without launching the game.

Campaign scripts get a curated `game.*` API (`game.message`,
`game.give_funds`, `game.start_battle`, ...) and read-only context
snapshots — see `assets/mods/base/campaigns/demo.lua`.
