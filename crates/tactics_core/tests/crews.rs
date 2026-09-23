//! The people inside the hulls: boarding a carrier, the chain of command
//! when it is put under load, and a wound that costs more than a line in
//! the log.
//!
//! The ride is boarding, carrying and dismounting — a taxi run is two
//! halves and both have to work. The chain of command under adversarial
//! load is succession and delegation pushed until they break. Wounds with
//! teeth is `substitution_penalty` and the two fatal chances: what a crew
//! that is not what it should be actually costs, at her station and after
//! the battle. Three sections about the crew as a roster of named people
//! rather than a substance total.
//!
//! - the ride: boarding, carrying, dismounting
//! - the chain of command under adversarial load
//! - wounds with teeth

use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, Evaluator, make_battle_planner};
use tactics_core::battle::{
    BattleState, EndReason, Event as BattleEvent, FireIntent, FormationId, Latitude, Mission,
    Order, SideState, UnitId,
};
use tactics_core::data::DataRegistry;
use tactics_core::map::{HexMap, UnitPlacement};
use tactics_core::overworld::{
    ArmyId, ArmyMission, BattleReport, OverworldEvent, OverworldOrder, OverworldState,
    make_overworld_planner,
};

mod common;
use common::{
    command_rules, commit_all, crewed_stage, in_formation, objective_battle, orders_planned,
    play_round, registry, registry_wireless, scripted_battle, seen, strike_down, strip_radios,
    taxi_run_stage, two_side_battle, unit_at,
};

// --- the ride: boarding, carrying, dismounting -----------------------------

/// A taxi, her platoon beside her, and an enemy across the field. Which
/// enemy matters: a recon car can watch the whole exercise and hurt none
/// of it, a tank destroyer makes the ride a coffin — each test picks.
fn taxi_stage(reg: &DataRegistry, enemy: &str, seed: u64) -> BattleState {
    let row = "g".repeat(12);
    two_side_battle(
        reg,
        &[&row, &row, &row],
        vec![
            unit_at([1, 1], 0, "apc", "Taxi"),
            unit_at([2, 1], 0, "rifle_platoon", "Riders"),
            unit_at([10, 1], 1, enemy, "Overwatch"),
        ],
        seed,
    )
}

#[test]
fn a_platoon_with_a_long_march_ahead_of_her_calls_for_the_taxi() {
    // The AI could mount nobody. `Order::Mount` has been in the engine since
    // the ride landed, the player's `M` has used it all along, and every
    // planner ignored it — so half the transport machinery was dead for
    // every side the player was not personally commanding, and a battle taxi
    // was a thing that made exactly one delivery per battle.
    //
    // What decides it is arithmetic and nothing else: rounds spent walking
    // the journey against rounds spent walking to the tailgate, being driven,
    // and getting out. Twenty-six hexes at one hex a round is a march; the
    // same trip at six is not. Nothing here knows what an APC is.
    let reg = registry_wireless();
    let mut state = taxi_run_stage(&reg, 26, 55);
    let (riders, taxi) = (UnitId(0), UnitId(1));
    let orders = orders_planned(&reg, &mut state, 0, 55);
    assert!(
        orders
            .iter()
            .any(|o| matches!(o, Order::Mount { unit, into } if *unit == riders && *into == taxi)),
        "she should be calling for the ride: {orders:?}"
    );
}

#[test]
fn a_platoon_with_a_short_walk_ahead_of_her_walks() {
    // The other half of the same comparison, and the reason it is a
    // comparison rather than a preference. Three hexes is a walk; mounting up
    // for it would cost more rounds than it saved, and a platoon that boarded
    // for every journey would spend a battle climbing in and out. Measured:
    // pricing boarding at two rounds instead of four let exactly this happen
    // near an objective and cost the commanded side a win and three platoons.
    let reg = registry_wireless();
    let mut state = taxi_run_stage(&reg, 3, 56);
    let orders = orders_planned(&reg, &mut state, 0, 56);
    assert!(
        !orders.iter().any(|o| matches!(o, Order::Mount { .. })),
        "the objective is three hexes away; she walks: {orders:?}"
    );
}

#[test]
fn the_taxi_drives_to_the_pickup_rather_than_leaving_without_her() {
    // The driver's half, and the half without which the feature is a joke: a
    // platoon walks at one hex a round and her ride drives at six, so a
    // passenger marching after a carrier that is doing its own planning never
    // catches it. The pickup has to be somebody's job. While anybody is
    // boarding her the carrier closes the gap and then holds the door, which
    // is a rendezvous — it cannot be got by making the infantry walk faster.
    let reg = registry_wireless();
    let mut state = taxi_run_stage(&reg, 26, 57);
    let (riders, taxi) = (UnitId(0), UnitId(1));
    // Put the two of them well apart, so "toward her" and "toward the
    // objective" are opposite directions and the assertion cannot pass by
    // accident.
    state.units[taxi.index()].pos = tactics_core::offset_to_hex(9, 0);
    state
        .apply(
            &reg,
            &Order::Mount {
                unit: riders,
                into: taxi,
            },
        )
        .expect("a foot unit may board a friendly transport with room");
    let before = state.unit(taxi).unwrap().pos;
    let gap_before = before.distance_to(state.unit(riders).unwrap().pos);

    let orders = orders_planned(&reg, &mut state, 0, 57);
    let dest = orders
        .iter()
        .find_map(|o| match o {
            Order::SetMove { unit, to } if *unit == taxi => Some(*to),
            _ => None,
        })
        .expect("the taxi has somewhere to be: the pickup");
    assert!(
        dest.distance_to(state.unit(riders).unwrap().pos) < gap_before,
        "she should be closing on her fare, not driving for the objective"
    );
}

#[test]
fn the_ai_runs_a_platoon_across_the_map_and_puts_her_down_on_the_objective() {
    // The whole run, end to end, with nobody steering: she calls the taxi,
    // the taxi comes for her, she boards, she is driven twenty-odd hexes, and
    // she gets off on the ground she was making for. On foot the same journey
    // is a twenty-six round march, so arriving inside ten is proof the ride
    // happened rather than proof she is a fast walker.
    let reg = registry_wireless();
    let mut state = taxi_run_stage(&reg, 26, 58);
    let riders = UnitId(0);
    let goal = tactics_core::offset_to_hex(27, 0);
    let start = state.unit(riders).unwrap().pos.distance_to(goal);

    let mut ai = AiDriver::new();
    for side in 0..2u8 {
        ai.insert(
            side,
            make_battle_planner(
                &AiConfig {
                    planner: "utility".into(),
                    difficulty: 5,
                    doctrine: Some("massed_armor".into()),
                },
                58 + side as u64,
                &reg,
            ),
        );
    }
    let mut mounted = false;
    let mut rounds = 0;
    while !state.is_over() && rounds < 10 {
        ai.plan_round(&reg, &mut state);
        for event in &state.resolve_round(&reg) {
            if matches!(event, BattleEvent::Mounted { unit, .. } if *unit == riders) {
                mounted = true;
            }
        }
        rounds += 1;
    }
    assert!(mounted, "she never got aboard");
    let ended = state
        .units
        .get(riders.index())
        .map(|u| u.pos.distance_to(goal))
        .expect("she is on the roll one way or another");
    assert!(
        ended < start / 2,
        "ten rounds of riding should have carried her most of the way: {start} -> {ended}"
    );
}

#[test]
fn a_platoon_boards_rides_hidden_and_steps_off_where_the_ride_ends() {
    // The whole taxi doctrine in one test: she mounts by order, vanishes
    // from the enemy's picture while the carrier stays plainly visible,
    // rides wherever it drives, and steps off beside it when told —
    // reappearing to the enemy the same tick her boots touch ground.
    let reg = seen(registry_wireless());
    let mut state = taxi_stage(&reg, "recon_car", 701);
    let (taxi, riders) = (UnitId(0), UnitId(1));
    state
        .apply(
            &reg,
            &Order::Mount {
                unit: riders,
                into: taxi,
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);
    let events = state.resolve_round(&reg);
    assert!(
        events.iter().any(
            |e| matches!(e, BattleEvent::Mounted { unit, into } if *unit == riders && *into == taxi)
        ),
        "standing alongside, she steps up the same round"
    );
    assert!(
        !state.fog.side(1).spotted.contains(&riders),
        "aboard, the enemy's picture holds only the taxi"
    );
    assert!(
        state.fog.side(1).spotted.contains(&taxi),
        "which it can see just fine"
    );

    // The ride: the taxi drives, the platoon's position mirrors hers.
    //
    // Two hexes rather than three, and the hex is the difference between a
    // stage and a firefight. Three put the taxi at exactly six from the
    // overwatch, which is the machine gun's maximum reach, so the platoon
    // stepped off into a belt and bailed out before the last assertion could
    // read her position — a correct outcome and a useless stage. She now
    // dismounts one hex outside it. Worth knowing that this only became
    // possible once a burst that cannot beat plate was worth firing: the
    // recon car used to hold its fire at the taxi's armour and only ever
    // spoke when infantry appeared.
    let dest = state.unit(taxi).unwrap().pos + tactics_core::Hex::new(2, 0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: taxi,
                to: dest,
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);
    assert_eq!(
        state.unit(riders).unwrap().pos,
        state.unit(taxi).unwrap().pos,
        "she rides where the carrier is"
    );

    state
        .apply(&reg, &Order::Dismount { unit: riders })
        .unwrap();
    commit_all(&reg, &mut state);
    let events = state.resolve_round(&reg);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, BattleEvent::Dismounted { unit, .. } if *unit == riders)),
        "and steps off when told"
    );
    let (r, t) = (
        state.unit(riders).unwrap().pos,
        state.unit(taxi).unwrap().pos,
    );
    assert_eq!(
        r, t,
        "onto the ground the ride is standing on — a section gets out of the back \
         of the vehicle, it does not walk a hundred metres first. That was not \
         expressible until a hex could hold two crews; before stacking this \
         asserted `distance_to(t) == 1`, which was the engine's limit rather than \
         anybody's intent."
    );
}

/// Score a tile with every doctrinal preference switched off except the two
/// the risk tests care about: the shot in front of her and the danger she is
/// in. Cover, elevation, mass, scouting and objectives are all zeroed, and
/// `withdraw_threshold` with them — the caution term reads damage too, and
/// holding it at zero is what leaves the fractional-risk factor as the only
/// thing that can move between two otherwise identical states.
fn risk_score(reg: &DataRegistry, state: &BattleState, unit: UnitId, tile: [i32; 2]) -> f32 {
    let evaluator = Evaluator::new(tactics_core::data::DoctrineDef {
        id: "risk_probe".into(),
        name: String::new(),
        description: String::new(),
        aggression: 0.5,
        cover_value: 0.0,
        elevation_value: 0.0,
        concentration: 0.0,
        scouting: 0.0,
        objective_value: 0.0,
        indirect_appetite: 1.0,
        withdraw_threshold: 0.0,
        initiative: 0.5,
        delegation: 0.5,
        route_caution: 0.0,
        contest_aversion: 0.0,
        screening: 0.0,
        teaches: Vec::new(),
    });
    evaluator
        .score_tile(
            reg,
            state,
            unit,
            tactics_core::offset_to_hex(tile[0], tile[1]),
        )
        .score
}

/// How much harder `unit` prefers ground away from the gun than ground near
/// it. A difference of scores rather than a score, because only a difference
/// is a decision — and because every term that does not involve the threat
/// cancels between two states that differ in one thing.
fn shyness(reg: &DataRegistry, state: &BattleState, unit: UnitId) -> f32 {
    risk_score(reg, state, unit, [1, 1]) - risk_score(reg, state, unit, [7, 1])
}

