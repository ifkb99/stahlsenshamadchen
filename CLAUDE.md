# CLAUDE.md

Engineering notes for this repo: how it fits together, the invariants worth
protecting, and the known defects. [DIRECTION.md](DIRECTION.md) is the current
design argument — why the next few chunks are being built at all — and carries
the live state of that plan. Gameplay and design work lives in
[TODO.md](TODO.md) — this file is for things that are wrong or fragile in the
code rather than things not yet built. Where an item is already tracked in
TODO.md it is cross-referenced, not repeated. Finished work and the reasoning
behind it lives in [DONE.md](DONE.md); read it before undoing a decision that
looks arbitrary. [PARKED.md](PARKED.md) says why code with no callers is
still in the tree, so that answer does not have to be carried here.
[STRUCTURE.md](STRUCTURE.md) carries the seams that are in the wrong place —
a rule living in the wrong crate, a contract asserted in prose that nothing
checks — as against the rules that are wrong, which are here.

**Start here:** `.claude/skills/tactics-dev/SKILL.md` is the working guide —
the instruments this project has for answering questions about itself, the
change loop, and the specific ways it has fooled people before. This file
covers what the code *is*; that one covers how to work on it.

## Commands

```sh
cargo run -p stahlsenshamädchen     # the game (starts on the overworld)
cargo run --bin validate-mods       # validate assets/mods; prints the scale table
cargo test -p tactics_core          # headless engine tests (the real suite)
cargo run -p tactics_core --example playthrough [seed]   # narrated AI battle
cargo run --release -p tactics_core --example perf       # hot-path timings
cargo run --release -p tactics_core --example balance    # what the data does
cargo run --release -p tactics_core --example balance -- --sim   # ...fought out
cargo run --release -p tactics_core --example balance -- --sim --points 100  # richer armies
cargo run --release -p tactics_core --example balance -- --brains --brain-games 64  # which planner
cargo run --release -p tactics_core --example balance -- --help  # every flag, with examples

# what does this number do that the old one did not?
cargo run --release -p tactics_core --example balance -- \
    --sim --games 36 --sweep balance.partial_penetration_percent=40,55,70
# ...and how much of that was the dice?
cargo run --release -p tactics_core --example balance -- --sim --games 36 --sweep seed=0,1000,2000
# what is a longer-sighted commander worth?
cargo run --release -p tactics_core --example balance -- \
    --sim --games 36 --sweep planner.horizon_rounds=2,4,6
# ...for one table, in about three seconds
cargo run --release -p tactics_core --example balance -- \
    --sim --games 36 --only skill --absolute --sweep seed=0,1000,2000,3000
```

**The battles run across every core** (`run_all`, `std::thread::scope`, no
new dependency). They are genuinely independent — each has its own map,
state, planners and seeded rng, sharing only the read-only registry — so what
parallelism could damage is not a battle but a *table*. That is guarded
rather than hoped for: every job writes into the slot it owns and results are
folded in job order, which is seed order, so the printed numbers cannot depend
on which core finished first. A balance figure that moved with scheduling
would be worse than a slow one, because it would look exactly like noise. If
you add a table, fold it in seed order too, and check a run against itself
before trusting it. `Tally::merge` is where that contract is written down:
every field is a sum, a concatenation or a union of sums, and a field that is
none of those — a maximum, a ratio, a last value — breaks it.

`--jobs N` caps how much of the machine one invocation takes, so several
sweeps can run beside each other; **no printed number moves with it**, and
that is checked rather than assumed (`--jobs 1 / 3 / 7` on the same batch are
byte-identical, as is the parallel fought-out pass against the sequential one
it replaced).

### Asking what a number does, instead of what it is

`--set path=value` changes one number before anything runs and `--sweep
path=a,b,c` runs the whole thing once per value and prints the rows side by
side, with a second table of differences from the first row. Both address
fields by the name the json uses — `balance.partial_penetration_percent`,
`weapon.howitzer_105.dispersion`, `morale.rungs[2].accuracy`,
`planner.horizon_rounds`, `vehicle.medium_tank.profile` — and they reach
*every* field of every block
and every content map, because the patch is a serde round trip through the
same representation a save file holds rather than a hand-written list of the
knobs somebody thought to expose. A field added to `Balance` tomorrow is
sweepable the same afternoon with no change to `balance.rs`. A path that
names nothing is an error listing what was actually at that level, because
the silent alternative is a sweep whose rows all measured the same game and
agreed with each other beautifully.

Three things about it are worth knowing before reaching for it:

- **It is the same thing as editing `mod.json`, and that is checked.** A
  hand-edited copy of the mod tree swept with `--sweep mods=a,b` and the
  equivalent `--sweep balance.moving_target_per_hex=5,40` print
  byte-identical difference rows. That equivalence is the whole claim; if it
  ever stops holding, the override machinery has become a second game.
- **Three axis names are not fields.** `mods=` selects the tree to load,
  which compares two *versions* of the content rather than two numbers in
  one. `seed=` selects which battles get fought, which puts the noise floor
  in the same table and the same columns as the difference being read — the
  most useful of the three: at 36 games, four seeds alone move the tank
  destroyer's kills between 74 and 89 and the mean battle length by 1.1
  rounds, larger than several differences this project has quoted as results.
  `points=` sets the requisition budget, so its rows are two *armies* rather
  than two numbers; at 100 points instead of 60 every doctrine buys armour
  and elastic defence goes from 0–12 to 8–4 against recon pull, which is the
  first thing the infantry-pricing item in TODO should be read against.
- **Every table with a win column prints the band a level pairing wanders
  in**, computed from the battle count rather than remembered: at 36 battles
  a genuinely even pairing still lands anywhere from 12–24 to 24–12 nineteen
  times in twenty, and at the default 12 it lands as far out as 9–3. That one
  line is the answer to the failure this section exists because of — `28–8`
  and `26–10` were quoted at each other for months as evidence about a change,
  and they are the same number. It is a *floor*, not the answer: these battles
  share maps, forces and doctrines, so they scatter wider than a coin does,
  and only a seed sweep measures how much wider.
- **A swept table with three or more variants prints its own `spread` line**
  — the widest gap between variants in that column, under every row. Under a
  `--sweep seed=` that line *is* the measured noise floor; it is printed
  unasked because the alternative is remembering to work it out.
  `--absolute` switches the variant lines from differences to their own
  values, which is what reading a *range* wants.
- **`ground` says what each battlefield is worth to the end that deploys on
  it**, which is worth knowing rather than worth removing: a scenario where one
  side holds the ridge is a scenario about holding a ridge. It separates the
  ground from the order of battle by fighting every map twice per seed **with
  the two armies exchanged between the ends** — placements stay exactly where
  the map put them and only the vehicles standing on them swap — so summed that
  way `west`/`east` differ only by the ground and `OB-0`/`OB-1` differ only by
  the force. Its own control is built in: `battle_plains` and `battle_forest`
  ship *identical* orders of battle, so their `OB` columns must read level
  whatever the ground does, and they do (36–36 and 37–35).

  Measured at 8 seeds × 36 battles, in
  `assets/wiki/reference/battlefields.md` with the method and the re-measure
  command: **`battle_forest` leans slightly east (45.8% west, −2.0 sd) and
  `battle_plains` slightly west (55.6%, +2.7 sd)** — small, real, and not
  worth changing. **`river_crossing`'s ground is level (49.5%) and its armies
  are not: 24.1% / 75.9% to the side fielding a tank destroyer where the other
  fields artillery**, which is −12.4 sd and by far the largest term on any of
  these maps. It is the determinism baseline *and* one third of the
  fought-out pass's sample, so every doctrine conclusion `--sim` prints is
  partly a conclusion about that tank destroyer.

  Note what the first reading of this table got wrong, because it is
  instructive: at 72 battles a map, before the coordinate tiebreaks were
  fixed, it said forest favoured the *west* 50–22 and plains the *east* 29–43.
  Both were one draw of a build that made every crew edge west. Sweep the
  seed, and re-measure after anything that changes how the AI moves.
- **`--only <tables>` prints just the ones named** (`roster`, `detect`, `hit`,
  `pen`, `kills`, `flight`, `flags`, `sim`, `delegation`, `mustered`, `skill`,
  `ground`).
  `--sim` runs four tables that fight battles, and paying for the other three
  while iterating on one is the kind of friction that ends in the instrument
  not being run at all. Sixteen seeds of the skill-gap table alone take about
  three seconds. `--csv` emits the digest and every swept table long-form
  (`table,row,variant,column,value`), which goes straight into a pivot.
- **The sweep compares every table that fights battles.** The fought-out
  digest — outcome, length, gunnery, artillery, crew cost, kills and losses
  per chassis — and then the delegation tax, mustered forces and the skill
  gap, each printed with the baseline's numbers on the first line of a row
  and everybody else's as differences from it. Those three go through `Grid`
  (named rows, named columns, numbers) and `fought_grids`, which is one list
  called by both the single run and the sweep: a table that prints itself
  cannot be compared, and a table added to only one of those two lists would
  quietly stop being swept. `--verbose` still prints each variant's full
  analytic pass and full fought-out report.
- **`--seed` is an offset, not a base.** Zero is the sample every number this
  project has quoted was measured on; any other value shifts all four
  fought-out tables together, which is what makes sweeping it a re-draw of
  the whole report rather than of one table in it. The per-table constants it
  is added to (`FOUGHT_SEED`, `DELEGATION_SEED`, `MUSTER_SEED`, `ARENA_SEED`)
  are the ones each table was born with, kept for exactly that reason.

`balance` is the content-iteration loop, and it is built around the kill chain
rather than around damage. The analytic pass is instant and answers "what did
that number just do" by standing two vehicles on an empty field and asking the
*real* combat code — `preview_attack` with the round under test forced into the
racks through `set_loadout`, not a reimplemented formula, so the report cannot
drift from the game. It prints P(pen) per gun × round × target × range band,
expected shots to knock out, shell flight times, and a "worth a look" section
that judges a gun on blast overmatch as well as penetration. `--sim` fights
whole battles and adds what killed them (brewed / wrecked / abandoned / crew
out), what it cost the cadets, the ammunition economy, artillery's hit rate on
occupied ground, the delegation tax, the mustered-forces table and the
skill-gap table.

**Mustered forces is the only table that does not fight a given order of
battle.** `tactics_core::force::muster` hands each doctrine the same
requisition budget (`--points`, default 60) and lets it buy its own army:
roles are recognised off the chassis — indirect weapon, on foot, carries
somebody, sees farther than it shoots, otherwise armour — and each is wanted
in proportion to the doctrine weight that already names that appetite, with
`concentration` deciding how repeatable a buy is. It deliberately **pays the
asking price rather than hunting for value per point**, because dividing
appetite by cost makes every doctrine buy a swarm of the cheapest chassis and
hides the thing the table is for: a doctrine that wins reliably at equal
points is one whose preferred hardware is underpriced. Its first verdict was
blunt — massed armour's three heavy vehicles beat elastic defence's seven
mixed ones 34–1 while losing 2.4 points a battle.

It earns its keep immediately. It reports that the 105 mm shell's blast
overmatches even the Löwe's thinnest plate, so a direct hit wrecks a heavy tank
without consulting the penetration gate at all; that half of all deaths are
ammunition fires; and that a difficulty-5 side still does not beat a
difficulty-1 one cleanly.

### Objectives

A battle map may declare `objectives` — named sets of hexes, each either ground
to `hold` or an `exit` to leave by — and optionally a `victory_score`. The
rules are small and the consequences are not:

- **Objectives are map data, not battle state.** They live on `HexMap`
  (`map.objectives()`), because which hexes are the bridge cannot change during
  a fight. The battle carries only `objective_held` (parallel to that list) and
  `score` (by side). That split is why both setup paths — `from_map` and the
  overworld's `from_placements` — get objectives with no new arguments.
- **Control persists and contest cancels.** Standing on a hex takes it; driving
  away does not give it back; two sides on it makes it nobody's. Points are
  paid once at the end of a round, never per tick, or the size of every score
  would be an accident of `ticks_per_round`.
