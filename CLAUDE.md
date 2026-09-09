# CLAUDE.md

Engineering notes for this repo: how it fits together, the invariants worth
protecting, and the known defects. [DIRECTION.md](DIRECTION.md) is the current
design argument — why the next few chunks are being built at all — and carries
the live state of that plan. Gameplay and design work lives in
[TODO.md](TODO.md) — this file is for things that are wrong or fragile in the
code rather than things not yet built. Where an item is already tracked in
TODO.md it is cross-referenced, not repeated. Finished work and the reasoning
behind it lives in [DONE.md](DONE.md); read it before undoing a decision that
looks arbitrary, and **read it for the measurements** — this file states the
rules and DONE.md holds the evidence, the wrong first drafts and the numbers.
[PARKED.md](PARKED.md) says why code with no callers is still in the tree.
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
# one table, in about three seconds
cargo run --release -p tactics_core --example balance -- \
    --sim --games 36 --only skill --absolute --sweep seed=0,1000,2000,3000
# ...on ground where a wrong choice is punished (the same flag on brains, mustered, ground)
cargo run --release -p tactics_core --example balance -- \
    --sim --games 36 --only skill --absolute --sweep seed=0,1000,2000,3000 --arena ridge_arena
# does the AI play a mirrored battle as its own reflection? (a coordinate leaking into a decision)
cargo run --release -p tactics_core --example mirror -- --arena ridge_arena
```

### Reading a balance number

`balance` is the content-iteration loop, built around the kill chain rather
than around damage. The analytic pass asks the *real* combat code —
`preview_attack` with the round under test forced into the racks through
`set_loadout`, never a reimplemented formula — so the report cannot drift from
the game. `--sim` fights whole battles. The rules for reading it:

- **`--set path=value` and `--sweep path=a,b,c` address fields by the name the
  json uses** (`balance.partial_penetration_percent`,
  `weapon.howitzer_105.dispersion`, `morale.rungs[2].accuracy`,
  `planner.horizon_rounds`, `doctrine.massed_armor.aggression`) and reach
  every field of every block through a serde round trip, so a field added
  tomorrow is sweepable the same afternoon. A path that names nothing is an
  error listing what was actually at that level. Sweeping is the same thing as
  editing `mod.json`, and that equivalence is tested; if it stops holding, the
  override machinery has become a second game.
- **Three axis names are not fields.** `mods=` compares two versions of the
  content; `points=` compares two *armies*; `seed=` re-draws the whole report
  and is the most useful of the three, because it puts the noise floor in the
  same columns as the difference being read.
- **Every table with a win column prints the band a level pairing wanders in**,
  and a swept table with three or more variants prints its own `spread` line.
  Read a difference against those before believing it. At 36 battles a level
  pairing lands anywhere from 12–24 to 24–12 nineteen times in twenty; the
  band is a *floor*, because these battles share maps and forces.
- **The battles run across every core and no printed number moves with
  `--jobs`.** Results fold in seed order; `Tally::merge` requires every field
  to be associative (a sum, a max, a concatenation) and `tests/harness.rs`
  checks it. A balance figure that moved with scheduling would look exactly
  like noise.
- **`--only <tables>`** prints just the ones named (`roster`, `detect`, `hit`,
  `pen`, `kills`, `flight`, `flags`, `sim`, `delegation`, `mustered`, `skill`,
  `ground`). `--csv` emits everything long-form. Every table that fights
  battles goes through `Grid` and `fought_grids`, one list called by both the
  single run and the sweep — a table added to only one would quietly stop
  being swept.
- **`--seed` is an offset, not a base.** Zero is the sample every quoted number
  was measured on; the per-table constants it is added to are the ones each
  table was born with.
- **`ground` separates the battlefield from the order of battle** by fighting
  every map twice per seed with the armies exchanged between the ends. Its
  control is that `battle_plains` and `battle_forest` ship identical orders of
  battle. **`river_crossing`'s armies are badly unbalanced** (24/76 to the side
  with the tank destroyer), and it is a third of the fought-out sample, so
  every doctrine conclusion `--sim` prints is partly about that tank
  destroyer. Numbers and method: DONE.md, and
  `assets/wiki/reference/battlefields.md`.
- **`--arena <id>` picks the mirrored battlefield** the `skill`, `brains`,
  `mustered` and `ground` tables fight on: `skill_arena` (the default, byte for
  byte the map every quoted number was measured on) or `ridge_arena` (radius
  12; a level-2 crest in the middle worth 3, a spur on each flank ridge worth
  2 between them, near woods the crest rim sees into and reverse-slope woods
  it cannot). `Arena` is a struct, so the map, the deployment and the
  symmetry checks are one choice. The ridge arena is written in the
  coordinates its symmetry group is diagonal in (`s = 2x + y`, `t = y`;
  `Arena::band`), so a feature described by `|s|` and `|t|` cannot come out
  asymmetric. Its `ground` row has no side edge (146–142, no draws, where the
  skill arena still leans 54.8% to the western end at difficulty 3).
  **`examples/mirror`** plays an arena at difficulty 5 on both sides and
  reports the first decision that is not its twin's reflection, with a
  verdict: *equal-key tiebreak* (two hexes of one objective the same distance
  from her, settled by the coordinate the invariants allow to go last — a
  coin, not a defect) or *different key* (a rule read the compass). Both
  arenas come back with nothing but the coin.
- **Mustered forces** is the one table that buys its own army per doctrine at
  a points budget (`tactics_core::force::muster`), recognising roles off the
  chassis. It pays the asking price rather than hunting value per point, on
  purpose: dividing appetite by cost makes every doctrine buy a swarm of the
  cheapest chassis and hides what the table is for, which is that a doctrine
  winning reliably at equal points has underpriced hardware.

## Rules that will bite an editor

Grouped by subsystem. Each is a rule that is easy to unpick by accident and
that some test or snapshot defends; the story of how it was arrived at is in
DONE.md under the same heading.

### Objectives

- **Objectives are map data, not battle state.** They live on `HexMap`; the
  battle carries only `objective_held` and `score`. That split is why both
  setup paths, `from_map` and the overworld's `from_placements`, get them
  with no new arguments.
- **Control persists and contest cancels.** Points are paid once at the end
  of a round, never per tick, or the size of every score is an accident of
  `ticks_per_round`.
- **`EndReason::Stalemate` no longer implies a draw.** `BattleState::leader()`
  says who won it; `winner: None` means the score was level. Anything reading
  a result must handle `(Some(winner), Stalemate)`.
- **An exit is not ground, and leaving is not dying.** A vehicle that takes an
  exit it is entitled to (`side` restricts it — an exit anyone may take is a
  lane both armies take on round one) has `Fate::Exited`, which is not
  `alive()` and not `lost()`. Classify at the end of a battle with
  `surviving_units()` / `lost_units()`, which read `Fate::lost`; since
  Phase 3c `!alive()` as a loss test is not expressible without going
  through the accessor that says so.
- **`check_victory` reads the score before the board**, or a withdrawing
  force that reaches its target on the tick its last vehicle drives off hands
  the battle to whoever is still standing.
- **The AI will not run for an exit unless it is losing**: the pull is gated
  on `withdraw_threshold` against her own damage, so an intact crew scores
  every exit at zero. Whether she is *permitted* to leave is a
  chain-of-command question, deliberately not the evaluator's.
- **A map that declares no objectives behaves exactly as before**, pinned by
  `a_map_that_names_no_objectives_is_fought_exactly_as_it_was_before`. Re-check
  it when touching `Evaluator::objective_value`.

### Looking is not seeing

Finding somebody inside your own field of view costs a roll
(`Balance::detection_chance`). Design record and table:
`assets/wiki/reference/detection.md`. What bites in `fog.rs`:

- **The geometry is untouched.** Line of sight, range and
  `VehicleDef::concealment` decide whether a look is possible; the roll
  decides how long it takes. The far-range term reads the spotter's **own**
  reach, never the concealment-shortened one, or the same fact is charged
  twice.
- **A search buys an acquisition, never the watching afterwards.** A held
  contact is not re-rolled, nor is a crew who has fired (`revealed`, checked
  before any die). Drop the first clause and every found enemy flickers.
- **One look per target per tick**, recorded in `SideFog::searched`. The fog
  is recomputed after movement, after fire and after every shot, so rolling
  per call would make finding somebody a function of how much shooting was
  going on. The look is spent even at zero chance — a test that pokes `moved`
  between recomputes has to let a tick go by.
- **The best-placed spotter rolls, not each of them.** A roll per pair of eyes
  makes the printed chance a lie by however many crews are looking.
- **`detection_certain_percent` is the near band and 100 is its neutral
  value**: no die is thrown, pinned by
  `a_mod_that_asks_for_no_search_spots_exactly_as_it_always_did` down to the
  rng's stream position — separately from the determinism snapshot, which
  records the rule *on*.
- **A staged test that needs two crews in plain sight goes through `seen(..)`**
  from `tests/common`. A test about shells must not also be a test of whether
  anybody happened to find anybody on the tick it was set up.
- **Line of sight is a mirror-symmetric relation.** A ray step that lands on
  a hex boundary resolves to *every* hex it could be in, and the ray is
  blocked if any blocks; `a_reflection_leaves_a_sight_line_alone` pins it over
  every ordered pair of the arena under both reflections. The fixed-nudge
  rounding it replaced gave 1.27% of mirrored pairs a different answer and
  handed a symmetric battle to one side (ARCH-TODO.md).
- **What the rule has not got is a reason**: nothing in the evaluator wants to
  be unseen, so it is a player-facing rule until the goal chooser learns to
  want it.

### The shot

The far side (struck face, obliquity, scatter, the loader's choice, the
interior) and the near side (`hit_chance_inner`) are both data
(`assets/wiki/reference/ballistics.md`). Rules:

- **`hit_chance` names a target by id, not by hex.** Half of what makes a
  shot hard is a fact about *her*. Every firing path has a real target, blind
  fire included: `fire_at_tile` resolves against whoever is standing there.
- **A shot has two ends, and both are ground.** `hit_chance`,
  `hit_breakdown`, `shot_profile`, `expected_damage` and
  `ai::best_weapon_against` take the target's hypothetical hex `at` beside
  the attacker's `from`; every real firing path passes her real hex. Range,
  cover, downhill, struck facing and obliquity resolve against `at`. **Two
  things deliberately do not**: `tgt.moved` (the mirror of `hexes_under_way`
  — charging a discount for a drive she has not made is the same bias with
  the sign flipped) and the loader's round choice (`best_round_against`
  judges from real positions). Her facing at `at` is her facing now.
- **`battle::danger::fire_on(registry, state, unit, at)` is the one answer to
  "what could the enemy put on her there".** Spotted enemies only, best
  weapon each through `combat::best_weapon_from` (the three gates — range
  band, sight unless indirect, expectation above zero — in one place), the
  resolver's arithmetic and nothing else: no doctrine weight, no planner
  number, no falloff. The evaluator's threat term and the player's danger
  overlay both read it, or they are two answers to one question.
- **`Unit.moved` is hexes crossed this round, incremented at the single place
  a unit changes hex** and zeroed in `begin_round`. It is not `move_credit`.
  One increment site is what makes an ordered march, a dash for cover and a
  flight all cost the same accuracy.
- **The motion terms are per hex**, and firing on the move costs more per hex
  than being fired at. If they cross, halting to shoot is worth nothing.
- **`hexes_under_way` reads state and never the hypothetical `from`.** The
  draft that charged the drive to a candidate tile produced three stalemates
  where the baseline had none (DONE.md).
- **`profile` is about being hit; `concealment` is about being found.** One
  reaches the gunner's arithmetic, the other scales a spotter's range. `profile`
  is deliberately not derived from armour or class, and is zero on every
  armoured chassis in the base mod, so the infantry numbers are attributable
  to this one field.
- **Suppression is `MoraleRung::accuracy`**, not a second fear system. A
  one-rung ladder has none without an `if`.
- **Dispersion is a different fact from flight time.** `ShellInFlight.impact`
  is rolled from `WeaponDef.dispersion` (a percentage of range) when the shot
  is fired; `shell_lands` resolves against `impact` everywhere — occupant,
  splashed neighbours and `ShellLanded`'s own hex. There is deliberately no
  hit roll for a shell on top. `scatter` indexes `hexx`'s ring order, a pure
  function of coordinates, never a set iteration. `impact` is not
  `#[serde(default)]`: the default would bring an old save's airborne shells
  down on the map corner.
