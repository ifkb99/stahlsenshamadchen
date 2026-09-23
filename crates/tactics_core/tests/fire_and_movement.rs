//! Fire and movement: what fire does to a crew who wants to move, and which
//! way she goes when she does.
//!
//! Contents: a near miss frightens by the round's own suppression, at the
//! share the mod declares; bullets do not pin a crew behind armour past the
//! share the mod lets through; a pinned crew will not step onto hotter
//! ground, is stopped half-way if the fire finds her mid-route, and her side
//! hears why; and the route a crew drives is priced by her order — a hex in
//! the enemy's sight costs what her order says it costs, so a scout goes
//! round and a crew who does not care goes straight.
//!
//! Each rule is data whose default is the game before it, and each test
//! turns its rule on in the registry it stages rather than relying on what
//! the base mod ships.

use tactics_core::Hex;
use tactics_core::battle::Latitude;
use tactics_core::battle::{
    BattleState, Event, FireIntent, Order, UnitId, incoming, path_to, reachable,
};
use tactics_core::data::{DataRegistry, RoundPressure, ShotFelt};

mod common;
use common::stage::{commit_all, two_side_battle, unit_at};
use common::{calm, registry, seen};

// --- near misses and armour ---------------------------------------------

/// The price list for a miss, read directly: nothing at `near_miss_percent`
/// zero — a miss was free before, and a mod that says nothing keeps that —
/// and the declared share of the round's own suppression above it. A round
/// that declares no suppression frightens nobody by missing, which is the
/// designer's "proper equipment" ruling written as arithmetic.
#[test]
fn a_near_miss_frightens_by_the_share_of_the_rounds_suppression_the_mod_declares() {
    let mut rules = registry().morale.clone();
    // The projectile half alone; knowing she is the target has its own test.
    rules.targeted = 0;
    let belt = RoundPressure {
        small_arms: true,
        suppression: 2,
        armoured: false,
        aimed: true,
    };
    rules.near_miss_percent = 0;
    assert_eq!(rules.pressure_for(ShotFelt::Missed, belt), 0.0);
    rules.near_miss_percent = 50;
    assert_eq!(rules.pressure_for(ShotFelt::Missed, belt), 1.0);
    let solid_shot = RoundPressure {
        small_arms: false,
        suppression: 0,
        armoured: true,
        aimed: true,
    };
    assert_eq!(
        rules.pressure_for(ShotFelt::Missed, solid_shot),
        0.0,
        "a round that declares no suppression frightens nobody by missing"
    );
}

/// Bullets against a crew behind plate: the round's suppression reaches her
/// only as far as `through_plate_percent` lets it, whether the belt bounced
/// off her or went past her. A bullet that gets *through* is not stopped by
/// plate and is charged in full. At the default of 100 nothing changes.
#[test]
fn a_belt_does_not_pin_a_crew_behind_armour_past_the_share_the_mod_lets_through() {
    let mut rules = registry().morale.clone();
    rules.targeted = 0;
    rules.near_miss_percent = 100;
    let at_a_tank = RoundPressure {
        small_arms: true,
        suppression: 2,
        armoured: true,
        aimed: true,
    };
    rules.through_plate_percent = 100;
    assert_eq!(rules.pressure_for(ShotFelt::Bounced, at_a_tank), 2.0);
    assert_eq!(rules.pressure_for(ShotFelt::Missed, at_a_tank), 2.0);
    rules.through_plate_percent = 0;
    assert_eq!(rules.pressure_for(ShotFelt::Bounced, at_a_tank), 0.0);
    assert_eq!(rules.pressure_for(ShotFelt::Missed, at_a_tank), 0.0);
    let at_a_platoon = RoundPressure {
        armoured: false,
        aimed: true,
        ..at_a_tank
    };
    assert_eq!(
        rules.pressure_for(ShotFelt::Missed, at_a_platoon),
        2.0,
        "a crew in the open is not behind anything"
    );
    let through = rules.pressure_for(ShotFelt::Penetrated { spent: 1.0 }, at_a_tank);
    assert!(
        through >= 2.0,
        "a bullet that came through is charged its suppression in full: {through}"
    );
}

