//! Modules as content: what lives inside a vehicle, and the rules about what
//! a vehicle is carrying before anything can shoot it out.
//!
//! This is the data layer of the outcome half of the ballistics rewrite, in
//! the same shape ammunition arrived in: definitions load, vehicles name what
//! is aboard, units carry per-module state, and the resolver has not heard of
//! any of it. Everything here is about *what exists*, never about what
//! happens to it.

use std::collections::BTreeMap;
use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, make_battle_planner};
use tactics_core::battle::{BattleState, Order, UnitId};
use tactics_core::data::{ModuleEffect, STANDARD_MODULES, ValidationReport};
use tactics_core::save::SaveGame;

mod common;
use common::registry;

/// The first unit of `vehicle` in a freshly set-up `river_crossing`.
fn a(state: &BattleState, vehicle: &str) -> UnitId {
    state
        .units
        .iter()
        .find(|u| u.vehicle == vehicle)
        .unwrap_or_else(|| panic!("river_crossing fields a {vehicle}"))
        .id
}

#[test]
fn a_unit_spawns_with_her_vehicles_modules_all_intact() {
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 4).expect("battle");

    // Not "some modules": exactly the hardware the chassis is written with,
    // each at full toughness, so editing `tanks.json` is the only way to
    // change what drives out.
    for unit in &state.units {
        let vehicle = reg.vehicle(&unit.vehicle).expect("vehicle exists");
        let expected: BTreeMap<String, u32> = reg
            .modules_for(vehicle)
            .into_iter()
            .map(|m| (m.id.clone(), m.toughness))
            .collect();
        assert_eq!(
            unit.modules, expected,
            "{} drives out with what {} declares",
            unit.name, vehicle.id
        );
        assert!(
            !unit.modules.is_empty(),
            "{} has nothing inside her at all",
            unit.name
        );
    }

    let panther = state.unit(a(&state, "medium_tank")).expect("alive");
    assert_eq!(panther.modules["main_gun"], 1);
    assert_eq!(
        panther.modules["tracks"], 2,
        "running gear takes a damaging hit before a killing one"
    );
    assert_eq!(
        panther.modules["ammo_rack_bulk"], 1,
        "a medium's long magazine is the bulk rack, a bigger target inside"
    );
    assert_eq!(panther.modules["radio_set"], 1);
}

#[test]
fn a_vehicle_that_declares_no_modules_inherits_the_standard_set() {
    // The rule that keeps every mod written before this chunk correct: an
    // empty list means "the usual four", because a chassis nobody has
    // re-equipped still has a gun, tracks, racks and a set. It is a lookup
    // into content, never a definition invented in Rust.
    let mut reg = registry();
    let mut panther = reg.vehicle("medium_tank").expect("medium_tank").clone();
    panther.modules.clear();
    reg.vehicles.insert(panther.id.clone(), panther);

    let state = BattleState::from_map(&reg, "river_crossing", 4).expect("battle");
    let panther = state.unit(a(&state, "medium_tank")).expect("alive");
    let inherited: Vec<&str> = panther.modules.keys().map(String::as_str).collect();
    let mut standard = STANDARD_MODULES;
    standard.sort_unstable();
    assert_eq!(
        inherited,
        standard.to_vec(),
        "she inherits precisely the standard set"
    );
    assert_eq!(
        panther.modules["tracks"], 2,
        "and it is the real definition"
    );
}

#[test]
fn a_mod_that_declares_no_modules_at_all_still_validates() {
    // Additivity read strictly: modules are a thing content may have, not a
    // thing content must have. Strip the definitions and every vehicle's
    // list, and what is left is the game exactly as it was — units with
    // nothing inside to hit but their crew.
    let mut reg = registry();
    reg.modules.clear();
    let ids: Vec<String> = reg.vehicles.keys().cloned().collect();
    for id in ids {
        reg.vehicles.get_mut(&id).expect("present").modules.clear();
    }

    let mut report = ValidationReport::default();
    reg.validate_into(&mut report);
    assert!(report.is_ok(), "must still load: {:?}", report.errors);
    assert!(
        !report.warnings.iter().any(|w| w.contains("module")),
        "and must not nag about it: {:?}",
        report.warnings
    );

    let state = BattleState::from_map(&reg, "river_crossing", 4).expect("battle");
    for unit in &state.units {
        assert!(
            unit.modules.is_empty(),
            "{} cannot carry hardware no mod describes",
            unit.name
        );
    }
}

