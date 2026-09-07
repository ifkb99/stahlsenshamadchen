//! Ammunition as content: the data layer of the ballistics rewrite.
//!
//! Rounds exist, guns name the ones they chamber, vehicles carry counts, and
//! those counts ride through a save. The pipeline chunk that spends them has
//! landed since this file was written: the inertness test the data chunk
//! shipped with (`ammunition_is_inert_and_a_fought_round_spends_none`) died
//! on schedule and its successor below requires the opposite — every shot
//! fired costs exactly one round.

use std::collections::BTreeMap;
use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, make_battle_planner};
use tactics_core::battle::{BattleState, Order, OrderError, UnitId};
use tactics_core::data::{AmmoClass, AmmoDef, ValidationReport};
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
fn a_unit_spawns_carrying_the_stowage_her_vehicle_declares() {
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 4).expect("battle");

    // Not "some ammunition": the exact racks the chassis is written with, so
    // that editing `tanks.json` is the only way to change what drives out.
    for unit in &state.units {
        let vehicle = reg.vehicle(&unit.vehicle).expect("vehicle exists");
        assert_eq!(
            unit.ammo, vehicle.stowage,
            "{} drives out with what {} declares",
            unit.name, vehicle.id
        );
    }

    let panther = state.unit(a(&state, "medium_tank")).expect("alive");
    assert_eq!(panther.ammo["ap_75"], 40);
    assert_eq!(panther.ammo["he_75"], 30);
    assert!(
        panther.ammo.contains_key("ball_mg"),
        "her coaxial is fed too"
    );
}

#[test]
fn the_racks_are_open_during_deployment_and_shut_once_the_shooting_starts() {
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 4).expect("battle");
    let panther = a(&state, "medium_tank");

    // Round one, planning: this is the assembly area, and she may trade
    // high explosive for shot on the word of whoever briefed her.
    state
        .set_loadout(&reg, panther, "he_75", 25)
        .expect("a legal swap during deployment");
    assert_eq!(state.unit(panther).expect("alive").ammo["he_75"], 25);

    // Zero takes the round off entirely rather than leaving an empty rack,
    // so the map says what is aboard and never what used to be.
    state
        .set_loadout(&reg, panther, "he_75", 0)
        .expect("unloading is a legal loadout");
    assert!(
        !state
            .unit(panther)
            .expect("alive")
            .ammo
            .contains_key("he_75")
    );

    // Once the round is resolving the racks are whatever she drove out with.
    for side in 0..state.sides.len() as u8 {
        state
            .apply(&reg, &Order::Commit { side })
            .expect("both sides commit");
    }
    assert!(
        !state.is_planning(),
        "committing puts the battle into resolution"
    );
    assert!(matches!(
        state.set_loadout(&reg, panther, "ap_75", 10),
        Err(OrderError::LoadoutClosed)
    ));
}

#[test]
fn a_later_planning_phase_is_not_the_assembly_area() {
    // The check is round one *and* planning, not merely planning: every
    // later planning phase happens with the enemy in the next field, and a
    // crew cannot restock from there.
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 4).expect("battle");
    let panther = a(&state, "medium_tank");
    for side in 0..state.sides.len() as u8 {
        state
            .apply(&reg, &Order::Commit { side })
            .expect("both sides commit");
    }
    state.resolve_round(&reg);
    assert_eq!(state.round, 2, "round one is behind us");
    assert!(state.is_planning(), "and orders are open again");
    assert!(matches!(
        state.set_loadout(&reg, panther, "ap_75", 10),
        Err(OrderError::LoadoutClosed)
    ));
}

#[test]
fn a_crew_cannot_load_a_round_that_does_not_exist() {
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 4).expect("battle");
    let panther = a(&state, "medium_tank");
    assert!(matches!(
        state.set_loadout(&reg, panther, "ap_128_wunderwaffe", 10),
        Err(OrderError::NoSuchAmmo)
    ));
}

