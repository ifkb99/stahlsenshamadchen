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

mod common;
use common::registry;
use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, make_battle_planner};
use tactics_core::battle::{BattleState, Order, UnitId};
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
    let registry = registry();
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
    let registry = registry();
    assert_eq!(
        record_all(&registry),
        record_all(&registry),
        "the same seeds produced different event streams within a single process"
    );
}

/// An id is a name, not a position (WORLD.md, W0.5). A vehicle that marches
/// out of one engagement and into another keeps the name the world gave her,
/// so a battle must not be able to tell a force numbered from zero from the
/// same force numbered by anybody else — as long as the names rise in the
/// same order, which is the one promise `Muster` asks for.
///
/// So the baseline is fought again with every unit renamed, the names mapped
/// back, and the transcript required to be the same text. Anything still
/// using an id as a position — a list indexed by it, a loop that counts to
/// it — reads the wrong unit or none, and the transcript diverges.
#[test]
fn a_battle_fought_under_other_names_is_the_same_battle() {
    let registry = registry();
    let count = registry
        .map("river_crossing")
        .expect("the baseline map")
        .units
        .len();
    let names: Vec<UnitId> = (0..count as u32).map(|i| UnitId(1000 + 7 * i)).collect();
    for seed in SEEDS {
        let plain = record(&registry, seed, None);
        let renamed = record(&registry, seed, Some(&names));
        assert_ne!(plain, renamed, "the renaming must show in the transcript");
        let back = rename_back(&renamed, &names);
        if plain != back {
            panic!("seed {seed}: {}", describe_difference(&plain, &back));
        }
    }
}

/// Every `UnitId(n)` in `text` whose `n` is one of `names`, written as the
/// position it was renamed from.
fn rename_back(text: &str, names: &[UnitId]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("UnitId(") {
        let (head, tail) = rest.split_at(at + "UnitId(".len());
        out.push_str(head);
        let digits: String = tail.chars().take_while(|c| c.is_ascii_digit()).collect();
        let n: u32 = digits.parse().expect("a UnitId prints a number");
        match names.iter().position(|id| id.0 == n) {
            Some(i) => out.push_str(&i.to_string()),
            None => out.push_str(&digits),
        }
        rest = &tail[digits.len()..];
    }
    out.push_str(rest);
    out
}

fn snapshot_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots/event_stream.txt")
}

fn record_all(registry: &DataRegistry) -> String {
    let mut out = String::new();
    for seed in SEEDS {
        out.push_str(&format!("=== seed {seed} ===\n"));
        out.push_str(&record(registry, seed, None));
    }
    out
}

/// Play `ROUNDS` rounds of `river_crossing` with both sides on the utility
/// planner and write every event out in order.
fn record(registry: &DataRegistry, seed: u64, names: Option<&[UnitId]>) -> String {
    let mut state =
        BattleState::from_map_numbered(registry, "river_crossing", seed, names).expect("battle");
    let mut ai = AiDriver::new();
    ai.insert(0, planner(registry, seed, "massed_armor"));
    ai.insert(1, planner(registry, seed + 1, "elastic_defense"));

    let mut out = String::new();
    for _ in 0..ROUNDS {
        if state.is_over() {
            break;
        }
        // Orders are part of what is being pinned: a planner that starts
        // choosing differently is as much a behaviour change as combat that
        // resolves differently, and both belong in the diff.
        ai.plan_round_with(registry, &mut state, |d| {
            out.push_str(&format!("order {:?}\n", d.order));
            if let Some(e) = &d.rejected {
                out.push_str(&format!("  rejected: {e}\n"));
            }
        });
        for event in state.resolve_round(registry) {
            out.push_str(&format!("{event:?}\n"));
        }
    }

    // Final positions and health, so a divergence that produces the same
    // events but a different board still fails.
    for unit in &state.units {
        out.push_str(&format!(
            "final {} {:?} crew {:?} modules {:?} alive {}\n",
            unit.name,
            unit.pos,
            unit.crew_state,
            unit.modules,
            unit.alive()
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
