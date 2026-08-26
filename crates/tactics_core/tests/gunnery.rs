//! The hit half of the resolver: everything a shot has to survive before the
//! plate is ever consulted.
//!
//! The B-chunks left this side of the model five lines long — base accuracy,
//! range, gunnery, a flat bonus for shooting downhill and half the terrain's
//! cover — while the far side of the same shot grew an impact geometry, an
//! enumerated scatter die and an interior weighted by what is physically in
//! it. These tests defend the terms that closed that gap: how big a target
//! is, whether either end of the shot is under way, and what being shot at
//! does to a gunner.
//!
//! Every one of them is also an additivity test in disguise. The arc's
//! standing rule is that each term's absence is exactly the game before it
//! existed, so the pins here come in pairs wherever that is checkable: what
//! the term does, and that zeroing it in data does nothing at all.

use std::path::PathBuf;
use tactics_core::battle::{
    BattleState, MAX_HIT, MIN_HIT, Order, SideState, UnitId, hit_breakdown, hit_chance,
};
use tactics_core::data::{DataRegistry, WeaponDef};
use tactics_core::map::{Facing, HexMap, MapFile, UnitPlacement};
use tactics_core::roster::Roster;

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

/// A strip of open grass with one vehicle at each end, `dist` hexes apart.
///
/// Deliberately featureless: every test here is about a term that is *not*
/// terrain, and a hedge in the way would make each of them a two-variable
/// experiment.
fn field(reg: &DataRegistry, attacker: &str, target: &str, dist: i32) -> BattleState {
    let width = (dist + 3) as usize;
    let row = "g".repeat(width);
    let file: MapFile = serde_json::from_value(serde_json::json!({
        "id": "gunnery_field",
        "kind": "battle",
        "shape": "free",
        "palette": { "g": "grass" },
        "rows": [row.clone(), row.clone(), row],
    }))
    .expect("map parses");
    let map = HexMap::from_map_file(&file).expect("map builds");
    let placements = vec![
        UnitPlacement {
            aboard_at: None,
            at: [1, 1],
            side: 0,
            vehicle: attacker.to_string(),
            crew: Vec::new(),
            name: Some("Shooter".into()),
            facing: Some(Facing::East),
            formation: None,
            leads: false,
        },
        UnitPlacement {
            aboard_at: None,
            at: [1 + dist, 1],
            side: 1,
            vehicle: target.to_string(),
            crew: Vec::new(),
            name: Some("Mark".into()),
            facing: Some(Facing::West),
            formation: None,
            leads: false,
        },
    ];
    let sides = vec![
        SideState {
            name: "Shooters".into(),
            ai: None,
        },
        SideState {
            name: "Marks".into(),
            ai: None,
        },
    ];
    let (roster, crews) = Roster::stamp_for(reg, &placements);
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

/// The attacker's main gun.
fn gun<'r>(reg: &'r DataRegistry, state: &BattleState) -> &'r WeaponDef {
    let vehicle = reg
        .vehicle(&state.units[0].vehicle)
        .expect("the shooter has a chassis");
    reg.weapon(&vehicle.weapons[0]).expect("and a main gun")
}

/// What the resolver would roll against, for the shot as the state currently
/// stands.
fn chance(reg: &DataRegistry, state: &BattleState) -> i32 {
    hit_chance(
        reg,
        state,
        UnitId(0),
        state.units[0].pos,
        gun(reg, state),
        UnitId(1),
        false,
    )
}

#[test]
fn a_crossing_target_is_harder_to_hit_than_a_parked_one() {
    let reg = registry();
    let mut state = field(&reg, "medium_tank", "medium_tank", 6);
    let parked = chance(&reg, &state);

    state.units[1].moved = 3;
    let crossing = chance(&reg, &state);

    assert!(
        crossing < parked,
        "a vehicle that spent the round crossing open ground must not be as \
         easy to hit as one sitting still: {crossing} against {parked}"
    );
}

#[test]
fn a_dash_costs_more_than_a_crawl_at_both_ends_of_the_shot() {
    // The rule the first draft got wrong. A hex is 100 m and a round 60 s, so
    // one hex is a walking pace and five is thirty km/h; a flat "she moved"
    // penalty prices them identically and throws away the only thing that
    // makes a fast chassis' speed a defence rather than a way of arriving
    // sooner.
    let reg = registry();
    let mut state = field(&reg, "medium_tank", "medium_tank", 6);

    state.units[1].moved = 1;
    let crawling = chance(&reg, &state);
    state.units[1].moved = 4;
    let dashing = chance(&reg, &state);
    assert!(
        dashing < crawling,
        "a target at a gallop is harder to hit than one edging forward: \
         {dashing} against {crawling}"
    );

    state.units[1].moved = 0;
    state.units[0].moved = 1;
    let nudged = chance(&reg, &state);
    state.units[0].moved = 4;
    let charging = chance(&reg, &state);
    assert!(
        charging < nudged,
        "and the same is true of the gun doing the shooting: {charging} \
         against {nudged}"
    );
}

