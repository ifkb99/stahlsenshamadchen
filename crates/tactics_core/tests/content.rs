//! What the shipped content has to be, as opposed to what the engine does.
//!
//! These are tests about `assets/mods` rather than about Rust: that every
//! battlefield really is one overworld tile, that the chain of command each
//! one writes down joins up, and — the only one that costs anything to run —
//! that the ground is *fightable*. A map can pass every structural check and
//! still be a scenario in which two armies never find each other, which is a
//! content bug the validator cannot see and only a battle can.
//!
//! They live apart from `engine.rs` because the thing under test is the data.
//! A failure here means somebody edited a map, not the simulation.

use std::path::PathBuf;
use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, make_battle_planner};
use tactics_core::battle::{BattleState, Event, Order};
use tactics_core::data::DataRegistry;
use tactics_core::map::MapKind;

/// The two battlefields added to stop the balance harness overfitting to
/// `river_crossing`. Named here rather than derived so that deleting one is a
/// test failure rather than a quietly smaller sample.
const NEW_MAPS: [&str; 2] = ["battle_plains", "battle_forest"];

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

fn planner(
    reg: &DataRegistry,
    seed: u64,
    doctrine: &str,
) -> Box<dyn AiPlanner<BattleState, Order>> {
    make_battle_planner(
        &AiConfig {
            planner: "utility".into(),
            difficulty: 4,
            doctrine: Some(doctrine.into()),
        },
        seed,
        reg,
    )
}

#[test]
fn every_battle_map_in_the_base_mod_is_the_regulation_hexagon() {
    let reg = registry();
    let expected = reg.scale.battle_map_tiles() as usize;
    let mut seen = 0;
    let mut ids: Vec<&str> = reg
        .maps
        .values()
        .filter(|m| m.kind == MapKind::Battle)
        .map(|m| m.id.as_str())
        .collect();
    ids.sort_unstable();
    for id in ids {
        let state = BattleState::from_map(&reg, id, 1).expect("battle map loads");
        assert_eq!(
            state.map.len(),
            expected,
            "battle map `{id}` is not one overworld tile"
        );
        seen += 1;
    }
    assert!(
        seen >= 3,
        "the base mod is meant to ship more than one battlefield; found {seen}"
    );
}

#[test]
fn the_new_maps_field_two_formations_a_side_with_a_leader_each() {
    let reg = registry();
    for id in NEW_MAPS {
        let file = reg.maps.get(id).unwrap_or_else(|| panic!("map `{id}`"));
        for side in 0..2u8 {
            let formations: Vec<&str> = file
                .formations
                .iter()
                .filter(|f| f.side == side)
                .map(|f| f.id.as_str())
                .collect();
            assert_eq!(
                formations.len(),
                2,
                "map `{id}` side {side} must field two formations for the command \
                 planner to have any structure to work with, not {formations:?}"
            );
            for formation in &formations {
                let members: Vec<_> = file
                    .units
                    .iter()
                    .filter(|u| u.formation.as_deref() == Some(*formation))
                    .collect();
                assert!(
                    !members.is_empty(),
                    "map `{id}`: formation `{formation}` is empty"
                );
                assert_eq!(
                    members.iter().filter(|u| u.leads).count(),
                    1,
                    "map `{id}`: formation `{formation}` needs exactly one girl in charge"
                );
            }
            assert_eq!(
                file.units.iter().filter(|u| u.side == side).count(),
                6,
                "map `{id}` side {side} is meant to field six vehicles"
            );
        }
    }
}

#[test]
fn a_battle_on_each_new_map_runs_to_a_verdict() {
    let reg = registry();
    for id in NEW_MAPS {
        let seed = 7;
        let mut state = BattleState::from_map(&reg, id, seed).expect("battle");
        let mut ai = AiDriver::new();
        ai.insert(0, planner(&reg, seed, "massed_armor"));
        ai.insert(1, planner(&reg, seed + 1, "elastic_defense"));
        let mut rounds = 0;
        let mut shots = 0;
        while !state.is_over() && rounds < 60 {
            ai.plan_round(&reg, &mut state);
            for event in state.resolve_round(&reg) {
                if matches!(event, Event::ShotFired { .. }) {
                    shots += 1;
                }
            }
            rounds += 1;
        }
        // The claim is not that somebody wins — a level score is a legitimate
        // afternoon — but that the map puts the two forces in contact at all.
        // A battlefield whose sightlines or objectives leave both armies
        // wandering is exactly the failure this guards, and it is invisible to
        // every structural check the validator makes.
        assert!(
            shots > 0,
            "nobody fired a shot in {rounds} rounds on `{id}`: the armies never met"
        );
    }
}