#[test]
fn a_vehicle_carrying_a_module_no_mod_declares_is_a_validation_error() {
    let mut reg = registry();
    let mut panther = reg.vehicle("medium_tank").expect("medium_tank").clone();
    panther.modules.push("gyrostabiliser".into());
    reg.vehicles.insert(panther.id.clone(), panther);

    let report = reg.validate();
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("medium_tank") && e.contains("gyrostabiliser")),
        "a phantom module must be an error, got {:?}",
        report.errors
    );
}

#[test]
fn two_modules_sharing_an_effect_are_fine_and_the_same_one_twice_is_not() {
    // Effects are behaviours, not slots. A hull machine gun beside a coaxial
    // is two guns and must pass without comment; the same id written twice is
    // a modder who meant that and spelt it wrong, and since per-module state
    // is keyed by id the repeat would silently vanish.
    let mut reg = registry();
    let mut coax = reg.module("main_gun").expect("main_gun").clone();
    coax.id = "hull_mg".into();
    coax.size = 1;
    reg.modules.insert(coax.id.clone(), coax);

    let mut panther = reg.vehicle("medium_tank").expect("medium_tank").clone();
    panther.modules.push("hull_mg".into());
    reg.vehicles.insert(panther.id.clone(), panther);

    let report = reg.validate();
    assert!(
        report.is_ok(),
        "two guns is legal content: {:?}",
        report.errors
    );
    assert!(
        !report.warnings.iter().any(|w| w.contains("hull_mg")),
        "and unremarkable: {:?}",
        report.warnings
    );

    let mut panther = reg.vehicle("medium_tank").expect("medium_tank").clone();
    panther.modules.push("main_gun".into());
    reg.vehicles.insert(panther.id.clone(), panther);

    let report = reg.validate();
    assert!(report.is_ok(), "still not fatal: {:?}", report.errors);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("medium_tank") && w.contains("main_gun") && w.contains("twice")),
        "the repeat must be called out, got {:?}",
        report.warnings
    );
}

#[test]
fn a_wireless_module_on_a_vehicle_that_mounts_no_radio_is_only_a_warning() {
    // A crew cannot lose a set they never had. Not an error, because the
    // reverse pairing — hardware with no module — is a perfectly good way to
    // write a set that cannot be shot off, and neither direction should stop
    // a mod loading.
    let mut reg = registry();
    let mut luchs = reg.vehicle("recon_car").expect("recon_car").clone();
    assert!(
        luchs.modules.iter().any(|m| m == "radio_set"),
        "the base mod fits her with one, which is what makes removing the \
         hardware the interesting half"
    );
    luchs.radio = None;
    reg.vehicles.insert(luchs.id.clone(), luchs);

    let report = reg.validate();
    assert!(report.is_ok(), "not fatal: {:?}", report.errors);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("recon_car") && w.contains("radio_set")),
        "got {:?}",
        report.warnings
    );
}

#[test]
fn every_vehicle_in_the_base_mod_says_what_is_aboard_her() {
    // The content half of the contract. Validation cannot demand this — an
    // empty list is legal and inherits the standard set — so the base mod's
    // own completeness is a test rather than a rule. Written out, a vehicle
    // that acquires a fifth module later does not have to be discovered.
    let reg = registry();
    for vehicle in reg.vehicles.values() {
        assert!(
            !vehicle.modules.is_empty(),
            "vehicle `{}` leaves what is inside her to the default",
            vehicle.id
        );
        for id in &vehicle.modules {
            assert!(
                reg.module(id).is_some(),
                "vehicle `{}` names module `{}`, which nothing declares",
                vehicle.id,
                id
            );
        }
    }
    for id in STANDARD_MODULES {
        assert!(
            reg.module(id).is_some(),
            "the base mod must declare the standard module `{id}`, or the \
             fallback for a chassis that names nothing quietly shrinks"
        );
    }
}