#[test]
fn a_loaded_taxi_reads_the_same_gun_as_a_bigger_danger_than_an_empty_one() {
    // Danger used to be an absolute: expected damage in substance points,
    // read identically by whoever it was aimed at. So a battle taxi with a
    // platoon in the back weighed a tank destroyer's gun exactly as she
    // weighed it empty, and drove into it exactly as readily — which is how
    // a harness run ends with 22 of 24 APCs lost and most of the infantry
    // dead aboard them.
    //
    // What changed is the currency, not the courage: the same expected
    // damage is divided by what she can still absorb and multiplied by what
    // is riding on her. Nothing here knows what an APC is — she is careful
    // because she is small and because the platoon is real, and a chassis a
    // mod adds tomorrow gets the same treatment for free.
    //
    // The two states differ in one field. The platoon stands on the taxi's
    // own hex in both, so the mass term, the fog and every friend-relative
    // distance are identical; only `aboard` is set, and only the passenger
    // stake can account for the difference.
    let reg = seen(registry_wireless());
    let mut empty = taxi_stage(&reg, "tank_destroyer", 703);
    let (taxi, riders) = (UnitId(0), UnitId(1));
    assert!(
        empty.fog.side(0).spotted.contains(&UnitId(2)),
        "the threat term only counts guns the side can actually see"
    );
    empty.units[riders.index()].pos = empty.units[taxi.index()].pos;

    let mut loaded = empty.clone();
    loaded.units[riders.index()].aboard = Some(taxi);

    let (empty, loaded) = (shyness(&reg, &empty, taxi), shyness(&reg, &loaded, taxi));
    assert!(
        loaded > empty,
        "with the platoon aboard the taxi should shy from the gun harder \
         than she does empty: {loaded} vs {empty}"
    );
}

#[test]
fn a_crew_with_less_of_herself_left_weighs_the_same_shell_more_heavily() {
    // The other half of pricing risk as a fraction, and the half that applies
    // to everything on the field rather than to carriers. A crew is a third
    // of a hit from being finished when a third of her is left, and the
    // arithmetic says so now without consulting a doctrine — `caution` reads
    // damage too, but as an appetite for withdrawing scaled by
    // `withdraw_threshold`, which this probe holds at zero.
    //
    // The damage is her radio set and nothing else: on a wireless registry it
    // does no work, it is not her gun and it is not her tracks, so her shot,
    // her reach and her speed are untouched and every term in the score but
    // the threat is identical between the two states. All that differs is
    // that there is less of her.
    let reg = registry_wireless();
    let fresh = taxi_stage(&reg, "tank_destroyer", 704);
    let mut hurt = fresh.clone();
    let radio = hurt.units[0]
        .modules
        .keys()
        .find(|id| {
            reg.module(id)
                .is_some_and(|m| m.effect == tactics_core::data::ModuleEffect::Radio)
        })
        .cloned()
        .expect("the taxi carries a set");
    hurt.units[0].modules.insert(radio, 0);

    let (fresh, hurt) = (
        shyness(&reg, &fresh, UnitId(0)),
        shyness(&reg, &hurt, UnitId(0)),
    );
    assert!(
        hurt > fresh,
        "a crew with less left should shy from the gun harder than a whole \
         one: {hurt} vs {fresh}"
    );
}

#[test]
fn a_penetrated_taxi_shares_its_luck_with_everyone_aboard() {
    // The shared-fate ruling: a round through a loaded carrier does not
    // check tickets. The pool a penetration rolls against includes the
    // passengers' cadets and troops, so riding a taxi under fire costs
    // exactly what the period says it cost.
    let reg = seen(registry_wireless());
    let mut state = taxi_stage(&reg, "tank_destroyer", 702);
    let (taxi, riders, gun) = (UnitId(0), UnitId(1), UnitId(2));
    state.units[riders.index()].aboard = Some(taxi);
    state.units[riders.index()].pos = state.units[taxi.index()].pos;
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: gun,
                fire: FireIntent::Target {
                    target: taxi,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let mut rider_hurt = false;
    for _ in 0..4 {
        if state.is_over() {
            break;
        }
        for event in play_round(&reg, &mut state) {
            if matches!(
                event,
                BattleEvent::CrewHit { unit, .. } | BattleEvent::ModuleHit { unit, .. }
                    if unit == riders
            ) {
                rider_hurt = true;
            }
        }
        commit_all(&reg, &mut state);
    }
    assert!(
        rider_hurt,
        "an 88 through the box finds the people packed inside it"
    );
}

#[test]
fn a_brewed_carrier_burns_its_passengers_and_spits_out_the_rest() {
    // The worst ride there is. The carrier's racks go up, every passenger
    // is rolled through the fire, and whoever is left picks herself up
    // beside the wreck — dismounted by catastrophe rather than by order.
    let mut reg = seen(registry_wireless());
    reg.balance.brewup_percent = 100;
    if let Some(rack) = reg.modules.get_mut("ammo_rack_sparse") {
        // The apc's rack becomes most of what a penetration can find, so
        // the brew is round one business and the test is about the fire,
        // not about waiting for it.
        rack.size = 100;
    }
    let mut state = taxi_stage(&reg, "tank_destroyer", 703);
    let (taxi, riders, gun) = (UnitId(0), UnitId(1), UnitId(2));
    state.units[riders.index()].aboard = Some(taxi);
    state.units[riders.index()].pos = state.units[taxi.index()].pos;
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: gun,
                fire: FireIntent::Target {
                    target: taxi,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let (mut brewed, mut rider_events) = (false, 0);
    for _ in 0..4 {
        if state.is_over() {
            break;
        }
        for event in play_round(&reg, &mut state) {
            match event {
                BattleEvent::BrewedUp { unit } if unit == taxi => brewed = true,
                BattleEvent::CrewHit { unit, .. } | BattleEvent::ModuleHit { unit, .. }
                    if unit == riders =>
                {
                    rider_events += 1;
                }
                _ => {}
            }
        }
        if brewed {
            break;
        }
        commit_all(&reg, &mut state);
    }
    assert!(brewed, "the racks go up");
    assert!(
        rider_events > 0,
        "and the fire rolls through the passengers"
    );
    let riders_unit = &state.units[riders.index()];
    assert!(riders_unit.aboard.is_none(), "nobody stays aboard a pyre");
    if riders_unit.alive() {
        assert_eq!(
            riders_unit.pos,
            state.units[taxi.index()].pos,
            "the survivors pick themselves up beside the wreck"
        );
    }
}

#[test]
fn a_carrier_that_leaves_the_map_takes_her_passengers_home() {
    // Withdrawal by taxi: the carrier drives onto her side's exit with the
    // platoon aboard, and both leave the battle as withdrawals — exited,
    // never mourned, each scored as a unit that came home.
    let reg = registry_wireless();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([
            { "id": "west_road", "at": [[0, 0]], "value": 1, "kind": "exit", "side": 0 }
        ]),
        None,
        vec![
            unit_at([2, 0], 0, "apc", "Taxi"),
            unit_at([3, 0], 0, "rifle_platoon", "Riders"),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
    );
    let (taxi, riders) = (UnitId(0), UnitId(1));
    state.units[riders.index()].aboard = Some(taxi);
    state.units[riders.index()].pos = state.units[taxi.index()].pos;
    let exit = tactics_core::offset_to_hex(0, 0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: taxi,
                to: exit,
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);

    for unit in [taxi, riders] {
        let u = &state.units[unit.index()];
        assert!(
            !u.alive() && u.exited(),
            "{} left the battle by the road, not the graveyard",
            u.name
        );
    }
}

#[test]
fn a_taxi_under_at_threat_puts_her_passengers_on_the_ground() {
    // The one dismount reflex that keeps taxis from being coffins: the
    // planner sees the carrier under a threat that can actually hurt her
    // and puts the platoon on the ground without being asked. Nothing
    // mounts on its own initiative — the reflex only ever gets people OFF.
    let reg = seen(registry_wireless());
    let mut state = taxi_stage(&reg, "tank_destroyer", 705);
    let (taxi, riders) = (UnitId(0), UnitId(1));
    state.units[riders.index()].aboard = Some(taxi);
    state.units[riders.index()].pos = state.units[taxi.index()].pos;
    // The tank destroyer across the field is spotted and can gut an apc
    // from there: the ride is a coffin and the planner must know it.
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(2)),
        "the stage needs the threat visible"
    );

    let mut ai = AiDriver::new();
    ai.insert(
        0,
        make_battle_planner(
            &AiConfig {
                planner: "utility".into(),
                difficulty: 5,
                doctrine: None,
            },
            705,
            &reg,
        ),
    );
    ai.plan_round(&reg, &mut state);
    assert!(
        state.units[riders.index()].dismounting,
        "the planner ordered her off the moment the ride was a target"
    );
}

#[test]
fn a_scenario_may_spawn_the_riders_already_riding() {
    // Mounted starts are map data: `aboard_at` names the carrier's tile
    // and the platoon spawns aboard — the way the campaign will hand a
    // motorised column to a battle.
    let reg = registry_wireless();
    let row = "g".repeat(8);
    let mut riders = unit_at([3, 1], 0, "rifle_platoon", "Riders");
    riders.aboard_at = Some([1, 1]);
    let state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([1, 1], 0, "apc", "Taxi"),
            riders,
            unit_at([6, 1], 1, "medium_tank", "East"),
        ],
        706,
    );
    let (taxi, platoon) = (UnitId(0), UnitId(1));
    assert_eq!(
        state.units[platoon.index()].aboard,
        Some(taxi),
        "she spawns in the back, not on the grass"
    );
    assert!(
        !state.fog.side(1).spotted.contains(&platoon),
        "and the enemy's opening picture holds only the taxi"
    );
}

// --- the chain of command under adversarial load ---------------------------

//
// Everything above tests one rule at a time on a stage built to show it. This
// section does the opposite: it puts the whole machine under load — two
// commanders against each other, a search planner reading state that did not
// exist when it was written, formations too small or too deaf to work — and
// asks only that nothing illegal, silent or wedged comes out. The properties
// are deliberately cheap to check and expensive to violate.

/// Both sides thinking through their own chain of command, which is the one
/// pairing nothing exercised: `river_crossing` names `command` for side 1
/// only, so every test and every measurement so far has had a flat-pool
/// opponent absorbing whatever the commander did.
fn commanded(
    reg: &DataRegistry,
    seed: u64,
    doctrine: &str,
) -> Box<dyn AiPlanner<BattleState, Order>> {
    make_battle_planner(
        &AiConfig {
            planner: "command".into(),
            difficulty: 4,
            doctrine: Some(doctrine.into()),
        },
        seed,
        reg,
    )
}

#[test]
fn two_commanders_fight_each_other_without_an_illegal_order_or_a_wedged_round() {
    // Twelve battles: six seeds, three doctrine pairings, each fought both
    // over the shipped wire and with no `command` block at all. What is being
    // defended is not who wins — that is the balance harness's question — but
    // that a commander on both ends of the field cannot produce an order the
    // battle refuses, and cannot leave a planning phase open. A refusal
    // force-commits the side, so a planner quietly emitting illegal orders
    // looks exactly like an AI that has stopped thinking, which is the sort
    // of thing that hides for months.
    let doctrines = ["massed_armor", "elastic_defense", "combined_arms"];
    let wired = registry();
    let wireless = registry_wireless();

    for (i, seed) in [11u64, 23, 37, 41, 59, 67].iter().enumerate() {
        for (name, reg) in [("the wire", &wired), ("no wire", &wireless)] {
            let (west, east) = (doctrines[i % 3], doctrines[(i + 1) % 3]);
            let mut state = BattleState::from_map(reg, "river_crossing", *seed).unwrap();
            let mut ai = AiDriver::new();
            ai.insert(0, commanded(reg, seed ^ 0x5EED, west));
            ai.insert(1, commanded(reg, seed ^ 0xC0DE, east));

            let mut rounds = 0;
            let mut refused = Vec::new();
            while !state.is_over() && rounds < 60 {
                ai.plan_round_with(reg, &mut state, |d| {
                    if let Some(error) = &d.rejected {
                        refused.push(format!("side {} sent {:?}: {error}", d.side, d.order));
                    }
                });
                assert!(
                    !state.is_planning(),
                    "{name}, seed {seed}: both commanders spoke and nobody committed"
                );
                state.resolve_round(reg);
                rounds += 1;
            }
            assert!(
                refused.is_empty(),
                "{name}, seed {seed}, {west} against {east}: {refused:?}"
            );
            assert!(
                state.is_over(),
                "{name}, seed {seed}: still fighting after {rounds} rounds"
            );
        }
    }
}