#[test]
fn laying_a_gun_from_a_moving_vehicle_costs_more_than_being_the_moving_vehicle() {
    // Not a tuning detail: tracking a mover from a stable platform is a
    // different job from laying a gun off one, in a period where almost
    // nothing is stabilised. If these two ever cross over, halting to shoot
    // has stopped being worth anything.
    let reg = registry();
    let mut state = field(&reg, "medium_tank", "medium_tank", 6);

    state.units[1].moved = 3;
    let at_a_mover = chance(&reg, &state);
    state.units[1].moved = 0;
    state.units[0].moved = 3;
    let from_a_mover = chance(&reg, &state);

    assert!(
        from_a_mover < at_a_mover,
        "firing on the move must cost more than shooting at somebody who is: \
         {from_a_mover} against {at_a_mover}"
    );
}

#[test]
fn a_platoon_on_its_feet_is_a_smaller_target_than_a_tank() {
    // Profile is the term that distinguishes thirty men lying in a field from
    // a Löwe, and until it existed the two were hit equally often. Note this
    // is a different question from being *found*, which is `concealment` and
    // scales the spotter's range instead — the platoon here is standing in
    // the open and plainly visible.
    let reg = registry();
    let at_a_tank = {
        let state = field(&reg, "medium_tank", "medium_tank", 6);
        chance(&reg, &state)
    };
    let at_a_platoon = {
        let state = field(&reg, "medium_tank", "rifle_platoon", 6);
        chance(&reg, &state)
    };
    assert!(
        at_a_platoon < at_a_tank,
        "a rifle platoon must not be as easy to hit as a medium tank: \
         {at_a_platoon} against {at_a_tank}"
    );
}

#[test]
fn a_chassis_that_declares_no_profile_is_the_target_she_always_was() {
    // Additivity, on the one term that lives on content rather than in the
    // balance block: every armoured vehicle in the base mod says nothing
    // about profile, and must therefore be hit exactly as often as before
    // the field existed.
    let reg = registry();
    for id in ["medium_tank", "heavy_tank", "recon_car", "apc"] {
        let vehicle = reg.vehicle(id).expect("base mod ships it");
        assert_eq!(
            vehicle.profile, 0,
            "{id} declares a profile, so this test can no longer prove \
             anything about the default"
        );
        let state = field(&reg, "medium_tank", id, 6);
        let breakdown = hit_breakdown(
            &reg,
            &state,
            UnitId(0),
            state.units[0].pos,
            gun(&reg, &state),
            UnitId(1),
            false,
        );
        assert!(
            !breakdown
                .modifiers
                .iter()
                .any(|m| m.label.contains("profile")),
            "{id} should contribute no profile line at all: {:?}",
            breakdown.modifiers
        );
    }
}

#[test]
fn a_frightened_crew_lays_her_gun_worse() {
    // Suppression, and deliberately not a second fear system: pressure is
    // already collected in one place and already walks the morale ladder, so
    // what being shot at costs a gunner is a number on the rung she has been
    // driven to.
    let reg = registry();
    let mut state = field(&reg, "medium_tank", "medium_tank", 6);
    let steady = chance(&reg, &state);

    let wavering = reg
        .morale
        .rungs
        .iter()
        .find(|r| r.accuracy != 0)
        .expect("the base ladder charges something for fear");
    state.units[0].pressure = wavering.at_pressure;
    let shaken = chance(&reg, &state);

    assert!(
        shaken < steady,
        "a crew being shot at must not shoot as well as one at peace: \
         {shaken} against {steady}"
    );
}

#[test]
fn a_ladder_that_charges_nothing_for_fear_has_no_suppression() {
    // The additivity twin, and the reason suppression is a rung field rather
    // than its own counter: a mod with one steady rung, or one whose rungs
    // simply say nothing about accuracy, is the game before this existed —
    // with no `if` in Rust to switch it off.
    let mut reg = registry();
    for rung in &mut reg.morale.rungs {
        rung.accuracy = 0;
    }
    let mut state = field(&reg, "medium_tank", "medium_tank", 6);
    let steady = chance(&reg, &state);
    state.units[0].pressure = 10_000;
    let terrified = chance(&reg, &state);

    assert_eq!(
        terrified, steady,
        "with the ladder charging nothing, fear must reach the gun not at all"
    );
}

