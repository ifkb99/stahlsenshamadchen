//! The instrument's own calibration.
//!
//! `examples/balance.rs` is what this project measures itself with, and the
//! house style is quoting its numbers in commit messages. Until these tests
//! existed it was the least-tested code in the tree: `cargo test` does not
//! build tests in examples, so every claim CLAUDE.md makes about the harness
//! was verified once, by hand, and thereafter believed.
//!
//! Four claims, one test each:
//!
//! - no printed number moves with `--jobs`
//! - a run's accounting folds the same way however the battles were grouped
//! - `--sweep balance.x=v` is the same thing as hand-editing `mod.json`
//! - the skill-gap arena is symmetric
//!
//! The point of a calibration test is that it fails when the *instrument* is
//! wrong rather than when the game changes, so none of these fight a battle.

use tactics_core::harness::arena::{ARENA_RADIUS, arena_centre, arena_map, arena_mirror};
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

/// Every feature of the arena has a mirror.
///
/// The skill-gap table measures commanders against each other, so any
/// asymmetry in the ground is measured as skill. This is not hypothetical: the
/// arena used to be written as rows of ASCII with forest at fixed columns —
/// symmetric as *text*, sheared into asymmetry by the odd-r offset conversion
/// — which gave side A nine forest hexes near its deployment against side B's
/// four, and cost about four points of measured win rate.
///
/// `arena_map` asserts this itself before handing the map back. This test
/// checks it independently, so a mistake in the assertion is caught too.
#[test]
fn every_feature_of_the_arena_has_a_mirror() {
    let map = arena_map().expect("the arena builds");
    let centre = arena_centre();

    let tiles: std::collections::HashMap<_, _> =
        map.iter().map(|(h, t)| (h, t.terrain.clone())).collect();
    assert!(!tiles.is_empty(), "the arena has tiles at all");

    for (hex, terrain) in &tiles {
        let mirror = arena_mirror(*hex);
        assert_eq!(
            tiles.get(&mirror),
            Some(terrain),
            "{hex:?} is '{terrain}' and its mirror {mirror:?} is not"
        );
        assert!(
            hex.distance_to(centre) <= ARENA_RADIUS as i32,
            "{hex:?} lies outside the arena's own radius"
        );
    }

    // The reflection has to be a genuine point reflection, or "mirrored" above
    // is checking a tile against itself and passing for free.
    let off_centre = tiles
        .keys()
        .find(|h| h.distance_to(centre) > 3)
        .expect("the arena is bigger than a handful of hexes");
    assert_ne!(
        arena_mirror(*off_centre),
        *off_centre,
        "the mirror maps a distant hex to itself, so it is not a reflection"
    );
    assert_eq!(
        arena_mirror(arena_mirror(*off_centre)),
        *off_centre,
        "reflecting twice must come back"
    );
}