#[test]
fn a_long_battle_never_says_anything_about_a_crew_who_has_left() {
    // The soak. Six seeds of commander against commander, and every event in
    // both the planning and the resolution stream checked against the few
    // things that must never be true however the fight goes: nothing happens
    // to a cadet who is dead or driven off the map, no order is refused, no
    // formation receives a mission nobody sent it, and no delivery is
    // announced for a crew with nothing waiting. These are cheap to check and
    // they are exactly the shapes a bug in the wire produces — an event about
    // a wreck is what a stale id looks like from the outside.
    let reg = registry();
    for seed in [3u64, 13, 29, 47, 71, 97] {
        let mut state = BattleState::from_map(&reg, "river_crossing", seed).unwrap();
        let mut ai = AiDriver::new();
        ai.insert(0, commanded(&reg, seed, "massed_armor"));
        ai.insert(1, commanded(&reg, seed + 1, "elastic_defense"));

        let mut gone: Vec<UnitId> = Vec::new();
        let mut waiting: Vec<UnitId> = Vec::new();
        let mut sent: Vec<String> = Vec::new();
        let mut wrong: Vec<String> = Vec::new();
        let mut rounds = 0;

        while !state.is_over() && rounds < 60 {
            let round = state.round;
            let mut stream = Vec::new();
            ai.plan_round_with(&reg, &mut state, |d| {
                if let Some(error) = &d.rejected {
                    wrong.push(format!("r{round}: {:?} refused: {error}", d.order));
                }
                stream.extend(d.events.iter().cloned());
            });
            stream.extend(state.resolve_round(&reg));

            for event in &stream {
                let mut departed = |unit: &UnitId, what: &str| {
                    if gone.contains(unit) {
                        wrong.push(format!("r{round}: {what} about departed {unit:?}"));
                    }
                };
                match event {
                    BattleEvent::UnitMoved { unit, .. } => departed(unit, "UnitMoved"),
                    BattleEvent::ShotFired { attacker, .. } => departed(attacker, "ShotFired"),
                    BattleEvent::UnitSpotted { unit, .. } => departed(unit, "UnitSpotted"),
                    BattleEvent::OutOfContact { unit } => departed(unit, "OutOfContact"),
                    BattleEvent::ContactRestored { unit } => departed(unit, "ContactRestored"),
                    BattleEvent::ContactReported { unit, by, .. } => {
                        departed(unit, "ContactReported");
                        departed(by, "a report filed by");
                    }
                    BattleEvent::CommandPassed { to, .. } => departed(to, "CommandPassed to"),
                    BattleEvent::Defied { unit, .. } => departed(unit, "Defied"),
                    BattleEvent::MoraleChanged { unit, .. } => departed(unit, "MoraleChanged"),
                    BattleEvent::OrdersWaiting { unit } => {
                        departed(unit, "OrdersWaiting");
                        waiting.push(*unit);
                    }
                    BattleEvent::OrdersDelivered { unit } => {
                        departed(unit, "OrdersDelivered");
                        match waiting.iter().position(|u| u == unit) {
                            Some(i) => {
                                waiting.remove(i);
                            }
                            None => wrong.push(format!(
                                "r{round}: {unit:?} was handed orders nobody was holding"
                            )),
                        }
                    }
                    BattleEvent::MissionAssigned { formation, .. } => sent.push(formation.clone()),
                    BattleEvent::MissionReceived { formation, .. } => {
                        match sent.iter().position(|f| f == formation) {
                            Some(i) => {
                                sent.remove(i);
                            }
                            None => wrong.push(format!(
                                "r{round}: {formation} received a mission nobody sent"
                            )),
                        }
                    }
                    _ => {}
                }
                match event {
                    BattleEvent::UnitDestroyed { unit, .. }
                    | BattleEvent::UnitExited { unit, .. } => gone.push(*unit),
                    _ => {}
                }
            }
            rounds += 1;
        }
        assert!(wrong.is_empty(), "seed {seed}: {wrong:#?}");
        assert!(state.is_over(), "seed {seed}: unfinished after {rounds}");
    }
}

#[test]
fn a_searching_planner_copes_with_missions_a_detachment_and_a_running_clock() {
    // `mcts_planner_produces_legal_orders` was written before formations
    // existed and fights a bare battle. MCTS clones the whole state and rolls
    // it forward, so everything the chain of command added — standing
    // missions, a crew under personal tasking, the spotting clocks, a
    // commander thinking on the other side of the field — is now inside the
    // search whether the search knows about it or not. The property is the
    // same modest one: legal orders, and a determinization that does not
    // panic.
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 7).expect("battle");
    let bridge = state.map.objectives()[0].anchor();
    for index in 0..state.formations().len() {
        state
            .apply(
                &reg,
                &Order::SetMission {
                    formation: FormationId(index as u32),
                    mission: Mission::Advance { to: bridge },
                    latitude: tactics_core::battle::Latitude::Delegated,
                },
            )
            .expect("the bridge is on the map");
    }
    // The commander takes personal charge of her scout, which is the one
    // piece of unit state a planner has never had to reason about.
    let scout = UnitId(1);
    let aside = state.unit(scout).expect("she is on the field").pos;
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: scout,
                to: Some(aside),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("a hex she is already standing on");
    assert!(state.units[scout.index()].detached());

    let mut ai = AiDriver::new();
    ai.insert(
        0,
        make_battle_planner(
            &AiConfig {
                planner: "mcts".into(),
                difficulty: 2,
                doctrine: Some("massed_armor".into()),
            },
            7,
            &reg,
        ),
    );
    ai.insert(1, commanded(&reg, 8, "elastic_defense"));

    let mut refused = Vec::new();
    for _ in 0..6 {
        if state.is_over() {
            break;
        }
        ai.plan_round_with(&reg, &mut state, |d| {
            if let Some(error) = &d.rejected {
                refused.push(format!("side {} sent {:?}: {error}", d.side, d.order));
            }
        });
        state.resolve_round(&reg);
    }
    assert!(refused.is_empty(), "{refused:?}");
}

#[test]
fn a_search_cannot_behead_an_enemy_it_has_never_seen() {
    // Determinization deletes the enemies this side has not spotted, and it
    // used to delete them by clearing `alive` alone — which is the engine's
    // word for a wreck. `check_victory` reads decapitation before anything
    // else and classifies a loss as `!alive && !exited`, so on a map that
    // staked the battle on a commanding officer, an MCTS side opened its
    // search on a world where that officer was already dead and the battle
    // was already won. Every branch scored the same and the tree was worth
    // nothing. She is now marked `exited` as well: off the board, not
    // destroyed, which is the only honest thing a search can say about a
    // vehicle it has never laid eyes on.
    let reg = registry_wireless();
    let mut state = scripted_battle(
        &reg,
        serde_json::json!({
            "id": "stakes",
            "palette": { "g": "grass", "f": "forest" },
            "rows": [
                "gggggggggggggggggggg",
                "ggggggggffffgggggggg",
                "gggggggggggggggggggg",
            ],
            "formations": [ { "id": "hq", "name": "Headquarters", "side": 1 } ],
            "loss_conditions": [ { "side": 1, "formation": "hq", "when": "leader_lost" } ],
        }),
        vec![
            unit_at([1, 1], 0, "medium_tank", "Hunter"),
            in_formation(unit_at([18, 1], 1, "medium_tank", "Boss"), "hq", true),
            unit_at([4, 1], 1, "medium_tank", "Picket"),
        ],
    );
    let (boss, picket) = (UnitId(1), UnitId(2));
    assert!(
        !state.fog.side(0).spotted.contains(&boss),
        "the forest wall has to hide the commanding officer"
    );
    assert!(
        state.fog.side(0).spotted.contains(&picket),
        "and the picket has to be in plain view, or there is nothing to search"
    );

    let known = tactics_core::ai::determinize(&state, 0, 1);
    assert!(
        !known.lost_units().any(|u| u.id == boss),
        "an officer nobody has seen is not a casualty the search may count"
    );
    // And the world the search plays in does not end before it starts.
    for side in known.living_sides() {
        state
            .apply(&reg, &Order::Commit { side })
            .expect("commit the real battle for comparison");
    }
    let mut sim = known;
    commit_all(&reg, &mut sim);
    sim.resolve_round(&reg);
    assert!(
        !matches!(
            sim.over,
            Some(tactics_core::battle::BattleResult {
                reason: EndReason::Decapitated,
                ..
            })
        ),
        "the search must not win by beheading somebody it invented: {:?}",
        sim.over
    );
}

#[test]
fn a_zeroed_command_block_is_the_game_without_one_with_a_commander_at_both_ends() {
    // `a_command_block_with_zero_coefficients_is_the_game_without_one` pins
    // the same property with a commander on one side and a flat pool on the
    // other, and it was written before the pulse, the drill, the spacing band
    // and the base of fire existed. Every one of those reads the command
    // rules or the picture, and every one of them now runs on BOTH sides of
    // this battle — so this is the same words-not-deeds comparison over the
    // machinery the original pin cannot reach, on two further seeds.
    let wire = |line: &String| {
        line.starts_with("OutOfContact")
            || line.starts_with("ContactRestored")
            || line.starts_with("ContactReported")
    };
    let run = |rules: Option<tactics_core::data::CommandRules>, seed: u64| -> Vec<String> {
        let mut reg = registry();
        reg.command = rules;
        // Hardware is content, not a coefficient: an eight-hex set would cap
        // the everywhere-net the zeroed block declares. Stripped in both runs.
        strip_radios(&mut reg);
        let mut state = BattleState::from_map(&reg, "river_crossing", seed).unwrap();
        let mut ai = AiDriver::new();
        ai.insert(0, commanded(&reg, seed, "massed_armor"));
        ai.insert(1, commanded(&reg, seed + 1, "elastic_defense"));
        let mut log = Vec::new();
        for _ in 0..6 {
            if state.is_over() {
                break;
            }
            ai.plan_round_with(&reg, &mut state, |d| {
                log.extend(d.events.iter().map(|e| format!("{e:?}")));
            });
            log.extend(state.resolve_round(&reg).iter().map(|e| format!("{e:?}")));
        }
        log
    };
    let zeroed = tactics_core::data::CommandRules {
        radius: 999,
        radius_per_signals: 0,
        relay: true,
        visual_range: 0,
        overworld_radius: 999,
        review: tactics_core::data::ReactionRules {
            skill: "command".into(),
            base_ticks: 0,
            levels_per_tick: 0,
            max_ticks: 0,
        },
        latency: tactics_core::data::ReactionRules {
            skill: "command".into(),
            base_ticks: 0,
            levels_per_tick: 0,
            max_ticks: 0,
        },
    };
    for seed in [31u64, 53] {
        let without = run(None, seed);
        assert!(
            without.iter().any(|line| line.contains("MissionAssigned")),
            "seed {seed}: both commanders should be issuing missions"
        );
        assert!(
            without.iter().any(|line| line.contains("ShotHit")),
            "seed {seed}: and fighting"
        );
        let (spoken, deeds): (Vec<String>, Vec<String>) =
            run(Some(zeroed.clone()), seed).into_iter().partition(wire);
        assert_eq!(
            deeds, without,
            "seed {seed}: a zeroed block must change words, never deeds"
        );
        assert!(spoken.iter().all(wire));
    }
}

/// A quiet field with one commanded formation, two pieces of ground worth
/// holding, and enough cadets in the formation to lose four commanders. The
/// only enemy is far beyond anyone's eyes, so nothing can interrupt the
/// commander's clock except what a test does to her on purpose.
fn succession_stage(reg: &DataRegistry) -> BattleState {
    let row = "g".repeat(40);
    let mut placements = vec![in_formation(
        unit_at([20, 1], 1, "medium_tank", "Lead"),
        "line",
        true,
    )];
    for i in 1..6 {
        placements.push(in_formation(
            unit_at([20 + i, 2], 1, "medium_tank", "Wing"),
            "line",
            false,
        ));
    }
    placements.push(unit_at([39, 0], 0, "medium_tank", "Hermit"));
    scripted_battle(
        reg,
        serde_json::json!({
            "id": "succession_stage",
            "palette": { "g": "grass" },
            "rows": [row.clone(), row.clone(), row],
            "objectives": [
                { "id": "bridge", "name": "Bridge", "at": [[5, 1]], "value": 3 },
                { "id": "ford", "name": "Ford", "at": [[35, 1]], "value": 2 },
            ],
            "formations": [ { "id": "line", "name": "The Line", "side": 1 } ],
        }),
        placements,
    )
}

