//! The instrument's own calibration.
//!
//! `examples/balance.rs` is what this project measures itself with, and the
//! house style is quoting its numbers in commit messages. Until these tests
//! existed it was the least-tested code in the tree: `cargo test` does not
//! build tests in examples, so every claim CLAUDE.md makes about the harness
//! was verified once, by hand, and thereafter believed.
//!
//! Five claims, one test each:
//!
//! - no printed number moves with `--jobs`
//! - a run's accounting folds the same way however the battles were grouped
//! - `--sweep balance.x=v` is the same thing as hand-editing `mod.json`
//! - the `planner` block is addressable, and ships at the constants it replaced
//! - every arena is symmetric, under both reflections
//! - every arena is one a table can be pointed at by name
//!
//! The point of a calibration test is that it fails when the *instrument* is
//! wrong rather than when the game changes, so none of these fight a battle.

use tactics_core::harness::arena::{ARENAS, DEFAULT_ARENA, REFLECTIONS, arena_named};
use tactics_core::harness::overrides::{Override, apply_override};
use tactics_core::harness::parallel::{JOBS, run_all};
use tactics_core::harness::tally::Tally;

mod common;
use common::registry;

/// Results come back in job order, whatever the machine did with them.
///
/// This is the property the whole parallel pass rests on: results are folded
/// in job order, which is seed order, so a printed number cannot depend on
/// which core finished first. A balance figure that moved with scheduling
/// would be worse than a slow one, because it would look exactly like noise.
///
/// The jobs are deliberately uneven in cost — the sleep is proportional to a
/// scrambled key — so that a thread which was handed the easy half really does
/// finish long before one handed the hard half. With equal-cost jobs a broken
/// implementation would pass by luck.
#[test]
fn no_printed_number_moves_with_how_many_threads_ran_it() {
    let jobs: Vec<u64> = (0..64).collect();
    let mut answers = Vec::new();
    for budget in [1usize, 3, 7, 64] {
        JOBS.store(budget, std::sync::atomic::Ordering::Relaxed);
        answers.push(run_all(&jobs, |job| {
            // Uneven work, so the threads genuinely finish out of order.
            let spin = (job * 2654435761) % 97;
            std::hint::black_box((0..spin * 500).fold(0u64, |a, b| a.wrapping_add(b)));
            job * 10
        }));
    }
    JOBS.store(0, std::sync::atomic::Ordering::Relaxed);

    let expected: Vec<u64> = jobs.iter().map(|j| j * 10).collect();
    for (budget, got) in [1, 3, 7, 64].iter().zip(&answers) {
        assert_eq!(
            got, &expected,
            "--jobs {budget} returned results in a different order"
        );
    }
}

/// A run's accounting folds the same way however the battles were grouped.
///
/// `Tally::merge` is what lets the batch be split across the machine and still
/// produce one table, and the property it actually needs is **associativity**:
/// `(a + b) + c` must equal `a + (b + c)`, because the only thing the harness
/// guarantees is that results are folded in job order, not how they were
/// divided into chunks.
///
/// Worth being precise, because the doc comment on `merge` was not. It says
/// every field must be "a sum, a concatenation or a union of sums" and that a
/// *maximum* would make the numbers depend on which core finished first.
/// `deepest_stack` is a maximum, and it is fine: max is associative and
/// commutative, which is all that is required. The genuinely dangerous field
/// would be one that is neither — a ratio, or a last-value-wins.
#[test]
fn a_tally_folds_the_same_way_however_the_battles_were_grouped() {
    let tally = |wins: usize, rounds: &[u32], deepest: usize| {
        let mut t = Tally {
            rounds: rounds.to_vec(),
            deepest_stack: deepest,
            shots: wins as u32 * 7,
            ..Default::default()
        };
        t.wins.insert("Kuhlmann".into(), wins);
        t.causes.insert("brewed up", wins);
        t
    };
    let (a, b, c) = (
        tally(3, &[10, 12], 1),
        tally(5, &[9], 2),
        tally(2, &[14, 8], 1),
    );

    let mut left = a.clone();
    left.merge(&b);
    left.merge(&c);

    let mut right = b.clone();
    right.merge(&c);
    let mut right_all = a.clone();
    right_all.merge(&right);

    assert_eq!(
        left, right_all,
        "merge is not associative, so the table depends on how the batch was chunked"
    );
    assert_eq!(
        left.deepest_stack, 2,
        "a maximum still folds to the maximum"
    );
    assert_eq!(
        left.rounds,
        vec![10, 12, 9, 14, 8],
        "job order is preserved"
    );
}