- **`penetration_roll` returns a share, not a bool.** `None` is a bounce;
  `Some(share)` is interpolated by `Balance::penetration_share` from
  `partial_penetration_percent` at parity to 1.0 at
  `clean_penetration_percent`, linear so there is no cliff.
  `clean_penetration_percent: 100` is the game before the band.
  `penetration_share(..)` is the analytic twin `round_worth` multiplies by.
  **`ShotHit::damage` is what was spent**, rolled once in `resolve_impact` and
  read by `behind_armor_effects` and `savage` as `spent`.
- **`round_worth` is the only shot price in the game**, backing the loader's
  AP-or-HE choice and every planner's pricing. Its blast half, `blast_worth`,
  is `overpressure`'s twin case for case, sharing `blast_overmatches`,
  `overpressure_chance` and `exterior_modules`; a fourth case in one and not
  the other sends the gun back to firing at a number. There is no
  blast-to-damage constant — `points_per_effect` already is one.
- **`ShotProfile::plate` is the listed plate and `effective_armor` is the
  sloped one.** Blast reads the first: a burst crushing a hull is not defeated
  by an angle solid shot would skip off.
- **The stalemate clock counts accomplishment, not effort.** Hits, breakages,
  burnings, deaths and departures reset it; **a bounce does not**. Adding an
  event to that list is adding a way for a battle never to end.
- **Plate zero is carved out of the overpressure overmatch rule.** For soft
  targets splash converts to casualty rolls instead of erasure.
  `effect_rolls` is shared with the loaded-carrier path — change one and you
  have changed both.
- **Ambush discipline applies to every unit.** `AMBUSH_PATIENCE` in
  `combat.rs` holds an *unseen* crew's opportunity fire below a quarter of the
  target's remaining substance. An ordered shot is exempt; `Fight` defiance
  turns it off.
- **The stray rule: the gunner aims, and only a miss is a lottery.**
  `balance.stray_percent` per hundred points of a bystander's `presence`
  (`100 + profile`), rolled per bystander in id order. `Event::ShotStrayed`
  is its own event after the `ShotMissed`.

### The log is a net, not a narrator

Presentation only — events carry the same ids and flags, so `ScriptFacts`, the
script harness and the replay are untouched.

- **`Event::heard_by` is the one filter, it lives in core
  (`battle/orders.rs`), and its match is exhaustive.** A new variant fails to
  compile until somebody has said who hears it. **Two callers must not
  drift:** the log filters after draining; `drive_ai` filters *before
  queueing*, because planning events go through the paced animation queue
  and `accepting_orders` is false while it holds anything — an event nobody
  prints still costs the player a beat. Anything new that emits events
  during planning goes through this.