- **`EndReason::Stalemate` no longer implies a draw.** Losing contact ends the
  shooting; `BattleState::leader()` then says who won it. `winner: None` now
  means the score was actually level. Anything reading a battle result must
  handle `(Some(winner), Stalemate)`.
- **An exit is not ground, and leaving is not dying.** A vehicle that reaches
  an `exit` it is entitled to use (`side` restricts it — an exit anyone may
  take is a lane both armies take on round one) leaves the board: `alive` goes
  false and `exited` goes true. **Never classify a unit at the end of a battle
  by `!alive`.** Use `surviving_units()` and `lost_units()`; `alive` means "on
  the battlefield", which is what targeting, movement and fog want, and reading
  it for "did she come home" records a successful withdrawal as a dead crew.
- **`check_victory` reads the score before the board.** A withdrawing force
  reaches its target on the same tick its last vehicle drives off; checking
  elimination first would hand that battle to whoever was still standing.
- **The AI will not run for an exit unless it is losing.** The pull is gated on
  the doctrine's `withdraw_threshold` against the vehicle's own damage, so an
  intact crew scores every exit at zero. Without that gate the lane is a free
  win — every unit drives off on the first round. Whether a unit is *permitted*
  to leave is a chain-of-command question and deliberately not the evaluator's.
- **A map that declares no objectives behaves exactly as before**, including in
  the evaluator — `a_map_that_names_no_objectives_is_fought_exactly_as_it_was_before`
  pins that by requiring two doctrines with wildly different `objective_value`
  to score every tile identically. This is the same additivity rule that
  difficulty-as-a-mod imposes, and it is the one to re-check when touching
  `Evaluator::objective_value`.

The reason they exist is not scenario variety, it is that the AI had no reason
to advance: with elimination as the only victory condition, holding the best
cover on the map is optimal play, and the stalemate rate *rose* as difficulty
noise fell (11/12 at zero noise). See TODO.md under Design Decisions.

### Looking is not seeing

Spotting used to be geometry and nothing else: a crew saw every tile inside a
hard `vision_range`, and an enemy standing on a tile she could see was an enemy
she had found, instantly, whatever the ground and whatever the target was
doing. At 100 m to the hex a commander with twenty hexes of vision is claiming
two kilometres — which she can genuinely see across — so certainty at 1,999 m
and nothing at 2,001 m is a cutoff rather than eyesight, and it was the only
thing on the battlefield keeping anything hidden.

Finding somebody inside your own field of view now costs a roll. The design
record, the shipped table and the measurements are in
`assets/wiki/reference/detection.md`; what follows is what will bite someone
editing `fog.rs`.

- **The geometry is untouched.** Line of sight, range and
  `VehicleDef::concealment` (which shortens a *spotter's* reach against one
  target) decide whether a look is possible at all. `Balance::detection_chance`
  decides how long that look takes, and the two are deliberately separate
  questions — which is also why the far-range term reads the spotter's **own**
  reach and never the concealment-shortened one. Compounding them put a platoon
  at three hexes on 75% of "reach" and charged the same fact twice.
- **A search buys an acquisition, never the watching afterwards.** A contact
  the side already holds is not re-rolled, and neither is a crew who has fired
  (`revealed`, checked before any die). Drop the first clause and every found
  enemy flickers in and out of the picture tick by tick.
- **One look per target per tick**, recorded in `SideFog::searched`. The fog is
  recomputed after movement, after fire and after **every individual shot**, so
  rolling per call would make finding somebody a function of how much shooting
  happened to be going on nearby. The look is spent even when the chance was
  zero — which is why a test that pokes `moved` between recomputes has to let a
  tick go by first.
- **The best-placed spotter rolls, not each of them.** A roll per pair of eyes
  makes the printed chance a lie by however many crews are looking: at four
  spotters a nominal 2% is 8%, and the whole usable range of the knob collapses
  into single digits. More eyes still pay, through watching more *ground*.
- **`detection_certain_percent` is the near band and it is not optional.** The
  first draft had none and gave two tanks three hexes apart on open grass an
  82% chance of noticing each other per tick; nobody searches for the tank
  300 m away in an open field. It surfaced as two dozen failing staged tests.
  100 is its neutral value — the whole reach is the near band, nothing fades
  in, **no die is thrown** — and that is the additivity contract, pinned by
  `a_mod_that_asks_for_no_search_spots_exactly_as_it_always_did` down to the
  rng's stream position. It is pinned separately from the determinism snapshot
  on purpose, because the base mod now declares detection numbers and that
  snapshot is therefore a record of the rule being *on*.
- **A staged test that needs two crews in plain sight goes through `seen(..)`**,
  the twin of `registry_wireless()` in `tests/engine.rs` and `tests/save.rs`. A
  test about shells, or a dismount reflex, or a binding order must not also be
  a test of whether anybody happened to find anybody on the tick it was set up.
- **What the rule has not got is a reason**, exactly as stacking has not. The
  mechanism is live and every fought-out column stays inside the seed noise
  floor, because nothing in the evaluator wants to be unseen: the AI never
  sits still to stay hidden and never prefers concealing ground *for its
  concealment*. It is a player-facing rule until the goal chooser learns to
  want it — and that is the shallow-goal-chooser item in TODO.

### Before the plate: what a gunner is up against

The far side of a shot is deep — struck hex face, obliquity, an enumerated
scatter die, the loader's choice of round, an interior weighted by what is
physically in it. The near side was five lines until the resolver-depth arc
(`assets/wiki/reference/ballistics.md`, Arc R). `hit_chance_inner` is now
eight terms and **every one of them is data**: `weapon.accuracy`, range
falloff, `balance.accuracy(gunnery)`, `balance.downhill_bonus`,
`balance.cover_against_accuracy`, `vehicle.profile`, the two motion terms,
the rung's `accuracy`, and `balance.blind_penalty`.

Four things about it are load-bearing.

- **`hit_chance` names a target by id, not by hex.** Half of what makes a
  shot hard is a fact about *her* — how big she is, whether she is moving —
  and a bare coordinate answers neither. Every firing path has a real
  target, blind fire included: shelling a tile resolves against whoever
  turns out to be standing on it (`fire_at_tile`).
- **`Unit.moved` is hexes crossed this round, incremented at the single
  place a unit changes hex** and zeroed in `begin_round`. It is not
  `move_credit`, which is what she has *left*: a vehicle parked all round
  and one that just finished a four-hex dash are both out of credit. One
  increment site is what makes an ordered march, the battle drill's dash
  for cover and a frightened crew's flight all cost the same accuracy.
  Zeroing it at `begin_round` is why the planning phase reads zero for
  everybody, which is the truth — nobody has driven yet.
- **The motion terms are per hex, not per "she moved".** A hex is 100 m and
  a round 60 s, so one hex is a walking pace and five is thirty km/h; a flat
  penalty prices them identically and throws away the only thing that makes
  a fast chassis' speed a defence rather than a way of arriving sooner.
  Firing on the move costs more per hex than being the thing fired at,
  because laying a gun off a moving vehicle is harder than tracking a mover
  from a stable one — if those ever cross over, halting to shoot has stopped
  being worth anything.
- **`hexes_under_way` reads state and never the hypothetical `from`, and
  this was measured rather than assumed.** A draft counted the distance from
  her real position to a candidate tile as driving, on the reasonable
  ground that reaching a tile means crossing to it. The planner scores
  *ground*, though, and what makes a hill worth taking is the shooting done
  from it over the rounds she sits there — almost all of it halted.
  Charging every candidate tile except the one under her tracks put a
  standing bias on staying put: 36 games gave **three stalemates where the
  baseline had none, at 15.9 rounds against 13.6**. That is precisely the
  pathology land objectives exist to remove, rebuilt one accuracy term
  lower down. Dropping the branch restored 13.6 rounds and zero stalemates
  with the terms fully live.

Two more distinctions worth keeping straight:

- **`profile` is about being hit; `concealment` is about being found.** One
  reaches the gunner's arithmetic, the other scales a spotter's range. A
  platoon in the open has been seen and is still thirty people lying in a
  field. `profile` is deliberately not derived from armour, capacity or
  class, so a mod can describe a lightly armoured but enormous vehicle.
  It is zero on every armoured chassis in the base mod, which is what makes
  the infantry numbers attributable to this one field.
- **Suppression is a number on a morale rung, not a second fear system.**
  Pressure is already collected in one place (`apply_pressure`) and already
  walks a ladder the mod declares, so what being shot at costs a gunner is
  `MoraleRung::accuracy`. Additivity falls out for free: a one-rung ladder,
  or one whose rungs say nothing about accuracy, has no suppression at all
  and needs no `if` in Rust to switch off.

**Dispersion is a different fact from flight time, and the shell carries
both.** `ShellInFlight` has `at` (the map reference the gunner laid on) and
`impact` (where the round comes down), rolled from `WeaponDef.dispersion` —
a percentage of the range flown — when the shot is fired, because that is
when the barrel, the charge and the lay stop being adjustable. Four things
about it:

- **Flight time models the target moving; dispersion models the gun.** Until
  this existed a shell aimed at a *parked* vehicle arrived on her with
  certainty, because the only inaccuracy modelled was the ticks she had to
  be elsewhere. There is still deliberately **no hit roll for a shell** — a
  round that comes down on an occupied hex hits what is on it, and adding a
  blind-fire penalty on top would price the same scatter twice.
- **A percentage of the range flown, not a flat radius**, because dispersion
  grows with range — which is the whole reason a battery registers before
  firing for effect. The base howitzer's 4% is exact at 300 m, one hex at
  2 km, two at 4 km.
- **The draw falls off toward the edge.** A ring at radius two holds twice
  the hexes of a ring at radius one, so drawing a hex uniformly from the
  disc would put most shells on the rim and almost none on the aiming point.
  `scatter` takes the smaller of two ring rolls, then walks `hexx`'s own
  ring order and indexes it — a pure function of coordinates, unlike
  iterating a set, which is the mistake this project has already made once.
- **`shell_lands` resolves against `impact` everywhere** — the direct
  occupant, the splashed neighbours, and `ShellLanded`'s own hex. Displacing
  the impact buys nothing if the burst still resolves against the aim, and
  the event reporting the impact is what lets a player see her own battery
  walk off the target instead of concluding the game moved her enemy.
  `impact` is deliberately not `#[serde(default)]`: the default would be
  `Hex::ZERO`, so an old save's airborne shells would come down on the map
  corner — a silent wrong answer where refusing to load is the loud one.

### Behind the plate: how far through is through

The pipeline in `ballistics.md` always had three outcomes — clean
penetration, partial penetration, bounce — and B2b shipped two, so a round
that scraped through a plate spent exactly the budget of one that vastly
overmatched it. That is the flattening the whole no-hit-points model exists
to avoid, one layer further in: it makes the *margin* of a penetration mean
nothing, and margin is most of what separates a gun that can just about
manage a target from one that eats it.

- **`penetration_roll` returns a share, not a bool.** `None` is a bounce;
  `Some(share)` is how much of its budget the round spends inside,
  interpolated by `Balance::penetration_share` from
  `partial_penetration_percent` at exactly parity up to 1.0 at
  `clean_penetration_percent`. Linear rather than a step, so there is no
  cliff for a marginal shot to sit on and no threshold a modder discovers by
  bisection. `clean_penetration_percent: 100` is the game before the band
  existed, which is the additivity contract, and it is checked the strong
  way: the previous chunk's event stream passes byte-identical.
- **The analytic twin enumerates what the die samples**, exactly as
  `penetration_chance` and `penetration_roll` already did for the gate.
  `penetration_share(balance, pen, armor, scatter)` averages the share over
  the scatter outcomes that get through, and `round_worth` multiplies by it.
  Once a marginal penetration is worth less, "did it get through" is no
  longer enough to price a shot with, and a planner without this trades a
  certainty for a technicality.