#[test]
fn every_module_effect_prints_the_name_a_mod_writes() {
    // `ModuleEffect::as_str` and serde's `rename_all` are two spellings of
    // the same contract, and a roster table that disagreed with the JSON
    // would be worse than no table.
    for effect in [
        ModuleEffect::Gun,
        ModuleEffect::Mobility,
        ModuleEffect::Ammo,
        ModuleEffect::Radio,
        ModuleEffect::Troops,
    ] {
        let json = serde_json::to_string(&effect).expect("serialises");
        assert_eq!(json, format!("\"{}\"", effect.as_str()));
    }
}

#[test]
fn a_module_that_takes_no_hits_to_destroy_is_a_validation_error() {
    // Hits remaining counts down to zero, and zero is what the outcome engine
    // will read as destroyed — so a `toughness: 0` module drives out already
    // broken. Nobody ever meant that.
    let mut reg = registry();
    let mut tracks = reg.module("tracks").expect("tracks").clone();
    tracks.toughness = 0;
    reg.modules.insert(tracks.id.clone(), tracks);

    let report = reg.validate();
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("tracks") && e.contains("toughness")),
        "got {:?}",
        report.errors
    );
}

#[test]
fn module_state_survives_a_save_and_a_reload() {
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 11).expect("battle");
    let panther = a(&state, "medium_tank");

    // A state nothing could re-derive from the vehicle definition, which is
    // the point: half-broken running gear has to come back half-broken, or
    // the save is quietly repairing tanks.
    state
        .unit_mut(panther)
        .expect("alive")
        .modules
        .insert("tracks".into(), 1);

    let text = SaveGame::new(&reg, None, Some(state.clone()))
        .to_json()
        .expect("serialises");
    let restored = SaveGame::from_json(&reg, &text)
        .expect("deserialises")
        .0
        .battle
        .expect("battle round-trips");

    let before: Vec<&BTreeMap<String, u32>> = state.units.iter().map(|u| &u.modules).collect();
    let after: Vec<&BTreeMap<String, u32>> = restored.units.iter().map(|u| &u.modules).collect();
    assert_eq!(before, after, "every vehicle comes back as she went in");
    assert_eq!(
        restored.unit(panther).expect("alive").modules["tracks"],
        1,
        "including the one that had been hit"
    );
}

#[test]
fn a_fought_battle_leaves_broken_modules_behind() {
    // The successor to `modules_are_inert_and_a_fought_battle_breaks_none_of_
    // them`, which the data chunk flagged for deletion by exactly this
    // engine: penetrations now roll against what is aboard, so a battle
    // fought with real guns must announce module damage and leave the
    // wreckage recorded in unit state — hits remaining below toughness
    // exactly where a ModuleHit event said so.
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 11).expect("battle");

    let planner = |seed: u64| -> Box<dyn AiPlanner<BattleState, Order>> {
        make_battle_planner(
            &AiConfig {
                planner: "utility".into(),
                difficulty: 3,
                doctrine: Some("massed_armor".into()),
            },
            seed,
            &reg,
        )
    };
    let mut ai = AiDriver::new();
    ai.insert(0, planner(7));
    ai.insert(1, planner(8));

    let mut reported: Vec<(tactics_core::battle::UnitId, String)> = Vec::new();
    for _ in 0..6 {
        if state.is_over() {
            break;
        }
        ai.plan_round(&reg, &mut state);
        for event in state.resolve_round(&reg) {
            if let tactics_core::battle::Event::ModuleHit { unit, module, .. } = event {
                reported.push((unit, module));
            }
        }
    }
    assert!(
        !reported.is_empty(),
        "six rounds of real guns should break something aboard somebody"
    );
    for (unit, module) in &reported {
        let u = state.lookup(*unit).expect("units are never removed");
        let hits = u.modules.get(module).copied().unwrap_or(u32::MAX);
        let toughness = reg.module(module).map(|m| m.toughness).unwrap_or(1);
        assert!(
            hits < toughness,
            "{}'s {module} was reported hit and the state must agree",
            u.name
        );
    }
}