- **Command traffic and the inside of her hull are a side's own business** —
  orders, contact troubles, the radio queue, where a crew has decided to go,
  what is broken or bleeding, ammunition, and her morale rung. Fighting events
  stay side-blind. The line is the deed rather than the reading of it.
- **A spot belongs to the side that made it.** Being found is not something
  the found crew is told.

### One currency: the evaluator reads the resolver

`score_tile` (`ai/eval.rs`) is the single place a rule becomes behaviour, and
its terms are denominated in **worth per round**: substance points the
resolver computes, for a round of fire, with what the fire does to her nerve
converted at the mod's exchange rate.

- **`combat::expected_shot` is the price of one shot and everything reads
  it**: damage, pressure, worth and cadence off one hit chance and one
  `shot_profile`. `expected_damage` and `expected_pressure` are its halves.
  `worth = damage + pressure × morale.point_worth`, applied in exactly one
  place (`worth_of`, mutation-checked — a draft applied it twice and a
  mutation to one site passed the suite). `round_worth`, the loader's price,
  carries the fear term too, which is what lets a crew fire a belt at plate
  she cannot beat. `best_weapon_from`'s third gate reads worth, not damage.
- **Suppression is a property of the round** (`AmmoDef::suppression`,
  `ammo.<id>.suppression`): what a shot that *strikes* costs the crew it
  struck, penetration or not, on top of the ladder's outcome prices. On the
  round rather than the ladder because the loader chooses rounds. Zero,
  the default, is the game before. **`MoraleRules::pressure_for` is the one
  price list**, spent by `apply_pressure` charging a tick's events and by
  `combat::round_pressure` expecting them;
  `fear_is_priced_by_the_same_arithmetic_that_charges_it` fires several
  hundred bursts and requires the two to agree within a fifth (they agree
  within a couple of percent). `ShotHit` / `ShotBounced` carry the round
  (`ammo`) so the event-reading pass can price what arrived.
- **`morale.point_worth` is the exchange rate** (0.5 shipped: a full ladder
  of fear is worth about a third of a medium tank) and it belongs in
  `morale`, not `planner`, because it reaches the loader's AP-or-HE choice
  and therefore a human's crew. At 0.0 fear is charged and invisible to
  every chooser, the game before.
- **Cadence is `WeaponDef::shots_per_round(&scale)`**, ticks per round over
  `reload(scale)`, fractional for a piece slower than a round. Ground is
  priced per round and a trigger pull per shot: `best_weapon_from`,
  `ai::best_weapon_against` and `danger::incoming` are per round;
  `best_opportunity_shot`, `best_round_against` and the `kill` flag (damage
  only — times cadence an autocannon believes it finishes everything) are
  per shot. `fire_on`'s bearings are per shot with `shots` beside them so a
  panel can name a gun and its rate. With `shots_per_round` pinned to 1.0
  the determinism stream was byte-identical to the baseline, which is how
  the suppression refactor was proved behaviour-neutral before the cadence
  diff was read.
- **The threat term is `incoming(registry, state, unit, tile).worth`**: the
  sum over every *found* enemy of a round of what the resolver says she
  would take standing there. There is no distance falloff and no range gate
  — both stood in for positional terms the arithmetic could not see, and
  either one reinstated charges the same fact twice. `caution * exposure`
  scales it afterwards; that is what she makes of the danger, and it is
  hers. `incoming` and `fire_on` walk one private `guns_bearing_on`, so
  there is one answer to "who can shoot her there".
- **The terrain prior speaks only where the arithmetic is silent.** Cover and
  elevation taste (`planner.cover_prior`, `elevation_prior` × the doctrine's
  `cover_value`, `elevation_value`) is paid on a tile no found gun can reach
  and withheld inside a found gun's envelope, where the resolver already
  prices the same timber. The prior is the **only** reader of those two
  doctrine fields; zeroing it retires them.
  `the_ground_prior_stands_down_where_the_arithmetic_speaks` pins the gate.
- **Changing the currency reaches every weight quoted in it.**
  `planner.deviation_cost` has been 2.0 → 3.0 → 12.0, each move the same
  event: its job is to sit between the shipped doctrines' `initiative`
  values, and each time the threat term grew all three doctrines deviated.
  It is measured on `pressed_stage` twice, with suppression declared nowhere
  and with the base mod's values, and sits at the bottom of the intersection
  so it means the same thing in a mod that declines the rule.
  `the_shipped_doctrines_straddle_the_price_of_deviating` is the check;
  expect it to fail the next time a term grows. The withdrawing crew's
  `attack_scale` in `eval.rs` is 0.0625, a quarter of one *shot* — a quarter
  of a round put 4.6 points back in front of a withdrawing crew and she left
  her lane. Thirteen test stages were re-staged for cadence with their
  margins written in their comments; none was weakened.
- **The objective and the order are quoted in it too** (Wave 2, 2026-09-07).
  `planner.score_worth` (3.0) is what one point of an objective's `value` is
  worth in substance a round, so the ridge's crest, worth 3, prices at 13.5
  against the 8 to 29 a round a found gun puts on a crew standing there —
  inside a factor of two of the fire it argues with. 1.0 is the game before
  and 0 is catastrophic (nothing leaves cover: 9.2% and 225 draws of 288).
  `planner.order_worth` (0.25) is the share of what she still has aboard,
  per round, that being on the ground her commander named is worth to her;
  it replaced `mission_weight`, and a mod still declaring the old field gets
  a `validate-mods` warning naming the new one. `exit_urgency` rides on
  `score_worth`; `Withdraw` alone stays on the objective scale, because
  being ordered out is not worth *less* to a crew who is nearly finished.
  **The order/threat ratio was quadratic in what she has left** (danger a
  fraction of her, an order a share of her), and the designer's ruling
  (2026-09-09) is `planner.order_complement` (0.5): the order is quoted
  against her *and*, scaled down, against her full complement —
  `order_worth × ((1 − c) × left + c × full)` — so a fresh crew reads it
  exactly as before at every value and a crew with nothing left still holds
  it at half. Below the instrument's floor: bit-identical in 140 of 160
  delegation-table rows and on every ridge row (no side there is under a
  mission), the snapshot unmoved at 0.5. The value is the designer's.
- **`score_worth` is a symmetric number and the skill table cannot see
  it.** Swept 1 to 12 on the ridge it is one spread of noise, because both
  commanders get the same rate: it changes what a battle is *about*
  (`ObjectiveTaken` 5 → 8 on the baseline, rounds 22 → 17), not who is
  better at it. A third species of null beside "no effect" and "never
  evaluated" — symmetric, invisible to every table this harness has. The
  harness wants a table whose question is what the battle was about.
- **There is one walk and one gate.** `ai::threats` is `fire_on`'s
  membership and `ai::threatened` is `incoming(..).worth > 0`;
  `there_is_one_answer_to_who_can_shoot_her` fails on any threshold above
  the stage's cheapest bearing. **The mid-round drill minimises fire and the
  rout maximises distance** — the designer's split between an orderly
  reaction and a frightened one. The drill (`run_crew_drill`, `orders.rs`)
  takes the reachable tile with the least `incoming_from` worth against the
  guns she has caught up with on her reaction clock, no distance term at
  all, and only strictly quieter ground, so it settles instead of
  oscillating; the rout (`flight_destination`) takes distance first and
  spends `incoming` on the ties. Neither reads terrain `cover` any more;
  they were the last "where to stand" rules outside the currency, and
  moving them was worth seven points on the ridge before any weight moved.
- **What the currency does not carry**: the mass band, the +4.0 kill bonus,
  the 0.3 advance slope and the 0.15 centre-seeking fallback are bare Rust
  in `score_tile`, unsweepable and therefore unmeasured — now the largest
  un-restated block in the sum.