- **`ShotHit::damage` is what was spent, not what the datasheet says.** The
  share is rolled once, in `resolve_impact`, and handed to
  `behind_armor_effects` as `spent` — which is also what `savage` now reads,
  so a round that broke up on the plate does not get the overmatch that
  skips "wounded". It replaced the whole `ShotProfile` parameter there,
  which is the tell that the profile was only ever consulted for that
  number.

Tuning note, because the shape of it will recur: at a floor of 40% the band
swung the doctrine table to 25-11 and the tank destroyer to 99 kills — the
rule was right and the number was loud. 55% keeps the rule visible (TD 88
kills against 76 before the band) at 19-17, which is the parity the other
chunks held.

The instrument for all of this is `balance`'s **`to hit`** table, the twin of
the penetration table: every cell is `hit_breakdown` against a medium tank on
open grass, with the attacker's motion shown as a gradient (1/3/5 hexes)
rather than one "moving" column, because the gradient *is* the rule. The
roster table gained a `profile` column and `--sim` reports what share of
shots were laid from a vehicle under way — a resolver term nobody's guns ever
meet is a term that changed nothing. `Event::ShotFired` carries `moving`
beside `blind` and `opportunity` for the same reason, and so the log can say
why. The shell-flight table gained a `spread` column beside `target moves`,
which is the two facts side by side. And a penetration cell reading `52·66%`
gets through half the time and spends two thirds of its budget when it does;
a bare number is a round with margin to spare.

### What a shot is worth, and what keeps a battle open

Both of these are one-line arithmetic with battle-length consequences, and
both were wrong in the same way: a number that looked reasonable and never
consulted the thing it was about.

- **`round_worth` is the only shot price in the game, and its blast half
  reads the struck plate.** It backs the loader's AP-or-HE choice *and* every
  planner's shot pricing, so the two can never disagree about what a round is
  for. Blast is priced by `blast_worth`, which is `overpressure`'s twin case
  for case — plate zero pays its rating outright, overmatch pays the target's
  remaining substance, and anything else pays one effect roll at
  `overpressure_chance`, or **nothing at all when no exterior module is left
  to break**. `blast_overmatches`, `overpressure_chance` and
  `exterior_modules` are shared between the pricing and the resolver on
  purpose; if a fourth case appears in one of them and not the other, the
  gun goes back to firing at a number instead of at a tank. There is
  deliberately **no blast-to-damage constant** — `points_per_effect` already
  is one, and the old `BLAST_WORTH = 0.3` is what made a 105 value a heavy
  tank's glacis exactly as highly as an open-topped carrier's roof.
- **`ShotProfile::plate` is the listed plate and `effective_armor` is the
  sloped one.** Blast reads the first because a burst crushing a hull is not
  defeated by an angle solid shot would skip off. Reaching for
  `effective_armor` in a blast calculation is the easy mistake here.
- **The stalemate clock counts accomplishment, not effort.** Hits, breakages,
  burnings, deaths and departures reset it; **a bounce does not**. That is
  what stops a gun that cannot hurt anybody from holding a decided battle
  open — and the anti-livelock intent the list was written for survives
  anyway, because a bounce that *does* something raises `ModuleHit` or
  `CrewHit` and those are still counted. Adding an event to that list is
  adding a way for a battle never to end; do it deliberately.

### The log is a net, not a narrator

Lines are radio traffic: one of ours speaks with her call sign in front of her
("Anvil 1: moving to the Great Glade"), and anything else is a spot report
with nobody's voice on it ("Contact — Wotan 3"). It is **presentation only** —
events carry the same ids, hexes and flags they always did, so `ScriptFacts`,
the script harness and the replay read exactly what they read before.

Two rules hold it together:

