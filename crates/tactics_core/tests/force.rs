//! Buying an army: what a budget and a doctrine produce.
//!
//! These run against the shipped `assets/mods`, like everything else here, so
//! they are as much about the base game's costs and roles as about the picker.
//! A failure means either the picker changed or somebody re-priced a chassis,
//! and the assertions are written to say which.

use std::path::PathBuf;
use tactics_core::data::DataRegistry;
use tactics_core::force::{self, Role};

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

fn role(reg: &DataRegistry, id: &str) -> Role {
    force::role_of(reg, reg.vehicle(id).unwrap_or_else(|| panic!("{id}")))
}

#[test]
fn a_chassis_role_is_read_off_the_hardware_and_never_declared() {
    // The rule the AI already follows for a base of fire and an infantry
    // element, applied to requisition: nothing in Rust names a vehicle, so a
    // mod that adds a mortar section or a paratroop platoon is bought
    // sensibly on the day it is written. Each of these is a different fact
    // about the chassis, and they are checked in priority order so that a
    // vehicle answering to two of them gets the more specific reading.
    let reg = registry();
    assert_eq!(role(&reg, "artillery"), Role::Guns, "it shoots over things");
    assert_eq!(
        role(&reg, "rifle_platoon"),
        Role::Infantry,
        "she walks, and that is the whole test"
    );
    assert_eq!(
        role(&reg, "apc"),
        Role::Lift,
        "she carries somebody, which beats her own modest gun"
    );
    assert_eq!(
        role(&reg, "recon_car"),
        Role::Eyes,
        "sees twenty hexes and shoots six — this project's own definition of \
         a scout, stated in the scale contract"
    );
    for gun_tank in ["medium_tank", "heavy_tank", "tank_destroyer"] {
        assert_eq!(
            role(&reg, gun_tank),
            Role::Armour,
            "{gun_tank} shoots farther than she sees, which is the deliberate \
             other half of that relationship"
        );
    }
}

#[test]
fn each_doctrine_buys_the_army_it_means_to_fight_with() {
    // The point of the whole exercise: three commanders, one budget, three
    // recognisably different armies. Asserted as *character* rather than as
    // an exact shopping list, so re-pricing a chassis or adding one to the
    // mod moves the list without breaking the test — what must survive is
    // that the doctrine weights still decide.
    let reg = registry();
    let army = |id: &str| {
        let doctrine = reg.doctrine(id).unwrap_or_else(|| panic!("{id}"));
        force::muster(&reg, doctrine, 60)
    };
    let roles = |ids: &[String]| -> Vec<Role> {
        ids.iter()
            .map(|id| force::role_of(&reg, reg.vehicle(id).expect("bought a real chassis")))
            .collect()
    };

    let massed = army("massed_armor");
    assert!(
        roles(&massed).iter().all(|r| *r == Role::Armour),
        "massed armour spends everything on armour and a gun: {massed:?}"
    );

    let elastic = army("elastic_defense");
    let elastic_roles = roles(&elastic);
    assert!(
        elastic_roles.contains(&Role::Infantry) && elastic_roles.contains(&Role::Guns),
        "elastic defence prizes cover and shoots over things, so it buys \
         people and howitzers: {elastic:?}"
    );

    let recon = army("recon_pull");
    assert!(
        roles(&recon).contains(&Role::Eyes),
        "recon pull buys eyes; nobody else on this roster does at 60 points: \
         {recon:?}"
    );
    assert!(
        !roles(&massed).contains(&Role::Eyes),
        "and massed armour, which scouts least of the three, does not"
    );
}

#[test]
fn nobody_buys_a_taxi_with_nobody_to_put_in_it() {
    // The one constraint in the picker that is not an appetite. Lift is
    // wanted in proportion to the troops bought and never for its own sake,
    // so a force can end up with fewer rides than platoons — it ran out of
    // points — but never with a spare.
    let reg = registry();
    for id in ["massed_armor", "elastic_defense", "recon_pull"] {
        let doctrine = reg.doctrine(id).expect("base doctrine");
        for budget in [20, 60, 100, 160] {
            let army = force::muster(&reg, doctrine, budget);
            let count = |want: Role| {
                army.iter()
                    .filter(|v| force::role_of(&reg, reg.vehicle(v).unwrap()) == want)
                    .count()
            };
            assert!(
                count(Role::Lift) <= count(Role::Infantry),
                "{id} at {budget} points bought {} rides for {} platoons: {army:?}",
                count(Role::Lift),
                count(Role::Infantry)
            );
        }
    }
}

#[test]
fn a_commander_never_overspends_and_always_fields_something() {
    let reg = registry();
    let cheapest = reg
        .vehicles
        .values()
        .map(|v| v.cost)
        .filter(|c| *c > 0)
        .min()
        .expect("the roster is priced");
    for id in ["massed_armor", "elastic_defense", "recon_pull"] {
        let doctrine = reg.doctrine(id).expect("base doctrine");
        for budget in [cheapest, 30, 60, 200] {
            let army = force::muster(&reg, doctrine, budget);
            assert!(
                force::cost_of(&reg, &army) <= budget,
                "{id} overspent {budget}: {army:?}"
            );
            assert!(
                !army.is_empty(),
                "{id} fielded nothing for {budget} points, which is enough for \
                 the cheapest chassis on the roster"
            );
        }
        // And a budget below the cheapest thing on the roster buys nothing,
        // rather than going into debt or looping forever.
        assert!(force::muster(&reg, doctrine, cheapest - 1).is_empty());
    }
}

#[test]
fn the_same_commander_with_the_same_points_fields_the_same_army() {
    // No rng, and no iteration over a `HashMap` reaching the result — the
    // roster is sorted by id before anything is chosen. A harness whose
    // armies differed between runs could not attribute any number it printed
    // to anything, and this is the determinism rule applied to a decision
    // rather than to an event stream.
    let reg = registry();
    let doctrine = reg.doctrine("elastic_defense").expect("base doctrine");
    let first = force::muster(&reg, doctrine, 100);
    for _ in 0..8 {
        assert_eq!(
            force::muster(&reg, doctrine, 100),
            first,
            "the same shopping trip, in the same order"
        );
    }
}

#[test]
fn a_commander_who_masses_fields_fewer_kinds_than_one_who_disperses() {
    // `concentration` finally reads as something. It is one of the two
    // doctrine weights the design doc listed as unused, and here it is the
    // rate at which owning one of a thing dulls the appetite for the next:
    // massed armour at 1.8 barely decays and brings a fist, recon pull at
    // 0.4 decays hard and brings one of everything.
    let reg = registry();
    let kinds = |id: &str| {
        let doctrine = reg.doctrine(id).expect("base doctrine");
        force::muster(&reg, doctrine, 160)
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    };
    assert!(
        kinds("massed_armor") < kinds("recon_pull"),
        "massing means fewer kinds of thing: {} vs {}",
        kinds("massed_armor"),
        kinds("recon_pull")
    );
}