- **The ladder charges a shell for what it spent** (the designer's ruling,
  2026-09-09). `pressure_for(ShotFelt::Penetrated { spent }, ..)` scales
  `hit + penetrated` by the share of the round's *listed* budget the
  penetration spent (`combat::spent_share`, one reading for the charge, the
  expectation through `pen_share`, and the bail-out's prospective rung, which
  asks the price list now rather than restating it); suppression is charged
  whole and a bounce is priced on the ring. `Round::listed` is the datasheet
  budget before `mustered`; `ShotHit::budget` carries it. What the ruling
  leaves: `resolve_impact` floors every penetration's spend at one point, so
  a remnant platoon with no riflemen charges a third of a rifle's price
  rather than nothing
  (`a_remnant_platoon_frightens_by_the_one_point_her_bullet_still_spends`
  records it) — whether she should be spending that point is a question
  about the floor. And the base mod declares `morale.hit` 3 and never
  declares `morale.penetrated`, so on shipped content a penetration costs
  three points plus the round's suppression, not the engine default's
  eight.

### Goals: the seam the AI is meant to be replaced at

`Goal` (`battle/command.rs`) is what one crew means to do, kept across rounds.

- **`ai/goal.rs::candidates` is shared knowledge; `GoalChooser` is
  judgement.** A learned policy replaces the chooser and nothing else, and
  inherits pathing, boarding, opportunity fire and defiance from an executor
  that already works.
- **A goal lives on the `Unit`, not in the planner.** `tests/save.rs`
  requires a forked battle to have the same future; a planner's private
  memory does not survive that.
- **A mission replaces the candidate list; it does not join it.** Letting it
  compete undid DIRECTION.md step 1 and
  `a_cut_off_unit_keeps_the_orders_she_had` caught it.
- **Subordinate initiative widens that list by exactly one entry** — the tile
  this round's own sweep picked — charged `planner.deviation_cost * (1 -
  initiative)`. It governs **how she carries out an order, never whether she
  believes it**. The wider version (her own objectives) was built and fails
  three tests, because (at the time) a mission's ground was worth 2.0 on the
  evaluator's scale and a shipped objective 2 to 5, so an order became
  cheaper than terrain — DIRECTION.md's opening complaint rebuilt inside the
  fix. Both are in the currency now (`order_worth`, `score_worth`) and the
  argument is unchanged: an order competing with her own objectives is a
  suggestion. At
  `initiative: 0` the list is the two entries it always was, a membership
  guard rather than a coefficient, so the rng stream is untouched.
- **`candidates` orders an objective's hexes by distance from the crew,
  coordinate last.** List position is the chooser's tie-break, and map-file
  order is a tiebreak with no invariant key in front of it.
- **A long march is walked through `movement::step_toward`**, shared with a
  commander's personal `tasking`. `SetMove` is refused past this round's
  budget, which is why the planner only ever *scores* ground it can reach.
- **One crew per piece of ground**: `candidates` drops a hex a friend is on
  or making for, counting footprints against capacity.
- **Difficulty applies to the chooser, not to the tile sweep.** A worse
  commander goes to the wrong place, which a player can see and punish.

### The road, not the crow flight

`UtilityChooser` prices the road, the fire along it and who gets there first,
all off one Dijkstra (`battle::roads`, `HORIZON` = `planner.horizon_rounds`).

- **Every step is priced through `BattleState::moves`, a `MoveGrid`**,
  `SightGrid`'s twin, rebuilt by `SavedBattle::rehydrate`. `movement::edge_cost` is
  the reference and shares `step_cost` with it;
  `the_move_grid_answers_exactly_what_the_reference_does` pins them.
- **`roads` has no occupancy at all.** A march takes rounds and the field
  will not hold still; opposition is a cost along the way and `step_toward`
  respects every vehicle when she actually drives. The inner loop touches
  terrain only, so it is not O(hexes × units).
- **Ground with no road inside the horizon is priced at the horizon**, not the
  crow flight — the first draft had it backwards and made the far bank of an
  unfordable river the nearest thing on the map.
- **`route_caution` and `contest_aversion` default to competent values, not
  zero**, so a doctrine written before the chooser could see a road does not
  silently march down the open one. The off switch is difficulty.
- **The route terms are a tie-break between comparable goals, not a veto.**
  An objective offering a shot is worth about seven points more than one that
  does not, against route costs of one or two; tests of these terms stage
  goals worth roughly the same.

### Difficulty has two axes, and is a lens rather than a lottery

- **`difficulty_noise` is misjudging what she read; `difficulty_foresight` is
  not having read it** (the blend between crow flight and road, and the gate
  on both doctrine terms above). More terms make the blur matter *less*, so
  anything added to the chooser should ask which axis it belongs to.
  `UtilityPlanner::new` takes a difficulty rather than a noise amplitude so
  nobody passes one derived number and forgets the other.
- **`UtilityPlanner::lean` draws one blur per unit per round**, applied as a
  smooth function of where a tile lies relative to her. Do not make it a flat
  per-unit offset (a no-op under an argmax) and do not reintroduce a per-tile
  draw anywhere in `score_tile`'s callers: the maximum of ninety draws from
  ±0.5 is about +0.49 against an objective gradient of 0.54 a hex, and it
  punished fast vehicles hardest. `span` normalises the lean against reach.
- **Zero at difficulty 5, exactly**, pinned by
  `a_side_that_sees_clearly_is_untouched_by_the_blur`. The determinism
  baseline fights at difficulty 3 and therefore moves when this changes.
- **The plateau rule** takes the nearest tile among those within
  `planner.plateau` of the best score, then the better, then the one further
  along her facing. It replaced a coordinate sweep that sent every noiseless
  crew to the same corner of every plateau. Zero is *not* its gentle setting.

### How the AI thinks is a mod, and it is not `balance`

The evaluator's and chooser's numbers are the `planner` block of `mod.json`
(`data::PlannerRules`): `impatience`, `horizon_rounds`, `boarding_rounds`,
`deviation_cost`, `devolved`, `order_worth`, `order_complement`,
`score_worth`, `pull_under_fire`, `distance_decay`, `plateau`,
`exit_urgency`, `cover_prior`, `elevation_prior`.
`mission_weight` is retired (2026-09-07): it deserialises into a field nothing
reads so that `validate-mods` can warn a mod that still declares it.

- **The line between `planner` and `balance` is load-bearing.** `balance`
  says what is *true on this battlefield* and reaches a human's shot exactly
  as a machine's; nothing in `planner` reaches a rule, so a mod that rewrote
  every field leaves a human-versus-human battle bit-identical. **The test
  for which block a new number belongs in is that question**, not which file
  it sits in.
- **Every field `#[serde(default)]`s to exactly the constant it replaced**,
  and each has a test that it is read at all (`tests/engine.rs`, under *the
  planner numbers are data*), mutation-checked.
  `the_planner_numbers_can_be_swept_and_ship_at_the_values_they_replaced`
  covers addressability and that the base mod ships the defaults.
- **A number that differs per doctrine belongs on the doctrine**
  (`route_caution`, `contest_aversion`); one the same for every commander
  belongs here. `devolved` is here because it is a threshold read *against*
  `DoctrineDef::delegation` — one decision about the whole roster.
- **`distance_decay` is one slope for objectives *and* missions.** It used
  to be shared because `mission_weight` was quoted in objective units and
  "an order pulls about as hard as the ford" was only true while the
  gradients matched. The two are quoted in different things now, and the
  reason is better: the slope is not a statement about what ground is worth
  at all, it is *how far off a crew can still tell which way to drive*, a
  fact about her and the map.
  `an_order_and_an_objective_are_led_to_by_the_same_slope` pins it. It moves
  the flat control too, so it is a global knob rather than an
  order-versus-terrain one. The centre-seeking `0.15` in `score_tile`'s
  `advance` fallback is deliberately *not* this number.