#[test]
fn a_crew_cannot_load_a_round_nothing_aboard_can_chamber() {
    // The 88 exists, and the Panther has room for it, and she still cannot
    // take it: a rack is not a calibre.
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 4).expect("battle");
    let panther = a(&state, "medium_tank");
    assert!(
        reg.ammo("ap_88").is_some(),
        "the round is real, which is what makes this the interesting refusal"
    );
    assert!(matches!(
        state.set_loadout(&reg, panther, "ap_88", 4),
        Err(OrderError::UnchamberedAmmo)
    ));
}

#[test]
fn a_vehicle_cannot_carry_more_than_it_has_racks_for() {
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 4).expect("battle");
    let panther = a(&state, "medium_tank");
    let capacity: u32 = reg
        .vehicle("medium_tank")
        .expect("vehicle")
        .stowage
        .values()
        .sum();

    assert!(matches!(
        state.set_loadout(&reg, panther, "ap_75", capacity + 1),
        Err(OrderError::StowageFull)
    ));

    // Capacity is the whole vehicle, not one rack, so emptying the belts
    // makes room for shells. That is the crude part of this model and it is
    // the part a real prep phase will price properly.
    state
        .set_loadout(&reg, panther, "ball_mg", 0)
        .expect("the belts come out");
    state
        .set_loadout(&reg, panther, "he_75", 0)
        .expect("and so does the high explosive");
    state
        .set_loadout(&reg, panther, "ap_75", capacity)
        .expect("which leaves the whole vehicle free for shot");
    assert_eq!(state.unit(panther).expect("alive").ammo["ap_75"], capacity);
}

#[test]
fn a_weapon_naming_ammunition_no_mod_declares_is_a_validation_error() {
    let mut reg = registry();
    let mut gun = reg.weapon("gun_75").expect("gun_75").clone();
    gun.ammo.push("ap_ghost".into());
    reg.weapons.insert(gun.id.clone(), gun);

    let report = reg.validate();
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("gun_75") && e.contains("ap_ghost")),
        "a phantom round must be an error, got {:?}",
        report.errors
    );
}

#[test]
fn stowage_nothing_aboard_can_fire_is_a_validation_error() {
    let mut reg = registry();

    // A round that exists, on a vehicle with no gun that chambers it. Dead
    // weight the crew could never spend, and there is no reading of the
    // content under which it was meant.
    let mut panther = reg.vehicle("medium_tank").expect("medium_tank").clone();
    panther.stowage.insert("ap_88".into(), 12);
    reg.vehicles.insert(panther.id.clone(), panther);

    // And one that does not exist at all, which is the other way to get it
    // wrong and deserves its own message.
    let mut marder = reg
        .vehicle("tank_destroyer")
        .expect("tank_destroyer")
        .clone();
    marder.stowage.insert("ap_ghost".into(), 5);
    reg.vehicles.insert(marder.id.clone(), marder);

    let report = reg.validate();
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("medium_tank") && e.contains("chamber")),
        "carrying an unchamberable round must be an error, got {:?}",
        report.errors
    );
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("tank_destroyer") && e.contains("ap_ghost")),
        "carrying a round no mod declares must be an error, got {:?}",
        report.errors
    );
}

#[test]
fn a_gun_with_no_rounds_aboard_is_only_a_warning() {
    // Not an error, because it is how you write a chassis whose loadout a
    // scenario fills in later — and because the day an empty rack starts to
    // mean "cannot fire", a modder wants to have been told rather than
    // stopped.
    let mut reg = registry();
    let mut panther = reg.vehicle("medium_tank").expect("medium_tank").clone();
    panther.stowage.remove("ap_75");
    panther.stowage.remove("he_75");
    reg.vehicles.insert(panther.id.clone(), panther);

    let report = reg.validate();
    assert!(report.is_ok(), "still loads: {:?}", report.errors);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("medium_tank") && w.contains("gun_75")),
        "a gun with nothing to feed it must warn, got {:?}",
        report.warnings
    );
}