#[test]
fn a_commander_woken_four_mornings_running_still_goes_back_on_her_own_clock() {
    // The pulse under repeated shock. A commander lost is an interrupt, and
    // an interrupt reschedules her next review from the moment she thinks —
    // so four decapitations in four rounds must make her think in all four,
    // and then leave her cadence anchored to the last of them rather than
    // pushed permanently into the future or, worse, brought forward for good.
    //
    // "Did she review" is made visible by handing her side whichever piece of
    // ground her formation is currently marching on: her doctrine's
    // initiative then always wants the other one, so a review she actually
    // ran always produces an order and one she skipped never does.
    let mut reg = registry();
    let mut rules = command_rules(999, true, 0);
    rules.review.base_ticks = 3;
    rules.review.max_ticks = 5;
    reg.command = Some(rules);
    strip_radios(&mut reg);
    // Defang every gun: legacy path, penetration zero. The scene needs
    // twelve rounds of a battle that can neither kill nor end — since the
    // planner learned to spread across score plateaus, her marchers really
    // do reach the far ford and really do meet the enemy picketed there,
    // and a contact that gets somebody killed starts the stalemate clock
    // on a battle this test needs to keep breathing. Toothless guns keep
    // the contact alive (bounces forever), the crews alive, and the
    // commander's pulse the only thing left to observe — which is the
    // test.
    for weapon in reg.weapons.values_mut() {
        weapon.ammo.clear();
        weapon.penetration = 0;
    }
    let mut state = succession_stage(&reg);
    assert!(state.fog.side(1).spotted.is_empty(), "a quiet field");

    let mut ai = AiDriver::new();
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            5,
            &reg,
        ),
    );

    let bridge = tactics_core::offset_to_hex(5, 1);
    let mut reviewed = Vec::new();
    for round in 1..=12u32 {
        let marching_on_the_bridge = matches!(
            state.formations()[0].mission,
            Some(Mission::Advance { to }) if to == bridge
        );
        state.objective_held[0] = marching_on_the_bridge.then_some(1);
        state.objective_held[1] = (!marching_on_the_bridge).then_some(1);
        if round <= 4 {
            let leader = state.formations()[0].leader.expect("somebody leads");
            strike_down(&mut state, leader);
        }
        let mut assigned = 0;
        ai.plan_round_with(&reg, &mut state, |d| {
            assigned += d
                .events
                .iter()
                .filter(|e| matches!(e, BattleEvent::MissionAssigned { .. }))
                .count();
            assert!(d.rejected.is_none(), "round {round}: {:?}", d.rejected);
        });
        commit_all(&reg, &mut state);
        state.resolve_round(&reg);
        reviewed.push(assigned > 0);
    }
    // Rounds 1-5 all think: the first because she has never thought, the next
    // four because a commander was lost. The lag of one is real and honest —
    // succession runs during resolution, so the loss she suffers in round N
    // is on her desk in round N+1.
    assert_eq!(
        &reviewed[..5],
        &[true, true, true, true, true],
        "four shocks in a row must wake her every time: {reviewed:?}"
    );
    // Then the clock she was left with: three rounds between reviews, counted
    // from the last one she ran, not from the last one she scheduled.
    assert_eq!(
        &reviewed[5..],
        &[false, false, false, true, false, false, false],
        "the pulse resumes on schedule rather than drifting: {reviewed:?}"
    );
}

/// A leader and one crew standing beside each other, so nothing about
/// distance can be the reason they cannot talk.
fn shoulder_to_shoulder(reg: &DataRegistry) -> BattleState {
    scripted_battle(
        reg,
        serde_json::json!({
            "id": "shoulder",
            "palette": { "g": "grass" },
            "rows": ["g".repeat(30)],
            "formations": [ { "id": "net", "name": "The Net", "side": 0 } ],
        }),
        vec![
            in_formation(unit_at([0, 0], 0, "recon_car", "Leader"), "net", true),
            in_formation(unit_at([1, 0], 0, "recon_car", "Wing"), "net", false),
            unit_at([29, 0], 1, "recon_car", "Far Foe"),
        ],
    )
}

#[test]
fn a_platoon_of_receivers_cannot_hear_a_leader_who_cannot_transmit() {
    // The degenerate case the directional net implies and nothing stated: a
    // formation whose every vehicle, the leader's included, carries a
    // receive-only set. Orders flow down a chain of TRANSMITTERS, and she has
    // none — so her platoon is off the net standing beside her, which is
    // exactly the 1941 line company the receive-only radio is modelled on.
    // The answer is not a radio at all: put the flags back and the same two
    // vehicles are talking again.
    let mut reg = registry();
    reg.command = Some(command_rules(8, true, 0));
    for vehicle in reg.vehicles.values_mut() {
        vehicle.radio = Some("receiver".into());
    }
    let mut deaf = shoulder_to_shoulder(&reg);
    commit_all(&reg, &mut deaf);
    deaf.resolve_round(&reg);
    assert!(
        deaf.hears_orders(UnitId(0)),
        "the leader always hears herself: she is the root of the net"
    );
    assert!(
        !deaf.hears_orders(UnitId(1)),
        "but nobody hears her, because she has nothing to speak with"
    );

    let mut with_flags = registry();
    let mut rules = command_rules(8, true, 0);
    rules.visual_range = 3;
    with_flags.command = Some(rules);
    for vehicle in with_flags.vehicles.values_mut() {
        vehicle.radio = Some("receiver".into());
    }
    let mut seen = shoulder_to_shoulder(&with_flags);
    commit_all(&with_flags, &mut seen);
    seen.resolve_round(&with_flags);
    assert!(
        seen.hears_orders(UnitId(1)),
        "a hand out of the cupola carries what the set cannot"
    );
}

#[test]
fn a_formation_of_one_can_be_given_any_mission_in_the_book() {
    // A formation with a single vehicle in it is the shape every rule about
    // formations has to survive: nobody to bound with, nobody to succeed her,
    // and — for a base of fire — somebody else's fight to shoot into. Each of
    // the six missions is given to her and then executed by her own
    // commander's executors for four rounds; the property is only that no
    // order comes back refused and the battle keeps moving.
    let mut reg = registry();
    reg.command = Some(command_rules(999, true, 0));
    strip_radios(&mut reg);
    let base = scripted_battle(
        &reg,
        serde_json::json!({
            "id": "lone",
            "palette": { "g": "grass" },
            "rows": ["g".repeat(30)],
            "objectives": [
                { "id": "hill", "name": "The Hill", "at": [[15, 0]], "value": 1 },
                { "id": "west_road", "name": "The Western Road", "at": [[0, 0]],
                  "value": 1, "kind": "exit", "side": 0 },
            ],
            "formations": [
                { "id": "solo", "name": "Solo", "side": 0 },
                { "id": "other", "name": "Other", "side": 0 },
            ],
        }),
        vec![
            in_formation(unit_at([5, 0], 0, "medium_tank", "Solo"), "solo", true),
            in_formation(unit_at([7, 0], 0, "medium_tank", "Other"), "other", true),
            unit_at([29, 0], 1, "medium_tank", "Foe"),
        ],
    );
    let hill = tactics_core::offset_to_hex(15, 0);
    for mission in [
        Mission::Advance { to: hill },
        Mission::Hold { at: Some(hill) },
        Mission::Hold { at: None },
        Mission::Recon { toward: hill },
        Mission::Withdraw {
            via: "west_road".into(),
        },
        Mission::Support {
            formation: "other".into(),
        },
    ] {
        let mut state = base.clone();
        state
            .apply(
                &reg,
                &Order::SetMission {
                    formation: FormationId(0),
                    mission: mission.clone(),
                    latitude: tactics_core::battle::Latitude::Delegated,
                },
            )
            .unwrap_or_else(|e| panic!("{mission:?} should be a legal order: {e}"));
        let mut ai = AiDriver::new();
        ai.insert(0, commanded(&reg, 5, "combined_arms"));
        let mut refused = Vec::new();
        for _ in 0..4 {
            if state.is_over() {
                break;
            }
            ai.plan_round_with(&reg, &mut state, |d| {
                if let Some(error) = &d.rejected {
                    refused.push(format!("{:?}: {error}", d.order));
                }
            });
            commit_all(&reg, &mut state);
            state.resolve_round(&reg);
        }
        assert!(refused.is_empty(), "under {mission:?}: {refused:?}");
    }
}

#[test]
fn a_battery_with_no_ground_and_nobody_to_shoot_for_is_told_nothing() {
    // The no-objective guard, re-checked now that a base of fire exists. A
    // fires formation is recognised off its hardware and assigned before the
    // ground is divided, so it would have been the one order that could
    // escape a map with nothing to hold — and it must not, because "a map
    // that names no ground is exactly the fight it was before commanders
    // existed" is the property the whole command layer is additive against.
    // Doubly degenerate here: the battery is also the side's only formation,
    // so there is nobody to support even if she were asked.
    let mut reg = registry();
    reg.command = Some(command_rules(999, true, 0));
    strip_radios(&mut reg);
    let mut state = scripted_battle(
        &reg,
        serde_json::json!({
            "id": "no_ground",
            "palette": { "g": "grass" },
            "rows": ["g".repeat(30)],
            "formations": [ { "id": "battery", "name": "The Battery", "side": 0 } ],
        }),
        vec![
            in_formation(unit_at([2, 0], 0, "artillery", "Guns"), "battery", true),
            in_formation(
                unit_at([3, 0], 0, "artillery", "More Guns"),
                "battery",
                false,
            ),
            unit_at([20, 0], 1, "medium_tank", "Foe"),
        ],
    );
    let mut ai = AiDriver::new();
    ai.insert(0, commanded(&reg, 5, "combined_arms"));
    let mut said = Vec::new();
    for _ in 0..5 {
        if state.is_over() {
            break;
        }
        ai.plan_round_with(&reg, &mut state, |d| {
            said.extend(
                d.events
                    .iter()
                    .filter(|e| matches!(e, BattleEvent::MissionAssigned { .. }))
                    .map(|e| format!("{e:?}")),
            );
            assert!(d.rejected.is_none(), "{:?}: {:?}", d.order, d.rejected);
        });
        commit_all(&reg, &mut state);
        state.resolve_round(&reg);
    }
    assert!(
        said.is_empty(),
        "a map with no ground gets no missions: {said:?}"
    );
}

/// A leader on an open road and one crew ten hexes out, under a two-hex net
/// with nobody relaying: she is stone deaf until she is driven back.
fn strung_wire(reg: &DataRegistry) -> BattleState {
    scripted_battle(
        reg,
        serde_json::json!({
            "id": "strung_wire",
            "palette": { "g": "grass" },
            "rows": ["g".repeat(30)],
            "formations": [ { "id": "net", "name": "The Net", "side": 0 } ],
        }),
        vec![
            in_formation(unit_at([0, 0], 0, "recon_car", "Leader"), "net", true),
            in_formation(unit_at([10, 0], 0, "recon_car", "Stray"), "net", false),
            unit_at([29, 0], 1, "recon_car", "Far Foe"),
        ],
    )
}