- **`horizon_rounds` is the one with a performance cost**: 61 / 106 / 155 /
  219 µs per `roads()` at two, three, four and six rounds.
- **`pull_under_fire` is reached only by `Advance` and `Recon`**, and
  `ai/command.rs` picks posture off `aggression` (≥0.7 `Assault`, ≥0.5
  `Advance`, else `Hold`), so which doctrines ever meet it is content. A
  sweep returning *exactly* zero is asking you to find out why: "no effect"
  and "never evaluated" look identical in the table. The measured results —
  the horizon null, `mission_weight`'s 37→2 tax and `order_worth`'s sweep
  that replaced it, `pull_under_fire`'s bit-identical row — are in DONE.md.
- **Every planner weight that is data was swept after the currency grew and
  came back null** (`impatience`, `plateau`, `route_caution`,
  `contest_aversion`, `score_worth` itself); every one that is bare Rust
  could not be asked. The sweep is the prize; a number that cannot be swept
  is a number nobody can argue with.

### Defiance: what a crew does instead

`DefianceResponse` (`data/morale.rs`) — `Freeze`, `Flight`, `Fight` — is what
a crew on a rung whose `obeys` is false does.

- **The rung decides *that* she defies; her temperament decides *how*.** Score
  is `base + core + trait modifiers`, highest wins, **ties to the earlier
  entry**; cores default to `AVERAGE`, so listing `freeze` first is what makes
  a cadet nobody wrote cores for behave as every crew did before. Empty list
  means freeze.
- **Traits reach it through `TraitEffect.defiance`**, beside the optional
  `skill`, because temperament under fire is not a competence.
- **The senior cadet still fighting decides**, not the best score aboard.
- **A crew cannot refuse her own decision.** `UnitIntent::own_idea` marks a
  route she laid herself (the drill's dash, flight). Without it the refusal
  check throws away a fleeing crew's own path every tick and she shakes in
  place. Anything laying a path from inside the engine sets it; an *order*
  clears it.
- **`Freeze` costs opportunity fire** (or it and `Fight` differ in nothing
  observable); an ordered shot still happens. **`Fight` turns ambush
  discipline off**, a cost.
- **Rallying reads sight, not the radio**: `recovery_near_leader` is shed by a
  crew who can see her formation's leader through `fog::sees` — not
  `fog.side(..).visible`, which is vacuous for own units, and not contact,
  which would make a zeroed `command` block differ in deeds from no block.
- **Flight goes away from contact, never toward an exit**, and it is the
  rout, not the drill: distance from every threat first, and the resolver's
  `incoming` only to settle the hexes that tie. The orderly reaction lives
  in `run_crew_drill` and minimises fire with no distance term; see One
  currency.
- **`Event::Defied` replaced `OrderRefused`** and carries what she did and
  where she went.

### Infantry, passengers and concealment

Infantry are a `VehicleDef` (`MovementClass::Foot`, armour 0, leadership seats
in the crew model, the rest a `ModuleEffect::Troops` module). Design record:
`assets/wiki/reference/infantry.md`.

- **A passenger is `alive` but not on the field.** Her `pos` mirrors the
  carrier's, she is invisible to spotting, and `unit_at` / `occupants` filter
  her out. **Never decide "is anybody here" by scanning `units`**; go through
  `occupants` / `unit_at` / `spotted_enemy_at`. `BattleState::passengers` is
  the only correct way to ask what a carrier is carrying.
- **Shared fate is not optional.** A penetration into a loaded carrier rolls
  every passenger into the same interior pool and a brew-up burns them.
- **The troops module means three things at once** — interior weight
  (leaders last), firepower (`mustered` scales damage by hits remaining over
  toughness), and effectiveness (at zero the platoon is a remnant) — and they
  are easy to separate by accident.
- **`concealment` scales the *spotter's* range**, doubled when the target
  stands in cover ≥ 30, bypassed once she fires. The tile-vision cache is a
  pure function of the map and must not learn about it.
- **The AI reads what a formation is made of, never what it is called.**
  `lays_indirect`, `goes_on_foot` and the taxi/foot columns recognise a role
  off the hardware; a chassis id in an `if` inside `ai/` is the smell.
- **A taxi run is two halves and the AI plans both.** The carrier's half
  (drive to the pickup, hold the door while anybody has
  `boarding == Some(her)`) is not redundant: a platoon at one hex a round
  never catches a carrier at six. `planner.boarding_rounds` is 4, not the
  mechanical 2, because the cheap price made platoons thrash against the
  dismount reflex. Where an emptied carrier goes is still not planned (TODO).

### Two crews on one hex

Stacking is `VehicleDef.footprint` against `TerrainDef.capacity`.

- **Capacity is `Option<u32>` and `None` is not `1`.** A terrain declaring
  nothing keeps "one crew, whatever size" rather than "one footprint", which
  would refuse a medium tank onto grass the moment anything declared a
  footprint of 2. `VehicleDef::footprint()` reads zero as one — the same
  field-versus-accessor trap as `WeaponDef::reload`.
- **`occupants` is the honest question and `unit_at` is a convenience** (the
  lowest id, for a HUD line or a click). Occupancy goes through `room_for`;
  who a shot or a burst finds is everybody. The passenger filter lives in
  `occupants` alone.
- **`room_for` is fog-aware and that is not an approximation.** An unspotted
  enemy takes no room, or a refused order announces her; the move resolves as
  an ambush and two crews can end a tick over capacity.
- **Crowding is a reason not to *stop*, never a reason not to drive through.**
  `passable` ignores it and `destination_blocked` reads it.
- **A claim takes up room exactly as a parked vehicle does**, in
  `claimed_by_friend` and `ai/goal.rs::claimed_by_another`; `Goal::finished`
  reads the same rule.
- **A platoon dismounts onto the carrier's own hex**, tried first.
- **Sharing a hex with an enemy is deliberately still impossible.** Storming a
  building is close assault, not stacking (TODO).
- **The campaign map makes the same distinction**: only a *hostile* army
  blocks a route, and only for a march that means to avoid contact.

### Latitude: an order a crew may not set aside

`Latitude` (`battle/command.rs`): `Delegated` is every order this engine ever
had; `Binding` is "I mean it". The player says it with `X` on a crew and Ctrl
on a mission key.

- **A crew's latitude answers *will she break off for cover*; a formation's
  answers *may her doctrine discount the order at all*.** Do not unify them.
- **On a formation it reaches exactly one number**: the floor in
  `mission_value`'s `(1.5 - delegation).clamp(floor, 1.5)`, 0.5 → 1.0 under
  `Binding`. `delegation` may make a subordinate more literal than she was
  asked to be, never less.
- **It does not lift the contact damping.** `Advance` and `Assault` differ in
  that damping and nothing else, so a binding `Advance` that skipped it would
  be a synonym for `Assault`. Reaching for `contact_scale` because binding
  "ought to press on" is building the second idiom the chunk exists to avoid.
- **It travels with the mission or it means nothing**: on `MissionChange`,
  in the `CutOff` snapshot (read through `Formation::latitude_for`), on
  `Order::SetMission` / `QueueMission`. It belongs to the orders as a whole,
  not one leg.
- **On a crew it is read in exactly one place, `Unit::yields_to_drill`, and
  both drills ask it**: the planner's gate in `ai/command.rs` and the
  engine's mid-round reflex in `run_crew_drill` (the designer's ruling,
  2026-09-09 — before it the reflex read nothing, so a binding crew pressed
  on through the planning phase and was pulled into the trees five seconds
  later). A second reader of `Latitude::yields_to_drill` means the model
  drifted: latitude buys priority over her *own judgment*, never over her
  nerve, and the nerve check comes first at both sites. A delegated crew
  who has arrived on her ordered hex under fire still backs off it; that is
  her judgment, and the gate is the counterweight.