#[test]
fn every_weapon_in_the_base_mod_names_the_rounds_it_fires() {
    // The content half of the contract. Validation cannot demand this — a
    // weapon with no ammunition list is legal and is what every mod written
    // before this chunk looks like — so the base mod's own completeness is a
    // test rather than a rule.
    let reg = registry();
    for weapon in reg.weapons.values() {
        assert!(
            !weapon.ammo.is_empty(),
            "weapon `{}` fires nothing this mod declares",
            weapon.id
        );
    }
    for vehicle in reg.vehicles.values() {
        assert!(
            !vehicle.stowage.is_empty(),
            "vehicle `{}` drives out with empty racks",
            vehicle.id
        );
    }
}

#[test]
fn penetration_falls_off_with_range_for_shot_and_holds_for_a_shaped_charge() {
    let reg = registry();
    let range = reg.weapon("gun_88").expect("gun_88").range;
    let ap = reg.ammo("ap_88").expect("ap_88");
    assert_eq!(ap.class, AmmoClass::Kinetic);

    // Clamped at both ends, linear in between, and the far figure is the one
    // that decides whether a long shot is worth taking.
    assert_eq!(ap.penetration_at(0, range), 9);
    assert_eq!(ap.penetration_at(range[0], range), 9);
    assert_eq!(ap.penetration_at(range[1], range), 7);
    assert_eq!(
        ap.penetration_at(99, range),
        7,
        "and past its reach it stays"
    );
    let middle = ap.penetration_at((range[0] + range[1]) / 2, range);
    assert!(
        (7..=9).contains(&middle),
        "an intermediate range lands between the two figures, got {middle}"
    );

    // A shaped charge forms its jet on impact, so the same arithmetic over
    // two equal entries yields a flat curve with no branch in Rust.
    let heat = AmmoDef {
        id: "heat_75".into(),
        name: "75mm HEAT".into(),
        description: String::new(),
        class: AmmoClass::Chemical,
        penetration: [7, 7],
        post_pen: 1.0,
        volatility: 1.0,
        blast: 2,
        suppression: 0,
        velocity: 450,
    };
    for hexes in 0..20 {
        assert_eq!(heat.penetration_at(hexes, [1, 12]), 7);
    }
}

#[test]
fn a_chemical_round_that_loses_penetration_downrange_is_a_warning() {
    let mut reg = registry();
    reg.ammo.insert(
        "heat_75".into(),
        AmmoDef {
            id: "heat_75".into(),
            name: "75mm HEAT".into(),
            description: String::new(),
            class: AmmoClass::Chemical,
            // Almost always a kinetic entry copied by mistake.
            penetration: [7, 5],
            post_pen: 1.0,
            volatility: 1.0,
            blast: 2,
            suppression: 0,
            velocity: 450,
        },
    );
    let report = reg.validate();
    assert!(report.is_ok(), "not fatal: {:?}", report.errors);
    assert!(
        report.warnings.iter().any(|w| w.contains("heat_75")),
        "got {:?}",
        report.warnings
    );
}

#[test]
fn every_ammo_class_prints_the_name_a_mod_writes() {
    // `AmmoClass::as_str` and serde's `rename_all` are two spellings of the
    // same contract, and a table that disagreed with the JSON would be worse
    // than no table.
    for class in [
        AmmoClass::Kinetic,
        AmmoClass::Chemical,
        AmmoClass::Explosive,
        AmmoClass::SmallArms,
    ] {
        let json = serde_json::to_string(&class).expect("serialises");
        assert_eq!(json, format!("\"{}\"", class.as_str()));
    }
}

