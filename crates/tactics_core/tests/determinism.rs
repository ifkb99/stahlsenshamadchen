//! Pins the event stream against a committed baseline.
//!
//! `battle_resolution_is_deterministic_per_seed` and
//! `the_same_intents_replay_the_same_way` in `engine.rs` prove that a run
//! matches *itself*. That is a weaker claim than it sounds: a sim whose
//! behaviour depends on `HashMap` iteration order is perfectly self-consistent
//! within one process, because the hash seed does not change mid-run. The
//! ordering bug this project has already shipped once — `fog::recompute`
//! emitting spotting events in set order — would pass both of those tests, and
//! did, until vision ranges grew enough to make it visible.
//!
//! What catches that class of bug is a baseline recorded *earlier*, from a
//! different process, and compared byte for byte. The fog rewrite was verified
//! exactly that way ("bit-identical across four seeds") but by hand, and the
//! evidence was thrown away when the terminal scrolled. This makes it a file.
//!
//! # When this test fails
//!
//! A red result here means one of two things, and they are easy to tell apart
//! by reading the diff:
//!
//! - **You changed balance, content, or the rules on purpose.** The diff will
//!   be broad and will make sense — different damage numbers, different
//!   choices. Regenerate and commit the new baseline as part of the same
//!   change, so the review sees what the change did to the battle:
//!
//!   ```sh
//!   UPDATE_SNAPSHOTS=1 cargo test -p tactics_core --test determinism
//!   ```
//!
//! - **You did not.** Then something is reading an unordered collection, and
//!   the diff will be small, local, and about the *order* of otherwise
//!   identical events. Do not regenerate. Sort the iteration, or walk
//!   `state.units` in id order.
//!
//! The baseline uses `Debug` rather than a formatter written for this test on
//! purpose: a hand-written one can only print the fields someone remembered,
//! and the field it forgets is exactly the field that will regress unnoticed.

use std::path::PathBuf;
use tactics_core::ai::{AiConfig, AiPlanner, make_battle_planner};
use tactics_core::battle::{BattleState, Order};
use tactics_core::data::DataRegistry;

/// Four seeds, matching what the fog rewrite was checked against. More would
/// catch more order-dependence; these already cost a second of suite time and
/// the failure mode is systemic rather than seed-specific — an unordered
/// iteration goes wrong on every seed, not a rare one.
const SEEDS: [u64; 4] = [1, 2, 3, 4];

/// Enough rounds for both sides to close, spot each other and trade fire,
/// without recording a whole battle's worth of text.
const ROUNDS: usize = 12;

#[test]
fn the_event_stream_matches_the_committed_baseline() {
    let registry = load_registry();
    let actual = record_all(&registry);
    let path = snapshot_path();

    if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).expect("snapshot dir");
        std::fs::write(&path, &actual).expect("write snapshot");
        eprintln!("wrote baseline: {}", path.display());
        return;
    }

    let expected = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => panic!(
            "no baseline at {}. Create it with:\n  \
             UPDATE_SNAPSHOTS=1 cargo test -p tactics_core --test determinism",
            path.display()
        ),
    };

    if expected != actual {
        panic!("{}", describe_difference(&expected, &actual));
    }
}

/// The same battle, replayed in one process, must also match — otherwise a
/// baseline comparison could pass for the wrong reason (both runs equally
/// wrong within this process). Cheap, and it localises the failure: if this
/// one fails too, the problem is not the baseline being stale.
#[test]
fn two_runs_in_one_process_agree_with_each_other() {
    let registry = load_registry();
    assert_eq!(
        record_all(&registry),
        record_all(&registry),
        "the same seeds produced different event streams within a single process"
    );
}

fn load_registry() -> DataRegistry {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (registry, report) = DataRegistry::load_dir(&root).expect("mods load");
    assert!(
        report.is_ok(),
        "base mod failed validation: {:?}",
        report.errors
    );
    registry
}

fn snapshot_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/event_stream.txt")
}

fn record_all(registry: &DataRegistry) -> String {
    let mut out = String::new();
    for seed in SEEDS {
        out.push_str(&format!("=== seed {seed} ===\n"));
        out.push_str(&record(registry, seed));
    }
    out
}

/// Play `ROUNDS` rounds of `river_crossing` with both sides on the utility
/// planner and write every event out in order.
fn record(registry: &DataRegistry, seed: u64) -> String {
    let mut state = BattleState::from_map(registry, "river_crossing", seed).expect("battle");
    let mut planners: [Box<dyn AiPlanner<BattleState, Order>>; 2] = [
        planner(registry, seed, "massed_armor"),
        planner(registry, seed + 1, "elastic_defense"),
    ];

    let mut out = String::new();
    for _ in 0..ROUNDS {
        if state.is_over() {
            break;
        }
        // Orders are part of what is being pinned: a planner that starts
        // choosing differently is as much a behaviour change as combat that
        // resolves differently, and both belong in the diff.
        for side in state.living_sides() {
            for _ in 0..64 {
                if state.has_committed(side) || !state.is_planning() {
                    break;
                }
                let order = planners[side as usize].next_order(registry, &state, side);
                out.push_str(&format!("order {order:?}\n"));
                if let Err(e) = state.apply(registry, &order) {
                    out.push_str(&format!("  rejected: {e}\n"));
                    let _ = state.apply(registry, &Order::Commit { side });
                    break;
                }
            }
        }
        for event in state.resolve_round(registry) {
            out.push_str(&format!("{event:?}\n"));
        }
    }

    // Final positions and health, so a divergence that produces the same
    // events but a different board still fails.
    for unit in &state.units {
        out.push_str(&format!(
            "final {} {:?} hp {} alive {}\n",
            unit.name, unit.pos, unit.hp, unit.alive
        ));
    }
    out
}

fn planner(
    registry: &DataRegistry,
    seed: u64,
    doctrine: &str,
) -> Box<dyn AiPlanner<BattleState, Order>> {
    make_battle_planner(
        &AiConfig {
            planner: "utility".into(),
            difficulty: 3,
            doctrine: Some(doctrine.into()),
        },
        seed,
        registry,
    )
}

/// Point at the first divergence rather than dumping both streams, since the
/// whole baseline is tens of thousands of lines and the first difference is
/// nearly always the informative one.
fn describe_difference(expected: &str, actual: &str) -> String {
    let mut message = String::from(
        "the event stream diverged from the committed baseline.\n\
         If this change was intentional, regenerate with:\n  \
         UPDATE_SNAPSHOTS=1 cargo test -p tactics_core --test determinism\n\
         If it was not, something is iterating an unordered collection — see \
         the note on determinism in CLAUDE.md.\n\n",
    );
    let mut expected_lines = expected.lines();
    let mut actual_lines = actual.lines();
    let mut n = 0;
    loop {
        n += 1;
        match (expected_lines.next(), actual_lines.next()) {
            (Some(e), Some(a)) if e == a => continue,
            (e, a) => {
                message.push_str(&format!(
                    "first difference at line {n}:\n  expected: {}\n  actual:   {}\n",
                    e.unwrap_or("<end of baseline>"),
                    a.unwrap_or("<end of run>"),
                ));
                break;
            }
        }
    }
    message.push_str(&format!(
        "\nbaseline {} lines, run {} lines\n",
        expected.lines().count(),
        actual.lines().count()
    ));
    message
}
