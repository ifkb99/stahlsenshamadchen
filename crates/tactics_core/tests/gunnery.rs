//! The resolver-depth arc's tests: everything a shot has to survive before
//! the plate is consulted, and where a shell aimed at ground actually goes.
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

use tactics_core::battle::{BattleState, Order, SideState, UnitId, hit_breakdown, hit_chance};
use tactics_core::data::{DataRegistry, WeaponDef};
use tactics_core::map::{Battlefield, Facing, MapFile, UnitPlacement};
use tactics_core::roster::Roster;

mod common;
use common::registry;

/// A strip of open grass with one vehicle at each end, `dist` hexes apart.
///
/// Deliberately featureless: every test here is about a term that is *not*
/// terrain, and a hedge in the way would make each of them a two-variable
/// experiment.
fn field(reg: &DataRegistry, attacker: &str, target: &str, dist: i32, seed: u64) -> BattleState {
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
    let map = Battlefield::from_map_file(&file).expect("map builds");
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
        seed,
    )
    .expect("the staged placements are content the base mod ships")
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
        state.units[1].pos,
        false,
    )
}

#[test]
fn a_crossing_target_is_harder_to_hit_than_a_parked_one() {
    let reg = registry();
    let mut state = field(&reg, "medium_tank", "medium_tank", 6, 1);
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
    let mut state = field(&reg, "medium_tank", "medium_tank", 6, 1);

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
    let mut state = field(&reg, "medium_tank", "medium_tank", 6, 1);

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
        let state = field(&reg, "medium_tank", "medium_tank", 6, 1);
        chance(&reg, &state)
    };
    let at_a_platoon = {
        let state = field(&reg, "medium_tank", "rifle_platoon", 6, 1);
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
        let state = field(&reg, "medium_tank", id, 6, 1);
        let breakdown = hit_breakdown(
            &reg,
            &state,
            UnitId(0),
            state.units[0].pos,
            gun(&reg, &state),
            UnitId(1),
            state.units[1].pos,
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
    let mut state = field(&reg, "medium_tank", "medium_tank", 6, 1);
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
    let mut state = field(&reg, "medium_tank", "medium_tank", 6, 1);
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
    let state = field(&reg, "medium_tank", "medium_tank", 6, 1);
    let weapon = gun(&reg, &state);
    let here = state.units[0].pos;

    let there = state.units[1].pos;
    let from_here = hit_chance(
        &reg,
        &state,
        UnitId(0),
        here,
        weapon,
        UnitId(1),
        there,
        false,
    );
    // Somewhere she is not standing, at the same range so nothing else moves.
    let elsewhere = here + tactics_core::Hex::new(0, 1);
    let from_there = hit_chance(
        &reg,
        &state,
        UnitId(0),
        elsewhere,
        weapon,
        UnitId(1),
        there,
        false,
    );

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
    let mut state = field(&reg, "medium_tank", "rifle_platoon", 6, 1);
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
        state.units[1].pos,
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
        assert!((reg.balance.min_hit..=reg.balance.max_hit).contains(&breakdown.total));
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
    let mut state = field(&reg, "medium_tank", "medium_tank", 12, 1);
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
    //
    // **Counted against the hexes she actually crossed rather than against
    // the two she was ordered across**, which is a stricter reading of the
    // same rule and the one Wave 2 forced. The ordered march ends inside the
    // enemy's arc, and since the mid-round drill started pricing ground in
    // the currency instead of in terrain `cover` it has something to say
    // about a flat field: a gun loses accuracy with range, so backing off one
    // more hex is strictly quieter and she takes it. That third hex is the
    // drill working, and the rule under test is precisely that it is counted
    // by the same increment the ordered leg is — asserting "2" would have
    // been asserting that no other system may ever move her.
    let mut seen = 0;
    let mut crossed: u32 = 0;
    let mut was = start;
    for _ in 0..reg.scale.ticks_per_round {
        state.step_tick(&reg);
        let now = state.units[0].pos;
        crossed += was.distance_to(now) as u32;
        was = now;
        seen = seen.max(state.units[0].moved);
    }
    assert!(
        crossed >= 2,
        "she drove the two hexes she was given, at least"
    );
    assert_eq!(
        seen, crossed,
        "every hex she crossed is counted once, whoever decided she should cross it"
    );
    assert_eq!(
        state.units[0].moved, 0,
        "and the count is zeroed for the next round's planning, so nobody \
         plans as though she were already under way"
    );
}

// --- where a shell actually goes -------------------------------------------

/// Fire the attacker's indirect piece at a hex and report where the shell
/// says it will come down.
///
/// Reads the shell out of `state.shells` rather than waiting for it to land,
/// because the impact point is rolled when the shot is fired — a shell in the
/// air is not still deciding where to go — and because a test about aim
/// should not also depend on what the round does when it arrives.
fn shell_impact(reg: &DataRegistry, seed: u64, dist: i32) -> Option<i32> {
    let mut state = field(reg, "artillery", "medium_tank", dist, seed);
    let at = state.units[1].pos;
    state
        .apply(
            reg,
            &Order::SetFire {
                unit: UnitId(0),
                fire: tactics_core::battle::FireIntent::Area { at, weapon: 0 },
            },
        )
        .expect("shelling a tile is always sayable");
    let _ = state.apply(reg, &Order::Commit { side: 0 });
    let _ = state.apply(reg, &Order::Commit { side: 1 });
    // Bounded, and never on `resolving_tick()`: `step_tick` returns early
    // once the battle is decided and leaves the phase on `Resolving`, so a
    // loop waiting for the phase to change spins forever the moment this
    // shell kills the only unit on the other side.
    for _ in 0..reg.scale.ticks_per_round {
        if let Some(shell) = state.shells.first() {
            return Some(shell.impact.distance_to(shell.at));
        }
        state.step_tick(reg);
    }
    None
}

#[test]
fn a_piece_that_declares_no_dispersion_lands_where_it_was_aimed() {
    // The additivity contract, and the one that matters most here: artillery
    // arriving exactly on its map reference is the game every mod written
    // before this field existed was tuned against.
    let mut reg = registry();
    for weapon in reg.weapons.values_mut() {
        weapon.dispersion = 0;
    }
    for seed in 0..12 {
        assert_eq!(
            shell_impact(&reg, seed, 20),
            Some(0),
            "with no dispersion declared every shell is exact (seed {seed})"
        );
    }
}

#[test]
fn a_shell_can_come_down_off_the_ground_it_was_aimed_at() {
    // Dispersion is a different fact from flight time, and this is the test
    // that says so. Flight time models the *target* being somewhere else; a
    // target standing perfectly still it does nothing to at all, and before
    // this a shell aimed at a parked vehicle arrived on her with certainty.
    let reg = registry();
    assert!(
        reg.weapon("howitzer_105").expect("base mod").dispersion > 0,
        "the base howitzer must declare dispersion or this proves nothing"
    );

    // Many battles rather than many shots in one, because the shell under
    // test tends to end the battle it is fired in — and because a scatter
    // that happens to roll zero once is not evidence of anything.
    let offsets: Vec<i32> = (0..24)
        .filter_map(|seed| shell_impact(&reg, seed, 24))
        .collect();
    assert!(!offsets.is_empty(), "the battery has to fire at all");
    assert!(
        offsets.iter().any(|d| *d > 0),
        "a battery firing two and a half kilometres must sometimes miss the \
         hex it was laid on: {offsets:?}"
    );
    assert!(
        offsets.contains(&0),
        "and must sometimes hit it, or this is not dispersion but a penalty: \
         {offsets:?}"
    );
}

#[test]
fn dispersion_grows_with_the_range_flown() {
    // A percentage of the distance flown, not a flat radius — which is the
    // whole reason a battery registers on a target before firing for effect.
    // Stated as a comparison of worst cases over the same seeds, because any
    // single pair of shots can roll the other way.
    let reg = registry();
    let worst = |dist: i32| {
        (0..24)
            .filter_map(|seed| shell_impact(&reg, seed, dist))
            .max()
            .unwrap_or(0)
    };
    let near = worst(6);
    let far = worst(36);
    assert!(
        far > near,
        "a shell sent three and a half kilometres must scatter further than \
         one sent six hundred metres: {far} against {near}"
    );
}

#[test]
fn a_shell_bursts_where_it_landed_and_not_where_it_was_aimed() {
    // The half of dispersion that is easy to leave half-done: displacing the
    // impact point buys nothing if the burst still resolves against the aim.
    // `ShellLanded` reports the impact for the same reason — a player
    // watching her own battery walk off the target should see it happen
    // rather than conclude the game moved her enemy.
    let reg = registry();
    let mut state = field(&reg, "artillery", "medium_tank", 24, 7);
    let at = state.units[1].pos;
    let _ = state.apply(
        &reg,
        &Order::SetFire {
            unit: UnitId(0),
            fire: tactics_core::battle::FireIntent::Area { at, weapon: 0 },
        },
    );
    let _ = state.apply(&reg, &Order::Commit { side: 0 });
    let _ = state.apply(&reg, &Order::Commit { side: 1 });

    let mut impacts = Vec::new();
    let mut landed = Vec::new();
    for _ in 0..reg.scale.ticks_per_round {
        for shell in &state.shells {
            if !impacts.contains(&shell.impact) {
                impacts.push(shell.impact);
            }
        }
        for event in state.step_tick(&reg) {
            if let tactics_core::battle::Event::ShellLanded { at, .. } = event {
                landed.push(at);
            }
        }
    }
    assert!(
        !landed.is_empty(),
        "the shell has to come down at some point"
    );
    for at in &landed {
        assert!(
            impacts.contains(at),
            "the burst is announced at the impact the shell carried, not the \
             aim: landed {at:?}, impacts {impacts:?}"
        );
    }
}

// --- what a penetration is worth once it is through ------------------------

#[test]
fn a_gun_that_scrapes_through_a_plate_spends_less_than_one_with_margin() {
    // The pipeline was always written with three outcomes and the outcome
    // engine shipped with two, so a round that barely beat a plate spent the
    // same budget inside as one that vastly overmatched it. That flattening
    // is the thing the whole no-hit-points model exists to avoid, one layer
    // further in: it makes the *margin* of a penetration mean nothing, which
    // is most of what separates a gun that can just about manage a target
    // from one that eats it.
    let reg = registry();
    let marginal = tactics_core::battle::penetration_share(&reg.balance, 10.0, 10.0, 0);
    let comfortable = tactics_core::battle::penetration_share(&reg.balance, 40.0, 10.0, 0);
    assert!(
        marginal < comfortable,
        "beating a plate by nothing must not pay like beating it fourfold: \
         {marginal} against {comfortable}"
    );
    assert!(
        (comfortable - 1.0).abs() < 1e-6,
        "and a round with margin in hand does its worst: {comfortable}"
    );
}

#[test]
fn nothing_that_bounces_is_priced_as_a_penetration() {
    // The share is an average over the outcomes that get *through*, so a
    // matchup with no such outcomes has no average to report. Returning 1.0
    // is the safe answer only because every caller multiplies it by a
    // penetration chance of zero; this pins that the two are read together.
    let reg = registry();
    let hopeless = 1.0;
    let plate = 40.0;
    assert_eq!(
        tactics_core::battle::penetration_chance(hopeless, plate, reg.balance.pen_scatter),
        0.0,
        "the premise of this test is a gun that cannot get through at all"
    );
    let share = tactics_core::battle::penetration_share(
        &reg.balance,
        hopeless,
        plate,
        reg.balance.pen_scatter,
    );
    assert_eq!(share * 0.0, 0.0, "chance times share is zero either way");
}

#[test]
fn the_preview_shows_a_marginal_penetration_as_marginal() {
    // The player-facing half. "It will get through eight times in ten, and
    // half of those barely" is a different tactical picture from a flat
    // number, and it is the picture that says to work round to the flank.
    let reg = registry();
    let state = field(&reg, "medium_tank", "medium_tank", 6, 1);
    let front = tactics_core::battle::preview_attack(&reg, &state, UnitId(0), 0, UnitId(1), false)
        .expect("two tanks facing each other");
    assert!(
        front.pen_share <= 100,
        "a share is a percentage of the budget, not a bonus"
    );
    assert!(
        front.pen_share > 0,
        "a matchup that penetrates at all spends something: {front:?}"
    );
    // The 75 against a medium tank's glacis is the roster's own example of a
    // gun that always gets through and does not always get through cleanly.
    assert!(
        front.pen_chance > 0 && front.pen_share < 100,
        "gun_75 into medium_tank front should be through but marginal: \
         {}% at {}% of budget",
        front.pen_chance,
        front.pen_share
    );
}