/// A medium tank's machine gun on a rifle platoon in the open, every shot
/// forced wide. The platoon feels the belt going past exactly when the mod
/// prices a near miss — the charge reads the same `ShotMissed` the log
/// prints, with the round and the target on it.
#[test]
fn a_belt_that_misses_a_platoon_still_puts_it_on_its_face() {
    let felt = |near_miss: u32| {
        let mut reg = seen(registry());
        reg.balance.min_hit = 0;
        reg.balance.max_hit = 0;
        reg.morale.near_miss_percent = near_miss;
        // The projectile going past, alone: at zero nothing else is charged.
        reg.morale.targeted = 0;
        let mut state = two_side_battle(
            &reg,
            &["gggggg", "gggggg", "gggggg"],
            vec![
                unit_at([0, 1], 0, "medium_tank", "Gun"),
                unit_at([3, 1], 1, "rifle_platoon", "Platoon"),
            ],
            3,
        );
        state
            .apply(
                &reg,
                &Order::SetFire {
                    unit: UnitId(0),
                    fire: FireIntent::Target {
                        target: UnitId(1),
                        weapon: 1,
                    },
                },
            )
            .expect("the gun can see the platoon");
        commit_all(&reg, &mut state);
        let mut missed = 0;
        for _ in 0..4 {
            missed += state
                .step_tick(&reg)
                .iter()
                .filter(|e| {
                    matches!(e, Event::ShotMissed { target: Some(t), ammo: Some(_), .. } if *t == UnitId(1))
                })
                .count();
        }
        assert!(missed > 0, "the stage is a belt going wide");
        state.unit(UnitId(1)).map(|u| u.pressure).unwrap_or(0)
    };
    assert_eq!(felt(0), 0, "a miss was free, and at zero it still is");
    assert!(felt(100) > 0, "a belt going past a platoon frightens it");
}

// --- pinned --------------------------------------------------------------

/// A medium tank on an open road with a tank destroyer watching her from
/// down it, the rung above steady marked `pinned`, and her pressure set on
/// that rung or below it.
fn under_the_gun(pressure_on_pinned_rung: bool) -> (DataRegistry, BattleState) {
    // The pinning gate, not how fast she gets there: pressure is set by
    // hand, and the gun's fire must not bail her out before she can stop.
    let mut reg = calm(seen(registry()));
    reg.morale.rungs[1].pinned = true;
    let road = "g".repeat(24);
    let mut state = two_side_battle(
        &reg,
        &[&road, &road, &road],
        vec![
            unit_at([6, 1], 0, "medium_tank", "Crew"),
            unit_at([14, 1], 1, "tank_destroyer", "Gun"),
        ],
        7,
    );
    if pressure_on_pinned_rung {
        let at = reg.morale.rungs[1].at_pressure;
        state.unit_mut(UnitId(0)).unwrap().pressure = at;
    }
    (reg, state)
}

/// Pinned: of the ground she could reach, she keeps only what no more fire
/// reaches than reaches her now — the quieter ground behind her is open, the
/// hex toward the gun is not. Steady, the same crew may go anywhere she
/// could before.
#[test]
fn a_pinned_crew_will_not_step_onto_hotter_ground_and_may_still_crawl_back() {
    let (reg, steady) = under_the_gun(false);
    let (_, pinned) = under_the_gun(true);
    let crew = UnitId(0);
    let here = pinned.unit(crew).unwrap().pos;
    let heat = |state: &BattleState, hex: Hex| incoming(&reg, state, crew, hex).worth;
    let now = heat(&pinned, here);
    assert!(now > 0.0, "the stage is a crew under a gun she knows of");

    let free = reachable(&reg, &steady, crew);
    let held = reachable(&reg, &pinned, crew);
    assert!(held.len() < free.len(), "being pinned takes ground away");
    assert!(
        held.keys().all(|h| heat(&pinned, *h) <= now + 1e-3),
        "every hex she may still reach is no hotter than where she is"
    );
    let forward = free
        .keys()
        .copied()
        .find(|h| heat(&pinned, *h) > now + 1e-3)
        .expect("somewhere hotter she could have gone");
    assert!(!held.contains_key(&forward));
    assert!(
        held.keys().any(|h| heat(&pinned, *h) < now - 1e-3),
        "and quieter ground is still open to her"
    );
}

