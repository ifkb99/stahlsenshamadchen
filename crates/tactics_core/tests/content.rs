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

use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, make_battle_planner};
use tactics_core::battle::{BattleState, Event, Order, SideState};
use tactics_core::data::{DataRegistry, MovementClass};
use tactics_core::map::{HexMap, MapKind, UnitPlacement};

/// The battlefields added since `river_crossing`: the first two to stop the
/// balance harness overfitting to one river, the second two so the campaign's
/// mountains, cities and factories are fought on their own ground rather than
/// on whichever map iterated first. Named here rather than derived so that
/// deleting one is a test failure rather than a quietly smaller sample.
///
/// They share an order of battle on purpose — nine placements a side, the
/// same nine both ways round — because it is the `ground` table's control:
/// with the armies exchanged between the ends, a map whose two sides field
/// identical forces has to read level in the force column whatever the ground
/// does, so a build where it drifts has a bug in the exchange rather than a
/// finding about the content.
const NEW_MAPS: [&str; 4] = [
    "battle_plains",
    "battle_forest",
    "battle_hills",
    "battle_town",
];

mod common;
use common::registry;

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
fn the_new_maps_field_enough_formations_a_side_with_a_leader_each() {
    // Three, and specifically these three, because the commander brain reads
    // what a formation is *made of* and can only do that if the map wrote
    // down an order of battle with coherent elements in it: an armoured
    // line, a gun section she recognises as her base of fire, and a
    // grenadier section she recognises as infantry. Mixing the taxi and her
    // platoon into the tank formation — which is how these maps first
    // shipped — gave the brain one order to cover three different jobs, and
    // the job it picked was the tanks'.
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
            assert!(
                formations.len() >= 3,
                "map `{id}` side {side} must field at least three formations for the \
                 command planner to have distinguishable elements to work with, not \
                 {formations:?}"
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
                    "map `{id}`: formation `{formation}` needs exactly one cadet in charge"
                );
            }
            // Six armoured vehicles, the taxi, her platoon, a scout section
            // and a reserve of two — a medium and a light, a second group
            // to manoeuvre with, so a commander who knows a play has the
            // elements to run it: eleven placements, of which ten stand on
            // the ground at the bell because the platoon starts in the back
            // of the taxi. `the_new_maps_field_infantry_and_their_rides`
            // checks that second number, which is the one an opponent sees.
            assert_eq!(
                file.units.iter().filter(|u| u.side == side).count(),
                11,
                "map `{id}` side {side} is meant to field eleven units: six vehicles, \
                 a platoon, her ride, a scout section and a reserve of two"
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
            "`{id}` is led by named cadets in ordinary crew seats"
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
    // A platoon is one piece led by named cadets: two leadership seats in the
    // ordinary crew model, and the rest of the platoon abstracted into a
    // module that spawns at full strength. Both halves of that have to be
    // true at spawn time before anything can wear either of them down.
    let reg = registry();
    let state = infantry_field(&reg);
    let platoon = &state.units[0];

    assert_eq!(
        platoon.crew.len(),
        2,
        "a placement naming no cadets gets one anonymous cadet per declared seat"
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
fn a_platoon_lays_her_own_weapons_as_well_as_a_gunner_lays_hers() {
    // The leadership seats were written with the skills a leader obviously has
    // — command, fieldcraft, small arms — and gunnery was not among them,
    // which is true of a rifle section and wrong for the platoon as a piece:
    // *somebody* aims the RPG and the platoon MG, and in this model that is
    // the cadets, because they are the only crew the chassis has.
    //
    // With no seat responsible for gunnery, `crew_skill` fell through to "best
    // aboard" and every cadet aboard read gunnery through her cores at the
    // untrained penalty, so a platoon shot at four levels below ordinary — a
    // silent, permanent −12 percentage points of hit chance on every infantry
    // weapon in the game. That is a data omission with an engine-sized effect,
    // and it is what this test exists to keep from creeping back.
    let reg = registry();
    let state = infantry_field(&reg);
    let platoon = &state.units[0];
    let chassis = reg.vehicle(&platoon.vehicle).expect("chassis");
    let gunnery = state.roster.crew_skill(
        &reg,
        Some(chassis),
        &platoon.crew,
        &platoon.crew_state,
        "gunnery",
        None,
    );
    assert_eq!(
        gunnery,
        tactics_core::data::AVERAGE,
        "an anonymous platoon aims at the ordinary standard, exactly as an \
         anonymous tank crew does; anything less is the untrained penalty \
         leaking back in through a role that does not claim the skill"
    );
    for role in &chassis.crew_slots {
        assert!(
            reg.role(role)
                .is_some_and(|r| r.skills.iter().any(|s| s == "gunnery")),
            "{role} must claim gunnery, or no seat is responsible for it and \
             the fallback path is what answers again"
        );
    }
}

#[test]
fn the_new_maps_field_infantry_and_their_rides() {
    // The replacement for `no_map_in_the_base_mod_fields_infantry_yet`, which
    // was the inertness assertion the data chunk shipped and which N3 exists
    // to delete. What it guarded is now the opposite claim: the two newer
    // battlefields put both foot chassis on the ground, and the rifle platoon
    // arrives *aboard* her taxi rather than walking — which is the whole
    // point of a motorised element and the only thing on any shipped map that
    // exercises `aboard_at`.
    //
    // `river_crossing` is deliberately not in `NEW_MAPS` and deliberately
    // untouched: it is the ground the determinism baseline is recorded on, so
    // fielding infantry there would mean regenerating the snapshot and losing
    // the one check that says this chunk changed no rules.
    let reg = registry();
    for id in NEW_MAPS {
        let state = BattleState::from_map(&reg, id, 3).expect("battle map loads");
        for chassis in ["rifle_platoon", "scout_section"] {
            for side in 0..2u8 {
                assert!(
                    state
                        .units
                        .iter()
                        .any(|u| u.side == side && u.vehicle == chassis),
                    "map `{id}` side {side} is meant to field `{chassis}`"
                );
            }
        }
        // Aboard, not beside: a passenger's position mirrors her carrier's,
        // so the check is `aboard` rather than a coordinate — and the carrier
        // it names has to be something with lift, or the placement quietly
        // degraded to on-foot and nobody would have noticed.
        let riders: Vec<_> = state
            .units
            .iter()
            .filter(|u| u.vehicle == "rifle_platoon")
            .collect();
        assert_eq!(riders.len(), 2, "map `{id}`: one platoon a side");
        for side in 0..2u8 {
            assert_eq!(
                state
                    .units
                    .iter()
                    .filter(|u| u.side == side && u.aboard.is_none())
                    .count(),
                10,
                "map `{id}` side {side} puts ten units on the ground at the bell; \
                 the eleventh is in the back of the taxi"
            );
        }
        for rider in riders {
            let carrier = rider
                .aboard
                .and_then(|c| state.unit(c))
                .unwrap_or_else(|| panic!("map `{id}`: {} spawned on foot", rider.name));
            assert_eq!(carrier.side, rider.side, "map `{id}`: she rides her own");
            assert!(
                reg.vehicle(&carrier.vehicle)
                    .is_some_and(|v| v.capacity > 0),
                "map `{id}`: {} is aboard `{}`, which lifts nobody",
                rider.name,
                carrier.vehicle
            );
        }
    }

    // The campaign's armies field the new kit too, so a field battle fought
    // out of the overworld is not an all-armour affair the battle maps alone
    // would suggest.
    let frontier = reg.map("frontier").expect("frontier");
    for side in 0..2u8 {
        let motorised = frontier
            .armies
            .iter()
            .filter(|a| a.side == side)
            .filter(|a| a.units.iter().any(|u| u.vehicle == "rifle_platoon"))
            .count();
        assert!(
            motorised > 0,
            "the campaign's side {side} has no infantry to put in a field battle"
        );
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
    .expect("the staged placements are content the base mod ships")
}

// -- The campaign's way into a battle refuses what a scenario's does --------
//
// `from_map` reads a file whose author can be shown warnings; the campaign's
// `from_placements` assembles its order of battle at run time out of armies,
// and used to trust it completely. A mod that dropped a vehicle between one
// save and the next reached `spawn_unit` and panicked there. These pin that it
// is now the same refusal a bad scenario map gets, and that the content
// actually shipped passes it.

/// A scrap of grass, so a test can say what is wrong with a placement without
/// also being a test of terrain.
fn scrap_of_grass() -> HexMap {
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "scrap_of_grass",
        "palette": { "g": "grass" },
        "rows": ["ggggg", "ggggg"],
    }))
    .expect("map file");
    HexMap::from_map_file(&file).expect("map")
}