- **`Event::heard_by` is the one filter, it lives in core, and its match is
  exhaustive.** It is a rule about *knowledge*, not about drawing — the same
  family as `spotted_enemy_at` and `picture()` — so it sits beside the event
  enum in `battle/orders.rs` where the engine's own tests can reach it. It
  spent a long time in `crates/game` instead, and everything that went wrong
  with it follows from that: no test could touch it, `playthrough` printed
  both sides' secrets, and the claim in this bullet that there was *one*
  filter was simply false. `UnitSpotted` was being filtered separately in the
  renderer by a differently-worded question ("was the spotter human-
  controlled" rather than "was the spotter mine"), which agree at one human
  side and diverge at two.

  The exhaustiveness is the other half. A `_ => true` gives every event added
  tomorrow an audience by default rather than by decision, and three variants
  rode that default for months — two of them printing an enemy crew's morale
  rung into the player's log. A new variant now fails to compile until
  somebody has said who hears it, the same bargain `Mission::slot` makes.

  **Two callers, and they must not drift.** The log filters after draining;
  `drive_ai` filters *before queueing*. The second is not tidiness: planning
  events go through the same paced animation queue as combat, and
  `accepting_orders` is false while that queue has anything in it — so an
  event nobody will print still costs the player a beat of not being able to
  give orders. One `SetOut` per unit put nine of them in front of every
  planning phase, and the symptom was the infantry tour clicking into a game
  that was not listening. Anything new that emits events during **planning**
  must go through this.
- **Command traffic is a side's own business, and so is the inside of her
  hull.** Orders, contact troubles, the radio queue, *where a crew has decided
  to go*, what is broken or bleeding in there, how much ammunition is left,
  and **what rung her nerve is on** are all on her own net. Fighting events —
  shots, wrecks, brew-ups, a tank visibly reversing out of the line — stay
  side-blind, because anybody on the field can see them. The line is drawn at
  the deed rather than the reading of it: you watch her withdraw and draw your
  own conclusion, and `Defied`'s named rung is not yours to have. Morale was
  the lone exception to this until 2026-08-26 — you could not see inside her
  tank but you could read her nerve.
- **A spot belongs to the side that made it.** Being found is not something
  the found crew is told; she learns it when the shooting starts.

### Goals: the seam the AI is meant to be replaced at

`Goal` (`battle/command.rs`) is what one crew means to do — `Take(hex)` or
`Hold` — kept across rounds and cleared when it finishes. The split around it
is deliberate and is the thing to preserve:

- **`ai/goal.rs::candidates` is shared knowledge; `GoalChooser` is judgement.**
  A learned policy replaces the chooser and nothing else, so its action space
  is five or six statements rather than 1261 hexes, and it inherits pathing,
  boarding, dismounting, opportunity fire and defiance from an executor that
  already works. In options-framework terms: candidates are the initiation
  set, the chooser is the policy over options, `Goal::finished` is the
  termination condition, and the utility planner is the intra-option policy.
- **A goal lives on the `Unit`, not in the planner.** `tests/save.rs` forks a
  battle through a save file and requires the same future; a planner's private
  memory does not survive that, so a commitment held there would make saving a
  game change how it is played.
- **A mission replaces the candidate list; it does not join it.** Letting a
  mission compete as one candidate among the objectives silently undid step 1
  of DIRECTION.md — a crew under orders and a crew with none started choosing
  the same ground, and `a_cut_off_unit_keeps_the_orders_she_had` caught it.
- **Subordinate initiative widens that list by exactly one entry**, and the
  boundary is the thing to preserve. `DoctrineDef::initiative` admits the tile
  this round's own sweep picked beside the ordered ground, and
  `planner.deviation_cost * (1 - initiative)` is charged for taking it. It
  governs **how she carries out an order, never whether she believes it** —
  step 1 part 3 of the memo, applied one layer down: she may fight from ground
  she chose short of the map reference, and she may not decide the far
  objective was the better idea.

  **The obvious wider version was built and is wrong.** Admitting her own
  objectives and pricing them the way an unordered crew prices them fails
  three tests, the sharpest being `a_cut_off_unit_keeps_the_orders_she_had`,
  because a mission's ground is worth 2.0 on the evaluator's scale and a
  shipped objective is worth 2 to 5 — so an order becomes systematically
  cheaper than terrain, which is DIRECTION.md's original complaint rebuilt
  inside the fix for it. No setting of `deviation_cost` repairs that: a flat
  charge large enough to hold a 0.9-initiative crew is one that freezes a
  0.3-initiative one, which is the tell that the model is wrong rather than
  the tuning. Deciding another objective matters more is `Unit::detached`, and
  it is a chain-of-command decision.

  At `initiative: 0` the list is the two entries it always was — the widening
  is a membership guard rather than a coefficient, so the chooser draws
  exactly as many blurs from the rng as before and a doctrine with no
  initiative is bit-for-bit the old game. Same shape as the `noise > 0.0`
  guard beside it, and for the same reason.
- **A long march is walked through `movement::step_toward`**, shared with a
  commander's personal `tasking`. `SetMove` is refused past this round's
  budget — correct for an order, and the reason the planner only ever *scored*
  ground it could reach. Two implementations of "closest reachable" would be
  two answers to where she is going.
- **One crew per piece of ground**: `candidates` drops a hex a friend is on or
  already making for. That is the dispersion job the `PLATEAU` tie-break was
  doing, said once and visibly instead of buried in a sweep.
- **`IMPATIENCE` prices the drive.** Without it every crew walks to whichever
  single hex scores highest, which is the queue `PLATEAU` was invented to
  break up, rebuilt a level higher. It belongs in `mod.json` with the rest of
  the evaluator's numbers — see TODO.
- **Difficulty applies to the chooser, not to the tile sweep.** That is what
  makes it mean something: a worse commander goes to the wrong place, which a
  player can see and punish, instead of twitching between interchangeable
  hexes. A per-candidate draw is safe over six meaningful options and was not
  safe over ninety interchangeable ones — the argmax is the difference.

### The road, not the crow flight

The goal chooser priced a march as `distance / speed`, which on a map with a
river in it is not a small error: the far bank is five hexes away and twenty
hexes of driving, and a crew who cannot tell the difference marches at the
water. It also knew nothing about what happens on the way there, or about who
else wants the same ground. `UtilityChooser` now prices all three, and every
one of them is read off one Dijkstra.

- **Every step is priced through `BattleState::moves`, a `MoveGrid`**, which
  is `SightGrid`'s twin: every tile's cost per movement class resolved once,
  shared behind an `Arc`, rebuilt by `save::rehydrate`. `movement::edge_cost`
  remains the reference implementation for tests, one-off queries and the
  campaign map, and the two share `step_cost` so the climb rule cannot drift.
  The grid is written to have tiles folded in a region at a time — see the
  streaming note on it.
- **`battle::roads` is the road-pricing twin of `path_to`**, and differs from
  it in the three ways a plan differs from an order: many destinations at
  once, a *horizon* in rounds rather than this round's budget, and **no
  occupancy at all**. Neither friends nor spotted enemies block. A march takes
  several rounds and the field will not hold still for it, so treating a tank
  parked on the bridge as a wall would make the bridge unreachable and the
  ground a crew most wants invisible; the opposition is a cost along the way
  instead, and `step_toward` still respects every vehicle on the field when
  she actually drives. A happy consequence: the inner loop touches terrain
  only, so unlike `reachable` it is **not O(hexes × units)**.
- **One Dijkstra for the whole candidate list, not an A* per candidate.** The
  first draft did the latter and cost five to ten times as much per planned
  order. Every goal is a place to drive from the same hex.
- **`HORIZON` is 4 rounds and is the only thing bounding the walk.** It is a
  number in rounds, so a recon car looks further ahead than a heavy tank,
  which is right; but it means the cost of `roads` is set by how many tiles
  that reaches rather than by how many candidates there are. See the
  performance section before changing it.
- **Ground with no road inside the horizon is priced at the horizon**, not at
  the crow flight. The first draft fell back to the crow flight and had it
  exactly backwards — the ground with no road inside the horizon is the ground
  whose road is *longest*, so that fallback made the far bank of an unfordable
  river the nearest thing on the map.
- **`DoctrineDef::route_caution` and `contest_aversion` default to competent
  values, not to zero**, for the reason `objective_value` does: a doctrine
  written before the chooser could see a road must not silently become one
  that marches down the open one. The off switch for the whole family is
  difficulty, below.
- **The route terms are a tie-break between comparable goals, not a veto.**
  Measured on a staged map, an objective that offers a shot at a visible tank
  is worth about seven points more to stand on than one that does not, against
  route costs of one or two — so any test of these terms has to stage goals
  that are worth roughly the same, and the ones in `tests/engine.rs` say so in
  their stage assertions. Reaching for a bigger coefficient instead would make
  a crew who refuses every contested objective, which is the stalemate
  objectives were introduced to end.

### Difficulty has two axes now

`difficulty_noise` is misjudging what she has read. `difficulty_foresight` is
not having read it: at 0 (difficulty 1) she sees the objective and how far off
it is *as the crow flies*, and not the river in between, nor the gun covering
the open ground, nor that the enemy is nearer to the bridge than she is. It
scales the blend between the crow flight and the road, and it gates both
doctrine terms above.

The second axis is not decoration, and this is the part to keep hold of:
**more terms make the blur matter less.** A value function that separates a
good goal from a bad one more sharply is one a blurred commander still ranks
correctly, so deepening the chooser while leaving difficulty as noise alone
would have made difficulty mean *less*. Anything else added to the chooser
should ask which axis it belongs to.

Zero at difficulty 1 is exactly the chooser as it stood before it could price
a road, so the weakest commander plays the game the AI has always played and
every level above her is an addition. `UtilityPlanner::new` therefore takes a
difficulty rather than a noise amplitude — a caller who passed one of the two
derived numbers and forgot the other would get a commander who misjudges the
map but reads all of it, which is nobody.

What this did to the measured skill gap is **almost nothing**: on the mirrored
arena at 8 seeds × 36 battles, 5-over-1 went 62.8% → 64.2% and 5-over-3 54.3%
→ 54.8%, both inside the noise at 576 battles a row. The arena is a radius-10
hexagon with a handful of forest hexes and no objectives worth arguing about,
so it has almost nothing for a road-reader to be better at. That is an
instrument limitation rather than a result about the terms — the three tests
in `tests/engine.rs` (`a_commander_who_reads_the_ground_goes_where_the_road_goes`,
`a_road_under_a_gun_is_worth_going_round`,
`ground_the_enemy_reaches_first_is_worth_less_marching_for`) are what actually
pin them, each isolating one term by holding the other two at zero. A
terrain-varied arena is the next instrument this question wants.

### Difficulty is a lens, not a lottery

`UtilityPlanner::lean` draws **one** blur per unit per round and applies it as
a smooth function of where a tile lies relative to her. It replaced an
independent draw per candidate tile, and the difference is not cosmetic: the
planner takes an argmax over every reachable tile, so independent draws made
the winner whichever tile drew luckiest — the maximum of ninety draws from
±0.5 is about +0.49, against an objective gradient of 0.54 a hex. Three things
follow that are easy to undo by accident:

- **Do not make it a flat per-unit offset.** That is the obvious reading of
  "one draw per unit" and it is a no-op: adding the same number to every
  candidate changes no argmax. It has to vary across tiles and be *smooth*,
  which is what removes the selection bias while keeping the handicap.
- **Do not reintroduce a per-tile draw anywhere in `score_tile`'s callers.**
  The bias scaled with how many tiles a unit could reach, so it silently
  punished fast vehicles hardest, and it is what the mirrored arena's
  long-tracked side-B edge turned out to be.
- **Zero at difficulty 5, exactly**, and
  `a_side_that_sees_clearly_is_untouched_by_the_blur` pins it. The
  determinism baseline fights at difficulty 3 and therefore moves when this
  changes, so that test is the one that can still tell you the noise leaked
  into a side meant to see the field as it is.

`span` normalises the lean against how far she can actually get, so a
difficulty level is worth the same to a howitzer as to a recon car.

### How the AI thinks is a mod, and it is not `balance`

The five numbers that were the last tuning constants in `ai/` live in the
`planner` block of `mod.json` (`data::PlannerRules`, one-in-effect like
`scale`, `balance` and `casualties`): `impatience`, `horizon_rounds`,
`boarding_rounds`, `devolved` and `exit_urgency`. The doc comment on each field
carries the reasoning that used to sit on the constant, which is where to look
before changing one.

- **It is a separate block from `balance` on purpose, and the line is
  load-bearing.** `balance` says what is *true on this battlefield* — what a
  point of gunnery is worth, how far through a plate a marginal round gets —
  and reaches a human player's shot exactly as it reaches a machine's. Nothing
  in `planner` reaches a rule: a mod that rewrote all five would leave a
  human-versus-human battle bit-for-bit identical. Putting a number in the
  wrong one of these merges "what is true here" with "how well is this side
  played", which is the conflation difficulty spent a whole arc separating.
  **The test for which block a new number belongs in is that question**, not
  which file it currently sits in.
- **Every field `#[serde(default)]`s to exactly the constant it replaced**, so
  a mod that declares no block — or declares the block and omits a field —
  gets the game it always had. That is checked the strong way: the determinism
  snapshot passed unregenerated when this landed, which is the whole of the
  evidence that the refactor changed no rules.
- **A number that differs per doctrine belongs on the doctrine, not here.**
  `route_caution` and `contest_aversion` are the worked examples: how much a
  commander minds driving under a gun is a thing doctrines disagree about,
  while how far ahead anybody looks is not. `devolved` is the interesting
  boundary case and is here rather than there because it is a *threshold read
  against* `DoctrineDef::delegation` — moving it changes which of a mod's
  shipped doctrines devolve, which is one decision about the whole roster
  rather than one per doctrine.
- **Each field has a test that it is read at all**, in `tests/engine.rs`
  (`what_a_round_of_driving_costs_a_commander_is_a_mod_decision` and its four
  siblings), and each was mutation-checked by pinning the field back to its
  old constant and requiring the test to fail. That is not ceremony: the
  failure this guards against is a field declared, printed and consulted by
  nothing, which is exactly what `Scale::elevation_meters` was for months.
  `the_planner_numbers_can_be_swept_and_ship_at_the_values_they_replaced`
  covers the other two halves — that the block is addressable from `--set`,
  and that what the base mod ships equals `PlannerRules::default()`.
- **`deviation_cost` is the sixth field and the one that is not a refactor.**
  It arrived with subordinate initiative and is the price a crew under orders
  pays for acting on her own judgment, eroded by her doctrine's `initiative`.
  At the shipped 2.0 the base mod's three doctrines straddle it — massed
  armour drives at the hex she was given, elastic defence and recon pull stop
  to fight from ground of their own — which is the property that keeps a
  number meaningful, and the one `balance.blind_penalty` turned out not to
  have.
- **`horizon_rounds` is the one with a performance cost attached.** It is the
  only thing bounding the Dijkstra in `battle::roads`: 61 / 106 / 155 / 219 µs
  at two, three, four and six rounds on the radius-20 map. Everything else
  here is free.

**The first sweep it made possible returned a null result, and that is worth
knowing before reaching for it.** At 36 games across the three shipped maps, a
horizon of one round and a horizon of eight are the same game — 19–17 to 20–16,
12.9 to 13.5 rounds — against a seed noise floor of ±3 wins and 1.2 rounds
measured in the same session. On the mirrored arena, `horizon_rounds` 4 and 6
produce *identical* skill tables in every row. Read it as the instrument
limitation the road-reading chunk already recorded rather than as a result
about the number: the shipped maps and the radius-10 arena have very little for
a longer-sighted commander to be better at. Re-measure on terrain-varied ground
before concluding the horizon does nothing.

### Defiance: what a crew does instead

A crew on a rung whose `obeys` is false used to be frozen in every sense —
she would not advance, would not fall back, and would not break for cover,
because all three ran through one gate. That is what REVIEW.md's second fun
tax was: morale narrated a death spiral instead of buying anything.

`DefianceResponse` (`data/morale.rs`) is what she does instead — `Freeze`,
`Flight`, `Fight` — and the rules around it are small and easy to unpick:

- **The rung decides *that* she defies; her temperament decides *how*.** The
  mod declares the responses in `morale.defiance`, each naming a `core` and a
  `base`; the score is `base + core + trait modifiers` and the highest wins,
  **ties to the earlier entry**. Cores default to `AVERAGE`, so listing
  `freeze` first is what makes a cadet nobody has written cores for behave
  exactly as every crew did before this existed. An empty list means freeze,
  full stop.
- **Traits reach it through a second effect kind.** `TraitEffect.skill` is now
  optional and `TraitEffect.defiance` sits beside it, because temperament
  under fire is not a competence and spelling it as a skill would have meant
  inventing a cowardice a cadet could be trained in. `reckless`, `craven` and
  `stolid` are content demonstrating it; the rest is character work.
- **The senior cadet still fighting decides**, not the best score aboard and
  not an average. A commander going out hands her temperament to the next
  woman down along with everything else.
- **A crew cannot refuse her own decision.** `UnitIntent::own_idea` marks a
  route the crew laid herself — the drill's dash for cover, and flight.
  Without it the refusal check picks up a fleeing crew's own path on the next
  tick, throws it away, lays it again, and she shakes in place forever.
  Anything new that lays a path from inside the engine sets it; anything that
  lays one from an *order* clears it.
- **`Freeze` costs something.** She takes no opportunity fire — `Fight` and
  `Freeze` are both "stay here" and would otherwise differ in nothing
  observable. An *ordered* shot still happens: her gun is not broken, her
  initiative is.
- **`Fight` turns ambush discipline off**, which is a cost and not a bonus:
  she spends her concealment on the first shot available rather than the
  right one.
- **Rallying reads sight, not the radio.** `recovery_near_leader` is shed by a
  crew who can see her formation's leader, through `fog::sees` — her own
  eyes, not `fog.side(..).visible`, which is vacuous because a side always
  sees its own units' hexes. Contact was the first draft and is wrong twice
  over: a commander steadies a crew by being visibly still in the fight
  rather than down a wire, and hanging it on `in_contact` makes a zeroed
  `command` block differ in deeds from no block at all (a crew with no radio
  is out of contact under one and not the other), which
  `a_zeroed_command_block_is_the_game_without_one...` forbids.
- **Flight goes away from contact, never toward an exit.** A frightened crew
  reverses out of the fight; she does not navigate to a designated lane
  twenty hexes off, and the map will not always have edges.
- **`Event::Defied` replaced `OrderRefused`**, because the old name became a
  lie the moment a crew could act without having been told anything. It
  carries what she did and where she went, since a vehicle reversing out of
  the line with nothing in the log behind it looks like the game
  malfunctioning.

Worth knowing why the determinism baseline did **not** move for this:
`river_crossing` has exactly one crew reach `Breaking` across the four seeds,
Anka's medium tank, and her temperament is `Fight` — which differs from the
old freeze only in ambush discipline, and she is already spotted by then.
That is a checkable coincidence, not a guarantee; crewing that map up (TODO)
would break it.

### Infantry, passengers and concealment

Infantry are a `VehicleDef` like everything else — `MovementClass::Foot`,
armour 0/0/0, leadership seats in the ordinary crew model and the rest of the
platoon abstracted into a `ModuleEffect::Troops` module. The design record is
`assets/wiki/reference/infantry.md`; what follows is only the parts that will
bite someone editing the code.

- **A passenger is `alive` but not on the field.** She has no independent
  position (hers mirrors the carrier's), she is invisible to spotting, and
  `unit_at` filters her out. That filter is the same shape as the exit rule
  above and has the same trap: **never decide "is anybody here" by scanning
  `units` yourself.** Go through `unit_at` / `spotted_enemy_at`, or a stack of
  a carrier and her platoon reads as two occupants of one hex and blocks
  movement onto a hex that is not full. `BattleState::passengers` is the
  reverse lookup, and it is the only correct way to ask what a carrier is
  carrying.
- **Shared fate is not optional.** A penetration into a loaded carrier rolls
  every passenger's cadets and troops into the same interior pool, and a
  brew-up burns them. `effect_rolls` is where that happens, and it is shared
  with the plate-zero splash path — change one and you have changed both.
- **Plate zero is carved out of the overpressure overmatch rule.** Blast ≥
  twice the struck plate wrecks a vehicle, and against armour 0 that would
  mean one shell in a neighbouring hex deletes a dispersed platoon. For soft
  targets splash converts to casualty rolls instead. Artillery against
  infantry is attrition, brutal but never a single-event erasure.
- **The troops module means three things at once** and they are easy to
  separate by accident: interior weight (casualty rolls find the sections far
  more often than the two cadets, which is the whole of the "leaders last"
  model), firepower (`mustered` scales every weapon's damage by hits
  remaining over toughness), and combat effectiveness (at zero the platoon is
  a remnant, alive and pulled hard toward withdrawal).
- **`concealment` scales the *spotter's* range, not the target's.** It is a
  per-target effective range inside the spotting pass, doubled when the
  target stands in cover ≥ 30, and bypassed entirely once she fires
  (`reveal_to_all` is untouched — an ambush is spent by springing it). The
  tile-vision cache is not involved and must not learn about it: that cache
  is a pure function of the map, which is what makes it exact rather than an
  approximation.
- **Ambush discipline applies to every unit, not just infantry.**
  `AMBUSH_PATIENCE` in `combat.rs` holds an *unseen* crew's opportunity fire
  below a quarter of the target's remaining substance — let them close. An
  ordered shot is exempt: discipline is about what a crew does on its own
  initiative. Leader-assignable postures are the designed future (TODO, under
  Chain of Command).
- **The AI reads what a formation is made of, and never what it is called.**
  `lays_indirect`, `goes_on_foot` and the balance instrument's taxi/foot
  columns all recognise a role off the hardware, so a mod that adds a mortar
  section or a paratroop platoon gets the behaviour on the day it is written.
  Follow that pattern rather than adding a `role` field; a chassis id in an
  `if` inside `ai/` is the smell.
- **A taxi run is two halves and the AI plans both.** The fare mounts when
  riding beats walking (rounds to cover the journey on foot against rounds
  to reach the tailgate, be driven, and get out, plus `BOARDING_ROUNDS`);
  the carrier drives to the pickup and holds the door while anybody has
  `boarding == Some(her)`. Without the driver's half a platoon at one hex a
  round never catches a carrier at six, so do not remove it as redundant.
  `BOARDING_ROUNDS` is 4 rather than the mechanical 2 because the cheap
  price let a delivered platoon re-board for a three-hex hop and thrash
  against the at-the-objective dismount reflex — see the constant's comment
  before retuning it.
- **What still is not planned is where an emptied carrier goes.** A mission
  belongs to a formation, so the taxi holds the ground her passengers were
  sent to hold, and roughly 21 of 24 die doing it whether the side fights
  flat or under command. That is per-unit tasking, tracked in TODO beside
  the assignable postures. A drop-off *short* of the objective was tried as
  a cheaper substitute and measurably lost platoons; the reason is in the
  passenger branch of `ai/utility.rs`.

### Two crews on one hex

A hex is 100 m across, and until this arc exactly one crew could stand on it.
That is why a section arrived as a queue and why the AI spent movement driving
around its own friends. Stacking is `VehicleDef.footprint` against
`TerrainDef.capacity`, and the parts that will bite someone editing this are
these:

- **Capacity is `Option<u32>` and `None` is not `1`.** A terrain that declares
  nothing keeps the rule this engine shipped with — *one crew, whatever size
  she is* — which is not the same statement as "one footprint". The latter
  would refuse a medium tank onto grass the moment anything declared a
  footprint of 2, which is an additivity break disguised as a default. So
  stacking is opt-in per terrain, and clearing the base mod's capacities in
  data reproduces the old game exactly: `--set balance.stray_percent=0` with
  every `terrain.*.capacity=null` gives 0 stacked rounds, deepest stack 1, 0
  strays and the infantry survival it had before. Footprint has the mirror
  rule and reads through `VehicleDef::footprint()`, where zero means one —
  the same field-versus-accessor trap as `WeaponDef::reload`.
- **`occupants` is the honest question and `unit_at` is a convenience.** The
  singular now returns whoever comes first in id order and is for a mouse
  click or a HUD line. Occupancy goes through `room_for`; who a shot or a
  burst finds is *everybody*. The passenger filter lives in `occupants` alone,
  so both inherit it.
- **`room_for` is fog-aware, and this is not an approximation.** An enemy the
  moving side has not spotted **takes up no room**, because an order refused
  for a full hex announces that somebody is standing there. The move resolves
  as an ambush instead, and two crews can therefore end a tick over capacity —
  which is correct, they have just driven into each other.
  `unspotted_enemies_still_ambush` and
  `hidden_enemies_do_not_show_up_as_holes_in_the_move_range` both failed the
  moment this counted everybody, which is how the rule came to be written down
  rather than rediscovered.
- **Crowding is a reason not to *stop*, never a reason not to drive through.**
  `passable` does not consult it and `destination_blocked` does. Collapsing
  the two would make a wood holding three platoons into a wall.
- **A claim takes up room exactly as a parked vehicle does.** `claimed_by_friend`
  and `ai/goal.rs::claimed_by_another` both count footprints rather than
  refusing on the first friend, which is what lets a section be ordered into
  one wood. `Goal::finished` reads the same rule: "somebody else got there
  first" now means the hex is *full*, not that anybody is on it.
- **A platoon dismounts onto the carrier's own hex**, tried first, before the
  neighbours. That is what happens on the day and it was not expressible
  before; `a_platoon_boards_rides_hidden_and_steps_off_where_the_ride_ends`
  used to assert `distance_to(carrier) == 1`, which was the engine's limit
  rather than anybody's intent.

**The campaign map makes the same distinction now.** A friendly army used to
be impassable there too, so a column could not follow the column in front of
it. Three places said so and only one of them moved anything: `reachable`'s
expansion, the `retain` that decides what may be *stopped* on, and the A* cost
function inside `move_army`. Only a *hostile* army can block a route now, and
only a march that means to avoid contact — an `Advance` still paths straight
through, because an operational advance that side-steps contact is not an
advance. Two armies still never share a tile: the trim after the walk backs
off to the last free one.

**What is deliberately still impossible is sharing a hex with an enemy.** A
spotted enemy blocks a destination whatever the capacity says, and the
movement tick ends the advance on contact. Storming the same building as the
defender is a real thing to want — it is in TODO — but it is close assault
rather than stacking: range zero, who counts as "in" the building, what a
gunner outside can see and shoot, and how a burst that lands there sorts
friend from enemy. None of that falls out of a capacity check.

**The stray rule: the gunner aims, and only a miss is a lottery.** She lays
her gun on a vehicle and the to-hit arithmetic answers for that vehicle
exactly as it always did. What is new is that a round which went *past* her
has a hex full of other people to end up among.
`balance.stray_percent` is the chance per hundred points of a bystander's
`presence`, rolled once per bystander in id order — so several make a stray
likelier without any one of them making it certain, and the arithmetic never
needs a cap. Three things about it:

- **`presence` is `100 + profile`**, reusing the term that already means "how
  much easier or harder she is to hit than a tank". A second size field would
  be a second opinion about the same fact and the two would drift.
- **`Event::ShotStrayed` is its own event**, not a flag on `ShotHit`. The two
  facts a reader needs are who was shot at and who was hit, and a hit quietly
  naming a different unit than the `ShotFired` before it reads as the log
  contradicting itself. The `ShotMissed` for the intended target still
  precedes it: she *was* missed.
- **The observed rate is below the rolled rate, on purpose.** A platoon's
  presence is 80, so 25% nominally means one miss in five; measured on a
  staged crowded wood it is 12%, because a stray that kills the bystander
  leaves the rest of that round's misses with nobody to stray onto.
  `a_round_that_goes_past_a_tank_can_find_the_platoon_beside_her` pins the
  band rather than the number.

**What stacking has *not* got is a reason.** The mechanism is live and
measured — crews share ground in 69 of ~460 rounds and the deepest stack seen
is 2 — but the AI never wants to, so strays fire 5 times in 1055 misses and
`river_crossing`'s four determinism seeds stack literally never (which is why
that snapshot did not move; a checkable coincidence, not a guarantee).
Nothing in the evaluator values sharing cover or massing on ground, so a hex
with room in it is worth exactly what an empty one is. That is the open item,
in TODO — and it is the same shape as subordinate initiative: the mechanism
waits on a *preference*, and nothing else has to move for it to arrive. What
stacking did change is infantry survival, from 79 of 96 to 65: they dismount
more often now that getting out costs nothing, and they stand with the
vehicles, and vehicles attract fire. Isolating it says the strays are not the
cause (64 survivors with `stray_percent: 0`) — the cause is infantry being
where the shooting is, which is the point.

### Latitude: an order a crew may not set aside

`Latitude` (`battle/command.rs`) is the per-unit twin of the
`Advance`/`Assault` distinction: `Delegated` is every order this engine has
ever had — she marches, and breaks off for cover under fire she has had time
to take in — and `Binding` is "I mean it", which the battle drill does not
preempt. The player says it with `X` on a selected crew, the same key that
orders a formation to assault.

**It now qualifies a formation's orders too, and it governs a different thing
there — read this before "unifying" them.** A crew's latitude answers *will
she break off for cover*; a formation's answers *may her doctrine discount the
order at all*, and that is the whole of it:

- **Only the strictness term.** `Formation::latitude` reaches exactly one
  number, the floor in `mission_value`'s `(1.5 - delegation).clamp(floor,
  1.5)`, which goes 0.5 → 1.0 under `Binding`. The rule that expresses is
  **`delegation` may make a subordinate more literal than she was asked to be,
  never less** — so insisting buys the letter of the order and never more than
  the letter, and a doctrine already at 1.2 hears nothing new.
- **It deliberately does not lift the contact damping.** `Advance` and
  `Assault` differ in that damping and in nothing else, so a binding `Advance`
  that skipped it would be an exact synonym for `Assault`. The verb answers
  "will she halt and fight when shot at"; the latitude answers "may her
  doctrine discount this"; they are orthogonal and they compose. Anyone
  reaching for `contact_scale` because binding "ought to press on" is about to
  build the second idiom the whole chunk exists to avoid.
- **It travels with the mission or it means nothing.** It is on
  `MissionChange` (an order held on the wire arrives meaning what it meant),
  in the `CutOff` snapshot (a crew who lost contact soldiers on the orders she
  was given *as she was given them* — read it through
  `Formation::latitude_for`, the twin of `mission_for`), and on
  `Order::SetMission` / `QueueMission`.
- **It belongs to the orders as a whole, not to one leg.** `plan` is still
  `VecDeque<Mission>` and an amendment sets the formation's latitude exactly
  as a replacement does. A plan is one intention; a commander who wants the
  third leg bound and the first loose countermands when it is time, which is
  what she would do on the day.
- The player says it with **Ctrl** on a mission key, composing with Shift's
  "…and then this". Ctrl rather than a key of its own because `X` is already
  the assault — the *other* axis — and there is no free key that would not
  lie.

- **It is read in exactly one place**, the drill gate in `ai/command.rs`. If a
  second `yields_to_drill()` appears, the model has drifted: latitude buys an
  order priority over the crew's *own judgment*, never over her nerve. Morale
  still refuses, `obeys()` is untouched, and a Binding order to a crew who has
  stopped listening is still not carried out.
- **It belongs to the destination, not to the cadet.** Set only where
  `tasking` is set, cleared everywhere `tasking` clears (recall, arrival, a
  fresh formation mission), and carried in `WaitingOrders` so an order held at
  the radio arrives meaning what it meant. A radioed order with `to: None` says
  nothing about the march and must leave latitude alone —
  `an_order_about_her_gun_says_nothing_about_her_march` pins that.
- **`Delegated` is the default everywhere and the AI never issues `Binding`**,
  which is what keeps the determinism baseline valid across this change. If
  `event_stream.txt` moves when you touch latitude, something has leaked into
  AI-vs-AI play; do not regenerate it. All four of `ai/command.rs`'s
  `Order::SetMission` sites spell `Latitude::Delegated` out rather than
  defaulting it, so that a reader can see it is a decision.
- **The drill can only preempt from round two.** `radio()` marches her itself
  the moment the order lands, so on the round she is ordered she is already
  planned and no planner is consulted. Any test of the drill-versus-order
  question has to fight a round first — this cost three test drafts, and the
  reasoning is in
  `a_binding_march_presses_on_where_an_ordinary_one_takes_cover`.
- **A deviation must announce itself.** `AiPlanner::last_was_drill` and
  `Decision::drill` exist so the presentation layer knows which orders were the
  planner's own idea. The game crate used to guess (keep only units whose
  formation had no mission) and thereby filtered out the single most important
  case — a personal march broken off for cover — leaving it silent. A vehicle
  that moves with no visible order behind it is indistinguishable from a bug;
  that is the bargain, and it is the whole reason the flag is plumbed rather
  than inferred.

### What an order promises, and what a battle costs

Two chunks of the DIRECTION.md work that are easy to unpick by accident,
because both are mostly *words* and words look like they belong in the UI.

- **A mission's promise lives beside the mission, not in the panel.**
  `Mission::promise()` / `Mission::verb()` / `Mission::vocabulary()` and
  `Latitude::promise()` are in `battle/command.rs` because a promise is a
  claim about the rules: whoever changes what `Advance` *does* is then looking
  straight at the sentence claiming what it does. One `VOCABULARY` table feeds
  all three accessors and `slot()` is an exhaustive match, so a new mission
  without a promise fails to compile. The game crate owns the *keys* and joins
  the two in `order_menu()`; `every_mission_key_has_a_promise` exists because
  the failure mode of that join is silent — rename a verb in core and the
  panel simply lists one order fewer.
- **`CrewCondition::Absent` is "on the roll, not in the vehicle".** A cadet
  still recovering does not deploy (`BattleState::who_deploys`, read once at
  spawn). Three things about it are load-bearing:
  - **Her seat leaves the substance reckoning entirely** — neither numerator
    nor denominator. Charging it as a loss would make a short-handed tank read
    as one already shot up, and every withdraw threshold and AI kill estimate
    in the game would price it that way.
  - **She stays in `Unit::crew`.** The campaign takes the crew list back at
    the end of the battle, so a cadet filtered out of it here is a cadet deleted
    from her tank for good.
  - **A vehicle nobody fit can crew goes out with the walking wounded.** The
    campaign has no replacement pool, and a crewless vehicle is one nothing
    inside can kill — the same invariant the anonymous-crew fallback in
    `spawn_unit` protects. `crew_state` stays empty in that case, which is
    also what keeps every scenario battle and every old save byte-identical.

  Ask "is she aboard / is she fighting" through `CrewCondition::aboard()` /
  `fighting()` rather than matching the variant, or the next state added will
  be missed by one of `interior()`, `substance()` and `fighting_crew()`.
- **A wound outlives its battle.** `CrewLoss::found` distinguishes "her
  vehicle did not come home" (`None`, priced by what killed it through
  `resolve_crew_fate`) from "she was found like this in a vehicle that did"
  (`Some(condition)`, priced by `resolve_station_fate` — gentler, never
  `Lost`, never fatal without permadeath). Before this, the entire in-battle
  crew model evaporated at the door for every vehicle that survived.
- **The casualty numbers are `casualties` in `mod.json`** (`data::Casualties`,
  one-in-effect like `scale` and `balance`). A harsh campaign is a mod.
- **One cadet, one seat.** `OverworldState::from_map` enlists each character
  once per academy; a map that names her again crews that vehicle
  anonymously, and `MapFile::validate_into` warns with the count. `frontier`
  used to spread ten characters over eighteen vehicles and trip this ten
  times; the base mod now ships forty-nine and every seat in the campaign is
  filled, which
  `every_seat_in_the_campaign_belongs_to_a_cadet_of_her_own` pins. Two things
  follow. Do not "fix" a future warning by letting one cadet crew three tanks
  again — the deduplication is a safety net, not a licence. And **a short crew
  is not cosmetic**: substance counts people aboard, so a partly-named medium
  tank dies about twice as fast as the identical anonymous-crewed one, which
  is why the campaign fills its seats and why an empty one now reliably means
  `CrewCondition::Absent`.
  **`river_crossing` still carries the old ten and its partial crews**, on
  purpose: it is the determinism baseline, so crewing it up means regenerating
  the snapshot. Tracked in TODO.
- **A battle never enlists anybody into an academy.** The anonymous crew a
  crewless vehicle gets is stamped into the *battle's* copy of the roster, so
  its handles mean nothing to the campaign; `apply_battle_result` drops any
  crew id the campaign roster does not know before writing the survivors
  back. Without that an army ends up holding ids that resolve to nobody,
  which is not a crash and therefore sits there.

### The academy roll

`R` on the campaign map opens the roster and `R`/`Esc` closes it —
`roster_page` in `game/src/overworld.rs`, a free function over plain data for
the same reason `after_action` is one: a page nobody can see in a diff is a
page that rots.

- **It is derived every frame and caches nothing.** `Overworld::roster` is a
  `bool`. A roll holding its own copy of who is wounded is stale exactly when
  the player opens it, which is after a battle.
- **A page over the map stops the world.** `pump_events`, `drive_ai` and
  `handle_input` all bail while it is open, exactly as they do for the muster
  prompt and the after-action report, and `roster_input` runs *after*
  `handle_input` because `Esc` also drops the map selection and one keystroke
  must not do both.
- **It sets `ScriptFacts::waiting`.** She opened it herself, but `waiting`
  means "a keystroke goes to a page rather than to the map", which is exactly
  true — and keeping it the strict complement of `idle` is what stops a tour
  photographing the map with a panel on top of it.
- **Availability is worded differently from the after-action report on
  purpose.** That page reports an event (*wounded today*); this one reports a
  state (*infirmary, 4 day(s)*). One vocabulary for both would make the roll
  read as a report the player had already dismissed.
- **A cadet with no vehicle is still on the roll**, under "Without a vehicle".
  She survives her tank far more often than not, and a page that only walked
  the order of battle would drop her from the school on the day she most needs
  to be on it.

### Saving

`tactics_core::save` serialises a game in progress; F5/F9 on the campaign map
drive it. The property that matters is not that the fields round-trip but that
**the future does**: `tests/save.rs` forks a battle in progress, sends one copy
through a save file, and requires both to produce the same events for the rest
of the fight. That is why the rng's stream position is saved rather than its
seed.

Three structures are `#[serde(skip)]` because they are caches: `SightGrid`,
`MoveGrid`, and the per-unit vision inside `FogMap`. They are pure functions of
the map, and either grid alone would be a thousand entries per save. The price
is that `save::rehydrate` *must* rebuild them — an empty sight grid answers
every line-of-sight question wrongly rather than loudly, an empty move grid
says every step is impossible so nobody can drive at all, and an empty
`visible_key` panics because recompute indexes it by side.