#[test]
fn a_waiting_order_arrives_as_an_order_however_far_she_has_come() {
    // The queue stores the destination rather than the path, because "she
    // re-paths from wherever she is when it reaches her" is the whole
    // promise of deliver-on-contact — and the delivered order is now a
    // standing personal destination (`Unit.tasking`), marched toward one
    // round at a time until she arrives. This is what a real crew does with
    // a movement order to distant ground: it does not expire for being far,
    // it is executed across as many periods as the ground demands. The
    // sender-side alternative — somebody driving out to carry the message —
    // is the courier feature the design doc keeps for later.
    let mut reg = registry();
    reg.command = Some(command_rules(2, false, 0));
    strip_radios(&mut reg);
    let mut state = strung_wire(&reg);
    let stray = UnitId(1);
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);
    assert!(!state.hears_orders(stray), "ten hexes on a two-hex net");

    let east = tactics_core::offset_to_hex(13, 0);
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: stray,
                to: Some(east),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("accepted and held at the radio");

    // She drives herself back onto the net over three rounds, by which time
    // the hex she was sent to is far behind one round's driving.
    let mut delivered = false;
    for stop in [7, 4, 2] {
        state
            .apply(
                &reg,
                &Order::SetMove {
                    unit: stray,
                    to: tactics_core::offset_to_hex(stop, 0),
                },
            )
            .expect("her own legs");
        commit_all(&reg, &mut state);
        delivered |= state
            .resolve_round(&reg)
            .iter()
            .any(|e| matches!(e, BattleEvent::OrdersDelivered { unit } if *unit == stray));
    }
    assert!(delivered, "the wire comes back up and the order goes out");
    assert!(
        !state.units[stray.index()].intent.path.is_empty(),
        "an order announced as delivered has to be an order she is carrying out"
    );
}

#[test]
fn a_campaign_run_by_standing_orders_and_planners_plays_itself_out() {
    // The campaign half under load: standing orders given on day one, both
    // sides' planners driving everything nobody ordered, sixty days, and
    // every battle the map throws up fed back through the real
    // `apply_battle_result` path. What is defended is that the loop runs to a
    // conclusion without a panic and that the mission machinery behaves as
    // written along the way — in particular that an order given to an army
    // out of radio range on day one waits at headquarters and goes out on the
    // first morning the wire is up, days later and unprompted, which is the
    // campaign's whole answer to command friction.
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let senior = state.senior_army(0).unwrap();
    let junior = state
        .side_armies(0)
        .map(|a| a.id)
        .find(|id| *id != senior)
        .expect("frontier gives side 0 two companies");
    assert!(
        !state.in_contact(junior),
        "frontier's second company starts off the net, which is the point"
    );
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army: senior,
                mission: ArmyMission::Advance {
                    to: tactics_core::offset_to_hex(12, 1),
                },
            },
        )
        .expect("advance on the enemy's ground");
    let queued = state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army: junior,
                mission: ArmyMission::Withdraw {
                    to: tactics_core::offset_to_hex(0, 8),
                },
            },
        )
        .expect("accepted, not refused");
    assert!(
        queued
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyOrdersWaiting { army } if *army == junior)),
        "an order she cannot be told waits at headquarters: {queued:?}"
    );

    let mut planners: Vec<_> = (0..2)
        .map(|side| {
            make_overworld_planner(
                &AiConfig {
                    planner: "simple".into(),
                    difficulty: 3,
                    doctrine: None,
                },
                42 + side,
            )
        })
        .collect();

    // A battle resolved the cheap way — the defender loses her leading
    // vehicle — but through the real feedback path, so army destruction,
    // crew fates and the victor taking the tile all happen as they would.
    fn resolve(reg: &DataRegistry, state: &mut OverworldState, attacker: ArmyId, defender: ArmyId) {
        let attacking = state
            .army(attacker)
            .map(|a| a.units.clone())
            .unwrap_or_default();
        let defending = state
            .army(defender)
            .map(|a| a.units.clone())
            .unwrap_or_default();
        let losses: Vec<tactics_core::overworld::CrewLoss> = defending
            .first()
            .into_iter()
            .flat_map(|u| {
                u.crew
                    .iter()
                    .map(move |cadet| tactics_core::overworld::CrewLoss {
                        cadet: *cadet,
                        vehicle: u.vehicle.clone(),
                        killed_by: None,
                        found: None,
                        aid: tactics_core::data::AVERAGE,
                    })
            })
            .collect();
        state.apply_battle_result(
            reg,
            &BattleReport::of(
                attacker,
                defender,
                vec![
                    (attacker, attacking),
                    (defender, defending.into_iter().skip(1).collect()),
                ],
                losses.clone(),
            ),
        );
    }

    let (mut battles, mut transmitted) = (0, false);
    for _ in 0..60 {
        let side = state.active_side;
        // The planner drives everything nobody gave standing orders; an army
        // under orders is left to carry them out, which is delegation.
        for _ in 0..12 {
            let order = planners[side as usize].next_order(&reg, &state, side);
            if order == OverworldOrder::EndTurn {
                break;
            }
            if let OverworldOrder::MoveArmy { army, .. } = order
                && state.army(army).is_some_and(|a| a.mission.is_some())
            {
                break;
            }
            let Ok(events) = state.apply(&reg, &order) else {
                break;
            };
            for event in &events {
                if let OverworldEvent::BattleTriggered {
                    attacker, defender, ..
                } = event
                {
                    battles += 1;
                    resolve(&reg, &mut state, *attacker, *defender);
                }
            }
        }
        let Ok(events) = state.apply(&reg, &OverworldOrder::EndTurn) else {
            break;
        };
        for event in &events {
            match event {
                OverworldEvent::BattleTriggered {
                    attacker, defender, ..
                } => {
                    battles += 1;
                    resolve(&reg, &mut state, *attacker, *defender);
                }
                OverworldEvent::ArmyMissionAssigned { army, mission }
                    if *army == junior && matches!(mission, ArmyMission::Withdraw { .. }) =>
                {
                    transmitted = true;
                }
                _ => {}
            }
        }
    }
    assert!(
        battles > 0,
        "sixty days of two planners should meet somewhere"
    );
    // The order held on day one has to *resolve*: either it goes out on the
    // first morning the wire is up, or the company it was for stops existing
    // and it is dropped, there being nobody to give it to. What must not happen
    // is that it sits in the drawer for ever while its army is alive and
    // reachable.
    //
    // Stated as the pair rather than as "it transmitted" because which of the
    // two a sixty-day brawl produces is an accident of the brawl, not a rule.
    // It used to be the first here — the senior company was destroyed, the
    // junior inherited headquarters and heard herself — and it is now the
    // second, because an advance that engages what blocks it fights different
    // battles on different days and this run gets the junior overrun instead.
    // The rule the waiting tray actually promises is pinned properly by
    // `an_army_mission_out_of_range_waits_and_then_transmits`.
    //
    // And a third exit, since the map learned what winning it is: the
    // campaign can *end* with the order still in the drawer, because the
    // senior company this test sends at the enemy's ground is the one
    // carrying headquarters and `frontier` says losing it loses. An order
    // nobody will ever carry out because the war is over is not an order
    // sitting in the drawer; there is no drawer. (Measured: this run ends
    // on day 9 after five battles, to the Valkyries.)
    let over = state.over.is_some();
    assert!(
        transmitted || state.army(junior).is_none() || over,
        "a held order must either go out or die with the army it was for, \
         never sit in the drawer while she is alive to receive it"
    );
    assert!(
        over || state.waiting_missions.is_empty(),
        "and headquarters is not still holding it: {:?}",
        state.waiting_missions
    );
}