/// A route laid while she was steady, and the fire finds her on it: she stops
/// short of the first hotter hex, the rest of the route is off, and her own
/// side — and only her side — hears why.
#[test]
fn a_crew_pinned_mid_route_stops_and_her_side_hears_why() {
    let (reg, mut state) = under_the_gun(false);
    let crew = UnitId(0);
    let start = state.unit(crew).unwrap().pos;
    let toward = start + Hex::new(3, 0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: crew,
                to: toward,
            },
        )
        .expect("she can drive at the gun while steady");
    commit_all(&reg, &mut state);
    let at = reg.morale.rungs[1].at_pressure;
    state.unit_mut(crew).unwrap().pressure = at;
    let mut events = Vec::new();
    for _ in 0..3 {
        events.extend(state.step_tick(&reg));
    }
    let pinned = events
        .iter()
        .find(|e| matches!(e, Event::PinnedDown { unit, .. } if *unit == crew))
        .expect("she goes to ground");
    assert!(pinned.heard_by(&state, 0) && !pinned.heard_by(&state, 1));
    let unit = state.unit(crew).unwrap();
    assert!(unit.intent.path.is_empty(), "the rest of the route is off");
    assert_eq!(unit.pos, start, "she did not take the step forward");
}

// --- routes by order -----------------------------------------------------

/// A road the enemy watches from the south, and a lane behind a wood to its
/// north he cannot see. `march` is what a hex in his sight costs a crew on a
/// personal march.
fn watched_road(march: u32) -> (DataRegistry, BattleState) {
    let mut reg = seen(registry());
    reg.balance.route_exposure.march = march;
    // Movement enough to go either way in one round, so the route is a
    // choice and not a budget.
    if let Some(v) = reg.vehicles.get_mut("light_tank") {
        v.movement.points = 40;
    }
    let rows = [
        "ggggggggggg",
        "ggggggggggg",
        "gffffffffgg",
        "ggggggggggg",
        "ggggggggggg",
        "ggggggggggg",
        "ggggggggggg",
    ];
    let state = two_side_battle(
        &reg,
        &rows,
        vec![
            unit_at([0, 3], 0, "light_tank", "Scout"),
            unit_at([5, 6], 1, "medium_tank", "Watcher"),
        ],
        11,
    );
    (reg, state)
}

/// The route follows the order. At no price she drives the road under his
/// gun, hex for hex the shortest way; at a price she goes round behind the
/// wood and is seen on fewer hexes for it — a longer road, the one a scout
/// is told to take. Both march orders carry the same destination; only
/// `route_exposure` differs.
#[test]
fn a_crew_whose_order_wants_dead_ground_goes_round_and_one_that_does_not_goes_straight() {
    let seen_on = |march: u32| {
        let (reg, mut state) = watched_road(march);
        let scout = UnitId(0);
        assert!(
            state.fog.side(0).spotted.contains(&UnitId(1)),
            "she knows he is watching"
        );
        let to = tactics_core::offset_to_hex(10, 3);
        state
            .apply(
                &reg,
                &Order::Radio {
                    unit: scout,
                    to: Some(to),
                    fire: None,
                    latitude: Latitude::Delegated,
                },
            )
            .expect("a march order");
        let (path, _) = path_to(&reg, &state, scout, to).expect("a way there");
        let watcher = state.unit(UnitId(1)).unwrap();
        let reach = reg.vehicle(&watcher.vehicle).unwrap().vision_range;
        let observers = [(watcher.pos, reach)];
        let exposed = path
            .iter()
            .skip(1)
            .filter(|h| tactics_core::ground::watched(&state, &observers, **h))
            .count();
        (path.len(), exposed)
    };
    let (straight, seen_straight) = seen_on(0);
    let (round, seen_round) = seen_on(6);
    assert!(
        seen_round < seen_straight,
        "the covered way is seen on fewer hexes ({seen_round} against {seen_straight})"
    );
    assert!(
        round > straight,
        "and it is the longer road ({round} against {straight})"
    );
}

