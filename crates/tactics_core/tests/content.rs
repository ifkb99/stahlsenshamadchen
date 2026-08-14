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
use tactics_core::battle::{BattleState, Event, Order, SideState};
use tactics_core::data::{DataRegistry, MovementClass};
use tactics_core::map::{HexMap, MapKind, UnitPlacement};

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

// --------------------------------------------------------------------------
// Infantry.
//
// The data layer only: five chassis, their kit and their lift exist as
// content, and the engine has not heard of any of it. Concealment scales no
// spotter's range, the troops module weighs nothing in a casualty roll that
// does not exist yet, capacity carries nobody, and the proof that all of that
// is true is that `tests/snapshots/event_stream.txt` did not move by a byte.
//
// These tests are therefore about the *content* being well-formed and about
// the base mod not yet fielding it. The chunks that make infantry mechanical
// are expected to grow tests here rather than replace these, with one
// exception noted on `no_map_in_the_base_mod_fields_infantry_yet`.
// --------------------------------------------------------------------------

/// Everything the infantry arc added to the roster, named rather than derived
/// so that deleting one of them is a test failure instead of a quietly
/// smaller check.
const INFANTRY_CHASSIS: [&str; 5] = ["rifle_platoon", "scout_section", "halftrack", "apc", "ifv"];

#[test]
fn the_infantry_chassis_load_and_validate() {
    // `registry()` already refuses a base mod that does not validate cleanly,
    // so reaching the body of this test is half the assertion. The rest is
    // that the five ids resolve and that each one is the *kind* of thing the
    // design says it is: the two foot chassis are unarmoured and concealed,
    // and the three carriers are armoured and have lift.
    let reg = registry();
    for id in INFANTRY_CHASSIS {
        assert!(
            reg.vehicle(id).is_some(),
            "the base mod is meant to ship `{id}`"
        );
    }

    for id in ["rifle_platoon", "scout_section"] {
        let v = reg.vehicle(id).expect("chassis");
        assert_eq!(
            v.movement.class,
            MovementClass::Foot,
            "`{id}` walks to the battle"
        );
        assert_eq!(
            (v.armor.front, v.armor.side, v.armor.rear),
            (0, 0, 0),
            "`{id}` is a soft target from every direction; squishiness is armour 0 rather than a rule"
        );
        assert!(
            v.concealment > 0,
            "`{id}` is meant to be hard to see; concealment is her whole defence"
        );
        assert_eq!(v.capacity, 0, "`{id}` walks and carries nobody");
        assert!(
            !v.crew_slots.is_empty(),
            "`{id}` is led by named girls in ordinary crew seats"
        );
        // Every terrain the shipped maps use has to be somewhere a foot unit
        // can stand or refuse to enter on purpose, and the platoon's whole
        // argument is that timber costs her less than it costs a tank.
        let forest = reg.terrain.get("forest").expect("forest");
        assert!(
            forest.cost_for(MovementClass::Foot) <= forest.cost_for(MovementClass::Tracked),
            "forest must not cost infantry more than it costs armour"
        );
    }

    for id in ["halftrack", "apc", "ifv"] {
        let v = reg.vehicle(id).expect("chassis");
        assert_eq!(v.capacity, 1, "`{id}` is a transport and lifts one unit");
        assert_eq!(
            v.concealment, 0,
            "`{id}` is a vehicle; concealment stays the infantry field it was added as"
        );
        assert!(
            v.armor.front > 0,
            "`{id}` stops rifle fire, which is the point of riding"
        );
    }

    // The scout section is the recon instrument: better eyes and better
    // concealment than the rifle platoon, and next to no teeth.
    let scouts = reg.vehicle("scout_section").expect("scouts");
    let platoon = reg.vehicle("rifle_platoon").expect("platoon");
    assert!(scouts.vision_range > platoon.vision_range);
    assert!(scouts.concealment > platoon.concealment);
    assert!(scouts.weapons.len() < platoon.weapons.len());
}

