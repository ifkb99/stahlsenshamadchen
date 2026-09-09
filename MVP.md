# MVP

What is left before the game is a minimum viable product, in one list.
Written 2026-09-09 at the close of the one-currency arc. The bar is the one
`assets/wiki/reference/command.md` set: **core gameplay solidified to the
point of a complete campaign or two.** The life layer — post-battle
warmth, sound, barks, art — deliberately comes after it, and three boundary
decisions stand: the first campaign ships **without the requisition loop**,
**every cadet is a placeholder**, and **campaign length is unnumbered**
until the gameplay says what a session wants to be.

Each item names where it is tracked in detail. [TODO.md](TODO.md) is the
gameplay backlog, [DIRECTION.md](DIRECTION.md) the design memo, and
[CLAUDE.md](CLAUDE.md) the engineering defects; this file only says which
of those stand between now and an MVP.

## What is already there

The engine is close. Formations, missions, latitude, radios and command
latency, succession, defiance, detection, ballistics with no hit points,
infantry and the ride, stacking, objectives and exits, the campaign map
with standing orders and withdrawal, saves, the academy roll, and the one
currency every planner and the danger overlay price ground in are built,
measured and pinned by 290 engine tests, four determinism seeds and eleven
scripted tours. The AI plays the whole thing against itself and the harness
can ask what any number does.

## What is left

### A campaign that can be finished

- **A win and a loss the campaign can reach.** `frontier` is a sandbox with
  a Lua wrapper that hands out funds; nothing declares what winning it is.
  The design says a side loses its academy or its command unit
  (TODO, *Units* and *Academy Mode*); the MVP needs one rule written and
  shown, even if it is "hold the factories on day N".
- **A campaign half to withdrawal.** An army whose battle half withdrew
  arrives nowhere. It should arrive somewhere (TODO, *retreating*;
  command.md chunk 8 built the engine side).
- **Campaign orders from the map.** The engine takes
  `OverworldOrder::SetMission` with relay; the player has no way to give one
  (TODO, *Chain of Command*). Same for moving units between companies.
- **A wider mission vocabulary once the map has something to do** — raid,
  screen — is explicitly *after* the map has more to do, so not MVP.

### Orders the player can trust

- **Permadeath decided and the wound system given teeth.** Permadeath is
  the intended campaign default and the code default is still off, waiting
  on a tuning pass over `resolve_crew_fate`'s first-draft numbers; a binding
  order can now kill a crew who would have lived, so the player is authoring
  the loss (TODO, first design decision; DIRECTION, open questions).
- **A muster choice for the wounded.** A wounded or lost cadet is kept out
  of her seat at spawn, but the player is never shown it or offered a
  choice (TODO, same item).
- **Allies boost morale** — the designer's ask of 2026-09-09, mechanism
  unwritten (TODO, *Combat Sim*).
- **The three things the currency still cannot say**: the four bare
  constants in `score_tile`, `morale.penetrated` undeclared in the base mod,
  and the spend floor that lets a platoon with no riflemen still frighten
  (TODO, *Combat Sim*; ARCH-TODO, Wave 4). Small, and each is a number the
  designer has to own before a campaign is tuned around it.
- **Better control of a unit's route**: waypoints, reverse, a face command
  (TODO, *Misc*). The single-destination march is what makes orders feel
  like drift on a winding road.
- **Placing units in a starting zone before a battle**, with a column
  spawn for an ambush (TODO, *Misc*).

### Screens the player needs

- **A start menu**: pick a mode, choose a campaign, settings (permadeath
  lives here), and **activating mods at runtime** — the whole
  difficulty-is-a-mod design has no switch without it (TODO, *Menus* and
  *Design Decisions*).
- **Moving cadets between vehicles and a reserve.** The roll shows the
  school; nothing lets the player change it. Seats are positional, so this
  is also the seat-assignment answer cadets.md still owes (TODO, *Menus*;
  DIRECTION step 3, "still open").
- **A post-battle report a player can read.** The `playthrough` narrator
  proves every event carries its story; the screen that tells it with
  portraits is deferred formatting, not archaeology — but a campaign is not
  playable without at least the plain version (command.md, *Sequencing*).

### Content a campaign needs

- **A battle map per terrain.** `choose_battle_map` falls through to the
  first battle map for anything but plains and forest; a fight on a hill or
  in a town lands on whichever iterates first. `battle_forest` also declares
  no exit (TODO, *Content gaps*; CLAUDE.md, *Content gaps*).
- **Cover that does not blind.** Forest and town are the only cover and both
  block sight, so "cover" and "dead ground" are one word to the evaluator; a
  hedge, a wall or a treeline is content nobody has written (CLAUDE.md,
  *Content gaps*).
- **Crew `river_crossing` and `battle_plains`.** Anonymous crews freeze when
  they break, which is correct and invisible; crewing the baseline map means
  regenerating the determinism snapshot deliberately (TODO, *Misc*).
- **A doctrine that orders a movement to contact.** No shipped doctrine ever
  issues `Advance` or `Recon`, so nothing in AI play exercises the
  `Advance`/`Assault` distinction the whole orders memo rests on (TODO,
  planner item; CLAUDE.md, *Content gaps*).
- **Rebalance the roster the harness can now see**: `river_crossing`'s
  orders of battle are 24/76 and a third of every `--sim` sample; the recon
  car is fodder; infantry lose badly at their price; elastic beats massed
  16–7 (TODO, *Balance* and *Design Decisions*).

### The minimum of life

- **Placeholder sound**: a gun report, an engine, one music bed. The
  designer's own note is that even placeholder sfx changes feel enormously
  (TODO, *Audio*).
- **The rest of the vehicle roster drawn.** Two of six chassis have sprites;
  the other four are the generated blob (TODO, *Sprites*).
- **A side identity that is not blue versus red**, decided now while there
  are two sides and one key colour (TODO, *Sprites*).
- **Visual indication of shots and hits** beyond the flash — the designer's
  list from the memo (DIRECTION, *The complaint*).

### Tooling that keeps the MVP honest

- ~~**Tours in CI.**~~ **done 2026-09-09.** The presentation layer is the
  half nothing else gates; `.github/workflows/tours.yml` runs the headless
  runner nightly and on `workflow_dispatch` rather than on push, since it is
  still an order of magnitude slower without a GPU — measured at 207s of
  tour time for all eleven (TODO, *Tooling*). `rust.yml` runs build, test,
  clippy, fmt and validate-mods today.
- **A headless test of a whole field battle into the roster.**
  `finish_battle`'s survivor accounting and `apply_battle_result` have no
  coverage and are the seam where campaign state corrupts silently (TODO,
  *Tooling*).
- **A replay viewer** — nearly free on a deterministic sim and the cheapest
  balance tool there is (TODO, *Tooling*). Useful rather than required.

## Deliberately not MVP

Written down so nobody re-litigates them mid-chunk.

- Requisition, resources, factories, academy mode, ronin mode, diplomacy.
- Character writing, traits earned from play, cadet portraits and barks.
- Hex streaming (one continuous world at two zoom levels), and therefore
  the shape of exits and pursuit.
- Close assault (storming a building the enemy holds), smoke, hit location
  within a facing, premium rounds, artillery leading a moving target.
- A random map generator, shadowcasting field of view, the occupancy index.
- MCTS, parked at parity.
