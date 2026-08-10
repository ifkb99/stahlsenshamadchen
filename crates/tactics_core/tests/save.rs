//! Saving must preserve the future, not just the present.
//!
//! A save that restores the right positions and hit points but resumes with a
//! fresh rng would play out differently from the run that produced it. Every
//! other guarantee in this engine — replays, search-based AI, the determinism
//! baseline — rests on that not happening, so these tests compare what happens
//! *after* the reload rather than what the file contains.

use std::path::PathBuf;
use tactics_core::ai::{AiConfig, AiPlanner, make_battle_planner};
use tactics_core::battle::{BattleState, Event, Order};
use tactics_core::data::DataRegistry;
use tactics_core::overworld::OverworldState;
use tactics_core::roster::GirlStatus;
use tactics_core::save::{SAVE_VERSION, SaveGame};

fn registry() -> DataRegistry {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (registry, report) = DataRegistry::load_dir(&root).expect("mods load");
    assert!(
        report.is_ok(),
        "base mod must validate: {:?}",
        report.errors
    );
    registry
}

fn planner(reg: &DataRegistry, seed: u64) -> Box<dyn AiPlanner<BattleState, Order>> {
    make_battle_planner(
        &AiConfig {
            planner: "utility".into(),
            difficulty: 3,
            doctrine: Some("massed_armor".into()),
        },
        seed,
        reg,
    )
}

/// Play a few rounds, and describe what happened.
fn play(reg: &DataRegistry, state: &mut BattleState, rounds: usize, seed: u64) -> Vec<String> {
    let mut planners = [planner(reg, seed), planner(reg, seed + 1)];
    let mut log = Vec::new();
    for _ in 0..rounds {
        if state.is_over() {
            break;
        }
        for side in state.living_sides() {
            for _ in 0..64 {
                if state.has_committed(side) || !state.is_planning() {
                    break;
                }
                let order = planners[side as usize].next_order(reg, state, side);
                if state.apply(reg, &order).is_err() {
                    let _ = state.apply(reg, &Order::Commit { side });
                    break;
                }
            }
        }
        for event in state.resolve_round(reg) {
            if !matches!(event, Event::TickStarted { .. }) {
                log.push(format!("{event:?}"));
            }
        }
    }
    log
}

#[test]
fn round_trips_and_keeps_the_future_identical() {
    let reg = registry();

    // Run a battle a few rounds in, then fork it: one copy carries straight
    // on, the other goes through a save file first.
    let mut original = BattleState::from_map(&reg, "river_crossing", 11).expect("battle");
    play(&reg, &mut original, 3, 7);

    let text = SaveGame::new(&reg, None, Some(original.clone()))
        .to_json()
        .expect("serialises");
    let mut restored = SaveGame::from_json(&reg, &text)
        .expect("deserialises")
        .0
        .battle
        .expect("battle round-trips");

    // The board matches...
    assert_eq!(restored.round, original.round);
    assert_eq!(restored.units.len(), original.units.len());
    for (a, b) in restored.units.iter().zip(&original.units) {
        assert_eq!(
            (a.id, a.pos, a.hp, a.alive, a.facing),
            (b.id, b.pos, b.hp, b.alive, b.facing)
        );
    }

    // ...and, the point of the exercise, so does everything that happens next.
    let mut original = original;
    let expected = play(&reg, &mut original, 5, 7);
    let actual = play(&reg, &mut restored, 5, 7);
    assert_eq!(
        actual, expected,
        "a reloaded battle must play out exactly as the unsaved one would"
    );
    assert!(
        !expected.is_empty(),
        "the test needs the battle to still be live"
    );
}

