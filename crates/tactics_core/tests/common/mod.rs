//! The stage-setup contract, in one place.
//!
//! Every test binary in this directory fights the *real* `assets/mods`
//! content, so every one of them needs a loaded registry, and several need
//! that registry with one rule switched off so the test is about the thing it
//! says it is about rather than about whether anybody happened to find
//! anybody. Those helpers used to be copied into each binary — `registry` was
//! byte-identical in six of them and appeared in a seventh under another name
//! — and the two that matter most, [`registry_wireless`] and [`seen`], existed
//! in one file each.
//!
//! That is the real cost of the duplication and the reason this module exists:
//! CLAUDE.md calls those two **mandatory** for a staged test that needs a crew
//! in plain sight or a game without command rules, and a new test binary
//! started life without them. The contract was enforced by whether its author
//! had read the right paragraph. Now it is something to import.
//!
//! `tests/common/mod.rs` rather than `tests/common.rs` on purpose: Cargo
//! builds every top-level file in `tests/` as its own test binary, and a
//! binary with no tests in it is a confusing empty line in the output.
//!
//! Helpers that are genuinely local to one binary — the stage builders in
//! `engine.rs`, `save.rs`'s fork-through-a-file rig — stay where they are. The
//! point is to make the *contract* importable, not to hollow out the files.

// Each test binary compiles this module separately and uses only the part of
// it that binary needs, so anything not wanted by, say, `force.rs` is dead
// code there. That is inherent to how Cargo builds integration tests and not
// a sign anything is unused — `registry_wireless` has 106 callers next door.
#![allow(dead_code)]

use std::path::PathBuf;
use tactics_core::data::DataRegistry;

/// The mod tree the tests fight on: the shipped `assets/mods`, not a fixture.
pub fn mods_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods")
}

/// The base game, loaded and validated.
///
/// The validation assert is not ceremony. A test that fails against a mod
/// tree which did not load cleanly is reporting the wrong thing, and the
/// content tests are the only ones that would otherwise notice.
pub fn registry() -> DataRegistry {
    let (registry, report) = DataRegistry::load_dir(&mods_root()).expect("mods load");
    assert!(
        report.is_ok(),
        "base mod must validate: {:?}",
        report.errors
    );
    registry
}

/// The base game with the radio switched off: command rules stripped, so a
/// test about missions themselves — what they store, how they steer units,
/// what the brain issues — is not also a test about latency and radio radius.
/// The wire has its own tests, and its own zero-coefficient pin
/// (`a_command_block_with_zero_coefficients_is_the_game_without_one`).
pub fn registry_wireless() -> DataRegistry {
    let mut reg = registry();
    reg.command = None;
    reg
}

/// The same game with the search switched off: a crew who can see a hex sees
/// what is standing on it, exactly as she did before detection rolls existed.
///
/// The twin of [`registry_wireless`] and there for the same reason. A stage
/// that puts two crews in plain sight in order to test shells, or a dismount
/// reflex, or a battery in sight of its quarry, or whether a binding order is
/// obeyed, must not *also* be a test of whether anybody happened to find
/// anybody on the tick it was set up — and at the base mod's numbers a target
/// seven hexes off an eight-hex reach is found in about three ticks rather
/// than instantly. Detection has tests of its own; `detection_certain_percent`
/// at 100 is the rule's neutral value, so this is its absence rather than a
/// gentle version of it.
pub fn seen(mut reg: DataRegistry) -> DataRegistry {
    reg.balance.detection_certain_percent = 100;
    reg
}