### Seeing the game without playing it

The presentation layer used to be checkable only by running the game and
looking at it, which made every rendering and UI change unreviewable by anyone
not sitting at the keyboard. `crates/game/src/devtools.rs` fixes that: a script
of timed actions drives the real input path and captures the window along the
way.

```sh
STAHL_DEBUG=1 STAHL_BATTLE=river_crossing \
  STAHL_SCRIPT=scripts/dev/battle-tour.txt cargo run -p stahlsenshamädchen
```

On this project's Linux box the game needs `STAHL_PRESENT=immediate` or it
loses the GPU a few seconds in — an NVIDIA Vulkan driver bug, not ours, proved
with `cargo run -p stahlsenshamädchen --example minimal_window` (a stock Bevy
window that reproduces it with no game code). See DONE.md.

`scripts/dev/` holds a tour of each screen; the module doc lists every action.
**`scripts/dev/run-tours.sh` runs all of them** and is the only thing that
makes "a tour is a test" true — nothing else does, not `cargo test
--workspace` and not CI, which has no display. Each tour declares the boot
environment it needs on a `#!env` line so the runner cannot start one on the
wrong map; that used to be prose at the top of the file, and three tours were
written off as broken for weeks when they had only ever been invoked wrongly.
Run it after anything that touches `crates/game`.
Two things about it are load-bearing:

- **The scripted cursor is a resource, not the window's.** Writing to
  `Window::cursor_position` makes `bevy_winit` warp the real OS pointer, which
  fights the user for their mouse and fails silently when unfocused or on
  Wayland. `map_render::View` consults `ScriptedCursor` first instead.
  Scripts therefore name a **hex**, which also makes them independent of zoom,
  pan, window size and view rotation.
- **`run_script` must stay `.after(InputSystems)`.** Bevy clears `just_pressed`
  at the top of `PreUpdate`, so a press injected before that is wiped before
  any handler sees it — the symptom is a click that silently selects nothing.
- **Scripts wait on the game, not on a stopwatch.** `until <predicate>` and
  `expect <predicate>` read [`ScriptFacts`], which whichever screen is on
  publishes for them; a failed `expect`, a timed-out `until` or a degenerate
  screenshot makes the process exit nonzero, so a tour is a test. Prefer
  `until idle` over `wait N` for anything that waits on the simulation — a
  `wait` that guessed short photographs a half-played round and says nothing
  about it.
- **A click on a stacked hex cycles through its occupants.** `unit_at` answers
  with whoever comes first in id order, which is right for a HUD line and was
  silently wrong for selection the day a hex could hold two crews: a platoon
  dismounts onto her carrier's own tile, so she sat behind the carrier and
  could not be selected by mouse at all. `ScriptFacts::selected` exists so a
  tour can assert *who* a click selected rather than discovering three actions
  later that a keystroke went nowhere.
- **Every screen answers for every fact, and the shape enforces it.**
  `ScriptFacts` is one resource shared by all of them, so a field a publisher
  leaves alone is still holding the *previous* screen's answer — a script
  would wait on a muster prompt dismissed two screens ago. That used to be a
  paragraph asking people to remember, and it was already being forgotten:
  the campaign publisher set seven of eight fields and left `selected` naming
  the last crew clicked in a battle. **Each publisher now assigns the whole
  struct through an exhaustive literal with no `..default()`**, so a field
  added here fails to compile in every publisher until each screen has said
  what it answers — the same bargain `Mission::slot`'s exhaustive match
  makes. The campaign map publishes too (`turn` as the day, `idle`,
  `waiting`, `log`, and `selected: None` said out loud, because a map has a
  selected *army* and that is a different question); it did not until the
  after-action report gave it something worth waiting for, and every campaign
  tour was a stopwatch. `waiting` means "held behind something the player must
  answer or dismiss" — a muster prompt, an after-action page — and is the
  complement of `idle`, not a second name for its negation.
- **`idle` is `Battle::listening`, and both must stay one predicate.** It
  means "a keystroke would be acted on this frame", which is not the same as
  "the phase is planning": sprites finishing a walk hold the keyboard, and a
  side that has committed is done talking. When those drifted apart, `until
  idle` came true a frame early, four `key Enter` presses advanced the battle
  by one round, and every screenshot after them described the wrong turn
  while the script reported success. Anything new that makes `handle_input`
  refuse a keystroke belongs inside `listening`, not beside it.

Rust edition 2024, resolver 3. `[profile.dev]` builds the workspace at
`opt-level = 1` and dependencies at 3, because AI search is slow at opt-level 0.
Anything that measures performance must be built `--release`; dev-profile
numbers are meaningless for the planners.

## The scale contract

Lives in the `scale` block of `assets/mods/base/mod.json`, typed as
`data::Scale` (`data/scale.rs`) and reachable as `registry.scale`. It is data,
not Rust: a mod that declares its own block replaces it wholesale, and a mod
that says nothing inherits what it extends. `cargo run --bin validate-mods`
prints the whole roster through it — every vehicle's speed, every weapon's
range and cadence, every map's real size — which is the check that did not
exist while these numbers lived only in this table.