#[test]
fn a_personal_march_carries_across_rounds_and_ends_in_a_hold() {
    // The rest of the promise: the delivered destination is not one round's
    // lunge but a march. The staff re-issues the leg every round until she
    // stands on the ordered ground, the tasking then clears, and she holds
    // there — detached still, because nobody has recalled her.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 111).unwrap();
    let unit = UnitId(0);
    let start = state.unit(unit).unwrap().pos;
    // Far up her own side of the river: several rounds' driving, no enemy
    // contact to muddy the march with drill moves.
    let far = start + tactics_core::Hex::new(3, -9);
    assert!(state.map.contains(far));
    state
        .apply(
            &reg,
            &Order::Radio {
                unit,
                to: Some(far),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("a far destination is an order now, not a refusal");
    assert_eq!(state.unit(unit).unwrap().march().map(|m| m.to), Some(far));

    let mut ai = AiDriver::new();
    ai.insert(
        0,
        Box::new(tactics_core::ai::SideCommand::executor_only(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            111,
            &reg,
        )),
    );
    let mut arrived_at = None;
    for round in 0..10 {
        ai.plan_round(&reg, &mut state);
        let _ = state.apply(&reg, &Order::Commit { side: 1 });
        state.resolve_round(&reg);
        if state.unit(unit).unwrap().pos == far {
            arrived_at = Some(round);
            break;
        }
    }
    let arrived = arrived_at.expect("she gets there");
    assert!(arrived > 0, "and it took more than one round: {arrived}");
    // The round after arrival opens with the tasking cleared and her holding.
    ai.plan_round(&reg, &mut state);
    let unit = state.unit(unit).unwrap();
    assert_eq!(unit.march(), None, "arrived is done");
    assert!(unit.detached(), "but she stays on her commander's post");
    assert!(
        unit.intent.path.is_empty(),
        "holding the ground she was sent to"
    );
}

// --- wounds with teeth -----------------------------------------------------

/// Rebuild the same stage with `hurt` marked wounded before the battle opens,
/// which is what a cadet carried out of last week's fight looks like.
fn stage_with_a_wounded_girl(
    reg: &DataRegistry,
    crew: &[&str],
    hurt: &[usize],
) -> (BattleState, UnitId) {
    stage_with_the_hurt(reg, crew, hurt, &[])
}

/// ...and with `called` among them put on the roll anyway, which is the
/// muster's answer.
fn stage_with_the_hurt(
    reg: &DataRegistry,
    crew: &[&str],
    hurt: &[usize],
    called: &[usize],
) -> (BattleState, UnitId) {
    let (state, ours) = crewed_stage(reg, crew);
    // Mark the roster, then rebuild: `who_deploys` reads the roster at spawn,
    // which is the only moment the question is asked.
    let mut roster = (*state.roster).clone();
    let ids: Vec<tactics_core::roster::CadetId> = state.unit(ours).unwrap().crew.clone();
    for seat in hurt {
        roster.get_mut(ids[*seat]).unwrap().status =
            tactics_core::roster::CadetStatus::Wounded { days: 3 };
    }
    let roster = roster.mustered(&called.iter().map(|seat| ids[*seat]).collect::<Vec<_>>());
    let rows = vec!["g".repeat(12), "g".repeat(12), "g".repeat(12)];
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "crewed_stage",
        "palette": { "g": "grass" },
        "rows": rows,
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let placements = vec![
        UnitPlacement {
            aboard_at: None,
            at: [1, 1],
            side: 0,
            vehicle: "medium_tank".into(),
            crew: crew.iter().map(|id| (*id).to_string()).collect(),
            name: Some("Ours".into()),
            facing: None,
            formation: None,
            leads: false,
        },
        unit_at([10, 1], 1, "medium_tank", "Theirs"),
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
    let crews: Vec<Vec<tactics_core::roster::CadetId>> = vec![ids.clone(), Vec::new()];
    let state = BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        7,
    )
    .expect("the staged placements are content the base mod ships");
    (state, ours)
}

/// A cadet who is still recovering does not climb into the tank — and is not
/// deleted from it either.
///
/// Both halves matter. Until this rule existed a wound cost a side nothing
/// it could see: she deployed, `crew_skill` quietly ignored her, and the
/// player was never told why her gunnery had gone off. And the campaign
/// takes the crew list back at the end of a battle, so a cadet *removed* from
/// the list here would be a cadet removed from her tank for good.
#[test]
fn a_girl_in_the_infirmary_does_not_climb_in() {
    let reg = registry();
    let crew = ["anka", "mina", "juno"];
    let (state, ours) = stage_with_a_wounded_girl(&reg, &crew, &[1]);
    let unit = state.unit(ours).unwrap();

    assert_eq!(
        unit.crew.len(),
        3,
        "she stays on the roll; the campaign hands this list back"
    );
    assert_eq!(
        unit.crew_state.get(1).copied(),
        Some(tactics_core::battle::CrewCondition::Absent),
        "the cadet who is still recovering is not aboard: {:?}",
        unit.crew_state
    );
    assert_eq!(
        state.fighting_crew(unit),
        2,
        "two cadets in a three-seat tank"
    );

    // The seat she is not sitting in is not a wound. A vehicle that deployed
    // short-handed must not read as one that has already been shot up, or
    // every withdrawal threshold and every AI kill estimate in the game
    // would price it as half dead.
    assert_eq!(
        state.condition(&reg, unit),
        1.0,
        "an empty seat is not damage"
    );
    let (_, whole) = state.substance(&reg, unit);
    let (fresh, _) = crewed_stage(&reg, &crew);
    let (_, full) = state.substance(&reg, fresh.unit(ours).unwrap());
    assert_eq!(
        whole + 2,
        full,
        "her seat leaves the reckoning entirely rather than counting as a loss"
    );
}

/// ...unless her academy calls her up, and then she rides with her wound.
///
/// The muster's answer, and the only choice the campaign offers about a
/// wound. Standing her down leaves the seat empty: her two points of
/// substance leave the reckoning entirely, and somebody covers her station at
/// the substitution penalty. Calling her up puts her back in it hurt — a
/// point of substance instead of two, a crew that reads as already knocked
/// about, and her own hands on her own instrument, worse than they were but
/// better than the commander reaching over from a lower base.
///
/// Nobody called up is the game exactly as it was, which is the rule every
/// harsh system in this project is built to satisfy: `called_up` is false on
/// every cadet the campaign owns, and only the copy of the roster a muster
/// hands one battle ever carries it.
#[test]
fn a_cadet_her_academy_calls_up_rides_with_her_wound() {
    use tactics_core::battle::CrewCondition;

    let reg = registry();
    let crew = ["anka", "mina", "juno"];
    let (stood_down, ours) = stage_with_the_hurt(&reg, &crew, &[1], &[]);
    let (called, _) = stage_with_the_hurt(&reg, &crew, &[1], &[1]);

    assert_eq!(
        called.unit(ours).unwrap().crew_state.get(1).copied(),
        Some(CrewCondition::Wounded),
        "called up, she is at her station and hurt: {:?}",
        called.unit(ours).unwrap().crew_state
    );
    assert_eq!(
        called.fighting_crew(called.unit(ours).unwrap()),
        3,
        "three cadets in a three-seat tank, one of them hurt"
    );

    // She is worth half of herself in the reckoning, which is the price of
    // riding: her tank is a point harder to finish than the short-handed one
    // and starts the battle looking as though somebody has already been at it.
    let (have, whole) = called.substance(&reg, called.unit(ours).unwrap());
    let (short, empty) = stood_down.substance(&reg, stood_down.unit(ours).unwrap());
    assert_eq!(have, short + 1, "a hurt cadet aboard is worth one point");
    assert_eq!(whole, empty + 2, "and her seat is back in the denominator");
    assert!(
        called.condition(&reg, called.unit(ours).unwrap()) < 1.0,
        "a crew with somebody hurt aboard is not a whole crew"
    );
    assert_eq!(
        stood_down.condition(&reg, stood_down.unit(ours).unwrap()),
        1.0,
        "an empty seat is still not damage"
    );
}

/// ...and nothing that happens to the tank she is not in happens to her.
///
/// The half of the rule that was asserted in prose and checked nowhere. A
/// cadet left behind is still listed in `Unit::crew` — she has to be, or the
/// campaign would take back a crew list she had been deleted from — and the
/// wreck loop walked that list without asking who was actually aboard. So a
/// gunner recovering in the infirmary could be pulled out of a burning tank
/// four kilometres away, roll `resolve_crew_fate` against what killed it,
/// and be buried by the same campaign that had her signed in sick that
/// morning. The other direction was just as wrong: an `Unharmed` roll wrote
/// `Ready` straight over her recovery and cured her.
#[test]
fn a_cadet_who_stayed_in_the_infirmary_is_no_casualty_of_the_battle_she_missed() {
    use tactics_core::battle::{Destruction, Fate};
    use tactics_core::overworld::CrewLoss;

    let reg = registry();
    let crew = ["anka", "mina", "juno"];
    let (mut state, ours) = stage_with_a_wounded_girl(&reg, &crew, &[1]);
    let missing = state.unit(ours).unwrap().crew[1];
    state.unit_mut(ours).unwrap().fate = Fate::Destroyed(Destruction::BrewedUp);

    let losses = CrewLoss::in_battle(&reg, &state);
    assert_eq!(
        losses.len(),
        2,
        "the two who were in the tank are the two the campaign has to account for: {losses:?}"
    );
    assert!(
        !losses.iter().any(|loss| loss.cadet == missing),
        "she was in the infirmary and the wreck loop pulled her out of it anyway"
    );
}

/// ...but a tank whose whole crew is in the infirmary drives out anyway.
///
/// The campaign has no pool of replacements to draw on, and a vehicle with
/// nobody aboard is one that nothing inside can kill — which is the invariant
/// the anonymous-crew fallback exists to protect. The walking wounded go, and
/// pay for it by being worth nothing at their stations.
#[test]
fn a_crew_with_nobody_fit_goes_out_anyway() {
    let reg = registry();
    let crew = ["anka", "mina", "juno"];
    let (state, ours) = stage_with_a_wounded_girl(&reg, &crew, &[0, 1, 2]);
    let unit = state.unit(ours).unwrap();
    assert!(
        unit.crew_state.is_empty(),
        "nobody is marked absent when there is nobody else to send: {:?}",
        unit.crew_state
    );
    assert_eq!(state.fighting_crew(unit), 3);
}

/// A wound taken at her station in a tank that came home is still a wound
/// when the campaign screen draws.
///
/// This is the hole the consequence loop had. The battle tracked every cadet's
/// condition seat by seat all fight, and the only casualties the campaign
/// ever heard about were the crews of *destroyed* vehicles — so a gunner
/// knocked out in the first round of a battle her side won was fit again by
/// the time anybody could look at her.
#[test]
fn a_wound_taken_at_her_station_survives_the_battle() {
    use tactics_core::battle::CrewCondition;
    use tactics_core::roster::CasualtyRules;

    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 11).expect("overworld");
    state.rules = CasualtyRules { permadeath: false };
    let attacker = state.side_armies(0).next().unwrap().id;
    let defender = state.side_armies(1).next().unwrap().id;
    let unit = state.army(attacker).unwrap().units[0].clone();
    let cadet = unit.crew[0];
    assert!(state.roster.get(cadet).unwrap().status.is_ready());

    // Her vehicle came home. She did not come home fit.
    let survivors = vec![(attacker, state.army(attacker).unwrap().units.clone())];
    let hurt = tactics_core::overworld::CrewLoss {
        cadet,
        vehicle: unit.vehicle.clone(),
        killed_by: None,
        found: Some(CrewCondition::Out),
        aid: tactics_core::data::AVERAGE,
    };
    state.apply_battle_result(
        &reg,
        &BattleReport::of(attacker, defender, survivors.clone(), vec![hurt]),
    );

    let status = state.roster.get(cadet).unwrap().status;
    assert!(
        !status.is_ready(),
        "she was carried out of her own tank and the campaign forgot: {status:?}"
    );
    assert!(
        !status.is_permanent(),
        "permadeath is off, so a station wound is never fatal"
    );
    let days = status.days_out().expect("she is coming back");
    assert!(
        days > 0,
        "a wound that keeps her out for no days is no wound"
    );
}

/// A grazing hit costs her less than being carried out, and both cost less
/// than a wreck. The ordering is the rule; the numbers live in mod data.
#[test]
fn how_badly_she_was_hurt_decides_how_long_she_is_out() {
    use rand::SeedableRng;
    use tactics_core::battle::CrewCondition;
    use tactics_core::data::Casualties;
    use tactics_core::roster::{CasualtyRules, CrewFate, resolve_station_fate};

    let table = Casualties::default();
    let rules = CasualtyRules { permadeath: false };
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(3);

    let days = |fate: CrewFate| match fate {
        CrewFate::Wounded { days } | CrewFate::Lost { days } => days,
        _ => 0,
    };
    // Averaged over many rolls, because each is a range and a single draw
    // proves nothing about the ordering.
    let mean = |found: CrewCondition, rng: &mut rand_chacha::ChaCha8Rng| {
        let total: u32 = (0..200)
            .map(|_| {
                days(resolve_station_fate(
                    rules,
                    &table,
                    found,
                    tactics_core::data::AVERAGE,
                    rng,
                ))
            })
            .sum();
        total as f64 / 200.0
    };
    let grazed = mean(CrewCondition::Wounded, &mut rng);
    let carried = mean(CrewCondition::Out, &mut rng);
    assert!(
        grazed < carried,
        "being carried out should cost more than being grazed: {grazed} vs {carried}"
    );
    // And a cadet who was never in the vehicle takes nothing home from a
    // battle she did not fight.
    assert_eq!(
        resolve_station_fate(
            rules,
            &table,
            CrewCondition::Absent,
            tactics_core::data::AVERAGE,
            &mut rng
        ),
        CrewFate::Unharmed
    );
}

/// Being carried home out of the fight is priced by its own number, not by
/// the one that says how bad a wreck's wound was.
///
/// They shared `severe_percent` until the attrition table could fight a
/// hundred battles and ask what each of them cost, and the answer was that
/// one cadet in nine the shipped campaign would have buried was somebody
/// whose tank drove back. That is not a tuning question — it is one
/// probability standing in for two situations the rest of the model is at
/// pains to keep apart — and the tell is this test: no value of
/// `severe_percent` can spare her, and no value of `carried_fatal_percent`
/// can spare the crew of a burnt-out hull.
#[test]
fn a_cadet_carried_home_is_priced_by_the_homecoming_and_not_by_the_wreck() {
    use rand::SeedableRng;
    use tactics_core::battle::CrewCondition;
    use tactics_core::data::{Casualties, DamageType};
    use tactics_core::roster::{CasualtyRules, CrewFate, resolve_crew_fate, resolve_station_fate};

    let lethal = CasualtyRules { permadeath: true };
    // A campaign that kills people, and a homecoming that is never fatal in
    // it. Zero is the rule's absence and it has to be reachable, or "her tank
    // came home" is a sentence the data cannot say.
    let table = Casualties {
        carried_fatal_percent: 0,
        severe_percent: 100,
        ..Casualties::default()
    };
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(7);
    for _ in 0..200 {
        assert!(
            !matches!(
                resolve_station_fate(
                    lethal,
                    &table,
                    CrewCondition::Out,
                    tactics_core::data::AVERAGE,
                    &mut rng
                ),
                CrewFate::Killed
            ),
            "her tank came home and the wreck table killed her anyway"
        );
    }
    // ...and the wreck case is untouched by the number that spared her: a
    // safety-0 hull with every wound fatal buries whoever was inside it.
    let mut buried = 0;
    for _ in 0..200 {
        if resolve_crew_fate(
            lethal,
            &table,
            0,
            Some(DamageType::Kinetic),
            tactics_core::data::AVERAGE,
            &mut rng,
        ) == CrewFate::Killed
        {
            buried += 1;
        }
    }
    assert!(
        buried > 0,
        "nobody died in two hundred burnt-out tanks with every wound fatal"
    );

    // The mirror of it: a campaign whose wrecks are survivable and whose
    // homecomings are not. Neither number reads the other.
    let cruel_ward = Casualties {
        carried_fatal_percent: 100,
        severe_percent: 0,
        ..Casualties::default()
    };
    assert_eq!(
        resolve_station_fate(
            lethal,
            &cruel_ward,
            CrewCondition::Out,
            tactics_core::data::AVERAGE,
            &mut rng
        ),
        CrewFate::Killed
    );
    for _ in 0..200 {
        assert!(
            !matches!(
                resolve_crew_fate(
                    lethal,
                    &cruel_ward,
                    0,
                    Some(DamageType::Kinetic),
                    tactics_core::data::AVERAGE,
                    &mut rng
                ),
                CrewFate::Killed
            ),
            "no wound in this campaign is severe, and one of them was fatal"
        );
    }
}

/// A campaign map that names the same character in two crews gets her in the
/// first of them and an anonymous crew in the second.
///
/// Found by the after-action screen rather than by reading the code, which is
/// the point of having built it: `frontier` used to spread ten characters over
/// eighteen vehicles, so the campaign stamped three separate cadets all called
/// Rosa Steiner and the report listed the name three times. A roster the player
/// cannot tell apart is a roster she cannot care about, and that is the entire
/// premise of having one.
///
/// The rule belongs to `from_map`, not to any particular map, so it is fought
/// out on a fixture that over-subscribes on purpose. `frontier` itself no
/// longer does — that is
/// `every_seat_in_the_campaign_belongs_to_a_cadet_of_her_own`.
#[test]
fn nobody_crews_two_vehicles_at_once() {
    let mut reg = registry();
    let doubled: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "double_booked",
        "kind": "overworld",
        "palette": { "p": "plains" },
        "rows": ["pppp", "pppp"],
        "sides": [{ "name": "Kuhlmann Academy" }, { "name": "Iron Valkyries" }],
        "armies": [
            { "at": [0, 0], "side": 0, "name": "First", "movement": 4, "units": [
                { "at": [0, 0], "side": 0, "vehicle": "light_tank", "crew": ["anka", "rosa"] },
                { "at": [0, 0], "side": 0, "vehicle": "light_tank", "crew": ["anka", "rosa"] },
            ] },
            { "at": [3, 1], "side": 1, "name": "Theirs", "movement": 4, "units": [
                { "at": [3, 1], "side": 1, "vehicle": "light_tank", "crew": ["irma"] },
            ] },
        ],
    }))
    .expect("fixture map");
    reg.maps.insert(doubled.id.clone(), doubled);
    let state = OverworldState::from_map(&reg, "double_booked", 13).expect("overworld");

    for side in 0..2u8 {
        let mut names: Vec<&str> = state.roster.of_side(side).map(|g| g.def.as_str()).collect();
        let before = names.len();
        names.sort();
        names.dedup();
        assert_eq!(
            names.len(),
            before,
            "side {side} enlisted somebody twice: {names:?}"
        );
    }

    // ...and the vehicle whose named crew was already taken is not left
    // crewless, because a vehicle nobody is in is one nothing inside can
    // kill. It picks up an anonymous crew at the battle, exactly as a
    // placement that named nobody always has.
    let crewless = state
        .side_armies(0)
        .flat_map(|a| a.units.iter())
        .filter(|u| u.crew.is_empty())
        .count();
    assert_eq!(
        crewless, 1,
        "the second vehicle to ask for Anka should be left for an anonymous crew"
    );
}