/// Every order a commander can give has a price in `route_exposure`: the
/// table is keyed by the mission vocabulary's verbs, and a verb added to
/// that vocabulary without a field here would drive on the shortest road
/// whatever the mod said.
#[test]
fn every_order_in_the_vocabulary_has_a_route_price() {
    let prices = tactics_core::data::RouteExposure {
        advance: 1,
        assault: 2,
        hold: 3,
        reconnoitre: 4,
        withdraw: 5,
        support: 6,
        march: 7,
    };
    let mut seen: Vec<u32> = tactics_core::battle::Mission::vocabulary()
        .iter()
        .map(|(verb, _)| prices.for_verb(verb))
        .collect();
    seen.push(prices.for_verb("march"));
    seen.sort_unstable();
    assert_eq!(seen, vec![1, 2, 3, 4, 5, 6, 7]);
}

// --- knowing she is the target -------------------------------------------

/// Half of suppression is knowing you're the target: `morale.targeted` is
/// charged for every shot aimed at her whatever it does, on top of the
/// projectile's own suppression. A shell bursting beside a crew it was not
/// aimed at charges the projectile half only. At zero, the game before.
#[test]
fn knowing_she_is_the_target_is_charged_for_every_shot_aimed_at_her() {
    let mut rules = registry().morale.clone();
    rules.near_miss_percent = 50;
    rules.targeted = 0;
    let solid_shot = RoundPressure {
        small_arms: false,
        suppression: 2,
        armoured: true,
        aimed: true,
    };
    let before = [
        rules.pressure_for(ShotFelt::Missed, solid_shot),
        rules.pressure_for(ShotFelt::Bounced, solid_shot),
        rules.pressure_for(ShotFelt::Penetrated { spent: 1.0 }, solid_shot),
    ];
    rules.targeted = 3;
    let after = [
        rules.pressure_for(ShotFelt::Missed, solid_shot),
        rules.pressure_for(ShotFelt::Bounced, solid_shot),
        rules.pressure_for(ShotFelt::Penetrated { spent: 1.0 }, solid_shot),
    ];
    for (b, a) in before.iter().zip(after) {
        assert_eq!(a - b, 3.0, "every aimed shot costs the same to know about");
    }
    let beside = RoundPressure {
        aimed: false,
        ..solid_shot
    };
    assert_eq!(
        rules.pressure_for(ShotFelt::Missed, beside),
        1.0,
        "a burst beside her that was not aimed at her is the projectile half alone"
    );
}

/// An armour-piercing round that goes past a tank frightens her crew once
/// anything flying by is priced: the gun is aimed at her, and she knows it.
/// Every shot forced wide, so what is read is the miss alone.
#[test]
fn a_tank_gun_that_misses_still_frightens_the_crew_it_was_aimed_at() {
    let felt = |targeted: u32| {
        let mut reg = seen(registry());
        reg.balance.min_hit = 0;
        reg.balance.max_hit = 0;
        reg.morale.targeted = targeted;
        let mut state = two_side_battle(
            &reg,
            &["gggggg", "gggggg", "gggggg"],
            vec![
                unit_at([0, 1], 0, "tank_destroyer", "Gun"),
                unit_at([4, 1], 1, "medium_tank", "Target"),
            ],
            5,
        );
        state
            .apply(
                &reg,
                &Order::SetFire {
                    unit: UnitId(0),
                    fire: FireIntent::Target {
                        target: UnitId(1),
                        weapon: 0,
                    },
                },
            )
            .expect("the gun can see her");
        commit_all(&reg, &mut state);
        let mut missed = 0;
        for _ in 0..6 {
            missed += state
                .step_tick(&reg)
                .iter()
                .filter(
                    |e| matches!(e, Event::ShotMissed { target: Some(t), .. } if *t == UnitId(1)),
                )
                .count();
        }
        assert!(missed > 0, "the stage is a gun going wide");
        state.unit(UnitId(1)).map(|u| u.pressure).unwrap_or(0)
    };
    let quiet = felt(0);
    assert!(
        felt(2) > quiet,
        "knowing she is the target costs her something"
    );
}