#[test]
fn a_platoon_spawns_with_her_troops_and_her_leaders() {
    // A platoon is one piece led by named girls: two leadership seats in the
    // ordinary crew model, and the rest of the platoon abstracted into a
    // module that spawns at full strength. Both halves of that have to be
    // true at spawn time before anything can wear either of them down.
    let reg = registry();
    let state = infantry_field(&reg);
    let platoon = &state.units[0];

    assert_eq!(
        platoon.crew.len(),
        2,
        "a placement naming no girls gets one anonymous girl per declared seat"
    );
    let roles: Vec<String> = platoon
        .crew
        .iter()
        .filter_map(|id| state.roster.get(*id))
        .map(|g| g.name.clone())
        .collect();
    assert_eq!(
        roles,
        vec!["Platoon Leader".to_string(), "Section Leader".to_string()],
        "the anonymous crew is named for the seats the chassis declares"
    );

    let sections = reg.module("rifle_sections").expect("rifle_sections");
    assert_eq!(
        platoon.modules.get("rifle_sections").copied(),
        Some(sections.toughness),
        "the platoon drives out at full strength; the map counts hits remaining"
    );
    assert!(
        platoon.modules.contains_key("field_radio"),
        "her set is a module, so it is a thing that can one day be shot off"
    );
    assert_eq!(
        platoon.ammo.get("rpg_heat").copied(),
        Some(6),
        "the ambush tube carries a handful of rockets and no more"
    );
}

#[test]
fn no_map_in_the_base_mod_fields_infantry_yet() {
    // This is the inertness assertion for this chunk, in the B0/B2a tradition
    // — and unlike theirs it is a statement about content rather than about
    // the engine, because the byte-identical event-stream snapshot is already
    // the proof that nothing *mechanical* changed. Nothing can shift while no
    // battle contains a foot unit, which is what makes the new foot movement
    // costs and the new chassis safe to land before the mechanics exist.
    //
    // **N3 is the chunk that deletes this test**, when infantry and their
    // transports take their places in all three orders of battle and the
    // campaign's armies learn the new vehicles exist. Until then a placement
    // naming one of these ids would be a unit whose concealment, troops and
    // lift all silently do nothing.
    let reg = registry();
    let mut ids: Vec<&str> = reg.maps.values().map(|m| m.id.as_str()).collect();
    ids.sort_unstable();
    for id in ids {
        let map = reg.map(id).expect("map");
        let placed = map
            .units
            .iter()
            .chain(map.armies.iter().flat_map(|a| a.units.iter()));
        for placement in placed {
            assert!(
                !INFANTRY_CHASSIS.contains(&placement.vehicle.as_str()),
                "map `{id}` fields `{}`, but the engine does not read concealment, \
                 troops or capacity yet; N3 is where infantry arrive",
                placement.vehicle
            );
        }
    }
}

/// One rifle platoon and one carrier on a scrap of grass, spawned through the
/// overworld's own path so that whatever a field battle does to a placement is
/// what these tests observe.
fn infantry_field(reg: &DataRegistry) -> BattleState {
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "infantry_field",
        "palette": { "g": "grass" },
        "rows": ["ggggg", "ggggg"],
    }))
    .expect("map file");
    let map = HexMap::from_map_file(&file).expect("map");
    let placements = vec![
        UnitPlacement {
            aboard_at: None,
            at: [0, 0],
            side: 0,
            vehicle: "rifle_platoon".into(),
            crew: Vec::new(),
            name: Some("Platoon".into()),
            facing: None,
            formation: None,
            leads: false,
        },
        UnitPlacement {
            aboard_at: None,
            at: [4, 1],
            side: 1,
            vehicle: "apc".into(),
            crew: Vec::new(),
            name: Some("Taxi".into()),
            facing: None,
            formation: None,
            leads: false,
        },
    ];
    let sides = vec![
        SideState {
            name: "West".into(),
            ai: None,
        },
        SideState {
            name: "East".into(),
            ai: None,
        },
    ];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        1,
    )
}