fn two_sides() -> Vec<SideState> {
    vec![
        SideState {
            name: "West".into(),
            ai: None,
        },
        SideState {
            name: "East".into(),
            ai: None,
        },
    ]
}

fn standing_at(at: [i32; 2], side: u8, vehicle: &str) -> UnitPlacement {
    UnitPlacement {
        aboard_at: None,
        at,
        side,
        vehicle: vehicle.into(),
        crew: Vec::new(),
        name: None,
        facing: None,
        formation: None,
        leads: false,
    }
}

#[test]
fn a_placement_naming_a_vehicle_no_mod_ships_is_refused_rather_than_fatal() {
    let reg = registry();
    let placements = vec![
        standing_at([0, 0], 0, "medium_tank"),
        standing_at([4, 1], 1, "chariot"),
    ];
    let crews = vec![Vec::new(), Vec::new()];
    let err = BattleState::from_placements(
        &reg,
        scrap_of_grass(),
        two_sides(),
        &placements,
        &crews,
        std::sync::Arc::new(tactics_core::roster::Roster::new()),
        1,
    )
    .expect_err("a chassis nothing declares cannot be put on the field");
    let said = err.to_string();
    assert!(
        said.contains("chariot") && said.contains("missing vehicle"),
        "the error should name what is missing, got: {said}"
    );
    // And only the one that is wrong: a campaign whose mod lost one chassis
    // should not be told its whole order of battle is broken.
    assert!(
        !said.contains("medium_tank"),
        "the tank the mod does ship was blamed too: {said}"
    );
}