/// The sight grid is left out of the file on purpose, so loading has to put it
/// back. An empty one answers every line-of-sight question wrongly rather than
/// failing, which is the sort of bug that would surface as "the AI has gone
/// blind" three systems away.
#[test]
fn loading_rebuilds_the_sight_grid_that_the_save_leaves_out() {
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 3).expect("battle");
    assert!(!state.sight.is_empty(), "a fresh battle has its sight grid");

    let text = SaveGame::new(&reg, None, Some(state.clone()))
        .to_json()
        .unwrap();
    // The sight grid serialises to an empty object because its only field is
    // skipped. (Checking for "tiles" would not work: `HexMap` has a field by
    // that name which genuinely is saved.)
    assert!(
        text.contains("\"sight\": {}"),
        "the sight grid should be empty in the file, not carried"
    );

    let restored = SaveGame::from_json(&reg, &text).unwrap().0.battle.unwrap();
    assert!(!restored.sight.is_empty(), "loading must rebuild it");

    // And it must be the *same* grid, not merely a non-empty one. Sampled
    // over a spread of pairs rather than every pair, which would be a million
    // line-of-sight tests on a 1261-tile map.
    let mut hexes: Vec<_> = restored.map.iter().map(|(h, _)| h).collect();
    hexes.sort_by_key(|h| (h.x, h.y));
    let mut checked = 0;
    for from in hexes.iter().step_by(37) {
        for to in hexes.iter().step_by(53) {
            assert_eq!(
                restored.sight.clear(*from, *to),
                state.sight.clear(*from, *to),
                "sight differs at {from:?} -> {to:?}"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 500,
        "sampled too little to mean anything: {checked}"
    );
}

/// The campaign is the half a player would actually mind losing: a girl with
/// nine battles behind her and a wound that has three days left on it.
#[test]
fn a_campaign_keeps_its_girls_and_their_scars() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 5).expect("overworld");
    let girl = state.side_armies(0).next().unwrap().units[0].crew[0];
    state.roster.get_mut(girl).unwrap().battles = 9;
    state.roster.get_mut(girl).unwrap().xp = 250;
    state.roster.get_mut(girl).unwrap().status = GirlStatus::Wounded { days: 3 };

    let text = SaveGame::new(&reg, Some(state.clone()), None)
        .to_json()
        .unwrap();
    let mut restored = SaveGame::from_json(&reg, &text)
        .unwrap()
        .0
        .overworld
        .unwrap();

    let back = restored
        .roster
        .get(girl)
        .expect("she is still on the roster");
    assert_eq!(back.battles, 9);
    assert_eq!(back.xp, 250);
    assert_eq!(back.status, GirlStatus::Wounded { days: 3 });
    assert_eq!(back.owner, 0, "and still belongs to her academy");

    // Armies still point at her, rather than at a dangling handle.
    assert_eq!(
        restored.side_armies(0).next().unwrap().units[0].crew[0],
        girl
    );

    // The recovery clock carries on from where it was, not from the start.
    restored.roster.advance_day();
    assert_eq!(
        restored.roster.get(girl).unwrap().status,
        GirlStatus::Wounded { days: 2 }
    );
}

#[test]
fn a_save_from_another_version_is_refused_rather_than_misread() {
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 1).expect("overworld");
    let text = SaveGame::new(&reg, Some(state), None)
        .to_json()
        .unwrap()
        .replace(
            &format!("\"version\": {SAVE_VERSION}"),
            &format!("\"version\": {}", SAVE_VERSION + 1),
        );
    assert!(
        SaveGame::from_json(&reg, &text).is_err(),
        "a future save must be refused, not silently half-read"
    );
}

/// Difficulty is a mod in this project — whether crews bail out, whether a
/// girl can refuse an order, whether death is permanent. So a save has to
/// remember which rules it was played under, or a campaign started gentle
/// could come back lethal without anyone being told.
#[test]
fn a_save_remembers_which_mods_were_playing() {
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 4).expect("overworld");
    let text = SaveGame::new(&reg, Some(state), None).to_json().unwrap();

    let (save, warnings) = SaveGame::from_json(&reg, &text).expect("its own mods load fine");
    assert!(warnings.is_empty(), "nothing has changed: {warnings:?}");
    assert!(
        save.mods.iter().any(|m| m.id == "base"),
        "the base mod should be stamped: {:?}",
        save.mods
    );

    // A save from a game that also had a difficulty mod loaded is refused,
    // because the rules it was played under are not the rules now.
    let harsher = text.replace(
        "\"mods\": [",
        "\"mods\": [\n    { \"id\": \"ironman\", \"version\": \"1.0.0\" },",
    );
    let err = SaveGame::from_json(&reg, &harsher).expect_err("should refuse");
    assert!(
        format!("{err}").contains("ironman"),
        "the error should name what is missing: {err}"
    );
}

/// A content patch must not break saves, or every balance tweak costs the
/// player their campaign.
#[test]
fn a_version_bump_warns_rather_than_refusing() {
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 6).expect("overworld");
    let text = SaveGame::new(&reg, Some(state), None)
        .to_json()
        .unwrap()
        .replace("\"version\": \"0.1.0\"", "\"version\": \"0.0.9\"");

    let (save, warnings) = SaveGame::from_json(&reg, &text).expect("still loads");
    assert!(save.overworld.is_some());
    assert_eq!(warnings.len(), 1, "should say so: {warnings:?}");
    assert!(warnings[0].contains("base"), "{warnings:?}");
}

/// Saves written before mods were stamped cannot be checked, and refusing them
/// would be worse than trusting them.
#[test]
fn a_save_from_before_this_existed_is_still_accepted() {
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 8).expect("overworld");
    let text = SaveGame::new(&reg, Some(state), None).to_json().unwrap();
    let stripped: serde_json::Value = {
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v.as_object_mut().unwrap().remove("mods");
        v
    };
    let (save, warnings) =
        SaveGame::from_json(&reg, &stripped.to_string()).expect("older saves still load");
    assert!(save.overworld.is_some());
    assert!(warnings.is_empty());
}