- **It lives inside the order, and arrives with her.** `Unit::orders` is
  `Option<PersonalOrder>` (`Holding { latitude }` or `Marching(March { to,
  latitude })`); `None` is a crew under her formation's mission, so
  *detached* is `is_some()`. A march that reaches its ground becomes a hold
  at the same latitude — "get there, I mean it" is not "get there and then
  use your judgment" — and an order that said nothing about her ground holds
  at `Delegated`, so an order about her gun cannot touch a latitude it does
  not carry (`an_order_about_her_gun_says_nothing_about_her_march`,
  `an_arrival_keeps_the_insistence_she_arrived_under`).
  `a_personal_order_is_taken_back_whole_or_not_at_all` pins both exits.
- **`Delegated` is the default everywhere and the AI never issues
  `Binding`**, spelled out at all four `Order::SetMission` sites in
  `ai/command.rs`. If `event_stream.txt` moves when you touch latitude,
  something leaked into AI-vs-AI play; do not regenerate.
- **The drill can only preempt from round two**: `radio()` marches her the
  moment the order lands. Any test of drill-versus-order fights a round
  first (`a_binding_march_presses_on_where_an_ordinary_one_takes_cover`).
- **A deviation must announce itself.** `AiPlanner::last_was_drill` and
  `Decision::drill` tell the presentation layer which orders were the
  planner's own idea. A vehicle that moves with no visible order behind it is
  indistinguishable from a bug.

### What an order promises, and what a battle costs

- **A mission's promise lives beside the mission.** `Mission::promise()` /
  `verb()` / `vocabulary()` and `Latitude::promise()` in `battle/command.rs`;
  one `VOCABULARY` table, exhaustive `slot()`. The game crate owns the keys
  and `every_mission_key_has_a_promise` guards the join.
- **`CrewCondition::Absent` is "on the roll, not in the vehicle"**, read once
  at spawn (`who_deploys`). Her seat leaves the substance reckoning entirely;
  she stays in `Unit::crew`; a vehicle nobody fit can crew goes out with the
  walking wounded. Ask "is she aboard / fighting" through
  `CrewCondition::aboard()` / `fighting()`, never by matching the variant.
- **`CrewLoss::found`** distinguishes a vehicle that did not come home
  (`resolve_crew_fate`) from a cadet found wounded in one that did
  (`resolve_station_fate`, gentler, never `Lost`).
- **The casualty numbers are `casualties` in `mod.json`.** A harsh campaign
  is a mod.
- **One cadet, one seat.** `OverworldState::from_map` enlists each character
  once per academy; a repeated name crews anonymously and `validate_into`
  warns. Do not "fix" a future warning by letting one cadet crew three tanks.
  **A short crew is not cosmetic**: substance counts people aboard, so a
  partly-named tank dies about twice as fast, which is why the campaign fills
  its seats (`every_seat_in_the_campaign_belongs_to_a_cadet_of_her_own`).
  `river_crossing` keeps its old ten and partial crews on purpose: it is the
  determinism baseline.
- **A battle never enlists anybody into an academy.** `apply_battle_result`
  drops crew ids the campaign roster does not know.

### The academy roll

`R` on the campaign map; `roster_page` in `game/src/overworld.rs`, a free
function over plain data.

- **Derived every frame, caches nothing.** A cached roll is stale exactly when
  the player opens it, which is after a battle.
- **A page over the map stops the world** (`pump_events`, `drive_ai`,
  `handle_input` bail), and `roster_input` runs *after* `handle_input` because
  `Esc` also drops the map selection.
- **It sets `ScriptFacts::waiting`**, the strict complement of `idle`.
- **Availability is worded as a state** (*infirmary, 4 day(s)*), not the
  after-action report's event (*wounded today*).
- **A cadet with no vehicle is still on the roll**, under "Without a vehicle".

### Saving

`tactics_core::save`; F5/F9 on the campaign map. The property that matters is
that **the future round-trips**, which `tests/save.rs` pins by forking a battle
through a save file. `SightGrid`, `MoveGrid` and the per-unit vision in
`FogMap` are `#[serde(skip)]` caches — an empty sight grid answers wrongly
rather than loudly, an empty move grid says nobody can drive, an empty
`visible_key` panics — and **rebuilding them is a type, not a duty**.
`BattleState` is `Battle<Built>`; what serde produces is `Battle<Unbuilt>`
(`SavedBattle`), because `Built` has no `Default` for a skipped field to take.
The one door between them is `SavedBattle::rehydrate(&registry)`, which
destructures every field, so a cache added tomorrow stops the build until
somebody says whether it travels in the file or is rebuilt on load. `SaveGame`
carries the same parameter; there is no route from a file to a playable battle
that does not pass a registry. `SAVE_VERSION` is 6, and **an older save is
refused, not migrated** (`SaveError::Version`): the two fields Phase 3
introduced default to the benign value, so a version-3 file would open with
every crew quietly back under her formation's mission and a version-4 file
with every wreck fighting again; version 6 gave `Holding` its latitude and a
version-5 `"holding"` no longer parses. Loud is right while there is no
released build to migrate from.

### Seeing the game without playing it

```sh
STAHL_PRESENT=immediate STAHL_DEBUG=1 STAHL_BATTLE=river_crossing \
  STAHL_SCRIPT=scripts/dev/battle-tour.txt cargo run -p stahlsenshamädchen
```

`crates/game/src/devtools.rs` drives the real input path from a script;
`scripts/dev/run-tours.sh` runs every tour on the map its `#!env` line
declares, and **is the only thing that makes "a tour is a test" true** — not
`cargo test --workspace`, not CI. `STAHL_HEADLESS=1` runs them without a GPU.
This machine needs `STAHL_PRESENT=immediate` (an NVIDIA driver bug, proved
with `examples/minimal_window.rs`). Rules:

- **The scripted cursor is a resource (`ScriptedCursor`), not the window's**;
  scripts name a hex.
- **`run_script` must stay `.after(InputSystems)`.**
- **Scripts wait on the game** (`until` / `expect` over `ScriptFacts`), never
  on a stopwatch.
- **`idle` is `Battle::listening`, one predicate.** Anything new that makes
  `handle_input` refuse a keystroke belongs inside `listening`, not beside
  it. `waiting` means "held behind something the player must dismiss".
- **Every screen assigns the whole `ScriptFacts` through an exhaustive
  literal**, no `..default()`; a field a publisher leaves alone would hold the
  previous screen's answer.
- **A click on a stacked hex cycles through its occupants**;
  `ScriptFacts::selected` lets a tour assert who.

### The danger overlay

The player's half of the currency: `panel::format_danger` (pure over a
`BattleState`, reading `fire_on` verbatim) leads the tile panel whenever one
of her own crews is selected and a hex is hovered, and `D` tints the selected
crew's reachable tiles by a round of fire in worth.