#[test]
fn scoring_a_tile_she_has_not_driven_to_does_not_charge_her_for_the_drive() {
    // The rule a measured wrong turn bought. A draft counted the distance to
    // the hypothetical `from` as driving, which sounds right — reaching a
    // tile does mean crossing the ground. But the planner scores *ground*,
    // and what makes a hill worth taking is the shooting done from it over
    // the rounds she sits there. Charging every candidate tile except the one
    // under her tracks put a standing bias on staying put: three stalemates
    // appeared in 36 games where the baseline had none, which is the exact
    // pathology land objectives were built to remove.
    let reg = registry();
    let state = field(&reg, "medium_tank", "medium_tank", 6);
    let weapon = gun(&reg, &state);
    let here = state.units[0].pos;

    let from_here = hit_chance(&reg, &state, UnitId(0), here, weapon, UnitId(1), false);
    // Somewhere she is not standing, at the same range so nothing else moves.
    let elsewhere = here + tactics_core::Hex::new(0, 1);
    let from_there = hit_chance(&reg, &state, UnitId(0), elsewhere, weapon, UnitId(1), false);

    assert_eq!(
        from_there, from_here,
        "a hypothetical firing position is not a drive: {from_there} against \
         {from_here}"
    );
}

#[test]
fn every_new_term_is_shown_to_the_player_and_adds_up() {
    // `hit_breakdown` is the player-facing twin of the roll, and the terms
    // added here have to appear in it or the panel quietly stops explaining
    // the number it is showing. Checked together with the arithmetic, since
    // a modifier that is listed but not applied is the other half of the
    // same failure.
    let reg = registry();
    let mut state = field(&reg, "medium_tank", "rifle_platoon", 6);
    state.units[0].moved = 2;
    state.units[1].moved = 3;
    state.units[0].pressure = reg
        .morale
        .rungs
        .iter()
        .find(|r| r.accuracy != 0)
        .map(|r| r.at_pressure)
        .unwrap_or(0);

    let breakdown = hit_breakdown(
        &reg,
        &state,
        UnitId(0),
        state.units[0].pos,
        gun(&reg, &state),
        UnitId(1),
        false,
    );
    for expected in ["profile", "Target under way", "Firing on the move", "Crew"] {
        assert!(
            breakdown
                .modifiers
                .iter()
                .any(|m| m.label.contains(expected)),
            "the breakdown must name {expected:?}: {:?}",
            breakdown.modifiers
        );
    }

    let summed: i32 = breakdown.base + breakdown.modifiers.iter().map(|m| m.delta).sum::<i32>();
    if breakdown.clamped {
        assert!((MIN_HIT..=MAX_HIT).contains(&breakdown.total));
    } else {
        assert_eq!(
            summed, breakdown.total,
            "the listed terms must be the total"
        );
    }
    assert_eq!(
        breakdown.total,
        chance(&reg, &state),
        "and the breakdown must explain the number the resolver rolls against"
    );
}

#[test]
fn driving_is_counted_once_however_she_came_to_drive() {
    // `moved` is incremented at the single place a unit changes hex, so an
    // ordered march, the battle drill's dash for cover and a frightened
    // crew's flight all cost the same accuracy. A second increment anywhere
    // else would mean two answers to whether she is under way.
    let reg = registry();
    let mut state = field(&reg, "medium_tank", "medium_tank", 12);
    let start = state.units[0].pos;
    let dest = start + tactics_core::Hex::new(2, 0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: dest,
            },
        )
        .expect("two hexes is inside a medium tank's round");
    let _ = state.apply(&reg, &Order::Commit { side: 0 });
    let _ = state.apply(&reg, &Order::Commit { side: 1 });

    // Watched tick by tick, because the round's end zeroes it again for the
    // next planning phase and the whole point is what the resolver sees
    // *during* the fighting.
    let mut seen = 0;
    while state.resolving_tick().is_some() {
        state.step_tick(&reg);
        seen = seen.max(state.units[0].moved);
    }
    assert_eq!(seen, 2, "two hexes driven is two hexes counted, once each");
    assert_eq!(
        state.units[0].moved, 0,
        "and the count is zeroed for the next round's planning, so nobody \
         plans as though she were already under way"
    );
}