/// ...and job order is load-bearing rather than decorative.
///
/// `merge` is deliberately *not* commutative — the sample vectors concatenate,
/// so `a + b` and `b + a` differ in the order of `rounds`. That is fine and it
/// is why [`run_all`] guarantees job order. This test exists so that nobody
/// "simplifies" the ordering away on the grounds that summing commutes.
#[test]
fn the_order_battles_are_folded_in_is_load_bearing() {
    let tally = |rounds: &[u32]| Tally {
        rounds: rounds.to_vec(),
        ..Default::default()
    };
    let (a, b) = (tally(&[1, 2]), tally(&[3]));
    let mut forward = a.clone();
    forward.merge(&b);
    let mut backward = b.clone();
    backward.merge(&a);
    assert_ne!(
        forward, backward,
        "if merge commuted, run_all would not need to preserve job order — \
         check whether a sample vector was quietly turned into a count"
    );
}

/// Sweeping a field is the same thing as hand-editing `mod.json`.
///
/// This is the claim the whole `--set` / `--sweep` machinery rests on: the
/// patch is a serde round trip through the same representation a save file
/// holds, so it is not a second way of configuring the game, it is the same
/// way. If it ever stops holding, the override machinery has become a second
/// game and every number swept with it is about something other than what
/// ships.
///
/// Checked by serialising the whole block both ways rather than by reading
/// back the one field: that is what catches a round trip which sets the right
/// number and quietly drops or defaults a neighbour.
#[test]
fn a_swept_override_is_the_same_thing_as_editing_the_mod_by_hand() {
    let mut swept = registry();
    let was = apply_override(
        &mut swept,
        &Override::parse("balance.moving_target_per_hex=40").expect("a well-formed override"),
    )
    .expect("the field exists");

    let mut by_hand = registry();
    assert_ne!(
        by_hand.balance.moving_target_per_hex, 40,
        "the test needs a value the base mod does not already use"
    );
    assert_eq!(was, by_hand.balance.moving_target_per_hex.to_string());
    by_hand.balance.moving_target_per_hex = 40;

    assert_eq!(
        serde_json::to_value(swept.balance).unwrap(),
        serde_json::to_value(by_hand.balance).unwrap(),
        "the patched block differs from the hand-edited one somewhere other \
         than the field that was swept"
    );
}

/// The `planner` block is reachable from the command line, and the block the
/// base mod ships is the game the constants gave.
///
/// Two claims in one test because they fail together and for the same reason.
/// The AI's constants became data across 2026-08-27 and -28, and **the prize
/// is the sweep** — `--sweep planner.horizon_rounds=2,4,6` asks what looking
/// further ahead is worth to a commander, and `--sweep
/// planner.mission_weight=1,2,4` asks what an order is worth against the
/// terrain. Neither had ever been measured, because the only way to ask was
/// to edit Rust and rebuild. A block nothing can address buys none of that.
///
/// The second half is the additivity contract, checked the strong way: every
/// field of the shipped block equals the constant it replaced, so a mod that
/// declares no `planner` block and the base mod that declares a full one are
/// the same game. A typo in `mod.json` would otherwise be a silent balance
/// change wearing a refactor's clothes.
#[test]
fn the_planner_numbers_can_be_swept_and_ship_at_the_values_they_replaced() {
    use tactics_core::data::PlannerRules;

    let mut reg = registry();
    assert_eq!(
        reg.planner,
        PlannerRules::default(),
        "the base mod's planner block must be the game the constants gave"
    );

    let was = apply_override(
        &mut reg,
        &Override::parse("planner.horizon_rounds=2").expect("a well-formed override"),
    )
    .expect("the field exists");
    assert_eq!(was, "4");
    assert_eq!(reg.planner.horizon_rounds, 2);
    assert_eq!(
        serde_json::to_value(reg.planner).unwrap(),
        serde_json::to_value(PlannerRules {
            horizon_rounds: 2,
            ..PlannerRules::default()
        })
        .unwrap(),
        "the patched block differs from the hand-edited one somewhere other \
         than the field that was swept"
    );

    // And a float field by the same path, written the way somebody actually
    // types it on a command line. `--set planner.score_worth=4` parses as a
    // json integer, and a field that refused to take one would send a
    // designer hunting for the mistake in their own sweep rather than in
    // ours.
    let was = apply_override(
        &mut reg,
        &Override::parse("planner.score_worth=4").expect("a well-formed override"),
    )
    .expect("the field exists");
    assert_eq!(
        was, "3",
        "a whole-numbered float reports its old value without the trailing zero, \
         which is what a sweep's legend prints"
    );
    assert_eq!(reg.planner.score_worth, 4.0);
    let was = apply_override(
        &mut reg,
        &Override::parse("planner.order_complement=1").expect("a well-formed override"),
    )
    .expect("the field exists");
    assert_eq!(was, "0.5", "the field Wave 4 added ships at a half");
    assert_eq!(reg.planner.order_complement, 1.0);

    // And the field Wave 2 retired is addressable by nothing, which is the
    // point of it never being serialised: a designer who sweeps the name they
    // remember gets an error listing the names that exist — including the one
    // that replaced it — instead of a table whose rows all measured the same
    // game.
    let err = apply_override(
        &mut reg,
        &Override::parse("planner.mission_weight=4").expect("a well-formed override"),
    )
    .expect_err("a retired field must not be silently sweepable");
    assert!(
        err.contains("order_worth"),
        "the error should point at what replaced it, got: {err}"
    );
}