#[test]
fn ammunition_counts_survive_a_save_and_a_reload() {
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 11).expect("battle");
    let panther = a(&state, "medium_tank");
    state
        .set_loadout(&reg, panther, "he_75", 17)
        .expect("a deliberate, unusual loadout");

    let text = SaveGame::new(&reg, None, Some(state.clone()))
        .to_json()
        .expect("serialises");
    let restored = SaveGame::from_json(&reg, &text)
        .expect("deserialises")
        .0
        .battle
        .expect("battle round-trips");

    let before: Vec<&BTreeMap<String, u32>> = state.units.iter().map(|u| &u.ammo).collect();
    let after: Vec<&BTreeMap<String, u32>> = restored.units.iter().map(|u| &u.ammo).collect();
    assert_eq!(before, after, "every rack comes back as it went in");
    assert_eq!(
        restored.unit(panther).expect("alive").ammo["he_75"],
        17,
        "including the one the player edited"
    );
}

#[test]
fn every_shot_fired_costs_exactly_one_round_from_the_racks() {
    // The successor to `ammunition_is_inert_and_a_fought_round_spends_none`,
    // which the data chunk flagged for deletion by exactly this pipeline:
    // now a fought battle's shot count and its emptied racks must agree to
    // the round. Blind shells at empty ground count too — shelling nothing
    // still costs the shell.
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 11).expect("battle");
    let before: Vec<BTreeMap<String, u32>> = state.units.iter().map(|u| u.ammo.clone()).collect();

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

    let mut shots_by: BTreeMap<UnitId, u32> = BTreeMap::new();
    for _ in 0..6 {
        if state.is_over() {
            break;
        }
        ai.plan_round(&reg, &mut state);
        for event in state.resolve_round(&reg) {
            if let tactics_core::battle::Event::ShotFired { attacker, .. } = event {
                *shots_by.entry(attacker).or_default() += 1;
            }
        }
    }
    assert!(
        shots_by.values().sum::<u32>() > 0,
        "the battle has to actually be fought for this to mean anything"
    );

    for unit in &state.units {
        // A destroyed ammunition rack zeroes what it held — rounds lost to
        // the hit, not fired — so the shot-for-shot accounting only binds
        // units whose racks came through the battle. For the others the
        // racks may only be *emptier* than the gun camera explains, never
        // fuller.
        let spent: u32 = before[unit.id.index()]
            .values()
            .sum::<u32>()
            .saturating_sub(unit.ammo.values().sum::<u32>());
        let fired = shots_by.get(&unit.id).copied().unwrap_or(0);
        let rack_intact = unit.modules.iter().all(|(id, hits)| {
            reg.module(id)
                .map(|m| m.effect != tactics_core::data::ModuleEffect::Ammo || *hits > 0)
                .unwrap_or(true)
        });
        if rack_intact && !unit.brewed {
            assert_eq!(
                spent, fired,
                "{}'s racks and her gun camera disagree",
                unit.name
            );
        } else {
            assert!(
                spent >= fired,
                "{} lost rounds to a rack hit; she cannot have conjured any",
                unit.name
            );
        }
    }
}

#[test]
fn a_mod_that_declares_no_ammunition_at_all_still_validates() {
    // The additivity rule read strictly: ammunition is a thing content may
    // have, not a thing content must have. Strip every round, every gun's
    // list and every rack, and what is left is the game as it was.
    let mut reg = registry();
    reg.ammo.clear();
    let ids: Vec<String> = reg.weapons.keys().cloned().collect();
    for id in ids {
        reg.weapons.get_mut(&id).expect("present").ammo.clear();
    }
    let ids: Vec<String> = reg.vehicles.keys().cloned().collect();
    for id in ids {
        reg.vehicles.get_mut(&id).expect("present").stowage.clear();
    }

    let mut report = ValidationReport::default();
    reg.validate_into(&mut report);
    assert!(report.is_ok(), "must still load: {:?}", report.errors);
    assert!(
        !report.warnings.iter().any(|w| w.contains("ammo")),
        "and must not nag about it: {:?}",
        report.warnings
    );
}