| Quantity | Field | Value |
| --- | --- | --- |
| Battle hex | `hex_meters` | 100 m |
| Round | `round_seconds` | 60 s |
| Tick | `round_seconds / ticks_per_round` | 5 s (12 ticks) |
| Elevation level | `elevation_meters` | 10 m |
| Overworld hex | `overworld_hex_meters` | 4 km = one battle map |
| Overworld turn | `overworld_turn_hours` | 24 h (a "day", as the campaign banner already said) |

**A battle map is a hexagon, not a rectangle.** The overworld draws a tile as
a hex and a battle is that tile zoomed in, so the battlefield is the same
shape: `Scale::battle_map_radius()` derives radius 20 from 4 km ÷ 100 m, which
is 41 hexes across and 1261 tiles. `MapKind::Battle` implies
`MapShape::Tile` and validation rejects anything that is not that exact
hexagon; a scenario map that means to be some other shape sets
`"shape": "free"`. The map format needed no changes for this — `HexMap` is a
sparse `HashMap<Hex, Tile>` and a space in a row has always meant "no tile
here" — so the hexagon is visible in the ASCII of `river_crossing.json`.

There is deliberately no `TICKS_PER_ROUND` constant any more, which is why
round resolution, weapon cooldowns and mod validation all take a registry. The
one thing that could not follow: `WeaponDef::reload_ticks` is an
`Option<u32>`, because serde's `default = "..."` is a `fn() -> u32` and cannot
see the mod being loaded. Read it through `weapon.reload(&registry.scale)`,
never the field.

Consequences that are easy to violate by accident:

- **1 movement point ≈ 1 hex of clear terrain per round ≈ 6 km/h.** A vehicle's
  `points` is a speed, not an abstract budget. 5 tracked MP on grass is 30 km/h.
- **Weapon `range` is in hexes, so ×100 m.** The 88 reaches 16 hexes = 1.6 km.
- **`reload_ticks` is a practical aimed rate**, ×5 s. Not mechanical reload.
- **Vision is deliberately shorter than gun range for gun tanks.** The tank
  destroyer sees 10 and shoots 16 because needing a spotter is its character.
  Preserve that relationship when adding vehicles.
- **A small test map can no longer put units out of contact by distance.**
  Vision is 10–20 hexes; use a forest curtain. `tests/engine.rs::standoff` does
  this and explains why.
- **`elevation_meters` is read by the climb rule *and* by line of sight**, so
  a mod that changes it changes both what a vehicle can drive up and what a
  ridge hides. That is one field and it was very nearly two: `fog.rs` carried
  its own `const ELEVATION_STEP = 10.0` until 2026-08-26, agreeing with the
  mod only because both said ten. Both sight paths — the per-step `los_clear`
  and the cached `SightGrid` — resolve heights through `Heights::of`, which is
  the one place a tile becomes metres;
  `a_mod_that_flattens_a_level_flattens_the_skyline` asserts they agree. The
  general rule: **a `Scale` field no rule reads is a bug**, the same way a
  core no skill names is.
- **Crew bonuses are percentages of the vehicle's base**, from the sibling
  `balance` block (`data::Balance`): +5% sight per awareness, +5% speed per
  driving, +3 percentage points of hit chance per gunnery. Stats run 0–5, so a
  gifted crew is worth about a quarter of their vehicle. Never reintroduce a
  flat divisor — that is exactly what the scale change silently devalued.

## Invariants worth protecting

- **`tactics_core` must not depend on Bevy.** It is the reusable half. All
  presentation, input, and asset loading lives in `crates/game`.
- **The simulation is deterministic given a seed.** `ChaCha8Rng`, and no
  behaviour may depend on `HashMap`/`HashSet` iteration order. This has already
  been violated once (`fog::recompute` emitted spotting events in set order);
  it was invisible until vision ranges grew. When iterating collections to
  produce events or AI decisions, sort first or iterate `state.units` in id
  order. The replay viewer depends entirely on this.
- **Content is data, not Rust.** Vehicles, weapons, terrain, doctrines, and
  maps live in `assets/mods/<mod>/`. The base game is a mod. Adding a tuning
  constant to Rust that a modder would want to change is a design smell.
- **Fog must not leak through the order system.** An order is never refused in
  a way that reveals an unspotted enemy — that is why `destination_blocked`
  treats unspotted enemies as passable and lets the move resolve as an ambush.
  `spotted_enemy_at` exists so callers do not reach for `unit_at` and leak.
- **Damage lands during a tick; death is reaped at the end of it.** That is what
  lets two crews kill each other simultaneously. Do not make `reap` eager.
- **A tiebreak may only read quantities a reflection preserves.** Distances
  and terrain costs qualify. A dot product of two differences qualifies,
  because a point reflection negates both and the product is unchanged. **A
  coordinate does not**, and no total order on coordinates can — a reflection
  maps the least element to the greatest, so asking for "the smallest x" is
  asking which way is west, and every crew in the game then edges that way
  wherever the real keys tie. That is forwards for a side attacking west and
  backwards for one attacking east, so it pays points to whichever end of the
  map is on the east. It shipped twice, in `movement::step_toward` and in the
  utility planner's plateau argmax, and cost about two points of win rate
  between them. Where a *total* order is still needed for determinism — and it
  is, because `reachable` returns a `HashMap` — put the coordinate key last,
  behind every invariant key, so the compass decides only a left-or-right
  choice off the line of advance. `a_march_is_the_same_march_from_either_end`
  and `a_reflection_leaves_a_bearing_alone_and_turns_a_coordinate_around`
  guard it.
- **`alive` and "standing on a hex" are two different questions.** `alive`
  goes false when she is destroyed *and* when she drives off by an exit, so
  classify outcomes with `surviving_units()` / `lost_units()` or a successful
  withdrawal is recorded as a dead crew. A passenger is the mirror case: she
  stays `alive` and her `pos` mirrors her carrier's, so she is on no hex that
  anybody may interact with — `unit_at` filters `aboard.is_none()` in one
  place precisely so every occupancy, targeting and collision read in the
  game inherits it. Never reimplement either check by scanning `units`.

## Style

Comments explain *why*, in prose, and are often several lines — match that
rather than trimming to terse one-liners. Public items carry doc comments.
Errors are `thiserror` enums. Optional mod-facing fields get `#[serde(default)]`
and enums get `#[serde(rename_all = "snake_case")]`. Tests are end-to-end
against the real `assets/mods` content and named as full sentences stating the
rule they defend (`unspotted_enemies_still_ambush`).

## Known issues

### Correctness

- ~~**Difficulty is inverted in practice: less noise plays worse.**~~
  Fixed, in two halves, and the fix is worth understanding before touching
  the utility planner's tile choice. The pathology: the greedy argmax
  broke score ties toward the first tile of a fixed (x, y) sweep, so on
  the broad plateaus open ground scores in, every identical unit drove to
  the SAME corner of every plateau — a noiseless side clumped, queued,
  and lost to anyone scattered by randomness (difficulty 5 lost to
  difficulty 1 in both orientations). The cure is deterministic, not more
  noise: among tiles within `PLATEAU` (0.3) of the best score, take the
  one nearest the unit's own position. Units standing apart stay apart
  when the ground between is all the same, and nobody burns movement
  crossing a plateau to park on identical grass. The second half was the
  instrument itself: the skill-gap table fought on `river_crossing`,
  whose sides field different vehicles, so it measured the map — it now
  fights on a mirrored arena inside `balance --sim`, and the deterministic
  36–0 sweep is gone.

  **The current numbers, and they are seed-swept rather than drawn once.**
  Measured 2026-08-26: 16 seeds at `--games 36`, so 576 battles per pairing
  row and 1152 per summary row, read off the table's own `both ends` rows —
  which add a pairing's two orientations and therefore cancel whatever being
  side A is worth.

  | | wins | share | per 72-battle draw | exchange |
  | --- | --- | --- | --- | --- |
  | difficulty 5 over 1 | 824–326 | **71.5%** | 46–58 | 1:1.2–1.8 |
  | difficulty 5 over 3 | 710–441 | **61.6%** | 36–54 | 1:0.9–1.6 |

  Read the fourth column before quoting the third. One draw of 72 battles
  puts 5-over-1 anywhere between 64% and 81%, and puts 5-over-3 **level, at
  36–36, on one seed of sixteen** — so a single run cannot tell you whether
  the 5-vs-3 gap discriminates at all. Every figure this section used to
  quote was one such draw near the top of that band: 29–7 / 28–8 at
  1:1.9–2.1 when the arena was built, 26–10 / 28–8 at 1:1.7–1.9 after the
  blast-pricing fix, 28–8 / 9–27 at 1:2.1 after difficulty noise became a
  per-round lean. B4's written success metric was "most battles at visibly
  better than 1:2"; against difficulty 1 the exchange reaches that on some
  seeds and not on others.

  ~~**Side B holds an edge on the mirrored arena.**~~ **Found and fixed
  2026-08-26, and it was three faults pointing the same way — none of them
  resolution order, which is where this note kept saying to look.**

  | | side A's share of equal-skill battles |
  | --- | --- |
  | as it stood | **42.9%** (494–652 of 1152), −4.8 sd |
  | arena rebuilt symmetric | 45.6% |
  | `step_toward` tiebreak fixed | 46.4% |
  | planner argmax tiebreak fixed | **49.2%** (1133–1169 of 2304), −0.8 sd |

  1. **The arena was not mirrored.** It was written as 25×13 rows of ASCII
     with forest at columns 8 and 16 — symmetric *as text* — and the odd-r
     offset conversion shears text into hexes. On the map it actually
     produced, six of 325 tiles had no mirror at all, 24 disagreed with their
     mirror on terrain, and side A had **nine** forest hexes within six of its
     deployment against side B's **four**. It is now a radius-10 hexagon whose
     every feature is declared once and reflected, and `arena_map` asserts
     that before handing it back.
  2. **and 3. Two coordinate tiebreaks**, in `movement::step_toward` and in
     the plateau argmax above. The general rule they broke is in the
     invariants section: a tiebreak may only read quantities a reflection
     preserves.

  The proof that it is *structurally* gone rather than merely smaller: on the
  symmetric arena, with difficulty 5 both sides (where the blur is exactly
  zero) and the same planner seed, round one now produces **eight of eight
  mirrored decisions and positions**. Before the tiebreak fixes the goals
  matched and three of four positions did not — the planner agreed and the
  march did not, which is what pointed at `step_toward`.

  What the fixes did **not** do is make difficulty matter more. Skill still
  tells strongly at the extreme — 5-over-1 pays 61.1% (+10.6 sd) — while
  5-over-3 pays 51.9%, which is +1.8 sd and therefore barely anything. That is
  the shallow-goal-chooser item in TODO, not a bias.

  **Read this table at `--games 36` or not at all**, and re-draw it before
  quoting it — `--only skill --sweep seed=0,1000,2000,3000` costs about
  three seconds. The default 12 put 5v1 at 6–6 and looked like a regression
  against the numbers above; twelve battles cannot resolve a 70% edge, and
  this is the table most often quoted at somebody.
