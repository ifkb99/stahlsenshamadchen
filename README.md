# Senshamädchen

A hex-based tactics roguelike in Rust + Bevy, inspired by Girls und Panzer,
Fire Emblem, and Advance Wars: cadets in tanks, fog of war, and an overworld
campaign feeding tactical hex battles.

Battles are simultaneous (WEGO): every side writes orders for all of its
units, then the round plays out for both at once. See
[The battle round](assets/wiki/battle/the-round.md).

## Running

```sh
cargo run -p stahlsenshamädchen     # the game (starts on the overworld)
cargo run --bin validate-mods       # modder tool: validate assets/mods
cargo test -p tactics_core          # headless engine tests
```

Full controls, mechanics, and modding notes live in the
[wiki](assets/wiki/INDEX.md).

## Crates

| Crate | Role |
| --- | --- |
| `crates/tactics_core` | Pure simulation — no Bevy. Intents in, events out. Enables headless AI search, replays, and tests. |
| `crates/game` | Bevy presentation: isometric hexes, UI, and the Lua campaign host. |

## Content

Everything the engine reads lives in `assets/mods/<mod>/`. The base game is
itself a mod. See [Modding](assets/wiki/reference/modding.md).

Player-facing encyclopedia articles (and developer reference) ship under
[`assets/wiki/`](assets/wiki/INDEX.md), so they can later be shown in-game.