/// Every seat in the campaign's order of battle belongs to a cadet with a name.
///
/// Two separate things are pinned here and both are content rules the engine
/// cannot enforce on a mod's behalf.
///
/// **Nobody is named twice**, because the deduplication above is a safety net
/// rather than a licence: a map that trips it silently hands a vehicle to an
/// anonymous crew, which is a worse version of what the author asked for.
///
/// **Every seat is filled**, because a crew shorter than the chassis is not a
/// cosmetic gap. Substance is counted per person aboard, so a medium tank
/// crewed by two named cadets dies roughly twice as fast as the identical tank
/// crewed by four anonymous ones — naming your characters used to be a
/// straight penalty. It also makes wounds legible: with the seats full at the
/// start of a campaign, an empty seat means somebody is in the infirmary and
/// nothing else.
#[test]
fn every_seat_in_the_campaign_belongs_to_a_cadet_of_her_own() {
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 13).expect("overworld");

    for army in &state.armies {
        for unit in &army.units {
            let vehicle = reg.vehicle(&unit.vehicle).expect("known chassis");
            assert_eq!(
                unit.crew.len(),
                vehicle.crew_slots.len(),
                "{} in {} has {} of {} seats filled",
                unit.vehicle,
                army.name,
                unit.crew.len(),
                vehicle.crew_slots.len()
            );
        }
    }
}

/// Those anonymous crews stay in the battle they were invented for.
///
/// They are enlisted into the *battle's* copy of the roster, so their handles
/// mean nothing to the campaign; handing them back with the survivors would
/// leave an army holding ids the academy cannot resolve. Not a crash — every
/// roster read simply returns nothing — which is exactly the kind of defect
/// that sits there for months, so it is pinned.
#[test]
fn a_battle_does_not_enlist_anybody_into_the_academy() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 17).expect("overworld");
    let attacker = state.side_armies(0).next().unwrap().id;
    let defender = state.side_armies(1).next().unwrap().id;

    // A survivor list of the sort a battle hands back: real cadets, plus a
    // handle from beyond the end of the campaign's roster, which is what an
    // anonymous crew member's id looks like from here.
    let stranger = tactics_core::roster::CadetId(state.roster.len() as u32 + 5);
    let mut units = state.army(attacker).unwrap().units.clone();
    units[0].crew.push(stranger);
    state.apply_battle_result(
        &reg,
        &BattleReport::of(attacker, defender, vec![(attacker, units)], Vec::new()),
    );

    for unit in &state.army(attacker).unwrap().units {
        for cadet in &unit.crew {
            assert!(
                state.roster.get(*cadet).is_some(),
                "{cadet:?} is in an army and in nobody's academy"
            );
        }
    }
}

// --- the skills a crew is asked for ----------------------------------------
//
// Four of the base mod's thirteen skills were declared and consulted by
// nothing at all, which is the same dead weight `morale` and `leadership`
// were in the old `CrewStats`, one level up. These are the rules that ask
// for them. Each test is a check that the *right* skill is read — the
// failure they guard against is not a wrong coefficient but a rule quietly
// asking for somebody else's trade, which is exactly what `move_points` was
// doing to every platoon in the game.

/// A platoon and a tank across a bare field, with every cadet in the mod
/// trained to `level` at `skill`.
///
/// Skills are stamped onto a cadet when the roster is built, so a stage that
/// wants to vary one has to say so before it stages, not after.
fn trained(reg: &DataRegistry, skill: &str, level: i32) -> DataRegistry {
    let mut reg = reg.clone();
    for def in reg.characters.values_mut() {
        def.skills.insert(skill.to_string(), level);
    }
    reg
}

/// What a platoon walks at is her chassis's, and no skill of anybody's.
///
/// It was `driving` for years, and a `rifle_platoon` fields a platoon leader
/// and a section leader and no driver — so `crew_skill` took its "nobody is
/// in that seat" path and charged a platoon `substitution_penalty` for a seat
/// her chassis has never had. The first fix pointed it at `athletics`
/// instead, and that could not express anything either: a foot unit has one
/// movement point, one hex a round already *is* walking pace, and no
/// percentage of one rounds to anything else below 80 a point. So her pace is
/// the chassis's listed allowance, `athletics` buys steep ground instead, and
/// this test is the guard that neither skill creeps back into it.
#[test]
fn what_a_platoon_walks_at_is_her_chassiss_and_nobodys_skill() {
    let reg = trained(&registry_wireless(), "athletics", 18);
    let row = "g".repeat(12);
    let mut reg = seen(reg);
    let state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            crewed(
                unit_at([1, 1], 0, "rifle_platoon", "Platoon"),
                &["anka", "juno"],
            ),
            unit_at([10, 1], 1, "medium_tank", "Theirs"),
        ],
        21,
    );
    let platoon = state.unit(UnitId(0)).expect("she is on the field");
    let listed = reg
        .vehicle(&platoon.vehicle)
        .map(|v| v.movement.points)
        .expect("a chassis the mod ships");
    let pace = |reg: &DataRegistry| {
        tactics_core::battle::move_points(
            reg,
            &state.roster,
            platoon,
            state.terrain_at(platoon.pos),
        )
    };

    assert_eq!(pace(&reg), listed, "she walks at what her chassis lists");
    reg.balance.speed_per_driving = 200;
    assert_eq!(
        pace(&reg),
        listed,
        "a commander who could drive a tank round the world buys a platoon \
         nothing, because nobody in a platoon is driving anything"
    );
    reg.balance.substitution_penalty = 40;
    assert_eq!(
        pace(&reg),
        listed,
        "and she is charged nothing for the driver's seat her chassis has \
         never had"
    );
}

/// A fit platoon goes over the face the rest of the army drives round.
///
/// The one thing infantry can have that no chassis can buy. It is a
/// threshold rather than a percentage because a level is a level and a foot
/// unit's whole allowance is one movement point, so there is nothing for a
/// percentage to land on. Note what the stage has to build: no shipped
/// battle map has an adjacent step steeper than two, and foot units already
/// climb two, so this rule cannot bite on any ground the game currently
/// ships — the relief here is the test's own.
#[test]
fn a_fit_platoon_climbs_a_face_that_turns_a_tank_back() {
    let mut reg = trained(&registry_wireless(), "athletics", 18);
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "a_steep_face",
        "palette": { "g": "grass" },
        "rows": ["ggg", "ggg", "ggg"],
        "elevation": ["000", "030", "000"],
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let placements = vec![
        crewed(
            unit_at([0, 1], 0, "rifle_platoon", "Platoon"),
            &["anka", "juno"],
        ),
        crewed(unit_at([2, 1], 0, "medium_tank", "Ours"), &["elsa", "ada"]),
        unit_at([2, 2], 1, "medium_tank", "Theirs"),
    ];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(&reg, &placements);
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
    let state = BattleState::from_placements(
        &reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        23,
    )
    .expect("the staged placements are content the base mod ships");
    let crest = tactics_core::offset_to_hex(1, 1);

    reg.balance.athletics_per_climb_level = 0;
    assert!(
        !tactics_core::battle::reachable(&reg, &state, UnitId(0)).contains_key(&crest),
        "three levels is past what her chassis allows, and with the rule off that is that"
    );

    // Eight points of athletics above average, four to a level: two levels
    // more than the two her chassis lists, and the face is four.
    reg.balance.athletics_per_climb_level = 4;
    assert!(
        tactics_core::battle::reachable(&reg, &state, UnitId(0)).contains_key(&crest),
        "a platoon this fit takes the face on her feet"
    );
    assert!(
        !tactics_core::battle::reachable(&reg, &state, UnitId(1)).contains_key(&crest),
        "and what a tank can climb is a fact about her suspension, whoever is aboard"
    );
}

/// Fieldcraft multiplies what her chassis gives her, and nothing is still
/// nothing.
///
/// Concealment is the chassis's — how close somebody has to be to find her
/// at all — and fieldcraft is how much of it she actually gets. Both halves
/// are the rule: a platoon that lies still well is found later, and a tank,
/// which declares no concealment at all in the base mod, gets nothing for
/// it however good her crew is. That second half is not pedantry. It went in
/// as a plain multiply and `Balance::scaled` floors at one, so every tank on
/// the field came out with a concealment of 1, `fog::search`'s fast path
/// stopped firing and the determinism baseline moved.
#[test]
fn fieldcraft_multiplies_what_her_chassis_gives_her_and_nothing_stays_nothing() {
    let mut reg = seen(trained(&registry_wireless(), "fieldcraft", 18));
    reg.balance.concealment_per_fieldcraft = 40;
    assert_eq!(
        reg.balance.concealed(0, 18),
        0,
        "a chassis with nothing to hide behind gets nothing for lying still well"
    );
    assert!(
        reg.balance.concealed(40, 18) > 40,
        "and a platoon who has something to hide behind gets more of it"
    );

    let row = "g".repeat(12);
    let seat = |reg: &DataRegistry| {
        let state = two_side_battle(
            reg,
            &[&row, &row, &row],
            vec![
                crewed(
                    unit_at([1, 1], 0, "rifle_platoon", "Platoon"),
                    &["anka", "juno"],
                ),
                unit_at([4, 1], 1, "medium_tank", "Theirs"),
            ],
            29,
        );
        state.fog.side(1).spotted.contains(&UnitId(0))
    };

    reg.balance.concealment_per_fieldcraft = 0;
    assert!(
        seat(&reg),
        "at three hexes on bare grass, a platoon who is not trying is in plain sight"
    );
    reg.balance.concealment_per_fieldcraft = 40;
    assert!(
        !seat(&reg),
        "the same platoon, lying still properly, is not found from there at all"
    );
}