- **Army-contained unit placements are never validated.**
  `map.rs:962` passes `a.at` (the army's own hex) instead of `u.at` when
  checking each unit inside an `ArmyPlacement`, so a unit's own coordinates are
  neither validated nor used. `frontier.json` accordingly carries 14 `at`
  fields on army units that are leftover battle coordinates and mean nothing.
  Either drop the field from `ArmyPlacement`'s unit entries or start honouring
  it; right now it is misleading dead data that will bite whoever edits a
  campaign map next.
- ~~**`HexMap::center` is not a centroid.**~~ Fixed: it averages in floating
  point and rounds through `Hex::round`, which respects `x + y + z == 0`.
  This stopped being cosmetic the moment battle maps became hexagons — it is
  the rotation pivot for both map views, and `a_battle_map_is_one_overworld_tile`
  now asserts the centroid lands on the map.
- **Overworld elevation is priced at the battle scale.** `Scale` has one
  `elevation_meters`, so `frontier`'s mountains at elevation 2 read as 20 m.
  Harmless today — the overworld panel does not print elevation and only
  `max_climb` reads it — but a strategic map wants its own vertical scale, or
  its elevation digits want to mean something other than levels.
- ~~**`raw_damage` has a `.max(1)` floor**~~ Fixed by the ballistics rewrite's
  B1 chunk: `raw_damage` is gone, a hit that does not penetrate does nothing
  structural, and the instrument that used to print "mg kills heavy_tank in 3
  rounds" now prints "mg cannot meaningfully hurt heavy_tank".

### Robustness

- **`spawn_unit` panics on unknown content.** `battle/mod.rs:252` expects the
  placement to have been validated, but `from_placements` — the path the
  overworld uses for field battles — never calls `validate_into`. A mod that
  removes a vehicle, or a loaded save referencing one, panics instead of
  erroring. Same shape at `game/src/overworld.rs:703`, where
  `choose_battle_map` expects the base mod to provide at least one battle map.
- **Elevation grids fail soft in a confusing way.** A missing or short
  `elevation` row silently defaults to 0 while a *mismatched* one only warns
  (`map.rs`), so a typo in a map file produces a playable but wrong battlefield
  rather than an error.

### Performance

- ~~**`fog::recompute` rebuilds every side's vision from scratch after every
  single shot.**~~ Fixed, and without the batching trade-off the note here
  proposed — a firing unit is still revealed instantly. Three things did it,
  none of which changes what the fog *says*:
  - `SightGrid` resolves every tile's sight heights once. `los_clear` was
    doing a `String`-keyed `registry.terrain()` lookup per ray step, on the
    order of a million times a round, for an answer that cannot change
    because no battle alters its own terrain. Shared via `BattleState::sight`
    behind an `Arc` so search branching stays cheap.
  - Vision is cached per unit against `(pos, range)`. Since the map is
    immutable for the whole battle, that cache is not an approximation of the
    answer, it *is* the answer.
  - A side whose whole `(unit, pos, range)` list is unchanged skips the union
    outright, which is what makes the after-every-shot recompute nearly free:
    firing moves nobody.

  Round resolution went 14.05 → 1.02 ms/round and the engine suite 25.6 →
  2.6 s, with the event stream and final fog state **bit-identical** across
  four seeds. When touching this, keep the reference `los_clear` and
  `SightGrid::clear` sharing `sight_line_clear` so the fast path cannot drift,
  and keep `cached_vision_is_the_same_answer_as_computing_it_fresh` passing —
  it rebuilds every side's visible set from scratch and demands a match.

  That "bit-identical across four seeds" check is no longer done by hand:
  `tests/determinism.rs` records the event stream for the same four seeds into
  `tests/snapshots/event_stream.txt` and compares byte for byte. It exists
  because a self-consistency test cannot catch iteration-order bugs — a process
  agrees with itself whatever its hash seed is — which is precisely how the
  original `fog::recompute` ordering bug survived. Regenerate deliberately with
  `UPDATE_SNAPSHOTS=1 cargo test -p tactics_core --test determinism`, and read
  the diff first: a broad diff means you changed balance, a small one about the
  *order* of otherwise identical events means you introduced the bug this test
  is for.
- **The numbers below are reproducible.** `cargo run --release -p tactics_core
  --example perf` prints them; `--mcts` adds the slow ones. Measured on the
  1261-tile `river_crossing` with 8 units, four seeds:

  | | |
  | --- | --- |
  | round resolution | 1.42 ms (0.89–1.78 across seeds) |
  | `reachable()` per call | 17.6 µs |
  | `roads()` per call | 134.8 µs |
  | `unit_vision` per unit, cold | 71.9 µs |
  | utility order | 0.09 ms |
  | mcts order, difficulty 3 / 4 | 1.84 s / 4.24 s |

  Utility order was 0.03 ms until the evaluator started pricing danger as a
  fraction of what a crew can absorb, which walks the unit list once more per
  candidate tile. Anything added to `score_tile` is paid for at that rate —
  it is the hottest function the AI has.

  Utility order was 0.04 ms until the goal chooser learned to price a road,
  which is one Dijkstra out to `HORIZON` rounds per crew per goal chosen —
  and only when a goal *finishes*, so the benchmark's figure (every crew
  choosing at once, on the first round) is the worst case rather than the
  usual one. An A* per candidate instead of one Dijkstra for the list cost
  0.21–0.42 ms, which is what the shape is for.

- **`MoveGrid` is `SightGrid`'s twin and was worth the same kind of money.**
  `edge_cost` resolves terrain by `String` through the registry, and the
  searches call it per edge of every tile they touch — one `roads` call over
  the radius-20 map is about 7,500 of those string hashes for an answer no
  battle can change. Resolving them once per tile took `roads` 322 → 219 µs
  and `reachable` 26.5 → 18.0 µs, and **the event stream was byte-identical**
  with the grid in and the horizon unchanged, which is the whole claim: it is
  allowed to be faster, it is not allowed to price a step differently.
  `the_move_grid_answers_exactly_what_the_reference_does` pins that against
  `edge_cost` for every neighbour of every tile, at three climb limits and all
  five movement classes.

  Cutting `ai::goal::HORIZON` from 6 rounds to 4 took `roads` the rest of the
  way, to 135 µs. Six was set on the impatience arithmetic without checking
  what it *covered*: six rounds is 30–42 movement points and the map is a
  radius-20 hexagon, so the horizon was the whole map and pruned nothing. At
  2 / 3 / 4 / 6 rounds the call costs 61 / 106 / 155 / 219 µs.

  Note what is **not** in either grid's inner loop, because it comes up: a
  hex's capacity and a vehicle's footprint. `roads` consults no occupancy at
  all by design, and stripping the friend check out of `destination_blocked`
  entirely — infinite stacking — moves `reachable` only 31.1 → 27.6 µs.
  Stacking is not a performance question.

  **Round resolution moves with how well the AI plays, not only with how much
  work the tick loop does**, and that is the thing to know before reading it.
  It was 1.07 ms; detection rolls took it to 1.87; the deepened goal chooser
  brought it to 1.38 — and re-measuring detection *after* the chooser landed
  now shows the rule making rounds **faster** (1.62 ms off against 1.38 on),
  the opposite of the sign it had a day earlier. Nothing in the spotting pass
  changed in between. What changed is that crews pick ground they can drive to
  and spend fewer ticks manoeuvring, which was the standing (unconfirmed)
  explanation for the detection cost and is now about as confirmed as it will
  get. `reachable()`, `roads()` and `unit_vision` measure work and are the
  numbers to read when the question is whether something got slower.

  Run it `--release` or the figures are meaningless. Note this supersedes the
  "~39 µs per call on the 768-tile map" figure that used to appear below: that
  map stopped existing when battle maps became the radius-20 hexagon.
- **MCTS is parked and costs you nothing to ignore.** It measured at parity
  with the utility planner over 256 controlled battles while costing ~1.8 s
  an order against 0.05 ms. Nothing ships it and **no planner change owes it
  a performance budget** — that is the only part of it you need while
  working. Why, and what would justify reviving it, is in
  [PARKED.md](PARKED.md).
- **`reachable()` is O(hexes × units) twice over** — tracked in TODO.md under
  Misc (the occupancy-index item). 32.8 µs per call, which is fine in isolation
  and not fine inside a search that calls it thousands of times.

### Content gaps the scale decision exposes

- ~~**Every overworld battle is fought on `river_crossing`.**~~ Half fixed.
  `choose_battle_map` (`game/src/overworld.rs:870`) looks for a map named
  `battle_<terrain>` and otherwise returns the first battle map in the
  registry, and the base mod shipped exactly one. It now ships three —
  `river_crossing`, `battle_plains`, `battle_forest`, each the radius-20
  hexagon, and `balance --sim` samples all of them so every map added is free
  balance signal. What is still missing is *coverage*: the terrain ids the
  overworld actually uses have no `battle_` map of their own except plains
  and forest, so a fight on a mountain or in a town still falls through to
  whichever map iterates first. Adding `battle_city`, `battle_hills` and so
  on is content work with no engine question left in it.
  **`river_crossing` is the determinism baseline's ground and deliberately
  fields no infantry** — putting a platoon on it means regenerating the
  snapshot and losing the check that says a chunk changed no rules.
- ~~**`frontier` was not checked against the 4 km hex.**~~ Checked, and it
  holds without changes. It is 14 × 9 hexes = 56 × 36 km, and an army with
  `movement: 4` covers 16 km per turn. The turn length is now stated:
  `overworld_turn_hours: 24`, i.e. a day — not the half-day this note
  guessed, because the campaign layer had already committed to a day in its
  own UI (the banner counts "Day N", tile income is quoted `/day`) and 16 km
  is an ordinary *sustained* advance for an armoured formation even though
  the same tanks do 30 km/h in a battle. An operational turn is mostly not
  spent driving.
- `deploy` (`game/src/battle.rs:761`) sorts deployable tiles by depth from the
  map edge, so it scaled to the larger map with no changes. Worth knowing it is
  already scale-independent before anyone "fixes" it. On a hexagon the lowest
  columns exist only in the middle rows, so a side now deploys out of the
  hexagon's west or east vertex and fans inland — a column, which is what the
  ambush case in TODO wants anyway, but it is a shape change rather than a bug.

### Hygiene

- **The tree is clean and CI enforces it.** `cargo clippy --workspace
  --all-targets -- -D warnings` gates, and it denies rustc's own lints too (an
  unused import fails the build). `cargo fmt --check` gates alongside it, with
  `style_edition = "2024"` pinned in `rustfmt.toml` so a toolchain upgrade
  cannot turn CI red on its own. Run both before pushing; `cargo fmt` fixes the
  second automatically. Keep it this way. What
  the cleanup produced is worth knowing, because the shapes it introduced are
  the ones to reach for next time:
  - `map_render::View` bundles rotation, centre, window, camera and the dev
    harness's scripted cursor. Those five were threaded separately through nine
    systems; they are all answers to "how are we looking at the world", so a
    system now asks for `view` and calls `view.hovered(&map)` or
    `view.face_at(&map, hex)` instead of re-deriving the projection.
  - `map_render::TextSlot` / `MarkerQuery` and `battle::BattleHud` name the
    query types whose `Without` filters exist only so Bevy can prove two
    `&mut` queries do not alias.
  - `MapFile::validate`'s placement checker is a `PlacementCheck` struct rather
    than an eight-argument nested fn.
  - Two `#[allow(clippy::too_many_arguments)]` remain, on `battle::pump_events`
    and `overworld::enter_overworld`. A Bevy system's parameters are its
    dependency list, and those two do not decompose into any smaller noun;
    inventing a struct to satisfy the lint would make them worse. Both carry a
    comment saying so.

  Note the previous version of this note claimed the worst `too_many_arguments`
  offenders were the combat functions and proposed a `Shot` struct. That was
  wrong: `combat.rs` never tripped the lint. The offenders were all Bevy
  systems in the game crate. A `Shot` struct may still be a good idea when
  penetration adds parameters to `hit_chance`/`raw_damage`, but it is a
  readability choice, not a lint fix.
- `crates/game/src/battle.rs` is ~3500 lines and `overworld.rs` ~2200 — this
  note said 1500 and 1060 for a long time and had simply stopped being true.
  They are the two files that absorb the prep phase, objectives and menus
  work. The seam that has already been taken is the one to keep taking:
  `battle/panel.rs` holds the ~600 lines of formatters that are pure over a
  `BattleState` and touch no Bevy, `overworld::roster_page` is the same move,
  and **"does it hold a Bevy type" is the line to draw** — a panel whose text
  is written between a `Query` and a `Commands` is read by nobody who is not
  already debugging the renderer. Tracked in [STRUCTURE.md](STRUCTURE.md).