- **It is `fire_on` and nothing else.** No arithmetic of its own beyond the
  sums and their share of `substance().0`; the AI and the player price the
  same ground or the player is pricing a different game from the one her
  opponent plays. Per gun the panel prints one shot and the gun's cadence
  ("3 shot(s) a round"); the totals are per round — damage ("expected this
  round"), pressure when there is any, and worth ("all told") with its share
  of what she has left. The share is of *worth*, the same figure the tint
  reads, or the panel would give two answers.
- **Never for an enemy crew**, and not on the shot preview or the ghost
  report — both already answer about a hex somebody else stands on.
- **Computed under the `range_dirty` gate and cached in `Battle::danger`**,
  never per frame: about 0.5 ms over a full reach.
- **The overlay replaces the move-range blue rather than stacking on it,
  and it is a gradient, not bands** (the designer's call: distance is most
  of what decides expected fire, and three cuts threw that away — one 88
  shot is a third of a medium tank, so every tile it saw was the top band
  from the first contact). `panel::danger_tint(share)` is the one judgment:
  the share of what she has left that a round there would cost, linear,
  clamped, so full red means one thing everywhere — stand here and she is
  gone this round. `battle::danger_color` mixes `DANGER_LOW` into
  `DANGER_HIGH` along it and paints `DANGER_NONE` (the move-range blue) at
  exactly nothing; `panel::DANGER_LEGEND` is three sentences, the blue and
  the two ends.
- **`ScriptFacts::danger`** counts tinted tiles, `None` when the overlay is
  off, so `danger >= 1` is false until it is up and has found something.
- **`Bearing` is per shot and the totals are per round.** `damage_per_round`
  / `pressure_per_round` / `worth_per_round` on the bearing do the
  multiplication; a 75 mm at a shot every 15 s fires four times a round.

Rust edition 2024, resolver 3. `[profile.dev]` builds the workspace at
`opt-level = 1` and dependencies at 3. Anything that measures performance must
be built `--release`.

## The scale contract

Lives in the `scale` block of `assets/mods/base/mod.json`, typed as
`data::Scale`, reachable as `registry.scale`. A mod that declares its own block
replaces it wholesale. `cargo run --bin validate-mods` prints the whole roster
through it.

| Quantity | Field | Value |
| --- | --- | --- |
| Battle hex | `hex_meters` | 100 m |
| Round | `round_seconds` | 60 s |
| Tick | `round_seconds / ticks_per_round` | 5 s (12 ticks) |
| Elevation level | `elevation_meters` | 10 m |
| Overworld hex | `overworld_hex_meters` | 4 km = one battle map |
| Overworld turn | `overworld_turn_hours` | 24 h (a day) |

**A battle map is a hexagon, not a rectangle.** `Scale::battle_map_radius()`
derives radius 20 from 4 km ÷ 100 m (41 across, 1261 tiles); `MapKind::Battle`
implies `MapShape::Tile` and validation rejects anything else. A scenario map
sets `"shape": "free"`. `HexMap` is a sparse `HashMap<Hex, Tile>` and a space
in a row means "no tile here".

There is deliberately no `TICKS_PER_ROUND` constant, which is why round
resolution, cooldowns and validation take a registry. `WeaponDef::reload_ticks`
is an `Option<u32>` because serde's default fn cannot see the mod; read it
through `weapon.reload(&registry.scale)`, never the field.

- **1 movement point ≈ 1 hex of clear terrain per round ≈ 6 km/h.** `points`
  is a speed. 5 tracked MP on grass is 30 km/h.
- **Weapon `range` is in hexes, so ×100 m.** `reload_ticks` is a practical
  aimed rate, ×5 s.
- **Vision is deliberately shorter than gun range for gun tanks.** The tank
  destroyer sees 10 and shoots 16 because needing a spotter is its character.
- **A small test map cannot put units out of contact by distance.** Use a
  forest curtain (`tests/engine.rs::standoff`).
- **`elevation_meters` is read by the climb rule *and* by line of sight**,
  both through `Heights::of`;
  `a_mod_that_flattens_a_level_flattens_the_skyline` asserts the two sight
  paths agree. **A `Scale` field no rule reads is a bug**, the same way a core
  no skill names is.
- **Crew bonuses are percentages of the vehicle's base** (`balance`: +5%
  sight per awareness, +5% speed per driving, +3 points of hit chance per
  gunnery). Never reintroduce a flat divisor.

## Invariants worth protecting

- **`tactics_core` must not depend on Bevy.** All presentation, input and
  asset loading lives in `crates/game`.
- **The simulation is deterministic given a seed.** `ChaCha8Rng`, and no
  behaviour may depend on `HashMap`/`HashSet` iteration order. Sort first, or
  iterate `state.units` in id order. `tests/determinism.rs` pins four seeds'
  event streams byte for byte, because a self-consistency test cannot catch
  iteration-order bugs. Regenerate deliberately with `UPDATE_SNAPSHOTS=1` and
  read the diff first: a broad diff means you changed balance, a small one
  about the *order* of otherwise identical events means you introduced the bug
  this test is for.
- **Content is data, not Rust.** The base game is a mod. A tuning constant in
  Rust that a modder would want to change is a design smell.
- **Fog must not leak through the order system.** An order is never refused
  in a way that reveals an unspotted enemy; `destination_blocked` treats them
  as passable and the move resolves as an ambush. `spotted_enemy_at` exists so
  callers do not reach for `unit_at`.
- **Damage lands during a tick; death is reaped at the end of it.** Do not
  make `reap` eager.
- **A tiebreak may only read quantities a reflection preserves.** Distances,
  terrain costs and a dot product of two differences qualify. **A coordinate
  does not**, and no total order on coordinates can: it asks which way is
  west, and every crew then edges that way wherever the real keys tie. It
  shipped twice in `ai/` and once in the sight geometry, and cost about two
  points of win rate. Where a total order is still needed (`reachable`
  returns a `HashMap`), put the coordinate key **last**.
  `a_march_is_the_same_march_from_either_end`,
  `a_reflection_leaves_a_bearing_alone_and_turns_a_coordinate_around` and
  `a_reflection_leaves_a_sight_line_alone` guard it.
- **`alive()` and "standing on a hex" are two different questions.** A
  unit's outcome is `Fate`: `Fighting { doom }` (on the field, targetable,
  blocking, shooting — `doom` is a destruction taken this tick and not yet
  reaped, kept *inside* the variant so "is she on the field" stays one
  arm), `Destroyed(Destruction)` and `Exited`. Only `Fighting` is `alive()`;
  only `Destroyed` is `lost()`; a passenger is `Fighting` on no hex anybody
  may interact with, and that half is about `aboard`, not fate. When two
  destructions land on one hull in a tick — it happens about once in forty
  losses — `Destruction::supersedes` keeps the more telling one: burning
  beats the crew leaving beats the hull being crushed, and the middle rung
  is load-bearing because `behind_armor_effects` will not roll a bail-out
  for a crew already gone. Ask occupancy through `unit_at` / `occupants`,
  and never reimplement either by scanning `units`.
- **Difficulty is a mod.** Every harsh system is an additive rule whose
  absence *is* the gentle game; if switching one off needs an `if` in Rust,
  it was built wrong. The determinism snapshot passing *unregenerated* is the
  evidence a chunk added a rule without changing one.

## Style

Comments explain *why*, in prose, and are often several lines — match that
rather than trimming to terse one-liners. Public items carry doc comments.
Errors are `thiserror` enums. Optional mod-facing fields get `#[serde(default)]`
and enums get `#[serde(rename_all = "snake_case")]`. Tests are end-to-end
against the real `assets/mods` content and named as full sentences stating the
rule they defend (`unspotted_enemies_still_ambush`). Commit messages quote the
instrument's numbers.

## Known issues

### Correctness