/// A mod still setting a field this engine has retired is told so.
///
/// The one kind of mod error the data machinery cannot otherwise report:
/// serde ignores what it does not recognise, so a modder who had tuned
/// `mission_weight` loads cleanly, plays a different game from the one they
/// wrote, and has nothing to grep for. It is a warning rather than an error
/// because carrying a stale key from an older engine is a legitimate thing
/// for a mod to do; what is not acceptable is silence.
#[test]
fn a_mod_setting_a_field_that_no_longer_exists_is_told_which_one_replaced_it() {
    use tactics_core::data::PlannerRules;

    let planner: PlannerRules = serde_json::from_value(serde_json::json!({
        "mission_weight": 2.0,
    }))
    .expect("a retired field must not stop the block from loading");
    assert_eq!(planner.retired_mission_weight, Some(2.0));
    assert_eq!(
        planner.order_worth,
        PlannerRules::default().order_worth,
        "and the mod gets the shipped price of an order rather than its own stale one"
    );

    let mut reg = registry();
    reg.planner = planner;
    let report = reg.validate();
    assert!(
        report.errors.is_empty(),
        "a stale key is a warning, not a refusal: {:?}",
        report.errors
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("mission_weight") && w.contains("order_worth")),
        "the warning has to name both the dead field and its replacement: {:?}",
        report.warnings
    );

    // And it is genuinely dead weight rather than a field with a new name:
    // what a mod wrote there reaches nothing that serialises, so a sweep of
    // the block round-trips without it.
    let round_tripped: PlannerRules =
        serde_json::from_value(serde_json::to_value(reg.planner).unwrap()).unwrap();
    assert_eq!(round_tripped.retired_mission_weight, None);
}

/// A path that names nothing is an error, and the error says what was there.
///
/// The silent alternative is a sweep whose rows all measured the same game and
/// agreed with each other beautifully, which is the failure this whole
/// instrument exists to avoid.
#[test]
fn an_override_that_names_nothing_says_what_was_there() {
    let mut reg = registry();
    let err = apply_override(
        &mut reg,
        &Override::parse("balance.no_such_knob=1").expect("well-formed"),
    )
    .expect_err("a field that does not exist must not silently succeed");
    assert!(
        err.contains("moving_target_per_hex"),
        "the error should list what was actually at that level, got: {err}"
    );
}

/// Every feature of every arena has a mirror, and a flip.
///
/// The skill-gap table measures commanders against each other, so any
/// asymmetry in the ground is measured as skill. This is not hypothetical: the
/// arena used to be written as rows of ASCII with forest at fixed columns —
/// symmetric as *text*, sheared into asymmetry by the odd-r offset conversion
/// — which gave side A nine forest hexes near its deployment against side B's
/// four, and cost about four points of measured win rate.
///
/// `Arena::map` asserts this itself before handing the map back. This test
/// checks it independently, so a mistake in the assertion is caught too — and
/// it walks **every** arena, because the failure this defends against is
/// somebody adding a battlefield and believing its symmetry rather than
/// asserting it, which is exactly how the first one went wrong.
#[test]
fn every_feature_of_every_arena_has_a_mirror() {
    for arena in ARENAS {
        let map = arena.map().expect("the arena builds");
        let centre = arena.centre_hex();

        let tiles: std::collections::HashMap<_, _> = map
            .terrain
            .iter()
            .map(|(h, t)| (h, (t.terrain, t.elevation)))
            .collect();
        assert!(!tiles.is_empty(), "{} has tiles at all", arena.id);

        for (name, image) in REFLECTIONS {
            for (hex, ground) in &tiles {
                let there = image(arena, *hex);
                assert_eq!(
                    tiles.get(&there),
                    Some(ground),
                    "{}: {hex:?} is {ground:?} and its {name} {there:?} is not",
                    arena.id
                );
            }
        }
        for hex in tiles.keys() {
            assert!(
                hex.distance_to(centre) <= arena.radius as i32,
                "{}: {hex:?} lies outside its own radius",
                arena.id
            );
        }

        // Both reflections have to be genuine reflections, or the checks above
        // are comparing a tile with itself and passing for free.
        //
        // The probe is written down rather than taken off the map, and both of
        // its arena coordinates are non-zero on purpose. A hex on the axis of
        // advance is *supposed* to be fixed by the flip — the axis is the
        // flip's mirror line — so a probe picked out of a `HashMap` fails this
        // check whenever hash order happens to hand back an axis hex, which is
        // a flaky test rather than a defect found. `rel(2, 2)` is four hexes
        // from the middle and off both lines, so every arena of any size has
        // it and neither reflection can fix it.
        let off_centre = arena.rel(2, 2);
        assert!(
            tiles.contains_key(&off_centre),
            "{}: the probe hex is not on the map",
            arena.id
        );
        assert!(
            off_centre.distance_to(centre) > 3,
            "{}: the probe hex is too close to the middle to prove anything",
            arena.id
        );
        for (name, image) in REFLECTIONS {
            let there = image(arena, off_centre);
            assert_ne!(
                there, off_centre,
                "{}: the {name} maps a distant hex to itself, so it is not a reflection",
                arena.id
            );
            assert_eq!(
                image(arena, there),
                off_centre,
                "{}: reflecting twice through the {name} must come back",
                arena.id
            );
        }
    }
}