#[test]
fn a_placement_crewed_by_a_cadet_the_roster_never_heard_of_is_refused() {
    let reg = registry();
    let placements = vec![standing_at([0, 0], 0, "medium_tank")];
    // A campaign roster with one cadet on it, and a placement asking for a
    // second who is not. That is the shape a save takes when the roster it was
    // written against has moved on: her seat would be empty, substance counts
    // people aboard, and the tank would go out about twice as easy to kill for
    // a reason nobody could see.
    let mut roster = tactics_core::roster::Roster::new();
    let commander = roster.enlist(
        0,
        &tactics_core::data::CharacterDef {
            id: "commander".into(),
            name: "Commander".into(),
            ..Default::default()
        },
        &reg,
    );
    let ghost = tactics_core::roster::CadetId(commander.0 + 7);
    let crews = vec![vec![commander, ghost]];
    let err = BattleState::from_placements(
        &reg,
        scrap_of_grass(),
        two_sides(),
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        1,
    )
    .expect_err("a cadet who is not on the roll cannot climb in");
    let said = err.to_string();
    assert!(
        said.contains("not on this roster"),
        "the error should say who is missing, got: {said}"
    );
}

#[test]
fn every_army_the_campaign_ships_can_be_put_on_a_battlefield() {
    // The refusal above is only worth having if the content actually passes
    // it. Every vehicle every army on the campaign map fields is spawned
    // through the campaign's own constructor, with the campaign's own roster,
    // one army at a time — so a chassis a mod renamed or a cadet a map forgot
    // shows up here rather than the first time a player is attacked.
    let reg = registry();
    let campaign = tactics_core::overworld::OverworldState::from_map(&reg, "frontier", 1)
        .expect("the campaign map builds");
    let map = scrap_of_grass();
    assert!(
        !campaign.armies.is_empty(),
        "this test is meaningless without armies"
    );
    for army in &campaign.armies {
        let placements: Vec<UnitPlacement> = army
            .units
            .iter()
            .enumerate()
            .map(|(i, u)| standing_at([i as i32 % 5, (i as i32 / 5) % 2], 0, &u.vehicle))
            .collect();
        let crews: Vec<Vec<tactics_core::roster::CadetId>> =
            army.units.iter().map(|u| u.crew.clone()).collect();
        BattleState::from_placements(
            &reg,
            map.clone(),
            two_sides(),
            &placements,
            &crews,
            std::sync::Arc::new(campaign.roster.clone()),
            1,
        )
        .unwrap_or_else(|e| panic!("army `{}` cannot take the field: {e}", army.name));
    }
}