- **Army-contained unit placements are never validated.** `map.rs` passes
  `a.at` (the army's own hex) instead of `u.at` when checking each unit inside
  an `ArmyPlacement`, so a unit's own coordinates are neither validated nor
  used. `frontier.json` carries 14 `at` fields on army units that mean
  nothing. Either drop the field or honour it.
- **Overworld elevation is priced at the battle scale.** `Scale` has one
  `elevation_meters`, so `frontier`'s mountains at elevation 2 read as 20 m.
  Harmless today; a strategic map wants its own vertical scale.
- **Difficulty discriminates on the ridge and not on the old arena — but
  only just.** At 8 seeds × 36 (576 a row) on `ridge_arena`, 5 over 1 is
  **51.4%** and 5 over 3 **49.7%** since the ladder scaled by damage spent
  (2026-09-09; 53.5% and 52.3% after Wave 2, both moves inside the table's
  own spread, and the control row 144–144); before Wave 2 they were 45.3%
  and 48.3%, the best commander declining the crest. Two thirds of the
  Wave 2 gain came from the drill and the rout reading the resolver instead
  of the terrain table, before any weight moved. On `skill_arena` the same
  rows are 50.8% and 49.7%: a radius-10 hexagon with two objectives has
  nothing for a commander to be better at, and it stays the default only
  because every quoted number was measured on it. Read the skill table on
  the ridge, at `--games 36` or not at all, seed-swept.
- **`score_tile` still holds four bare constants** — the mass band
  (−0.45 / −0.15 / −0.12 × `concentration`), the +4.0 kill bonus, the 0.3
  advance slope, the 0.15 centre-seeking fallback — in a currency that has
  grown by an order of magnitude since they were written. Nobody has noticed
  because nobody can ask; they want to be `planner` fields.

### Robustness

- **Both setup paths refuse bad content rather than panicking.**
  `from_placements` returns the same `BattleSetupError` as `from_map`,
  collecting every problem (a hex off the map, an unknown vehicle or side, a
  crew id the roster lacks), and `spawn_unit` is fallible. The campaign asks
  first: `launch_battle` runs `battle::field_battle_problem` and declines the
  clash with a log line *before* `commit_to_battle`; `choose_battle_map`
  returns `None` when no mod ships a battle map.
  `every_clash_the_campaign_map_can_produce_can_be_staged` is what makes the
  check worth having.
- **Elevation grids fail soft in a confusing way.** A missing or short
  `elevation` row silently defaults to 0 while a mismatched one only warns.
- **"Where is she going" is still four fields** (`intent.path`, `orders`,
  `goal`, `boarding`) plus the formation mission. Phase 3b took one off the
  list; the four left are genuinely different time scales (this tick, this
  errand, her own plan, a rendezvous) and folding them wants a design
  decision rather than a refactor.
- **The bail-out guard reads two of the four destructions, and that is the
  rule**: a hull crushed by blast this tick can still roll for a bail-out,
  because there may be survivors (the designer's ruling, 2026-09-09). Its
  prospective rung asks `pressure_for` for the shell's real price now, so
  it counts the round's suppression, which it did not before.

### Performance

`cargo run --release -p tactics_core --example perf` reproduces these; `--mcts`
adds the slow ones. Measured on the 1261-tile `river_crossing` with 8 units,
four seeds, after the ladder scaled by damage spent (2026-09-09):

| | |
| --- | --- |
| round resolution | 1.96 ms (was 1.84 after Wave 1, 1.59 after Phase 2, 1.49 the same day on the same machine) |
| `reachable()` per call | 15.4 µs |
| `roads()` per call | 108.7 µs |
| `unit_vision` per unit, cold | 90.8 µs (94.4 the run before: the machine is comparable) |
| utility order | 0.08 ms |
| mcts order, difficulty 3 / 4 | 1.84 s / 4.24 s (not re-measured since Wave 1) |

`reachable` and `roads` measure work *in a particular game state*; they moved
with Phase 2 because the units they are measured on stand somewhere else by
then, not because either function changed. `unit_vision` is the row that
says whether the machine is comparable.

- **Round resolution moves with how well the AI plays, not only with how much
  work the tick loop does.** Crews that pick ground they can drive to spend
  fewer ticks manoeuvring. `reachable()`, `roads()` and `unit_vision` measure
  work and are the numbers to read when the question is whether something got
  slower.
- **`score_tile` is the hottest function the AI has**, paid per candidate
  tile, and its threat loop now costs what its attack loop does (a
  `best_weapon_from` per found enemy per tile). Utility order paid 12% for
  that and nothing for the pressure expectation, which comes off the same
  profile. Round resolution is up a fifth since cadence and suppression:
  crews stand and trade fire where they used to manoeuvre, and the shot
  census agrees (ShotHit 26 → 36 per snapshot, ShotMissed 59 → 46). `fog::recompute` after every shot is nearly free because a side whose
  `(unit, pos, range)` list is unchanged skips the union — keep the reference
  `los_clear` and `SightGrid::clear` sharing `sight_line_clear`, and keep
  `cached_vision_is_the_same_answer_as_computing_it_fresh` passing.
- **Neither grid's inner loop reads occupancy**, and stripping the friend
  check out of `destination_blocked` entirely moves `reachable` only 31 → 28
  µs. Stacking is not a performance question.
- **String-keyed registry lookups cost about 21% of a round**, 62% of it
  `module` lookups from `substance` and its neighbours. Full interning is not
  justified by the numbers; making `substance` stop hashing is (STRUCTURE.md
  item 8).
- **MCTS is parked** at parity with the utility planner over 256 battles and
  ~30,000× the cost. No planner change owes it a budget (PARKED.md).
- **`reachable()` is O(hexes × units) twice over** — TODO, the
  occupancy-index item.

### Content gaps

- **Terrain coverage.** `choose_battle_map` looks for `battle_<terrain>` and
  falls through to the first battle map; only plains and forest exist, so a
  fight on a mountain or in a town lands on whichever iterates first.
  `river_crossing` deliberately fields no infantry and keeps its partial
  crews: it is the determinism baseline.
- **`battle_forest` declares no `exit`**, so nobody on it can withdraw.
- **There is no cover without `vision_block`.** Forest (30) and town (40)
  are the only covering terrains and both block sight at 2, so everything
  commanding is bare and everything covered is blind: a crew in the middle
  of the skill arena's hilltop village can barely see out of it. A hedge, a
  wall or a treeline that hides a hull without blinding the crew is content
  nobody has written, and until one exists "cover" and "dead ground" are the
  same word to the evaluator.
- **The base mod's doctrines exercise a narrow slice of the command model.**
  `Recon` is issued by nobody, elastic defence devolves and issues no ground
  missions, and only `bounding_overwatch` ever orders an `Advance`. Every
  instrument measures that slice (DONE.md, the order-terms entry).
- `deploy` sorts deployable tiles by depth from the map edge and is already
  scale-independent; on a hexagon a side deploys out of a vertex and fans
  inland, which is a shape change rather than a bug.

### Hygiene

- **The tree is clean and CI enforces it**: `cargo clippy --workspace
  --all-targets -- -D warnings` (which denies rustc's lints too) and
  `cargo fmt --check`, with `style_edition = "2024"` pinned. Run both before
  pushing. The shapes the cleanup introduced and the two `#[allow]`s that
  remain are in DONE.md under Tooling.
- `crates/game/src/battle.rs` is ~3500 lines and `overworld.rs` ~2200.
  `battle/panel.rs` holds the pure formatters; **"does it hold a Bevy type" is
  the line to draw** for the next extraction (STRUCTURE.md).
- `crates/tactics_core/tests/engine.rs` is ~14,500 lines in one binary,
  sectioned by subject with a contents list in the module doc. Splitting it
  is cheap now that `tests/common/` exists, and not yet done.