/// A feature written as a band is doubly symmetric because of what a band is.
///
/// [`Arena::band`] is the primitive the ridge arena is built out of, and its
/// whole claim is that `|s|` and `|t|` — the along-axis and lateral
/// coordinates — are precisely the quantities both reflections preserve. That
/// claim is what lets somebody draw new ground on paper and be *sure* it comes
/// out symmetric, instead of drawing it, running the assertion and moving it
/// until the assertion stops firing. If it stopped holding, a band would be a
/// suggestion rather than a guarantee and the next arena would drift the way
/// the ASCII one did.
#[test]
fn a_band_is_the_same_band_from_either_end_and_either_flank() {
    for arena in ARENAS {
        // A deliberately lopsided band: no reflection symmetry of its own, and
        // both ranges offset from zero, so a bug that happened to be symmetric
        // for a centred band would still show.
        let band = arena.band(3..=9, 2..=5);
        assert!(
            !band.is_empty(),
            "{}: the probe band should land on some tiles",
            arena.id
        );
        let set: std::collections::HashSet<_> = band.iter().copied().collect();
        for hex in &band {
            for (name, there) in [
                ("mirror", arena.mirror(*hex)),
                ("flip", arena.flip(*hex)),
                ("both", arena.mirror(arena.flip(*hex))),
            ] {
                assert!(
                    set.contains(&there),
                    "{}: the band holds {hex:?} but not its {name} {there:?}",
                    arena.id
                );
            }
        }
        // ...and the coordinates really are what the module doc says they are:
        // the mirror negates both and the flip negates only the lateral one.
        let probe = arena.rel(-3, 2);
        assert_eq!(arena.axis_key(probe), (-4, 2));
        assert_eq!(arena.axis_key(arena.mirror(probe)), (4, -2));
        assert_eq!(arena.axis_key(arena.flip(probe)), (-4, -2));
    }
}

/// Every arena a table can fight on is one `--arena` can name, and the default
/// is the one the numbers were measured on.
///
/// The failure this catches is an arena added to [`ARENAS`] under a name
/// nothing accepts, or — much worse — a default quietly moved, which would
/// re-point every quoted figure in the tree at a battlefield it was never
/// measured on.
#[test]
fn every_arena_can_be_named_and_the_default_is_the_measured_one() {
    assert_eq!(
        DEFAULT_ARENA.id, "skill_arena",
        "every number this project has quoted was measured on the skill arena"
    );
    assert!(
        ARENAS.iter().any(|a| std::ptr::eq(*a, DEFAULT_ARENA)),
        "the default must be one of the arenas the flag lists"
    );
    for arena in ARENAS {
        let found = arena_named(arena.id).expect("an arena answers to its own name");
        assert!(
            std::ptr::eq(found, *arena),
            "{} resolved elsewhere",
            arena.id
        );
        assert!(!arena.blurb.is_empty(), "{} has no blurb", arena.id);
        // Four vehicles a side is what the skill table stages, and the two
        // halves of each flip pair have to be adjacent for that force to be
        // laterally symmetric. `assert_is_mirrored` checks the pairing; this
        // checks there is enough of it to fight the table at all.
        assert!(
            arena.deployment().len() >= 4,
            "{} has standing room for only {} vehicles a side",
            arena.id,
            arena.deployment().len()
        );
    }
    assert!(
        arena_named("no_such_arena").is_none(),
        "a name nothing answers to must not resolve to something"
    );
}