/// A platoon's marksmanship reaches her damage and deliberately not her
/// suppression.
///
/// `mustered` already scales a round by the riflemen still standing; this is
/// how well the ones still standing shoot, charged in the same place so both
/// arrive by the same route. Volume of fire is what frightens a crew, and
/// volume is the first term — a section that has lost half its riflemen puts
/// out half of it whether the survivors are marksmen or not.
#[test]
fn a_platoons_marksmanship_reaches_her_damage_and_not_her_suppression() {
    let mut reg = seen(trained(&registry_wireless(), "small_arms", 18));
    let row = "g".repeat(12);
    let state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            crewed(
                unit_at([1, 1], 0, "rifle_platoon", "Platoon"),
                &["anka", "juno"],
            ),
            crewed(
                unit_at([3, 1], 1, "rifle_platoon", "Theirs"),
                &["mina", "rosa"],
            ),
        ],
        31,
    );
    let her = state.unit(UnitId(0)).expect("she is on the field");
    let weapon = registry_weapon(&reg, her);
    let target = state.unit(UnitId(1)).expect("he is on the field");

    let damage = |reg: &DataRegistry| {
        tactics_core::battle::expected_damage(
            reg,
            &state,
            UnitId(0),
            her.pos,
            &weapon,
            UnitId(1),
            target.pos,
            false,
        )
    };
    let pressure = |reg: &DataRegistry| {
        tactics_core::battle::expected_pressure(
            reg,
            &state,
            UnitId(0),
            her.pos,
            &weapon,
            UnitId(1),
            target.pos,
            false,
        )
    };

    reg.balance.troops_per_small_arms = 0;
    let (plain_damage, plain_pressure) = (damage(&reg), pressure(&reg));
    assert!(
        plain_damage > 0.0,
        "the stage needs a shot that does something: {plain_damage}"
    );

    reg.balance.troops_per_small_arms = 40;
    assert!(
        damage(&reg) > plain_damage,
        "riflemen who can shoot do more with the same rifles: {} against {plain_damage}",
        damage(&reg)
    );
    assert_eq!(
        pressure(&reg),
        plain_pressure,
        "and what frightens the crew they are shooting at is the volume, not the aim"
    );
}

/// The same placement with named cadets in its seats.
///
/// An anonymous crew answers [`crate::common`]'s average for every skill, so
/// a test about a skill has to put people in the seats or it is measuring the
/// fallback.
fn crewed(mut placement: UnitPlacement, crew: &[&str]) -> UnitPlacement {
    placement.crew = crew.iter().map(|id| (*id).to_string()).collect();
    placement
}

/// The weapon a crew's own chassis carries, by index zero.
fn registry_weapon(
    reg: &DataRegistry,
    unit: &tactics_core::battle::Unit,
) -> tactics_core::data::WeaponDef {
    let vehicle = reg.vehicle(&unit.vehicle).expect("a chassis the mod ships");
    let id = vehicle.weapons.first().expect("a chassis that shoots");
    reg.weapon(id).expect("a weapon the mod ships").clone()
}

/// Somebody beside her has to know what to do about it.
///
/// `first_aid` was one of the skills no rule read, and the seat that answers
/// for it is the loader's — which is why leaving a loader behind was free.
/// What it buys is severity rather than days: a medic decides whether a cadet
/// is buried or out for a fortnight, which is the stake the muster screen was
/// given teeth for. Her *own* first aid is deliberately not in it. She is the
/// one bleeding.
#[test]
fn what_saves_her_is_a_crewmate_who_knows_what_to_do_and_never_her_own_hands() {
    let mut reg = registry_wireless();
    // One cadet aboard who can do something about a wound, and she is not the
    // one who is going to be hurt.
    for def in reg.characters.values_mut() {
        def.skills.insert("first_aid".to_string(), 4);
    }
    if let Some(medic) = reg.characters.get_mut("juno") {
        medic.skills.insert("first_aid".to_string(), 20);
    }
    let (mut state, ours) = crewed_stage(&reg, &["anka", "juno", "mina"]);
    let hurt = state.unit_mut(ours).unwrap();
    hurt.crew_state = vec![
        tactics_core::battle::CrewCondition::Out,
        tactics_core::battle::CrewCondition::Fine,
        tactics_core::battle::CrewCondition::Fine,
    ];

    let losses = tactics_core::overworld::CrewLoss::in_battle(&reg, &state);
    let hers = losses
        .iter()
        .find(|loss| loss.cadet == state.unit(ours).unwrap().crew[0])
        .expect("the cadet carried out of her seat is a casualty of this battle");
    assert_eq!(
        hers.aid, 20,
        "the best first aid still working beside her is what she gets"
    );

    // And a crew with nobody but her in it gets ordinary care rather than a
    // penalty: the rule's absence is the game before it.
    let (alone, one) = crewed_stage(&reg, &["juno"]);
    let mut alone = alone;
    alone.unit_mut(one).unwrap().crew_state = vec![tactics_core::battle::CrewCondition::Out];
    let lonely = tactics_core::overworld::CrewLoss::in_battle(&reg, &alone);
    assert_eq!(
        lonely.first().map(|loss| loss.aid),
        Some(tactics_core::data::AVERAGE),
        "a cadet with nobody left aboard is neither helped nor punished for it"
    );
}

/// A medic aboard buries fewer cadets, and a bad one buries no more.
///
/// The one-sidedness is not tidiness. Most of the roster is untrained in
/// `first_aid` and an untrained skill sits five points under its core base,
/// so a two-sided rule would have made the shipped campaign bury *more*
/// cadets the moment it was switched on — measured at `severe_per_aid` 5,
/// `buried` went up 0.30 a battle instead of down. That is
/// `untrained_penalty` reaching the casualty table through a side door.
#[test]
fn a_crewmate_who_knows_what_to_do_buries_fewer_and_one_who_does_not_buries_no_more() {
    use tactics_core::roster::{CasualtyRules, CrewFate, resolve_crew_fate};
    let mut reg = registry_wireless();
    reg.casualties.severe_per_aid = 5;
    let lethal = CasualtyRules { permadeath: true };
    let buried = |aid: i32, seed: u64| {
        let mut rng = <rand_chacha::ChaCha8Rng as rand::SeedableRng>::seed_from_u64(seed);
        (0..2000)
            .filter(|_| {
                matches!(
                    resolve_crew_fate(
                        lethal,
                        &reg.casualties,
                        0,
                        Some(tactics_core::data::DamageType::Kinetic),
                        aid,
                        &mut rng,
                    ),
                    CrewFate::Killed
                )
            })
            .count()
    };
    let ordinary = buried(tactics_core::data::AVERAGE, 5);
    assert!(
        buried(tactics_core::data::AVERAGE + 4, 5) < ordinary,
        "a crew with a medic aboard buries fewer of her own"
    );
    assert_eq!(
        buried(tactics_core::data::AVERAGE - 6, 5),
        ordinary,
        "and a crew with nobody who knows what to do buries no more than one \
         the rule was never switched on for"
    );
}

/// A crew with somebody who knows the vehicle gets a broken thing working
/// again, and a mod that declines the rule throws no die at all.
///
/// `maintenance` had nothing to reach: there was no breakdown rule and no
/// repair rule anywhere in the engine, so wiring the skill meant inventing
/// one. The seat that answers for it is the driver's, which is the
/// consequence worth having — a tank that has lost her driver is also a tank
/// nobody can get the tracks back on.
///
/// The second half is the additivity contract and it is checked down to the
/// rng stream: at `field_repair_percent` zero no die is thrown, so a mod that
/// declines this rule is the game before it existed rather than a gentler
/// version of it.
#[test]
fn a_crew_who_knows_the_vehicle_gets_a_broken_thing_working_again() {
    let mut reg = registry_wireless();
    let (mut state, ours) = crewed_stage(&reg, &["anka", "juno", "mina"]);
    // Break something, and give her a round to get it back.
    let broken = {
        let her = state.unit_mut(ours).unwrap();
        let id = her
            .modules
            .keys()
            .next()
            .expect("a medium tank carries something that can break")
            .clone();
        *her.modules.get_mut(&id).unwrap() = 0;
        id
    };

    let played = |reg: &tactics_core::data::DataRegistry, state: &BattleState| {
        let mut state = state.clone();
        let events = play_round(reg, &mut state);
        let repaired = events
            .iter()
            .any(|e| matches!(e, BattleEvent::ModuleRepaired { .. }));
        (repaired, state.rng.clone())
    };

    reg.balance.field_repair_percent = 0;
    reg.balance.repair_per_maintenance = 0;
    let (never, untouched) = played(&reg, &state);
    assert!(!never, "a mod that declines the rule mends nothing");

    // The rng stream is the real check: the absence must not cost a draw, or
    // every battle in a mod that switched this off would resolve differently
    // from the one that never had it.
    reg.balance.field_repair_percent = 0;
    reg.balance.repair_per_maintenance = 50;
    let (_, still_untouched) = played(&reg, &state);
    assert_eq!(
        format!("{untouched:?}"),
        format!("{still_untouched:?}"),
        "with the chance at zero, no die is thrown however skilled the crew"
    );

    reg.balance.field_repair_percent = 100;
    let (mended, _) = played(&reg, &state);
    assert!(
        mended,
        "a certainty gets the {broken} working again inside a round"
    );
    let mut state = state;
    play_round(&reg, &mut state);
    assert_eq!(
        state.unit(ours).unwrap().modules.get(&broken).copied(),
        Some(1),
        "and working again is working, not as new"
    );
}

/// A quick loader fires more often, and everything that prices a round of
/// fire believes the same thing about how often.
///
/// The trap this rule had in it: a reload reaches the game twice, once as the
/// cooldown the resolver sets after a shot and once as the cadence every
/// price in the currency is quoted per round of. Wiring only the cooldown
/// would have left the evaluator, `danger::fire_on` and the player's danger
/// overlay all quoting a rate nobody achieves — the same drift
/// `edge_cost` and `MoveGrid::cost` avoid by sharing `step_cost`.
#[test]
fn a_quick_loader_fires_more_often_and_every_price_knows_it() {
    let mut reg = seen(registry_wireless());
    for def in reg.characters.values_mut() {
        def.skills.insert("loading".to_string(), 18);
    }
    // Four, because a medium tank has four seats and the fourth is the
    // loader's: crewing three of them leaves the seat that answers for
    // `loading` empty, and a stand-in pays `substitution_penalty` for it —
    // which is the rule working, and not what this test is about.
    let (state, ours) = crewed_stage(&reg, &["anka", "juno", "mina", "rosa"]);
    let her = state.unit(ours).expect("she is on the field");
    let weapon = registry_weapon(&reg, her);

    reg.balance.reload_per_loading = 0;
    let listed = tactics_core::battle::crewed_reload(&reg, &state, ours, &weapon);
    assert_eq!(
        listed,
        weapon.reload(&reg.scale),
        "with the rule off, a reload is the datasheet's and nobody else's"
    );

    reg.balance.reload_per_loading = 5;
    let quick = tactics_core::battle::crewed_reload(&reg, &state, ours, &weapon);
    assert!(
        quick < listed,
        "a loader this good gets the next round in sooner: {quick} against {listed}"
    );

    // And the cadence the currency is quoted in follows it, which is the half
    // that would have drifted.
    let target = state.unit(UnitId(1)).expect("somebody to shoot at");
    let cadence = |reg: &tactics_core::data::DataRegistry| {
        tactics_core::battle::expected_shot(
            reg,
            &state,
            ours,
            her.pos,
            &weapon,
            UnitId(1),
            target.pos,
            false,
        )
        .shots
    };
    reg.balance.reload_per_loading = 0;
    let slow = cadence(&reg);
    reg.balance.reload_per_loading = 5;
    assert!(
        cadence(&reg) > slow,
        "the price of a round of fire is quoted at the rate her loader \
         actually achieves: {} against {slow}",
        cadence(&reg)
    );
}
