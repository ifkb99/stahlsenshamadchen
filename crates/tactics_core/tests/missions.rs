//! Standing orders that outlive a single battle, and what a crew does
//! with one she has decided not to obey.
//!
//! Campaign missions are the overworld's half of the order system — orders
//! that outlive the map they were given on — and a `Goal` is a battle's own
//! intention, kept across rounds the same way. Defiance is what a crew on a
//! rung whose `obeys` is false does instead, and missions steering units is
//! the executor that turns any of the above into a route. The four sections
//! share the same object, an order or an intention with a lifetime longer
//! than a tick, at two different scales (the campaign map and the battle).
//!
//! - campaign missions: orders that outlive the map
//! - defiance: what a crew does instead
//! - goals: an intention that outlives a round
//! - missions steering units

use tactics_core::ai::{
    AiConfig, AiDriver, AiPlanner, Evaluator, UtilityPlanner, make_battle_planner,
};
use tactics_core::battle::{
    BattleState, Destruction, EndReason, Event as BattleEvent, Fate, FireIntent, FormationId,
    Mission, Order, SideState, UnitId, reachable,
};
use tactics_core::data::DataRegistry;
use tactics_core::map::{HexMap, UnitPlacement};
use tactics_core::overworld::{
    Army, ArmyId, ArmyMission, BattleReport, OverworldError, OverworldEvent, OverworldOrder,
    OverworldState,
};

mod common;
use common::{
    always, breaking, crewed_stage, curtained_pair, duel, formation_named, maul, objective_battle,
    play_round, registry, registry_wireless, seen, sharp_planner, soften, standoff, strike_down,
    two_side_battle, unit_at,
};

// --- campaign missions: orders that outlive the map ------------------------

/// The campaign under a stated radio net. The base mod's own figure is four
/// overworld hexes; these tests state their own so that what they are about is
/// the rule rather than the tuning.
fn registry_with_net(radius: u32, relay: bool) -> DataRegistry {
    let mut reg = registry();
    let mut rules = reg.command.clone().unwrap_or_default();
    rules.overworld_radius = radius;
    rules.relay = relay;
    reg.command = Some(rules);
    reg
}

/// Give a side a third company, so a chain of armies can be strung out across
/// the map. It fields nothing: what these tests weigh is where an army *is*,
/// and a battle is not one of the things that can happen to it.
fn extra_army(state: &mut OverworldState, side: u8, name: &str, at: [i32; 2]) -> ArmyId {
    let id = ArmyId(state.armies.len() as u32);
    state.armies.push(Army {
        id,
        side,
        name: name.into(),
        pos: tactics_core::offset_to_hex(at[0], at[1]),
        movement: 3,
        moved: false,
        units: Vec::new(),
        alive: true,
        mission: None,
        headquarters: false,
    });
    id
}

/// Push the campaign round to the next turn of `side`, so contact is
/// recomputed against wherever everybody now stands.
fn next_turn_of(reg: &DataRegistry, state: &mut OverworldState, side: u8) -> Vec<OverworldEvent> {
    let mut events = Vec::new();
    for _ in 0..8 {
        events.extend(
            state
                .apply(reg, &OverworldOrder::EndTurn)
                .expect("end turn"),
        );
        if state.active_side == side {
            break;
        }
    }
    events
}

#[test]
fn an_army_mission_is_stored_and_said_out_loud() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let army = state.senior_army(0).expect("side 0 has armies");
    let bridge = tactics_core::offset_to_hex(6, 2);

    let events = state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Advance { to: bridge },
            },
        )
        .expect("her own army, on her own turn, within her own net");
    assert!(
        events.iter().any(|e| matches!(
            e,
            OverworldEvent::ArmyMissionAssigned { army: a, mission: ArmyMission::Advance { to } }
                if *a == army && *to == bridge
        )),
        "a decision somebody made is news: {events:?}"
    );
    assert_eq!(
        state.army(army).unwrap().mission,
        Some(ArmyMission::Advance { to: bridge })
    );

    // Countermanding is ordinary business and replaces silently.
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Hold,
            },
        )
        .expect("orders may be changed");
    assert_eq!(state.army(army).unwrap().mission, Some(ArmyMission::Hold));

    // Ground that is not there is refused, and so is somebody else's army.
    assert_eq!(
        state.apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Advance {
                    to: tactics_core::offset_to_hex(400, 400)
                },
            },
        ),
        Err(OverworldError::NotOnMap)
    );
    let enemy = state.senior_army(1).expect("side 1 has armies");
    assert_eq!(
        state.apply(
            &reg,
            &OverworldOrder::SetMission {
                army: enemy,
                mission: ArmyMission::Hold,
            },
        ),
        Err(OverworldError::NotYourTurn)
    );
}

#[test]
fn an_army_mission_out_of_range_waits_and_then_transmits() {
    // frontier's two companies per side start six hexes apart, so a two-hex
    // net with nobody relaying leaves the junior one on its own. An order for
    // her is *not* refused — it waits at headquarters and goes out on the
    // first morning the wire is up, which is the whole of this chunk on the
    // campaign side.
    let reg = registry_with_net(2, false);
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let senior = state.senior_army(0).unwrap();
    let junior = state
        .side_armies(0)
        .map(|a| a.id)
        .find(|id| *id != senior)
        .expect("frontier gives side 0 two companies");
    assert!(!state.in_contact(junior), "she is six hexes from anybody");
    assert!(state.in_contact(senior), "headquarters hears itself");

    let order = OverworldOrder::SetMission {
        army: junior,
        mission: ArmyMission::Hold,
    };
    let queued = state.apply(&reg, &order).expect("accepted, not refused");
    assert!(
        queued
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyOrdersWaiting { army } if *army == junior)),
        "an order parked without a word would be as bad as one dropped: {queued:?}"
    );
    assert_eq!(
        state.army(junior).unwrap().mission,
        None,
        "she has not been told anything yet"
    );
    assert_eq!(
        state.waiting_missions,
        vec![(junior, ArmyMission::Hold)],
        "it is sitting in the tray"
    );

    // A second order replaces the first rather than queueing behind it: only
    // one of them was ever going to be transmitted.
    let to = tactics_core::offset_to_hex(6, 2);
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army: junior,
                mission: ArmyMission::Advance { to },
            },
        )
        .expect("accepted too");
    assert_eq!(
        state.waiting_missions,
        vec![(junior, ArmyMission::Advance { to })],
        "the newer order is the one headquarters means"
    );

    // Closing up is what fixes it, and the campaign says so when it does —
    // then the order transmits, in that order, as an ordinary assignment.
    let beside = state.army(senior).unwrap().pos + hexx::Hex::new(1, 0);
    state.army_mut(junior).unwrap().pos = beside;
    let events = next_turn_of(&reg, &mut state, 0);
    let restored = events
        .iter()
        .position(|e| matches!(e, OverworldEvent::ArmyContactRestored { army } if *army == junior))
        .expect("coming back on the net is news too");
    let assigned = events
        .iter()
        .position(
            |e| matches!(e, OverworldEvent::ArmyMissionAssigned { army, .. } if *army == junior),
        )
        .expect("and the order she could not be given lands with it");
    assert!(
        restored < assigned,
        "the wire comes back before anything goes down it: {events:?}"
    );
    assert_eq!(
        state.army(junior).unwrap().mission,
        Some(ArmyMission::Advance { to }),
        "and it is the order she was actually given"
    );
    assert!(
        state.waiting_missions.is_empty(),
        "nothing is transmitted twice"
    );

    // With no command block there is no net to be outside of: the same order,
    // from the same six hexes away, is simply an order, landing at once and
    // never touching the queue.
    let wireless = registry_wireless();
    let mut open = OverworldState::from_map(&wireless, "frontier", 1).unwrap();
    assert!(open.out_of_contact.is_empty(), "nothing was ever computed");
    open.apply(&wireless, &order)
        .expect("a campaign with no radios has no radio range");
    assert_eq!(open.army(junior).unwrap().mission, Some(ArmyMission::Hold));
    assert!(open.waiting_missions.is_empty());
}

#[test]
fn relay_carries_orders_through_a_chain_of_armies() {
    // Three companies in a line, each three hexes from the next: the far one
    // is six from headquarters and can only be reached through the middle.
    let build = |reg: &DataRegistry| {
        let mut state = OverworldState::from_map(reg, "frontier", 1).unwrap();
        let senior = state.senior_army(0).unwrap();
        state.army_mut(senior).unwrap().pos = tactics_core::offset_to_hex(1, 1);
        let middle = extra_army(&mut state, 0, "3rd Company", [4, 1]);
        let far = extra_army(&mut state, 0, "4th Company", [7, 1]);
        // Recomputed at the top of a turn, so give it one.
        let events = next_turn_of(reg, &mut state, 0);
        (state, middle, far, events)
    };

    let relaying = registry_with_net(3, true);
    let (state, middle, far, _) = build(&relaying);
    assert!(state.in_contact(middle), "she is three hexes out");
    assert!(
        state.in_contact(far),
        "and she is three hexes from her, which is what relaying is for"
    );

    let alone = registry_with_net(3, false);
    let (mut state, middle, far, events) = build(&alone);
    assert!(state.in_contact(middle), "still inside the net herself");
    assert!(
        !state.in_contact(far),
        "with nobody passing the signal on, six hexes is six hexes"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyOutOfContact { army } if *army == far)),
        "an army the player cannot order must be told about: {events:?}"
    );
    // Her orders are taken and held rather than refused; what relaying buys
    // is that they go out today instead of whenever she closes up.
    state
        .apply(
            &alone,
            &OverworldOrder::SetMission {
                army: far,
                mission: ArmyMission::Hold,
            },
        )
        .expect("accepted, and waiting for a wire");
    assert_eq!(state.waiting_missions, vec![(far, ArmyMission::Hold)]);
    assert_eq!(state.army(far).unwrap().mission, None);
}

#[test]
fn a_standing_mission_moves_the_army_when_its_turn_ends() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let army = state.senior_army(0).unwrap();
    // Along the northern highway, well clear of the enemy: this test is about
    // orders being carried out, not about what happens when they meet
    // somebody.
    let target = tactics_core::offset_to_hex(6, 2);
    let start = state.army(army).unwrap().pos;
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Advance { to: target },
            },
        )
        .unwrap();

    let events = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyMoved { army: a, .. } if *a == army)),
        "nobody ordered her anywhere this turn and she went anyway: {events:?}"
    );
    let after_one = state.army(army).unwrap().pos;
    assert_ne!(after_one, start, "she set off");

    // ...and keeps going, day after day, until she is standing on it.
    let mut days = 1;
    while state.army(army).unwrap().pos != target && days < 12 {
        next_turn_of(&reg, &mut state, 0);
        state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
        days += 1;
    }
    assert_eq!(
        state.army(army).unwrap().pos,
        target,
        "she should have arrived within {days} days"
    );
    assert!(days > 1, "or this test proves nothing about the days after");

    // Arrived is arrived: the order stands, and standing on it is obeying it.
    next_turn_of(&reg, &mut state, 0);
    let events = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyMoved { army: a, .. } if *a == army)),
        "she is already there: {events:?}"
    );
    assert!(
        state.army(army).unwrap().mission.is_some(),
        "and she is still under orders, not released from them"
    );
}

#[test]
fn a_hand_moved_army_is_not_second_guessed_by_its_mission() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let army = state.senior_army(0).unwrap();
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Advance {
                    to: tactics_core::offset_to_hex(6, 2),
                },
            },
        )
        .unwrap();

    // The player has changed her mind today, and hers is the newer decision.
    let elsewhere = tactics_core::offset_to_hex(1, 3);
    state
        .apply(
            &reg,
            &OverworldOrder::MoveArmy {
                army,
                to: elsewhere,
            },
        )
        .unwrap();
    let by_hand = state.army(army).unwrap().pos;

    let events = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyMoved { army: a, .. } if *a == army)),
        "her turn was already spent: {events:?}"
    );
    assert_eq!(state.army(army).unwrap().pos, by_hand);
    assert!(
        state.army(army).unwrap().mission.is_some(),
        "the standing order survives the day it was overruled"
    );
}

/// One company on the trunk road with an enemy standing on it three hexes
/// ahead, and open ground beyond that to be ordered onto.
///
/// `frontier`'s row 2 is an unbroken highway from column 2 to column 11 with
/// plains either side, so the enemy blocks the direct road without walling it
/// off — which is the interesting case. A march that means to avoid contact can
/// go round; the question these tests ask is which marches take that offer.
///
/// Wireless, because what is being weighed is what an order *does*, not whether
/// headquarters could get it out.
fn blocked_road(reg: &DataRegistry) -> (OverworldState, ArmyId, ArmyId, hexx::Hex) {
    let mut state = OverworldState::from_map(reg, "frontier", 1).unwrap();
    let army = state.senior_army(0).unwrap();
    let blocker = state.senior_army(1).unwrap();
    state.army_mut(army).unwrap().pos = tactics_core::offset_to_hex(2, 2);
    state.army_mut(blocker).unwrap().pos = tactics_core::offset_to_hex(5, 2);
    (state, army, blocker, tactics_core::offset_to_hex(9, 2))
}

#[test]
fn a_standing_advance_engages_the_army_blocking_its_road() {
    // The bug this pins: an advance aimed at ground *beyond* an enemy used to
    // treat that enemy as a wall, so it either detoured around him or halted
    // beside him and stood there for the rest of the campaign, because the only
    // thing that ever started a battle was an enemy sitting on the tile the
    // order named. An operational advance is movement to contact. It fights
    // what is in the road.
    let reg = registry_wireless();
    let (mut state, army, blocker, target) = blocked_road(&reg);
    let at = state.army(blocker).unwrap().pos;
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Advance { to: target },
            },
        )
        .unwrap();

    let events = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    assert!(
        events.iter().any(|e| matches!(
            e,
            OverworldEvent::BattleTriggered { attacker, defender, at: hex }
                if *attacker == army && *defender == blocker && *hex == at
        )),
        "she was told to take the road and somebody is on it: {events:?}"
    );
    assert_eq!(
        state.army(army).unwrap().pos.distance_to(at),
        1,
        "and she is up against him, not on him — the battle decides the tile"
    );
    // One move, one battle. Nothing walks past the enemy it just found to go
    // looking for the next one.
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, OverworldEvent::BattleTriggered { .. }))
            .count(),
        1,
        "{events:?}"
    );
    assert_eq!(
        state.army(army).unwrap().mission,
        Some(ArmyMission::Advance { to: target }),
        "the order stands; the battle is how she carries it out"
    );
}

#[test]
fn a_standing_withdraw_goes_round_the_enemy_rather_than_through_him() {
    // The same road and the same enemy, under the opposite order. An army
    // falling back is trying to be somewhere else; one that started a battle on
    // the way out would be obeying the reverse of what it was told. So a
    // withdrawal keeps the avoid semantics a hand order has — the enemy is a
    // wall, it goes round him if there is a way round, and it does not go
    // anywhere at all if there is not.
    //
    // On this road there is a way round — plains either side of the highway —
    // and she takes it, ending the day beside him without a shot. Adjacency is
    // not contact; the order is.
    let reg = registry_wireless();
    let (mut state, army, blocker, target) = blocked_road(&reg);
    let at = state.army(blocker).unwrap().pos;
    let start = state.army(army).unwrap().pos;
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Withdraw { to: target },
            },
        )
        .unwrap();

    let events = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OverworldEvent::BattleTriggered { .. })),
        "a withdrawal does not pick fights: {events:?}"
    );
    let ended = state.army(army).unwrap().pos;
    assert_ne!(ended, at, "and it certainly does not drive through him");
    assert_ne!(
        ended, start,
        "she is not stuck either — the enemy is a wall, not a full stop"
    );
    assert_eq!(
        state.army(army).unwrap().mission,
        Some(ArmyMission::Withdraw { to: target }),
        "the order survives the day, as every standing order does"
    );
}

#[test]
fn a_hand_ordered_march_past_an_enemy_is_not_a_declaration_of_war() {
    // The player's click is not an order to attack. She may put a company on
    // the tile in front of an enemy to hold a line, or send it somewhere the
    // short way happens to run past him, and neither is a decision to fight
    // today. Only pointing *at* the enemy is that. This is the half of the
    // ruling that the advance change must not quietly take away.
    let reg = registry_wireless();
    let (mut state, army, blocker, target) = blocked_road(&reg);
    let at = state.army(blocker).unwrap().pos;

    // Straight up to his front bumper and stop.
    let beside = tactics_core::offset_to_hex(4, 2);
    let events = state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: beside })
        .unwrap();
    assert_eq!(state.army(army).unwrap().pos, beside);
    assert_eq!(state.army(army).unwrap().pos.distance_to(at), 1);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OverworldEvent::BattleTriggered { .. })),
        "standing next to somebody is not attacking him: {events:?}"
    );

    // And a march aimed at ground on the far side of him is a march, not an
    // assault: it finds its own way there and starts nothing on the way.
    next_turn_of(&reg, &mut state, 0);
    let events = state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: target })
        .unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OverworldEvent::BattleTriggered { .. })),
        "the ordered destination was empty ground: {events:?}"
    );
    assert_ne!(state.army(army).unwrap().pos, at);

    // Pointing at him, on the other hand, is exactly that — unchanged.
    next_turn_of(&reg, &mut state, 0);
    let events = state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: at })
        .unwrap();
    assert!(
        events.iter().any(|e| matches!(
            e,
            OverworldEvent::BattleTriggered { attacker, defender, at: hex }
                if *attacker == army && *defender == blocker && *hex == at
        )),
        "she was aimed at him: {events:?}"
    );
}

#[test]
fn a_withdrawing_army_fights_its_battle_toward_the_exit() {
    let reg = registry_wireless();
    let file = reg.map("river_crossing").expect("shipped battle map");
    let map = HexMap::from_map_file(file).expect("map parses");

    // Two companies facing each other along the trunk road, each one a
    // formation, exactly as the campaign's `deploy` assembles them.
    let placement = |col: i32, side: u8, formation: &str, leads: bool| UnitPlacement {
        aboard_at: None,
        at: [col, 20],
        side,
        vehicle: "medium_tank".into(),
        crew: Vec::new(),
        name: Some(format!("{formation}-{col}")),
        facing: None,
        formation: Some(formation.into()),
        leads,
    };
    let placements = vec![
        placement(10, 0, "kuhlmann_armor", true),
        placement(11, 0, "kuhlmann_armor", false),
        placement(30, 1, "valkyrie_line", true),
        placement(31, 1, "valkyrie_line", false),
    ];
    let sides = vec![
        SideState {
            name: "Kuhlmann".into(),
            ai: None,
        },
        SideState {
            name: "Valkyries".into(),
            ai: None,
        },
    ];
    let mut state = BattleState::from_placements(
        &reg,
        map,
        sides,
        &placements,
        &[Vec::new(), Vec::new(), Vec::new(), Vec::new()],
        std::sync::Arc::new(tactics_core::roster::Roster::new()),
        11,
    )
    .expect("the staged placements are content the base mod ships");

    // The helper first, on its own terms: each side is sent down its own
    // road, and a side the map offers no lane to is sent nowhere.
    let west = state.side_units(0).next().unwrap().pos;
    let east = state.side_units(1).next().unwrap().pos;
    assert_eq!(
        tactics_core::battle::nearest_exit(&state, 0, west).as_deref(),
        Some("west_road")
    );
    assert_eq!(
        tactics_core::battle::nearest_exit(&state, 1, east).as_deref(),
        Some("east_road"),
        "a lane belongs to the side that was given it, however close the other is"
    );
    assert_eq!(
        tactics_core::battle::nearest_exit(&state, 2, west),
        None,
        "a side with no road home holds where it stands"
    );

    // Now the campaign's order, as the field-battle setup issues it: every
    // formation of the withdrawing side, out by its nearest lane.
    let index = state
        .formations()
        .iter()
        .position(|f| f.side == 1)
        .expect("side 1 fields a formation");
    let via = tactics_core::battle::nearest_exit(&state, 1, east).unwrap();
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: FormationId(index as u32),
                mission: Mission::Withdraw { via: via.clone() },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("her own lane");

    let lane: Vec<tactics_core::Hex> = state
        .map
        .objectives()
        .iter()
        .find(|o| o.id == via)
        .unwrap()
        .hexes
        .clone();
    let mut ai = AiDriver::new();
    ai.insert(1, sharp_planner(&reg, 11, "massed_armor"));
    ai.plan_round(&reg, &mut state);

    let toward = |hex: tactics_core::Hex| lane.iter().map(|h| h.distance_to(hex)).min().unwrap();
    for id in state.formations()[index].members.clone() {
        let unit = state.unit(id).expect("planning harms nobody");
        assert!(
            toward(unit.planned_destination()) < toward(unit.pos),
            "{} was told on the campaign map to break off, so she drives for the road",
            unit.name
        );
    }
}

/// Stand a platoon and a tank on one hex of forest, and shoot at the tank.
///
/// `f` is forest, which is the only terrain in the base mod roomy enough (5)
/// to hold a medium tank (3) and a rifle platoon (1) with room to spare — the
/// one-crew-per-hex rule is still what a terrain declaring no capacity means.
fn crowded_wood(reg: &DataRegistry, seed: u64) -> BattleState {
    // Ten hexes is a kilometre and the wood is worth 30 cover, so the gunner
    // misses often enough to sample what a miss does. At two hexes on open
    // grass she hits almost every time and the stage proves nothing.
    let row: String = format!("{}f{}", "g".repeat(10), "g".repeat(4));
    two_side_battle(
        reg,
        &[&row, &row, &row],
        vec![
            unit_at([0, 1], 0, "medium_tank", "Gunner"),
            unit_at([10, 1], 1, "medium_tank", "Quarry"),
            unit_at([10, 1], 1, "rifle_platoon", "Bystanders"),
        ],
        seed,
    )
}

/// The rule the designer asked for: the gunner aims, and only a *miss* is a
/// lottery over who else is standing there.
#[test]
fn a_round_that_goes_past_a_tank_can_find_the_platoon_beside_her() {
    let reg = seen(registry_wireless());
    let (mut misses, mut strays, mut onto_the_tank) = (0, 0, 0);
    // Many seeds rather than many rounds of one battle: a stray is a second
    // roll behind a first one, so a single stage does not sample it.
    for seed in 0..400u64 {
        let mut state = crowded_wood(&reg, seed);
        let events = state.apply(
            &reg,
            &Order::SetFire {
                unit: UnitId(0),
                fire: FireIntent::Target {
                    target: UnitId(1),
                    weapon: 0,
                },
            },
        );
        assert!(events.is_ok(), "the gunner can see her quarry");
        for event in play_round(&reg, &mut state) {
            match event {
                BattleEvent::ShotMissed { .. } => misses += 1,
                BattleEvent::ShotStrayed { intended, onto, .. } => {
                    assert_eq!(intended, UnitId(1), "she was aiming at the tank");
                    assert_eq!(onto, UnitId(2), "and the platoon is the only bystander");
                    strays += 1;
                }
                BattleEvent::ShotHit {
                    target: UnitId(1), ..
                } => onto_the_tank += 1,
                _ => {}
            }
        }
    }
    assert!(misses > 20, "the stage has to produce misses: {misses}");
    assert!(onto_the_tank > 0, "and hits on what she aimed at");
    // 25% per hundred points of presence, and a platoon's presence is
    // 100 + profile = 80, so the nominal rate is one miss in five. Measured
    // 148 of 1265, which is 12% — lower on purpose and not a discrepancy: a
    // stray that kills the platoon leaves the rest of that round's misses
    // with nobody to stray onto, so the *observed* rate is always below the
    // roll. Loose bounds, because this pins that the rule fires at roughly
    // its stated rate and not the rate itself, which is a tuning number and
    // lives in mod.json.
    let rate = 100 * strays / misses;
    assert!(
        (8..=34).contains(&rate),
        "{strays} of {misses} misses strayed ({rate}%), which is nowhere near the \
         20% the balance block asks for"
    );
}

/// The additivity pin: a ladder with no rungs is no ladder.
#[test]
fn a_mod_that_prices_no_strays_has_a_miss_that_is_simply_a_miss() {
    let mut reg = registry_wireless();
    reg.balance.stray_percent = 0;
    for seed in 0..200u64 {
        let mut state = crowded_wood(&reg, seed);
        let _ = state.apply(
            &reg,
            &Order::SetFire {
                unit: UnitId(0),
                fire: FireIntent::Target {
                    target: UnitId(1),
                    weapon: 0,
                },
            },
        );
        for event in play_round(&reg, &mut state) {
            assert!(
                !matches!(event, BattleEvent::ShotStrayed { .. }),
                "stray_percent 0 must be the game before stacking existed"
            );
        }
    }
}

/// And nobody to stray onto is nobody to stray onto.
#[test]
fn a_shot_at_a_crew_standing_alone_never_finds_anybody_else() {
    let reg = registry_wireless();
    for seed in 0..200u64 {
        let mut state = two_side_battle(
            &reg,
            &[
                &format!("{}f{}", "g".repeat(10), "g".repeat(4)),
                &format!("{}f{}", "g".repeat(10), "g".repeat(4)),
                &format!("{}f{}", "g".repeat(10), "g".repeat(4)),
            ],
            vec![
                unit_at([0, 1], 0, "medium_tank", "Gunner"),
                unit_at([10, 1], 1, "medium_tank", "Quarry"),
            ],
            seed,
        );
        let _ = state.apply(
            &reg,
            &Order::SetFire {
                unit: UnitId(0),
                fire: FireIntent::Target {
                    target: UnitId(1),
                    weapon: 0,
                },
            },
        );
        for event in play_round(&reg, &mut state) {
            assert!(!matches!(event, BattleEvent::ShotStrayed { .. }));
        }
    }
}

/// Room is counted in footprints against the terrain's capacity, and a
/// terrain that declares neither is the one-crew-per-hex game this engine
/// shipped with.
#[test]
fn a_wood_holds_a_platoon_and_her_taxi_where_a_road_holds_only_the_taxi() {
    let reg = registry_wireless();
    // forest capacity 5, grass capacity 4; medium_tank 3, rifle_platoon 1.
    // A tank and one platoon fit on either; a tank and two only fit in timber.
    let state = two_side_battle(
        &reg,
        &["ggfggg", "ggfggg", "ggfggg"],
        vec![
            unit_at([2, 1], 0, "medium_tank", "In The Wood"),
            unit_at([2, 1], 0, "rifle_platoon", "With Her"),
            unit_at([1, 1], 0, "medium_tank", "In The Open"),
            unit_at([1, 1], 0, "rifle_platoon", "With Him"),
            unit_at([4, 1], 0, "rifle_platoon", "Latecomer"),
            unit_at([5, 1], 1, "medium_tank", "Bystander"),
        ],
        11,
    );
    let platoon = state.unit(UnitId(4)).expect("she exists");
    let wood = tactics_core::offset_to_hex(2, 1);
    let open = tactics_core::offset_to_hex(1, 1);
    assert!(
        state.room_for(&reg, platoon, wood),
        "forest holds 5 and a tank and two platoons are 5"
    );
    assert!(
        !state.room_for(&reg, platoon, open),
        "grass holds 4 and the tank and platoon on it are already 4"
    );
    // And the rule a terrain that says nothing keeps: grass in this test's own
    // palette does declare a capacity, so use a registry that does not.
    let mut old = registry_wireless();
    for terrain in old.terrain.values_mut() {
        terrain.capacity = None;
    }
    assert!(
        !old.terrain("forest").expect("forest").capacity.is_some(),
        "the stage needs a registry with stacking switched off"
    );
    assert!(
        !state.room_for(&old, platoon, wood),
        "no declared capacity is one crew to a hex, whatever size she is"
    );
}

/// A column can follow the column in front of it.
///
/// The campaign map used to treat *any* army as impassable, so a friend on the
/// road refused the whole route — the same conflation of "cannot stop here"
/// with "cannot cross here" that a hex holding one crew was in battle. Two
/// armies still do not share a tile; they just do not wall each other off.
///
/// The budget is pinned to exactly the cost of driving straight through,
/// which is what makes this a test rather than a coincidence: two tiles at
/// distance two along one axis share exactly one neighbour, so blocking the
/// midpoint leaves only a three-step detour, and a detour does not fit. An
/// earlier draft of this asserted only that the far tile was reachable, and
/// it passed against the old rule because the army simply drove around.
#[test]
fn an_army_drives_past_a_friend_and_stops_beyond_her() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let ids: Vec<_> = state.side_armies(0).map(|a| a.id).collect();
    let (follower, ahead) = (ids[0], ids[1]);

    let pos = state.army(follower).unwrap().pos;
    let reach = state.reachable(&reg, follower);
    // A direction with two tiles of ground in it, both free and both in range:
    // one to park the friend on and one to finish beyond her.
    let (step, beyond, through) = pos
        .all_neighbors()
        .into_iter()
        .map(|next| (next, pos + (next - pos) * 2))
        .filter(|(next, far)| state.army_at(*next).is_none() && state.army_at(*far).is_none())
        .filter_map(|(next, far)| Some((next, far, *reach.get(&far)?)))
        .min_by_key(|(next, _, _)| (next.x, next.y))
        .expect("frontier gives her two tiles of room somewhere");

    // Exactly enough fuel for the straight line and not a point more.
    state.army_mut(follower).unwrap().movement = through;
    state.army_mut(ahead).unwrap().pos = step;

    let reach = state.reachable(&reg, follower);
    assert!(!reach.contains_key(&step), "she may not park on her friend");
    assert_eq!(
        reach.get(&beyond),
        Some(&through),
        "the road past her is still a road, at the same price"
    );

    state
        .apply(
            &reg,
            &OverworldOrder::MoveArmy {
                army: follower,
                to: beyond,
            },
        )
        .expect("the route exists");
    assert_eq!(
        state.army(follower).expect("alive").pos,
        beyond,
        "and she ends up where she was sent"
    );
}

/// A hexagon of open grass, so that "the same problem from the other end" is
/// a thing that exists.
///
/// A hexagon rather than a rectangle of text because the offset conversion
/// *shears* text: a rectangle of ASCII is symmetric on the page and is not
/// symmetric on the map, which is a mistake this project has already made
/// once and paid for (see `a_march_is_the_same_march_from_either_end`). A
/// hexagon is closed under point reflection through its own centre for the
/// same reason a circle is.
fn open_hexagon(reg: &DataRegistry, radius: i32, placements: Vec<UnitPlacement>) -> BattleState {
    let centre = tactics_core::offset_to_hex(radius + radius / 2, radius);
    let rows: Vec<String> = (0..=2 * radius)
        .map(|row| {
            (0..=2 * radius + radius / 2 + 1)
                .map(|col| {
                    if tactics_core::offset_to_hex(col, row).distance_to(centre) > radius {
                        ' '
                    } else {
                        'g'
                    }
                })
                .collect()
        })
        .collect();
    let borrowed: Vec<&str> = rows.iter().map(String::as_str).collect();
    two_side_battle(reg, &borrowed, placements, 1)
}

/// Driving from A to B and driving from B to A are the same problem reflected,
/// and the engine has to answer them the same way.
///
/// This is the regression test for a bias that cost this project months of
/// misread balance numbers. `movement::step_toward` used to break ties with
/// `(h.x, h.y)` — and smallest x is *west*, so wherever two hexes were equally
/// close to the destination every crew in the game edged west. That is
/// forwards for a side attacking west and backwards for a side attacking east,
/// so it was worth real points to whichever end of a map was on the east; the
/// mirrored arena that was supposed to detect exactly this had the same fault
/// in its own terrain and the two pointed the same way.
///
/// The rule it enforces: **a tiebreak may only read quantities a reflection
/// preserves.** Distances and terrain costs qualify; a dot product of two
/// differences qualifies, because a reflection negates both; a coordinate does
/// not, and no total order on coordinates can — a reflection maps the least
/// element to the greatest, so asking for the smallest is asking which way is
/// west.
#[test]
fn a_march_is_the_same_march_from_either_end() {
    let reg = registry();
    let radius = 6;
    let centre = tactics_core::offset_to_hex(radius + radius / 2, radius);
    let mirror = |h: tactics_core::Hex| centre * 2 - h;

    // Off both axes on purpose: a destination straight ahead has one closest
    // reachable hex and never reaches the tiebreak at all.
    let west = centre + tactics_core::Hex::new(-5, 1);
    let goal_west = centre + tactics_core::Hex::new(3, -2);
    let state = open_hexagon(
        &reg,
        radius,
        vec![
            unit_at(tactics_core::hex_to_offset(west), 0, "medium_tank", "West"),
            unit_at(
                tactics_core::hex_to_offset(mirror(west)),
                1,
                "medium_tank",
                "East",
            ),
        ],
    );

    let theirs = reachable(&reg, &state, UnitId(0)).len();
    assert!(
        theirs > 1,
        "the stage is pointless if she cannot go anywhere: {theirs} tiles"
    );

    let a = tactics_core::battle::step_toward(&reg, &state, UnitId(0), goal_west)
        .expect("west has somewhere to go");
    let b = tactics_core::battle::step_toward(&reg, &state, UnitId(1), mirror(goal_west))
        .expect("east has somewhere to go");
    assert_eq!(
        a,
        mirror(b),
        "west stepped to {a:?} and east to {b:?}, whose mirror is {:?} — the same \
         problem from the other end got a different answer",
        mirror(b)
    );
}

/// The projection a tiebreak is allowed to read, and why it is allowed.
#[test]
fn a_reflection_leaves_a_bearing_alone_and_turns_a_coordinate_around() {
    use tactics_core::Hex;
    use tactics_core::battle::along_the_bearing;
    let centre = Hex::new(4, -7);
    let mirror = |h: Hex| centre * 2 - h;
    for step in [Hex::new(1, 0), Hex::new(-2, 3), Hex::new(0, -4)] {
        for bearing in [Hex::new(5, -1), Hex::new(-3, -2)] {
            // A reflection negates both differences, so their product stands.
            assert_eq!(
                along_the_bearing(step, bearing),
                along_the_bearing(-step, -bearing),
                "the bearing projection must survive a reflection"
            );
        }
    }
    // And the thing that does not: whichever hex has the smaller x, its
    // mirror has the larger. This is the whole reason a coordinate cannot be
    // a tiebreak, stated as an assertion rather than as a comment.
    let (lo, hi) = (Hex::new(-3, 1), Hex::new(2, 1));
    assert!(lo.x < hi.x);
    assert!(
        mirror(lo).x > mirror(hi).x,
        "a reflection reverses a coordinate order, which is what made `min_by_key` \
         on `h.x` a compass"
    );
}

#[test]
fn ground_is_taken_by_standing_on_it_and_stays_taken_after_leaving() {
    // Control persists on purpose: ground you have taken has to be taken back
    // rather than merely vacated, or an objective would be worth nothing to
    // anyone who has somewhere else to be.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{ "id": "crossroads", "at": [[2, 0]], "value": 1 }]),
        None,
        curtained_pair(),
    );
    assert_eq!(state.objective_held, vec![None], "nobody starts holding it");

    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: tactics_core::offset_to_hex(2, 0),
            },
        )
        .expect("west can drive to the crossroads");
    let events = play_round(&reg, &mut state);
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::ObjectiveTaken { objective, side: Some(0), .. } if objective == "crossroads"
        )),
        "taking ground is announced, or a battle decided on points reads as arbitrary"
    );
    assert_eq!(state.objective_held, vec![Some(0)]);

    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: tactics_core::offset_to_hex(0, 0),
            },
        )
        .expect("west can drive home again");
    let events = play_round(&reg, &mut state);
    assert_eq!(
        state.objective_held,
        vec![Some(0)],
        "walking away does not hand the ground back"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::ObjectiveTaken { .. })),
        "ground that did not change hands says nothing"
    );
}

#[test]
fn ground_two_sides_stand_on_belongs_to_neither() {
    // The objective is deliberately two hexes at opposite ends of the map, so
    // that one crew from each side can stand on it without being in a
    // position to shoot the other. What is under test is the contest rule,
    // not gunnery.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{ "id": "the_valley", "at": [[0, 0], [10, 0]], "value": 4 }]),
        None,
        curtained_pair(),
    );
    play_round(&reg, &mut state);
    assert_eq!(
        state.objective_held,
        vec![None],
        "ground both sides are standing on is nobody's"
    );
    assert_eq!(
        (state.score(0), state.score(1)),
        (0, 0),
        "and a contested objective pays nobody, or a defender could collect \
         points while being overrun"
    );
}

#[test]
fn holding_ground_pays_once_a_round_however_many_ticks_a_round_has() {
    // Paying per tick would make the size of every score an accident of
    // `ticks_per_round`, which is mod data and may be anything.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{ "id": "crossroads", "at": [[0, 0]], "value": 3 }]),
        None,
        curtained_pair(),
    );
    for round in 1..=3 {
        play_round(&reg, &mut state);
        assert_eq!(
            state.score(0),
            3 * round,
            "three points a round, not three a tick"
        );
        assert_eq!(state.score(1), 0);
    }
}

#[test]
fn a_side_that_holds_the_ground_wins_a_battle_that_loses_contact() {
    // The rule this whole feature exists for. Two crews who never find each
    // other used to produce a draw, which made sitting still unbeatable; now
    // the one that walked to the objective has something to show for it.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{ "id": "crossroads", "at": [[2, 0]], "value": 1 }]),
        None,
        curtained_pair(),
    );
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: tactics_core::offset_to_hex(2, 0),
            },
        )
        .expect("west can drive to the crossroads");

    let mut ended = None;
    for _ in 0..(reg.balance.stalemate_rounds as usize + 2) {
        let events = play_round(&reg, &mut state);
        if let Some(BattleEvent::BattleEnded { winner, reason }) = events
            .iter()
            .find(|e| matches!(e, BattleEvent::BattleEnded { .. }))
        {
            ended = Some((*winner, *reason));
            break;
        }
    }
    assert_eq!(
        ended,
        Some((Some(0), EndReason::Stalemate)),
        "breaking contact ends the shooting; the points say who won"
    );
    assert_eq!(
        state.alive_units().count(),
        2,
        "and it still costs nobody their tanks"
    );
}

#[test]
fn reaching_the_victory_score_ends_the_battle_outright() {
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{ "id": "the_hill", "at": [[0, 0]], "value": 5 }]),
        Some(10),
        curtained_pair(),
    );
    let mut ended = None;
    for _ in 0..4 {
        let events = play_round(&reg, &mut state);
        if let Some(BattleEvent::BattleEnded { winner, reason }) = events
            .iter()
            .find(|e| matches!(e, BattleEvent::BattleEnded { .. }))
        {
            ended = Some((*winner, *reason));
            break;
        }
    }
    assert_eq!(ended, Some((Some(0), EndReason::Objectives)));
    assert_eq!(
        state.round, 2,
        "two rounds at five points a round, and not a round later"
    );
}

#[test]
fn driving_off_an_exit_takes_the_crew_home_rather_than_killing_them() {
    // The distinction the whole exit mechanism rests on. `alive` is "on the
    // battlefield" and answers targeting and fog; it is not "came home", and
    // the campaign reads the second.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{
            "id": "west_road", "at": [[0, 0]], "value": 5,
            "kind": "exit", "side": 0
        }]),
        None,
        vec![
            unit_at([1, 0], 0, "medium_tank", "Leaver"),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
    );
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: tactics_core::offset_to_hex(0, 0),
            },
        )
        .expect("west can reach the road");
    let events = play_round(&reg, &mut state);

    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::UnitExited { unit, objective, .. }
                if *unit == UnitId(0) && objective == "west_road"
        )),
        "leaving is announced in its own right, never as a destruction"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::UnitDestroyed { .. })),
        "nobody was destroyed"
    );

    let leaver = &state.units[0];
    assert!(!leaver.alive(), "she is off the board");
    assert!(leaver.exited(), "but she left under her own power");
    assert_eq!(state.score(0), 5, "and the exit paid its value once");

    assert!(
        state.surviving_units().any(|u| u.id == UnitId(0)),
        "the campaign must count her among the survivors"
    );
    assert!(
        !state.lost_units().any(|u| u.id == UnitId(0)),
        "and must not count her among the losses"
    );
}

#[test]
fn a_hull_that_takes_two_ends_in_one_tick_is_remembered_by_the_more_telling_one() {
    // `Destruction` is one value where three independent flags used to
    // stand, and a hull really can take two ends inside a single tick: over
    // 900 AI battles and 9,131 losses the pairs land about 250 times, in all
    // three combinations. So which one sticks is a rule, not a corner, and
    // it is the read precedence those flags were consulted in, moved to the
    // write.
    //
    // Burning outranks the crew leaving because a brew-up rolls every
    // passenger through the fire and a bail-out does not. The crew leaving
    // outranks the hull being crushed for a reason the simulation depends
    // on: the bail-out check refuses to roll for a crew who has already
    // gone, so an abandonment a later shell's blast overwrote would let the
    // same cadets abandon the same tank twice.
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["ggggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Doomed"),
            unit_at([4, 0], 1, "medium_tank", "Spare"),
        ],
        7,
    );
    let unit = UnitId(0);

    let end = |state: &mut BattleState, first, second| {
        let u = state.unit_mut(unit).expect("she is on the board");
        u.fate = Fate::default();
        u.doomed_by(first);
        u.doomed_by(second);
        u.destruction()
    };
    for (a, b) in [
        (Destruction::Abandoned, Destruction::BrewedUp),
        (Destruction::Crushed, Destruction::BrewedUp),
        (Destruction::Crushed, Destruction::Abandoned),
    ] {
        assert_eq!(end(&mut state, a, b), Some(b), "{b:?} outranks {a:?}");
        assert_eq!(
            end(&mut state, b, a),
            Some(b),
            "{b:?} still outranks {a:?} from the other order"
        );
    }

    // She is on the board the whole time she is doomed — damage lands during
    // a tick and death is reaped at the end of it — and the reaping carries
    // the end she took rather than losing it.
    assert_eq!(
        end(&mut state, Destruction::Crushed, Destruction::BrewedUp),
        Some(Destruction::BrewedUp)
    );
    let doomed = state.unit(unit).expect("still there");
    assert!(doomed.alive(), "a doom is not yet a death");
    let u = state.unit_mut(unit).expect("still there");
    u.destroy();
    let dead = &state.units[unit.index()];
    assert!(!dead.alive() && !dead.exited(), "reaped as a loss");
    assert_eq!(
        dead.destruction(),
        Some(Destruction::BrewedUp),
        "and remembered by what ended her"
    );

    // A hull nothing had doomed is remembered as having nobody left to work
    // her, which is the case `reap` finds and nothing ever writes down.
    let spare = UnitId(1);
    state.unit_mut(spare).expect("the other one").destroy();
    assert_eq!(
        state.units[spare.index()].destruction(),
        Some(Destruction::CrewSpent)
    );
}

#[test]
fn an_exit_belongs_to_the_side_it_names() {
    // An exit anyone may use is a lane both armies leave by on round one.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{
            "id": "west_road", "at": [[0, 0], [10, 0]], "value": 5,
            "kind": "exit", "side": 0
        }]),
        None,
        // Side 1 starts standing on a hex of side 0's exit. Both stay behind
        // the curtain: the rule under test is eligibility, and a firefight
        // would settle it by killing somebody instead.
        vec![
            unit_at([1, 0], 0, "medium_tank", "West"),
            unit_at([10, 0], 1, "medium_tank", "Squatter"),
        ],
    );
    play_round(&reg, &mut state);
    assert!(
        state.units[1].alive() && !state.units[1].exited(),
        "side 1 may not leave by side 0's road"
    );
    assert_eq!(state.score(1), 0);
}

#[test]
fn an_exit_is_not_ground_anybody_holds() {
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{
            "id": "west_road", "at": [[0, 0]], "value": 5,
            "kind": "exit", "side": 0
        }]),
        None,
        curtained_pair(),
    );
    play_round(&reg, &mut state);
    assert_eq!(
        state.objective_held,
        vec![None],
        "an exit is passed through, not held, so it never pays per round"
    );
}

#[test]
fn a_withdrawal_that_reaches_its_target_wins_on_the_tick_it_completes() {
    // Elimination is checked *after* the score for exactly this case: the
    // last vehicle of a withdrawing force leaves the board and reaches the
    // target in the same tick. Checking the board first would award the
    // battle to an enemy holding a field nobody wanted.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{
            "id": "west_road", "at": [[0, 0]], "value": 10,
            "kind": "exit", "side": 0
        }]),
        Some(10),
        vec![
            unit_at([1, 0], 0, "medium_tank", "Last Out"),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
    );
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: tactics_core::offset_to_hex(0, 0),
            },
        )
        .expect("west can reach the road");
    let events = play_round(&reg, &mut state);

    let ended = events.iter().find_map(|e| match e {
        BattleEvent::BattleEnded { winner, reason } => Some((*winner, *reason)),
        _ => None,
    });
    assert_eq!(
        ended,
        Some((Some(0), EndReason::Objectives)),
        "the force that got away won, though it has nothing left on the field"
    );
}

#[test]
fn an_intact_crew_will_not_run_for_the_exit_but_a_broken_one_will() {
    // Withdrawal has to be conditional or the lane is a free win: every unit
    // would drive off on round one. The gate is the doctrine's
    // `withdraw_threshold` against the vehicle's own damage.
    let reg = registry();
    let state = objective_battle(
        &reg,
        serde_json::json!([{
            "id": "west_road", "at": [[0, 0]], "value": 5,
            "kind": "exit", "side": 0
        }]),
        None,
        curtained_pair(),
    );
    let eval = Evaluator::new(reg.doctrine("elastic_defense").cloned().unwrap());
    let exit = tactics_core::offset_to_hex(0, 0);
    let away = tactics_core::offset_to_hex(4, 0);

    assert!(
        eval.score_tile(&reg, &state, UnitId(0), exit).score
            <= eval.score_tile(&reg, &state, UnitId(0), away).score,
        "an undamaged crew cannot see the exit at all"
    );

    let mut hurt = state;
    maul(&reg, &mut hurt, UnitId(0));
    assert!(hurt.condition(&reg, hurt.unit(UnitId(0)).unwrap()) < 0.5);
    assert!(
        eval.score_tile(&reg, &hurt, UnitId(0), exit).score
            > eval.score_tile(&reg, &hurt, UnitId(0), away).score,
        "a crew that is nearly finished should run for the road"
    );
}

#[test]
fn a_map_that_names_no_objectives_is_fought_exactly_as_it_was_before() {
    // Objectives have to be an additive rule whose absence is the old game —
    // the same constraint difficulty-as-a-mod puts on every harsh system. The
    // check that bites is the evaluator's: on a map with no objectives, how
    // much a doctrine cares about objectives must not change a single score.
    let reg = registry();
    let state = standoff(&reg, 1);
    assert!(state.map.objectives().is_empty());
    assert!(
        state.leader().is_none(),
        "nobody leads a battle with nothing to lead on"
    );

    let mut indifferent = reg.doctrine("massed_armor").cloned().unwrap();
    indifferent.objective_value = 0.0;
    let mut greedy = indifferent.clone();
    greedy.objective_value = 25.0;

    let (a, b) = (Evaluator::new(indifferent), Evaluator::new(greedy));
    for (tile, _) in state.map.iter() {
        assert_eq!(
            a.score_tile(&reg, &state, UnitId(0), tile).score,
            b.score_tile(&reg, &state, UnitId(0), tile).score,
            "a map with no objectives cannot be scored differently by a \
             doctrine that wants them"
        );
    }
}

#[test]
fn an_objective_the_map_does_not_contain_is_a_validation_error() {
    let reg = registry();
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "bad_objectives",
        "palette": { "g": "grass" },
        "rows": ["ggg"],
        "shape": "free",
        "objectives": [
            { "id": "nowhere", "at": [[99, 99]], "value": 1 },
            { "id": "nowhere", "at": [], "value": 1 },
        ],
    }))
    .unwrap();
    let mut report = tactics_core::data::ValidationReport::default();
    file.validate_into(&reg, &mut report);

    let errors = report.errors.join("\n");
    assert!(
        errors.contains("outside the map"),
        "an objective nobody can stand on must not load quietly: {errors}"
    );
    assert!(
        errors.contains("names no hexes"),
        "nor one with no ground at all: {errors}"
    );
    assert!(
        errors.contains("share the id"),
        "nor two that cannot be told apart: {errors}"
    );
}

/// A map file whose chain of command is deliberately broken in every way
/// there is, so one call to `validate_into` can be asked about all of them.
fn tangled_command_map() -> tactics_core::map::MapFile {
    serde_json::from_value(serde_json::json!({
        "id": "tangled_command",
        "palette": { "g": "grass" },
        "rows": ["gggggg"],
        "shape": "free",
        "sides": [{ "name": "West" }, { "name": "East" }],
        "formations": [
            { "id": "twins", "side": 0 },
            { "id": "twins", "side": 0 },
            { "id": "nobody", "side": 1 },
            { "id": "mixed", "side": 0 },
        ],
        "units": [
            { "at": [0, 0], "side": 0, "vehicle": "medium_tank",
              "formation": "twins", "leads": true },
            { "at": [1, 0], "side": 0, "vehicle": "medium_tank",
              "formation": "twins", "leads": true },
            { "at": [2, 0], "side": 1, "vehicle": "medium_tank",
              "formation": "mixed" },
            { "at": [3, 0], "side": 0, "vehicle": "medium_tank",
              "formation": "ghost_platoon" },
            { "at": [4, 0], "side": 0, "vehicle": "medium_tank", "leads": true },
        ],
    }))
    .unwrap()
}

#[test]
fn a_chain_of_command_that_does_not_join_up_is_a_validation_error() {
    // Formations say who obeys whom, so a typo does not merely look wrong: it
    // leaves a vehicle outside the chain, which once missions exist is
    // indistinguishable from a crew that was told to sit still. Every one of
    // these is refused rather than warned about for that reason.
    let reg = registry();
    let mut report = tactics_core::data::ValidationReport::default();
    tangled_command_map().validate_into(&reg, &mut report);
    let errors = report.errors.join("\n");

    assert!(
        errors.contains("does not declare"),
        "a unit in a formation nobody declared must be refused: {errors}"
    );
    assert!(
        errors.contains("has no members"),
        "nor a formation nobody is in: {errors}"
    );
    assert!(
        errors.contains("only one cadet can be in command"),
        "nor two crews both claiming to lead: {errors}"
    );
    assert!(
        errors.contains("which belongs to side"),
        "nor a formation spanning two armies: {errors}"
    );
    assert!(
        errors.contains("marked `leads` but is in no formation"),
        "nor a leader of nothing: {errors}"
    );
    assert!(
        errors.contains("two formations share the id"),
        "nor two formations that cannot be told apart: {errors}"
    );
}

#[test]
fn a_formation_asking_for_an_unknown_doctrine_falls_back_rather_than_failing() {
    // Doctrine degrades everywhere else it is named — an unknown one on a side
    // becomes the balanced default rather than refusing to field the side —
    // and a formation's own doctrine is the same bargain one level down.
    let reg = registry();
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "odd_doctrine",
        "palette": { "g": "grass" },
        "rows": ["gg"],
        "shape": "free",
        "sides": [{ "name": "West" }],
        "formations": [
            { "id": "first", "side": 0, "doctrine": "napoleonic_squares" },
        ],
        "units": [
            { "at": [0, 0], "side": 0, "vehicle": "medium_tank", "formation": "first" },
        ],
    }))
    .unwrap();
    let mut report = tactics_core::data::ValidationReport::default();
    file.validate_into(&reg, &mut report);

    assert!(
        report.is_ok(),
        "an unknown doctrine must not stop the map loading: {:?}",
        report.errors
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("napoleonic_squares")),
        "but it must be said out loud: {:?}",
        report.warnings
    );
}

#[test]
fn a_map_that_declares_formations_puts_them_on_the_battle() {
    // The shipped scenario is the fixture on purpose: it is what the
    // determinism baseline is fought on, so populating command state here is
    // what makes "nothing reads it yet" a checkable claim rather than a hope.
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 1).expect("battle");

    let ids: Vec<&str> = state.formations().iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "kuhlmann_armor",
            "kuhlmann_recon",
            "valkyrie_line",
            "valkyrie_screen"
        ],
        "formations must arrive in the order the map declared them"
    );

    let armor = state
        .formations()
        .iter()
        .find(|f| f.id == "kuhlmann_armor")
        .expect("the map declares it");
    assert_eq!(armor.side, 0);
    // Anka's medium tank is placed first and says `leads`; Mina's light tank
    // follows her. Members are unit ids, which is placement order.
    assert_eq!(armor.members, vec![UnitId(0), UnitId(2)]);
    assert_eq!(armor.leader, Some(UnitId(0)));

    // A formation that names no leader falls back to its first member, which
    // is the seniority a map author controls by declaration order.
    let screen = state
        .formations()
        .iter()
        .find(|f| f.id == "valkyrie_screen")
        .expect("the map declares it");
    assert_eq!(screen.side, 1);
    assert_eq!(screen.leader, Some(UnitId(7)), "Greta's car says `leads`");

    // Every unit answers to exactly one formation, and to the right one.
    for unit in state.units.iter() {
        let formation = state
            .formation_of(unit.id)
            .unwrap_or_else(|| panic!("{} is in no formation", unit.name));
        assert_eq!(
            formation.side, unit.side,
            "{} answers to the other army",
            unit.name
        );
    }
}

#[test]
fn a_map_that_declares_no_formations_has_no_chain_of_command() {
    // The additivity rule: saying nothing is one flat pool per side, which is
    // every battle this engine fought before formations existed.
    let reg = registry();
    let state = standoff(&reg, 1);
    assert!(state.formations().is_empty());
    assert!(state.formation_of(UnitId(0)).is_none());
}

#[test]
fn setting_a_mission_stores_it_on_the_formation_and_says_so_out_loud() {
    // The whole point of routing missions through `apply` is that the human,
    // the AI and a replay all speak one vocabulary — so the order has to land
    // in state *and* produce the event a log can carry. Silence would be the
    // failure mode: a mission nobody can see is indistinguishable from one
    // that was dropped.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 5).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let bridge = state.map.objectives()[0].anchor();

    let events = state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: bridge },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("the bridge is on the map and the platoon exists");
    assert_eq!(
        events,
        vec![BattleEvent::MissionAssigned {
            formation: "kuhlmann_armor".into(),
            mission: Mission::Advance { to: bridge },
        }],
        "the event names the formation a reader would recognise, not its index"
    );
    assert_eq!(
        state.formations()[armor.index()].mission,
        Some(Mission::Advance { to: bridge })
    );

    // A second mission replaces the first rather than queueing behind it:
    // countermanding an order is ordinary business, and a formation holding
    // two missions at once means nothing anyone could act on.
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Hold { at: None },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("countermanding is legal");
    assert_eq!(
        state.formations()[armor.index()].mission,
        Some(Mission::Hold { at: None })
    );
}

#[test]
fn a_mission_is_a_standing_order_and_outlives_the_round_it_was_given_in() {
    // The distinction the whole command layer rests on: a unit's intent is
    // this round's instructions and is wiped when the next one opens, while a
    // formation told to take the bridge is still taking it tomorrow. If
    // `begin_round` ever cleared this, missions would silently become
    // per-round orders and every executor built on top would be wrong.
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 6).expect("battle");
    let recon = formation_named(&state, "kuhlmann_recon");
    let ford = state.map.objectives()[1].anchor();
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: recon,
                mission: Mission::Recon { toward: ford },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("the upper ford is on the map");

    let round = state.round;
    play_round(&reg, &mut state);
    assert!(state.round > round, "a whole round must have gone by");
    assert_eq!(
        state.formations()[recon.index()].mission,
        Some(Mission::Recon { toward: ford }),
        "a standing order stands"
    );
}

#[test]
fn a_mission_for_a_formation_that_does_not_exist_is_refused() {
    // The handle arrives from outside — a save, a replay, a brain that is not
    // this process — so a stale one is an ordinary refusal, never a panic.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 7).expect("battle");
    let past_the_end = FormationId(state.formations().len() as u32);
    assert_eq!(
        state.apply(
            &reg,
            &Order::SetMission {
                formation: past_the_end,
                mission: Mission::Hold { at: None },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        ),
        Err(tactics_core::battle::OrderError::NoSuchFormation),
    );
}

#[test]
fn a_mission_set_after_the_side_has_committed_is_refused() {
    // A mission is an order, and orders close when a side hands its planning
    // in — the same bargain `SetFire` and `SetMove` already make. Ordering a
    // platoon about after the round has been sealed would let a side plan
    // twice.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 8).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let line = formation_named(&state, "valkyrie_line");
    state
        .apply(&reg, &Order::Commit { side: 0 })
        .expect("commit");

    assert_eq!(
        state.apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Hold { at: None },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        ),
        Err(tactics_core::battle::OrderError::AlreadyCommitted),
    );
    // The other army has not committed and is unaffected: the refusal is
    // about whose side spoke, not about the phase.
    assert!(
        state
            .apply(
                &reg,
                &Order::SetMission {
                    formation: line,
                    mission: Mission::Hold { at: None },
                    latitude: tactics_core::battle::Latitude::Delegated,
                },
            )
            .is_ok(),
        "side 1 is still planning"
    );
}

#[test]
fn a_withdrawal_must_name_an_exit_this_side_may_use() {
    // Three ways to get this wrong, all of which would otherwise send a
    // formation to the map edge to wait for a way out that is not there: a
    // name nobody declared, ground that is held rather than left by, and the
    // enemy's lane. `river_crossing` gives west_road to side 0 and east_road
    // to side 1, which is what makes the last one checkable at all.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 9).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let withdraw = |via: &str| Order::SetMission {
        formation: armor,
        mission: Mission::Withdraw { via: via.into() },
        latitude: tactics_core::battle::Latitude::Delegated,
    };
    let refused = Err(tactics_core::battle::OrderError::NoSuchExit);

    assert_eq!(
        state.apply(&reg, &withdraw("the_scenic_route")),
        refused,
        "no objective by that name"
    );
    assert_eq!(
        state.apply(&reg, &withdraw("bridge")),
        refused,
        "the bridge is ground to hold, not a way off the map"
    );
    assert_eq!(
        state.apply(&reg, &withdraw("east_road")),
        refused,
        "the eastern road is the Valkyries' lane"
    );
    assert!(
        state.apply(&reg, &withdraw("west_road")).is_ok(),
        "but her own road is hers to leave by"
    );
    assert_eq!(
        state.formations()[armor.index()].mission,
        Some(Mission::Withdraw {
            via: "west_road".into()
        }),
        "and only the accepted one is remembered"
    );
}

#[test]
fn a_mission_that_names_ground_off_the_map_is_refused() {
    // Only what cannot change as the round plays out is checked — a hex being
    // on the map is that; the ground being reachable or wise is the
    // executor's problem. Every variant that carries a hex is covered,
    // because `Hold`'s optional one is exactly the sort of field a validator
    // forgets.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 10).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let nowhere = tactics_core::offset_to_hex(500, 500);
    assert!(!state.map.contains(nowhere));
    let not_on_map = Err(tactics_core::battle::OrderError::NotOnMap);
    for mission in [
        Mission::Advance { to: nowhere },
        Mission::Recon { toward: nowhere },
        Mission::Hold { at: Some(nowhere) },
    ] {
        assert_eq!(
            state.apply(
                &reg,
                &Order::SetMission {
                    formation: armor,
                    mission: mission.clone(),
                    latitude: tactics_core::battle::Latitude::Delegated,
                },
            ),
            not_on_map,
            "{mission:?} names a tile that is not there"
        );
    }
    assert!(
        state.formations()[armor.index()].mission.is_none(),
        "a refused mission leaves the formation as it was"
    );
}

#[test]
fn a_column_advances_without_ambushing_itself() {
    // One hex holds one unit, so a friend in the way is traffic rather than an
    // enemy: the unit behind waits a tick and follows, and nothing about it
    // resembles an ambush.
    let reg = registry();
    // The bystander sits behind a forest curtain: in the open it would be
    // spotted and shot at, and the battle could end before the column moves.
    let mut state = two_side_battle(
        &reg,
        &["ggggggfgggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Rear"),
            unit_at([1, 0], 0, "medium_tank", "Lead"),
            unit_at([10, 0], 1, "medium_tank", "Bystander"),
        ],
        1,
    );
    let (rear, lead) = (UnitId(0), UnitId(1));
    let start = state.unit(rear).unwrap().pos;
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: lead,
                to: tactics_core::offset_to_hex(3, 0),
            },
        )
        .expect("the lead tank has open ground");
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: rear,
                to: tactics_core::offset_to_hex(2, 0),
            },
        )
        .expect("routing behind a friend is legal");

    let events = play_round(&reg, &mut state);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::UnitTrapped { .. })),
        "a friend is never an ambush: {events:?}"
    );
    assert_ne!(
        state.unit(rear).unwrap().pos,
        start,
        "the rear tank should have followed the lead one up the road"
    );
}

#[test]
fn friendlies_are_never_ordered_onto_the_same_hex() {
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["ggggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "First"),
            unit_at([1, 0], 0, "medium_tank", "Second"),
            unit_at([4, 0], 1, "medium_tank", "Bystander"),
        ],
        1,
    );
    let (first, second) = (UnitId(0), UnitId(1));
    let contested = tactics_core::offset_to_hex(3, 0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: first,
                to: contested,
            },
        )
        .expect("an empty hex is a fine destination");
    assert!(
        !reachable(&reg, &state, second).contains_key(&contested),
        "a hex a friend is already driving to is taken"
    );
    assert_eq!(
        state.apply(
            &reg,
            &Order::SetMove {
                unit: second,
                to: contested
            }
        ),
        Err(tactics_core::battle::OrderError::NoPath),
        "two units must not be ordered into the same hex"
    );
}

#[test]
fn unspotted_enemies_still_ambush() {
    // Crews look between ticks, not between hexes. A unit crossing several
    // hexes in one tick outruns its own eyes and can drive into somebody it
    // never saw; a unit ambling along a hex at a time normally spots the
    // enemy the tick before contact and simply stops.
    let reg = registry();
    // A forest curtain hides the ambusher, so the mover plans a route
    // through ground it cannot see into.
    let mut state = two_side_battle(
        &reg,
        &["ggfgggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Mover"),
            unit_at([4, 0], 1, "medium_tank", "Ambusher"),
        ],
        2,
    );
    let mover = UnitId(0);
    let ambusher = UnitId(1);
    assert!(
        !state.fog.side(0).spotted.contains(&ambusher),
        "ambusher must start unseen for this test"
    );
    // The ambusher's own tile: an unspotted enemy never blocks a
    // destination, which is what makes this order an ambush rather than a
    // refusal. Grass, forest, grass, grass costs exactly one round's fuel.
    let dest = tactics_core::offset_to_hex(4, 0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: mover,
                to: dest,
            },
        )
        .expect("pathing through fog should be attempted");
    for side in state.living_sides() {
        state.apply(&reg, &Order::Commit { side }).unwrap();
    }
    // Enough banked movement to run the whole leg inside the first tick,
    // before any fog recompute can warn the driver.
    state.unit_mut(mover).unwrap().move_credit = 64 * reg.scale.ticks_per_round;

    let events = state.step_tick(&reg);
    let trapped = events.iter().find_map(|e| match e {
        BattleEvent::UnitTrapped { unit, at } if *unit == mover => Some(*at),
        _ => None,
    });
    assert_eq!(
        trapped,
        Some(tactics_core::offset_to_hex(3, 0)),
        "the advance should stop on the tile before the ambusher, short of {dest:?}: {events:?}"
    );
    let mover = state.unit(mover).expect("the mover survives one tick");
    assert_eq!(mover.pos, tactics_core::offset_to_hex(3, 0));
    assert!(
        mover.intent.path.is_empty(),
        "the rest of the route is abandoned"
    );
}

#[test]
fn an_enemy_you_can_see_halts_the_advance_without_surprising_anyone() {
    // The other half of the same rule: contact with a spotted enemy ends the
    // route, but nobody was ambushed, so no `UnitTrapped`.
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["gggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Mover"),
            unit_at([2, 0], 1, "medium_tank", "Seen"),
        ],
        3,
    );
    let mover = UnitId(0);
    assert!(state.fog.side(0).spotted.contains(&UnitId(1)));
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: mover,
                to: tactics_core::offset_to_hex(1, 0),
            },
        )
        .unwrap();
    for side in state.living_sides() {
        state.apply(&reg, &Order::Commit { side }).unwrap();
    }
    // Point the route at the enemy the way a resolved tick would find it,
    // then hand the mover the fuel to try to drive through.
    state.unit_mut(mover).unwrap().intent.path = vec![
        tactics_core::offset_to_hex(1, 0),
        tactics_core::offset_to_hex(2, 0),
    ];
    state.unit_mut(mover).unwrap().move_credit = 64 * reg.scale.ticks_per_round;

    let events = state.step_tick(&reg);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::UnitTrapped { .. })),
        "an enemy in plain sight is a roadblock, not an ambush: {events:?}"
    );
    assert_eq!(
        state.unit(mover).map(|u| u.pos),
        Some(tactics_core::offset_to_hex(1, 0)),
        "the mover stops on the tile before the enemy"
    );
}

#[test]
fn hidden_enemies_do_not_show_up_as_holes_in_the_move_range() {
    // Refusing a move because an unseen enemy stands there would announce
    // its position, so the tile stays offered and the order becomes an
    // ambush instead.
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["ggfgggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Mover"),
            unit_at([4, 0], 1, "medium_tank", "Hidden"),
        ],
        4,
    );
    let mover = UnitId(0);
    let hidden = UnitId(1);
    let hidden_pos = state.unit(hidden).unwrap().pos;
    assert!(
        !state.fog.side(0).spotted.contains(&hidden),
        "precondition: the enemy is unseen"
    );
    assert!(
        reachable(&reg, &state, mover).contains_key(&hidden_pos),
        "an unseen enemy must not punch a hole in the move overlay"
    );

    // Ordering the move onto that tile is legal; the advance simply stops
    // when it runs into whoever is standing there.
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: mover,
                to: hidden_pos,
            },
        )
        .expect("the order must be accepted, not refused with NoPath");
    let events = play_round(&reg, &mut state);
    assert!(
        state.unit(mover).is_none_or(|u| u.pos != hidden_pos),
        "nobody drives through an occupied hex: {events:?}"
    );
}

#[test]
fn spotted_enemies_still_block_a_destination() {
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["ggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Mover"),
            unit_at([1, 0], 1, "medium_tank", "Seen"),
        ],
        5,
    );
    let mover = UnitId(0);
    let seen = UnitId(1);
    let seen_pos = state.unit(seen).unwrap().pos;
    assert!(
        state.fog.side(0).spotted.contains(&seen),
        "precondition: the enemy is in plain sight"
    );
    assert_eq!(
        state.apply(
            &reg,
            &Order::SetMove {
                unit: mover,
                to: seen_pos
            }
        ),
        Err(tactics_core::battle::OrderError::NoPath),
        "you cannot drive onto an enemy you can see; that is an attack"
    );
}

#[test]
fn faster_units_arrive_earlier_in_the_same_round() {
    // Movement points buy time, not just distance: both tanks cover three
    // hexes this round, but the light one is there long before the heavy one.
    // The enemy sits behind a forest curtain, so the round runs its full
    // length instead of ending in a shootout.
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["gggggfgggggg", "gggggfgggggg", "gggggfgggggg"],
        vec![
            unit_at([0, 0], 0, "light_tank", "Quick"),
            unit_at([0, 2], 0, "heavy_tank", "Slow"),
            unit_at([11, 1], 1, "medium_tank", "Bystander"),
        ],
        8,
    );
    assert!(
        state.fog.side(0).spotted.is_empty() && state.fog.side(1).spotted.is_empty(),
        "nobody should be in contact; this test is about the clock"
    );
    let (quick, slow) = (UnitId(0), UnitId(1));
    let targets = [
        (quick, tactics_core::offset_to_hex(3, 0)),
        (slow, tactics_core::offset_to_hex(3, 2)),
    ];
    for (unit, to) in targets {
        state.apply(&reg, &Order::SetMove { unit, to }).unwrap();
    }
    for side in state.living_sides() {
        state.apply(&reg, &Order::Commit { side }).unwrap();
    }

    let mut arrived: Vec<(UnitId, u32)> = Vec::new();
    for tick in 0..reg.scale.ticks_per_round {
        state.step_tick(&reg);
        for (unit, to) in targets {
            if state.unit(unit).is_some_and(|u| u.pos == to)
                && !arrived.iter().any(|(u, _)| *u == unit)
            {
                arrived.push((unit, tick));
            }
        }
    }
    let at = |unit: UnitId| arrived.iter().find(|(u, _)| *u == unit).map(|(_, t)| *t);
    let (quick_tick, slow_tick) = (at(quick), at(slow));
    assert!(
        quick_tick.is_some() && slow_tick.is_some(),
        "both should complete a three-hex move inside one round: {arrived:?}"
    );
    assert!(
        quick_tick < slow_tick,
        "the faster tank should get there first, but arrived at {quick_tick:?} against {slow_tick:?}"
    );
}

#[test]
fn reload_time_sets_the_rate_of_fire() {
    // An MG chatters through a round; an 88 gets a couple of shots off. The
    // targets are artillery, which never answers: indirect guns do not
    // snap-fire, so the cadence is measured undisturbed.
    let mut reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["gggggggg"],
        vec![
            unit_at([0, 0], 0, "recon_car", "Gunner"),
            unit_at([1, 0], 1, "artillery", "Near"),
            unit_at([4, 0], 0, "tank_destroyer", "Sniper"),
            unit_at([7, 0], 1, "artillery", "Far"),
        ],
        12,
    );
    let (mg_carrier, sniper) = (UnitId(0), UnitId(2));
    let (near, far) = (UnitId(1), UnitId(3));
    // Cadence, not lethality: with real penetration an 88 that gets in can
    // end the battle mid-round and cut the count short, so the targets are
    // given absurd plate. Ordered fire has no value gate — the guns keep
    // shooting, the rounds keep bouncing, and only the metronome is
    // measured.
    for target in [near, far] {
        let vid = state.unit(target).unwrap().vehicle.clone();
        if let Some(v) = reg.vehicles.get_mut(&vid) {
            v.armor = tactics_core::data::ArmorSpec {
                front: 99,
                side: 99,
                rear: 99,
            };
        }
    }
    for (unit, target) in [(mg_carrier, near), (sniper, far)] {
        state
            .apply(
                &reg,
                &Order::SetFire {
                    unit,
                    fire: FireIntent::Target { target, weapon: 0 },
                },
            )
            .expect("both targets are spotted and in range");
    }

    let events = play_round(&reg, &mut state);
    let shots = |weapon: &str| {
        events
            .iter()
            .filter(|e| matches!(e, BattleEvent::ShotFired { weapon: w, .. } if w == weapon))
            .count()
    };
    // 12 ticks a round at five seconds each: an MG reloads in 2 ticks, the
    // 88 in 4, so a minute of fighting is six bursts against three shells.
    assert_eq!(shots("mg"), 6, "an MG should fire every second tick");
    assert_eq!(shots("gun_88"), 3, "an 88 gets three shots a round");
}

#[test]
fn two_crews_can_kill_each_other_in_the_same_tick() {
    // Shots inside one tick happen together, so being processed first is not
    // an advantage. Both wrecks burn and the battle is a draw.
    let reg = registry();
    let mut drawn = false;
    for seed in 0..40 {
        let mut state = duel(&reg, seed);
        let (west, east) = (UnitId(0), UnitId(1));
        // One cadet still fighting — already wounded, so any hit that finds
        // her is her last — and nothing else aboard but the gun: a single
        // penetration finishes the vehicle, and both crews are in that
        // state when both rounds arrive in the same tick. (Wounded rather
        // than fine because a 75 is deliberately below the savage
        // threshold now: it wounds before it kills.)
        for unit in [west, east] {
            let u = state.unit_mut(unit).unwrap();
            let seats = u.crew.len();
            u.crew_state = vec![tactics_core::battle::CrewCondition::Out; seats];
            if seats > 0 {
                u.crew_state[0] = tactics_core::battle::CrewCondition::Wounded;
            }
            for (id, hits) in u.modules.iter_mut() {
                if id != "main_gun" {
                    *hits = 0;
                }
            }
        }
        for (unit, target) in [(west, east), (east, west)] {
            state
                .apply(
                    &reg,
                    &Order::SetFire {
                        unit,
                        fire: FireIntent::Target { target, weapon: 0 },
                    },
                )
                .unwrap();
        }
        for side in state.living_sides() {
            state.apply(&reg, &Order::Commit { side }).unwrap();
        }
        let events = state.step_tick(&reg);
        let dead = events
            .iter()
            .filter(|e| matches!(e, BattleEvent::UnitDestroyed { .. }))
            .count();
        if dead == 2 {
            assert_eq!(
                state.over.map(|r| (r.winner, r.reason)),
                Some((None, EndReason::Eliminated)),
                "if everyone dies at once nobody won"
            );
            drawn = true;
            break;
        }
    }
    assert!(
        drawn,
        "in forty tries, two tanks shooting each other point blank never both died"
    );
}

#[test]
fn a_unit_that_spent_the_round_driving_still_shoots_back() {
    // What used to be a hard-coded counterattack is now ordinary opportunity
    // fire, and it costs nothing to have been busy: the crew answers whoever
    // shoots at them, even mid-move. Softened so the scene stays about
    // movement: an unsoftened 75 can destroy the main gun with the opening
    // hit, and a disarmed crew proves nothing either way.
    let mut reg = registry();
    soften(&mut reg);
    let mut state = duel(&reg, 21);
    let (west, east) = (UnitId(0), UnitId(1));
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: west,
                to: tactics_core::offset_to_hex(0, 0),
            },
        )
        .expect("west has room to reposition");
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: east,
                fire: FireIntent::Target {
                    target: west,
                    weapon: 0,
                },
            },
        )
        .unwrap();

    let events = play_round(&reg, &mut state);
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::ShotFired {
                attacker,
                opportunity: true,
                ..
            } if *attacker == west
        )),
        "a unit under orders to move should still answer fire: {events:?}"
    );
}

#[test]
fn holding_fire_means_watching_not_idling() {
    // `Hold` is overwatch, not passivity: the crew shoots at whatever their
    // fog turns up without being told to.
    let reg = registry();
    let mut state = duel(&reg, 5);
    let west = UnitId(0);
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: west,
                fire: FireIntent::Hold,
            },
        )
        .unwrap();
    let events = play_round(&reg, &mut state);
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::ShotFired { attacker, opportunity: true, .. } if *attacker == west
        )),
        "a unit holding fire should engage a visible enemy: {events:?}"
    );
}

#[test]
fn orders_are_closed_once_the_round_is_resolving() {
    let reg = registry();
    let mut state = duel(&reg, 9);
    let west = UnitId(0);
    assert_eq!(
        state.apply(&reg, &Order::Commit { side: 0 }),
        Ok(Vec::new()),
        "one side committing is not enough to start the round"
    );
    assert!(state.is_planning(), "still waiting on the other side");
    assert_eq!(
        state.apply(
            &reg,
            &Order::SetFire {
                unit: west,
                fire: FireIntent::Hold
            }
        ),
        Err(tactics_core::battle::OrderError::AlreadyCommitted),
        "a side cannot rewrite orders it has already handed in"
    );
    state.apply(&reg, &Order::Commit { side: 1 }).unwrap();
    assert_eq!(state.resolving_tick(), Some(0), "now the round runs");
    assert_eq!(
        state.apply(
            &reg,
            &Order::SetMove {
                unit: west,
                to: tactics_core::offset_to_hex(1, 1)
            }
        ),
        Err(tactics_core::battle::OrderError::NotPlanningPhase),
    );
}

#[test]
fn a_round_clears_last_round_orders() {
    let reg = registry();
    // Out of contact, so the round runs to its end instead of the battle
    // being decided inside it.
    let mut state = standoff(&reg, 13);
    let west = UnitId(0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: west,
                to: tactics_core::offset_to_hex(0, 0),
            },
        )
        .unwrap();
    assert!(state.unit(west).unwrap().planned);
    let round_before = state.round;

    play_round(&reg, &mut state);
    if let Some(unit) = state.unit(west) {
        assert!(
            state.round > round_before,
            "the round should have turned over"
        );
        assert!(!unit.planned, "orders do not carry into the next round");
        assert!(unit.intent.path.is_empty());
        assert_eq!(unit.move_credit, 0, "unspent movement does not bank");
    }
}

/// How far side 0's plan leaves it from the nearest enemy, averaged over
/// seeds. Lower means it closed the distance; higher means it kept its
/// distance. Used to show that doctrine changes behaviour.
///
/// The ground is chosen to pose the question: two tanks in the open with a
/// visible enemy ahead of them and a belt of woods behind. Closing and
/// digging in are both available, and the doctrine decides which.
fn mean_approach(reg: &DataRegistry, doctrine: &str, difficulty: u8) -> f32 {
    let mut total = 0.0;
    let seeds = 0u64..8;
    let count = (seeds.end - seeds.start) as f32;
    for seed in seeds {
        let mut state = two_side_battle(
            reg,
            &["ffgggggg", "ffgggggg", "ffgggggg"],
            vec![
                unit_at([2, 0], 0, "medium_tank", "Ours"),
                unit_at([2, 2], 0, "medium_tank", "Theirs"),
                unit_at([4, 1], 1, "medium_tank", "Enemy"),
            ],
            seed,
        );
        assert_eq!(
            state.fog.side(0).spotted.len(),
            1,
            "the doctrines are being asked what to do about an enemy they can see"
        );
        let cfg = AiConfig {
            planner: "utility".into(),
            difficulty,
            doctrine: Some(doctrine.into()),
        };
        let mut planner = make_battle_planner(&cfg, seed, reg);
        for _ in 0..32 {
            if state.has_committed(0) {
                break;
            }
            let order = planner.next_order(reg, &state, 0);
            let _ = state.apply(reg, &order);
        }
        let enemies: Vec<_> = state.side_units(1).map(|u| u.pos).collect();
        for unit in state.side_units(0) {
            let dest = unit.planned_destination();
            total += enemies
                .iter()
                .map(|e| dest.distance_to(*e))
                .min()
                .unwrap_or(0) as f32;
        }
    }
    total / count
}

#[test]
fn doctrine_changes_how_a_side_fights() {
    let reg = registry();
    let massed = mean_approach(&reg, "massed_armor", 5);
    let elastic = mean_approach(&reg, "elastic_defense", 5);
    assert!(
        massed < elastic,
        "massed armour should close ({massed}) where elastic defence holds back ({elastic})"
    );
}

#[test]
fn doctrine_survives_a_bad_commander() {
    // Difficulty is competence, doctrine is character. A clumsy massed-armour
    // opponent still comes at you; it just does it badly.
    let reg = registry();
    let massed = mean_approach(&reg, "massed_armor", 1);
    let elastic = mean_approach(&reg, "elastic_defense", 1);
    assert!(
        massed < elastic,
        "dropping difficulty must not turn one doctrine into the other: {massed} against {elastic}"
    );
}

#[test]
fn the_evaluator_reads_doctrine_rather_than_hard_coded_weights() {
    let reg = registry();
    let state = duel(&reg, 4);
    let tile = state.unit(UnitId(0)).unwrap().pos;
    let score = |doctrine: &str| {
        let eval = Evaluator::new(reg.doctrine(doctrine).unwrap().clone());
        eval.score_tile(&reg, &state, UnitId(0), tile).score
    };
    assert_ne!(
        score("massed_armor"),
        score("elastic_defense"),
        "two doctrines should not value the same ground identically"
    );
}

#[test]
fn an_unknown_planner_falls_back_instead_of_crashing() {
    let reg = registry();
    let state = duel(&reg, 6);
    let cfg = AiConfig {
        planner: "does_not_exist".into(),
        difficulty: 3,
        doctrine: None,
    };
    let mut planner = make_battle_planner(&cfg, 1, &reg);
    let order = planner.next_order(&reg, &state, 0);
    assert!(
        matches!(
            order,
            Order::SetMove { .. }
                | Order::SetFire { .. }
                | Order::SetGoal { .. }
                | Order::Commit { .. }
        ),
        "a typo in a mod should degrade to a working planner, got {order:?}"
    );
}

#[test]
fn a_searching_planner_cannot_read_the_enemys_orders() {
    // Planning is simultaneous, so nobody's orders are knowable while they
    // are being written. Search runs on a determinized copy of the battle:
    // unspotted enemies are gone, and the other side's plan is blank even
    // when it has already been written down and committed.
    let reg = registry();
    let mut state = duel(&reg, 21);
    let east = UnitId(1);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: east,
                to: tactics_core::offset_to_hex(4, 1),
            },
        )
        .expect("the east tank has open ground behind it");
    state.apply(&reg, &Order::Commit { side: 1 }).unwrap();
    assert!(
        state.unit(east).unwrap().planned,
        "precondition: side 1 has a plan"
    );

    let known = tactics_core::ai::determinize(&state, 0, 7);
    let seen = known
        .unit(east)
        .expect("a spotted enemy is still on the board");
    assert!(
        !seen.planned && seen.intent.path.is_empty(),
        "side 0 must not see what side 1 was ordered to do"
    );
    assert!(
        !known.has_committed(1),
        "and must not treat the enemy as done planning, or it would expect them to stand still"
    );
    assert!(
        known.unit(UnitId(0)).is_some_and(|u| u.side == 0),
        "its own units are untouched"
    );
}

#[test]
fn the_policy_planner_is_usable_on_its_own() {
    // MCTS leans on a plain utility planner to stand in for the enemy, so
    // that planner has to be constructible without any mod data at all.
    let reg = registry();
    let state = duel(&reg, 15);
    let mut planner = UtilityPlanner::with_difficulty(3, 99);
    let order = planner.next_order(&reg, &state, 1);
    assert!(!matches!(order, Order::ClearIntent { .. }));
}

/// Units used to spawn facing due east no matter where the enemy was, which on
/// a map where the sides deploy east and west handed the eastern side's rear
/// armour to its opponent until it happened to move or turn. A Panther is
/// armour 5 from the front and 2 from behind and the damage formula divides by
/// that number, so the bias was real and it fell on one side only.
#[test]
fn units_spawn_facing_the_enemy_rather_than_due_east() {
    let reg = registry();
    let state = duel(&reg, 7);
    let west = state.unit(UnitId(0)).expect("west alive");
    let east = state.unit(UnitId(1)).expect("east alive");

    assert_eq!(
        west.facing,
        west.pos.main_direction_to(east.pos),
        "the western unit should be looking at its enemy"
    );
    assert_eq!(
        east.facing,
        east.pos.main_direction_to(west.pos),
        "the eastern unit should be looking at its enemy, not away from it"
    );

    // The specific regression: the two must not be pointing the same way.
    assert_ne!(
        west.facing, east.facing,
        "two units facing each other cannot share a facing"
    );

    // And what actually matters: a head-on shot lands on front armour.
    assert_eq!(
        tactics_core::battle::struck_facing(east.pos, east.facing, west.pos),
        tactics_core::data::ArmorFacing::Front,
        "a head-on shot should strike the front, not the rear"
    );
}

/// A scenario may still say which way someone is looking — that is what makes
/// an ambush placeable rather than something the engine decides for you.
#[test]
fn a_map_can_place_a_unit_looking_the_wrong_way() {
    use tactics_core::map::Facing;
    let reg = registry();
    let mut placements = vec![
        unit_at([0, 1], 0, "medium_tank", "West"),
        unit_at([3, 1], 1, "medium_tank", "East"),
    ];
    placements[1].facing = Some(Facing::East);
    let state = two_side_battle(&reg, &["ggggg", "ggggg", "ggggg"], placements, 7);

    let east = state.unit(UnitId(1)).expect("east alive");
    assert_eq!(
        east.facing,
        hexx::EdgeDirection::POINTY_EAST,
        "an explicit facing must survive the point-at-the-enemy pass"
    );
    let west = state.unit(UnitId(0)).expect("west alive");
    assert_eq!(
        tactics_core::battle::struck_facing(east.pos, east.facing, west.pos),
        tactics_core::data::ArmorFacing::Rear,
        "a unit told to look away presents its rear, which is the point"
    );
}

/// The point of the roster: a cadet is the same person on either side of a
/// battle. Before this she was a lookup into static mod data, so nothing that
/// happened to her could be recorded anywhere.
#[test]
fn girls_persist_across_battles_and_recover_over_days() {
    use tactics_core::roster::{CadetStatus, CasualtyRules, CrewFate};

    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 9).expect("overworld");

    // The map named crews by definition id; the world turned them into people.
    assert!(
        state.roster.len() >= 3,
        "frontier's armies should have enlisted their crews"
    );
    let army = state.side_armies(0).next().unwrap();
    let cadet = army.units[0].crew[0];
    assert_eq!(
        state.roster.get(cadet).unwrap().owner,
        0,
        "a cadet belongs to the academy whose army she rides with"
    );
    assert_eq!(state.roster.get(cadet).unwrap().battles, 0);

    // Surviving a battle is recorded on her, not on the vehicle.
    let attacker = state.side_armies(0).next().unwrap().id;
    let defender = state.side_armies(1).next().unwrap().id;
    let survivors = vec![(attacker, state.army(attacker).unwrap().units.clone())];
    state.apply_battle_result(
        &reg,
        &BattleReport::of(attacker, defender, survivors.clone(), Vec::new()),
    );
    assert_eq!(state.roster.get(cadet).unwrap().battles, 1);

    // And so is being shot out of it. With permadeath off, the worst case is
    // a long recovery rather than a funeral.
    state.rules = CasualtyRules { permadeath: false };
    let loss = tactics_core::overworld::CrewLoss {
        cadet,
        vehicle: state.army(attacker).unwrap().units[0].vehicle.clone(),
        killed_by: Some(tactics_core::data::DamageType::Kinetic),
        found: None,
        aid: tactics_core::data::AVERAGE,
    };
    let events = state.apply_battle_result(
        &reg,
        &BattleReport::of(attacker, defender, Vec::new(), vec![loss]),
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            OverworldEvent::CrewCasualty { fate, .. } if !matches!(fate, CrewFate::Killed)
        )),
        "permadeath is off, so nobody should die: {events:?}"
    );

    // Whatever befell her, it is temporary, and the campaign clock resolves it.
    let status = state.roster.get(cadet).unwrap().status;
    assert!(
        !status.is_permanent(),
        "no permanent losses with the rule off"
    );
    if let Some(days) = status.days_out().filter(|d| *d > 0) {
        for _ in 0..days {
            state.roster.advance_day();
        }
        assert_eq!(
            state.roster.get(cadet).unwrap().status,
            CadetStatus::Ready,
            "she should come back after her days are served"
        );
    }
}

/// `Lost` is not a euphemism: she bailed out, could not reach her own side
/// before the shooting stopped, and is walking home.
#[test]
fn a_lost_girl_walks_back_rather_than_being_gone() {
    use tactics_core::roster::{CadetStatus, Roster};
    let mut roster = Roster::new();
    let reg = registry();
    let cadet = roster
        .enlist_from_registry(&reg, 0, "anka")
        .expect("anka exists");
    roster.get_mut(cadet).unwrap().status = CadetStatus::Lost { days: 2 };

    assert!(!roster.get(cadet).unwrap().status.is_permanent());
    roster.advance_day();
    assert_eq!(
        roster.get(cadet).unwrap().status,
        CadetStatus::Lost { days: 1 },
        "still walking"
    );
    roster.advance_day();
    assert!(
        roster.get(cadet).unwrap().status.is_ready(),
        "she made it back"
    );
}

/// A trait changes *whether or when* a rule applies, which is what separates
/// it from a skill. Juno's lead foot is the clearest case: the same cadet in the
/// same tank drives differently depending on what is under her tracks.
#[test]
fn a_trait_can_depend_on_where_the_check_is_happening() {
    let reg = registry();
    let mut roster = tactics_core::roster::Roster::new();
    let juno = roster
        .enlist_from_registry(&reg, 0, "juno")
        .expect("juno exists");
    assert!(
        reg.character("juno")
            .unwrap()
            .traits
            .contains(&"lead_foot".into()),
        "this test is about her lead foot"
    );

    let on_road = roster
        .skill_level(
            &reg,
            juno,
            "driving",
            &tactics_core::data::CheckContext {
                terrain: Some("road"),
                ..Default::default()
            },
        )
        .unwrap();
    let off_road = roster
        .skill_level(
            &reg,
            juno,
            "driving",
            &tactics_core::data::CheckContext {
                terrain: Some("mud"),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        on_road > off_road,
        "a lead foot should be quick on a road and worse off it: {on_road} vs {off_road}"
    );

    // And the gift and the cost are both real, measured against the cadet she
    // would have been without it.
    let plain = reg.skill("driving").unwrap().level_for(
        &reg.core_index,
        &roster.get(juno).unwrap().cores,
        roster.get(juno).unwrap().skills.get("driving").copied(),
    );
    assert!(on_road > plain, "the gift");
    assert!(off_road < plain, "and the cost");
}

/// Traits that are always on still have to cut both ways, or they are just a
/// skill with a name.
#[test]
fn a_paired_trait_costs_something() {
    let reg = registry();
    let mut roster = tactics_core::roster::Roster::new();
    let nadja = roster
        .enlist_from_registry(&reg, 0, "nadja")
        .expect("nadja exists");
    let ctx = tactics_core::data::CheckContext::default();

    let cores = roster.get(nadja).unwrap().cores.clone();
    let plain = |skill: &str| {
        reg.skill(skill).unwrap().level_for(
            &reg.core_index,
            &cores,
            roster.get(nadja).unwrap().skills.get(skill).copied(),
        )
    };
    assert!(
        roster.skill_level(&reg, nadja, "gunnery", &ctx).unwrap() > plain("gunnery"),
        "deliberate makes her a better shot"
    );
    assert!(
        roster
            .skill_level(&reg, nadja, "observation", &ctx)
            .unwrap()
            < plain("observation"),
        "and she stops watching anything else while she does it"
    );
}

/// The reaction rules answer "how long before she acts", which is the number
/// slice 5 will spend when a cadet has to respond to something she was not
/// told about. Nothing consumes it yet — see the note in
/// `assets/wiki/reference/cadets.md` on why gating *planned* execution was the
/// wrong place for it.
#[test]
fn reaction_delay_reads_the_crew_that_is_aboard() {
    let reg = registry();
    let mut roster = tactics_core::roster::Roster::new();

    let make = |roster: &mut tactics_core::roster::Roster, id: &str, speed: i32, trained: i32| {
        let def: tactics_core::data::CharacterDef = serde_json::from_value(serde_json::json!({
            "id": id, "name": id,
            "cores": { "speed": speed, "will": 10 },
            "skills": { "reactions": trained },
        }))
        .unwrap();
        roster.enlist(0, &def, &reg)
    };
    let quick = make(&mut roster, "quick", 16, 16);
    let slow = make(&mut roster, "slow", 5, 5);

    let delay = |cadet| {
        reg.reaction.delay(
            roster
                .skill_level(&reg, cadet, "reactions", &Default::default())
                .unwrap(),
        )
    };
    assert!(
        delay(quick) < delay(slow),
        "a quick crew should be ready sooner: {} vs {}",
        delay(quick),
        delay(slow)
    );
    // Reactions 16 against an average of 10 shaves one tick off the base of
    // two at four points a tick; it takes a genuinely exceptional crew to act
    // the instant they are told.
    assert_eq!(delay(quick), 1);
    assert_eq!(
        reg.reaction.delay(30),
        0,
        "and there is a ceiling: nobody acts before they are told"
    );
}

/// Difficulty is a mod, so the whole system has to switch off in data. Nothing
/// in Rust may need changing to get orders that simply happen.
#[test]
fn a_gentle_mod_removes_reaction_delay_entirely() {
    let mut reg = registry();
    let ordinary = reg.reaction.delay(tactics_core::data::AVERAGE);
    assert!(ordinary > 0, "the shipped rules make crews take a moment");

    reg.reaction = tactics_core::data::ReactionRules {
        base_ticks: 0,
        max_ticks: 0,
        ..reg.reaction.clone()
    };
    for level in [0, tactics_core::data::AVERAGE, 20] {
        assert_eq!(reg.reaction.delay(level), 0, "level {level}");
    }
}

/// A crew that has taken enough will not drive into more of it. They are not
/// out of the fight — they still shoot — they simply stop advancing, which is
/// what frightened people do.
#[test]
fn a_breaking_crew_refuses_to_advance_and_says_so() {
    let reg = registry();
    let mut state = duel(&reg, 21);

    // Put one crew past the last rung of the ladder.
    let breaking = reg
        .morale
        .rungs
        .last()
        .expect("the shipped ladder has rungs")
        .at_pressure;
    state.units[0].pressure = breaking;
    assert!(
        !state.obeys(&reg, state.unit(UnitId(0)).unwrap()),
        "this test needs a crew that has stopped obeying"
    );

    // Open ground toward the enemy, not the enemy's own tile, which is
    // occupied and therefore unpathable.
    let forward = tactics_core::offset_to_hex(1, 1);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: forward,
            },
        )
        .expect("the order is accepted; it is the crew who decline");
    let events = play_round(&reg, &mut state);

    // Asserted as behaviour rather than as a position: a duel can kill her
    // during the round, and a dead unit has no position to compare.
    assert!(
        !events.iter().any(|e| matches!(
            e,
            BattleEvent::UnitMoved { unit, .. } if *unit == UnitId(0)
        )),
        "a breaking crew should not have advanced a hex: {events:?}"
    );
    // And it must be attributable. An order that quietly fails is
    // indistinguishable from a bug.
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::Defied { unit, .. } if *unit == UnitId(0)
        )),
        "the refusal has to be said out loud: {events:?}"
    );
}

// --- defiance: what a crew does instead ------------------------------------

/// Two mediums far enough apart on a long field that a frightened crew has
/// somewhere to reverse to. `duel`'s map is five columns wide, which is a
/// fine place to shoot at somebody and no place at all to run away.
fn flight_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    let row = "g".repeat(20);
    two_side_battle(
        reg,
        &[&row, &row, &row],
        vec![
            unit_at([8, 1], 0, "medium_tank", "Runner"),
            unit_at([13, 1], 1, "medium_tank", "Gun"),
        ],
        seed,
    )
}

/// The seed the flight stage is fought on.
///
/// Shared by the two tests below because it is one stage, and it is a
/// constant because it is *staging* rather than the rule: `flight_stage` is a
/// single duel, so on most seeds the gun behind her gets through on the
/// opening round and her crew bails, which proves nothing either way. It
/// moved from 61/62 to 84 when the resolver-depth arc changed what a shot at
/// a moving vehicle is worth and shifted every roll after the first, and from
/// 84 to 3 when Wave 2 gave the rout its second key: among the hexes that tie
/// on distance from the gun she now takes the quietest rather than the
/// woodiest, so she reverses to a different hex of the same ring, presents a
/// different aspect, and on seed 84 the gun killed her in round two before
/// there was anything left to watch. Scanned rather than guessed: 63 of the
/// first 400 seeds have her open the range in the first round and in two of
/// three, and 3 is the lowest. If it has to move again, scan for a seed on
/// which she survives to be watched — never weaken what is asserted about
/// her, which is the part that is not staging.
const FLIGHT_SEED: u64 = 3;

#[test]
fn a_frightened_crew_reverses_out_of_contact() {
    // The review's second fun tax, and the shape of the defect was worse than
    // it read: a crew who would not advance would not retreat either, and
    // would not even break for cover, because all three went through one
    // `obeys` gate. "A broken unit that cannot retreat is free kills for the
    // enemy" — so morale narrated a death spiral instead of buying anything.
    //
    // Away from what is shooting at her, note, and not toward a lane. She is
    // not navigating.
    let mut reg = registry_wireless();
    always(&mut reg, "flight");
    let mut state = flight_stage(&reg, FLIGHT_SEED);
    state.units[0].pressure = breaking(&reg);

    let before = state.unit(UnitId(0)).expect("on the field").pos;
    let enemy = state.unit(UnitId(1)).expect("on the field").pos;
    let events = play_round(&reg, &mut state);
    let after = state.unit(UnitId(0)).expect("on the field").pos;

    assert!(
        after.distance_to(enemy) > before.distance_to(enemy),
        "she should have put ground between herself and the gun: {before:?} -> {after:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::Defied { unit, to: Some(_), .. } if *unit == UnitId(0)
        )),
        "and it has to be said out loud, with where she went: {events:?}"
    );
}

#[test]
fn a_crew_cannot_refuse_the_decision_she_made_herself() {
    // The trap under the whole feature. A refusing crew has her ordered path
    // thrown away; flight then lays a path of her own. If the refusal check
    // cannot tell the two apart it selects her again on the next tick, throws
    // her own route away, lays it again, and she stands in place shaking for
    // the rest of the battle — a livelock that looks exactly like the freeze
    // this was built to remove.
    let mut reg = registry_wireless();
    always(&mut reg, "flight");
    let mut state = flight_stage(&reg, FLIGHT_SEED);
    state.units[0].pressure = breaking(&reg);
    let enemy = state.unit(UnitId(1)).expect("on the field").pos;

    let mut range = state
        .unit(UnitId(0))
        .expect("on the field")
        .pos
        .distance_to(enemy);
    let mut opened = 0;
    for _ in 0..3 {
        if state.is_over() {
            break;
        }
        play_round(&reg, &mut state);
        let Some(me) = state.unit(UnitId(0)) else {
            break;
        };
        let now = me.pos.distance_to(enemy);
        if now > range {
            opened += 1;
        }
        range = now;
    }
    assert!(
        opened >= 2,
        "she has to keep going, not re-argue with herself every tick"
    );
}

#[test]
fn a_crew_gone_to_ground_will_not_fire_on_her_own_initiative() {
    // Freeze had to cost something or the third response was a label rather
    // than a rule. She is a passenger in her own vehicle: nothing in front of
    // her prompts her to shoot. Her gun is not broken, though, and the
    // difference is the whole of it — an order still reaches the gunner.
    let mut reg = registry_wireless();
    always(&mut reg, "freeze");
    let mut state = duel(&reg, 63);
    state.units[0].pressure = breaking(&reg);

    let events = play_round(&reg, &mut state);
    assert!(
        !events.iter().any(|e| matches!(
            e,
            BattleEvent::ShotFired { attacker, opportunity: true, .. } if *attacker == UnitId(0)
        )),
        "she is not looking for a shot: {events:?}"
    );

    let mut told = duel(&reg, 63);
    told.units[0].pressure = breaking(&reg);
    told.apply(
        &reg,
        &Order::SetFire {
            unit: UnitId(0),
            fire: FireIntent::Target {
                target: UnitId(1),
                weapon: 0,
            },
        },
    )
    .expect("the order is accepted");
    let ordered = play_round(&reg, &mut told);
    assert!(
        ordered.iter().any(|e| matches!(
            e,
            BattleEvent::ShotFired { attacker, .. } if *attacker == UnitId(0)
        )),
        "but a target called by her commander is still shot at: {ordered:?}"
    );
}

#[test]
fn a_mod_that_names_no_defiance_freezes_exactly_as_it_always_did() {
    // Additivity, the same rule difficulty-as-a-mod and the zeroed command
    // block are held to. Freezing was the only thing a broken crew could ever
    // do, so a mod that declines to describe defiance must still get it —
    // which is also why `Freeze` is the enum's `#[default]` and why ties in
    // the score go to the first entry listed.
    let mut reg = registry_wireless();
    reg.morale.defiance.clear();
    let mut state = flight_stage(&reg, 64);
    state.units[0].pressure = breaking(&reg);

    let forward = tactics_core::offset_to_hex(10, 1);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: forward,
            },
        )
        .expect("the order is accepted");
    let events = play_round(&reg, &mut state);

    // Behaviour rather than a final position, for the reason
    // `a_breaking_crew_refuses_to_advance_and_says_so` states: the round can
    // kill her, and a dead unit has no position to compare.
    assert!(
        !events.iter().any(|e| matches!(
            e,
            BattleEvent::UnitMoved { unit, .. } if *unit == UnitId(0)
        )),
        "she does not move a hex, in any direction, which is what she always did: {events:?}"
    );
}

#[test]
fn the_senior_cadet_still_fighting_decides_how_the_crew_breaks() {
    // Not the best score aboard and not an average: somebody says "back her
    // out" or "keep firing" and the rest do it. In this engine that is
    // whoever is left in the most forward seat, so a commander going out
    // hands her temperament to the next woman down along with everything
    // else — the same leaders-last ordering the interior model already uses.
    let mut reg = registry_wireless();
    reg.morale.defiance = vec![
        tactics_core::data::DefianceDef {
            id: "fight".into(),
            name: "fights on".into(),
            response: tactics_core::data::DefianceResponse::Fight,
            core: Some("will".into()),
            base: 0,
        },
        tactics_core::data::DefianceDef {
            id: "flight".into(),
            name: "falls back".into(),
            response: tactics_core::data::DefianceResponse::Flight,
            core: Some("speed".into()),
            base: 0,
        },
    ];
    // Anka has will 13 and no speed of her own; Sofia has speed 12 and no
    // will. Read off the shipped characters on purpose — a temperament rule
    // that only works on invented cadets is not a rule about this game.
    let (mut state, ours) = crewed_stage(&reg, &["anka", "sofia"]);
    state.unit_mut(ours).expect("on the field").pressure = breaking(&reg);
    assert_eq!(
        state.defiance(&reg, state.unit(ours).expect("on the field")),
        tactics_core::data::DefianceResponse::Fight,
        "her commander is the steady one, so the tank is"
    );

    let unit = state.unit_mut(ours).expect("on the field");
    unit.crew_state = vec![
        tactics_core::battle::CrewCondition::Out,
        tactics_core::battle::CrewCondition::Fine,
    ];
    assert_eq!(
        state.defiance(&reg, state.unit(ours).expect("on the field")),
        tactics_core::data::DefianceResponse::Flight,
        "with the commander out it is the next cadet's nerve that answers"
    );
}

#[test]
fn an_officer_in_sight_settles_a_crew_faster() {
    // The half of the chain of command that had never paid anybody anything.
    // Losing a leader has cost a formation its nerve since `leader_lost` was
    // added; still having one bought nothing, so there was no reason beyond
    // succession bookkeeping to keep an officer alive.
    //
    // Sight rather than the radio net, and deliberately: a commander steadies
    // a frightened crew by being visibly still in it, which does not travel
    // down a wire. It is also the only version that leaves a zeroed `command`
    // block behaving identically to no block at all, because a crew with no
    // radio is out of contact under one and not the other.
    let mut reg = registry_wireless();
    reg.morale.recovery_near_leader = 3;
    let settled = |reg: &DataRegistry, blind: bool| -> u32 {
        // A map that declares a formation, because a rally is a thing a chain
        // of command does and `two_side_battle`'s map has no chain.
        let row = "g".repeat(50);
        let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
            "id": "rally_stage",
            "palette": { "g": "grass" },
            "rows": [&row, &row, &row],
            "formations": [{ "id": "ours", "side": 0 }],
        }))
        .expect("fixture map");
        let map = HexMap::from_map_file(&file).expect("map parses");
        let formed = |at: [i32; 2], name: &str, leads: bool| UnitPlacement {
            aboard_at: None,
            at,
            side: 0,
            vehicle: "medium_tank".into(),
            crew: Vec::new(),
            name: Some(name.into()),
            facing: None,
            formation: Some("ours".into()),
            leads,
        };
        // A second side is required or the battle is over before anybody
        // recovers anything — one living side wins immediately, and pressure
        // sheds at the *start* of the next round. She is parked forty hexes
        // off and blind to everyone, because a firefight would move this
        // number for reasons that are not the officer.
        let placements = vec![
            formed([0, 1], "Leader", true),
            formed([2, 1], "Follower", false),
            unit_at([45, 1], 1, "medium_tank", "Nobody"),
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
        let mut state = BattleState::from_placements(
            reg,
            map,
            sides,
            &placements,
            &crews,
            std::sync::Arc::new(roster),
            71,
        )
        .expect("the staged placements are content the base mod ships");
        // High enough that neither branch reaches the floor: pressure
        // saturates at zero, and a comparison against a floor measures the
        // floor.
        state.units[1].pressure = 60;
        if blind {
            // Put the officer where her crew cannot see her, by taking her
            // sight line rather than her life: killing her would charge the
            // formation `leader_lost` and measure that instead.
            // Twenty-odd hexes from her crew and twenty from the enemy:
            // vision is ten to twenty, so she is out of everybody's sight and
            // this measures the officer and nothing else. Parking her beside
            // the enemy instead started a firefight and moved the number.
            state.units[0].pos = tactics_core::offset_to_hex(25, 1);
        }
        // Recovery is paid when a round opens, which `resolve_round` reaches
        // at the end of the round it played.
        play_round(reg, &mut state);
        state.unit(UnitId(1)).expect("on the field").pressure
    };
    let with_her = settled(&reg, false);
    let without = settled(&reg, true);
    assert!(
        with_her < without,
        "a crew who can see her officer settles faster: {with_her} against {without}"
    );
    assert_eq!(
        without - with_her,
        reg.morale.recovery_near_leader,
        "and by exactly what the mod said, so the number means what it says"
    );
}

#[test]
fn a_side_that_sees_clearly_is_untouched_by_the_blur() {
    // Difficulty noise changed shape — from an independent draw per candidate
    // tile to one lean per unit per round — and the pin that has to survive
    // that is the additivity rule: difficulty is content, and the top of the
    // scale has none of it. A planner at difficulty 5 must plan exactly as it
    // would with the whole mechanism deleted.
    //
    // This is the cheap half of the check. The expensive half is the
    // determinism baseline, which fights at difficulty 3 and therefore moves
    // when this changes; if a future edit makes THIS test fail, the noise has
    // leaked into a side that is supposed to see the field as it is.
    let reg = registry_wireless();
    let orders_from = |difficulty: u8| -> Vec<String> {
        // A shipped map, because the point is a field with enough ground on
        // it to choose between. A twenty-hex test strip gives every planner
        // the same answer whatever it can see, which pins nothing.
        let mut state =
            BattleState::from_map(&reg, "battle_plains", 93).expect("a shipped battle map");
        let mut planner = UtilityPlanner::with_difficulty(difficulty, 5);
        let mut log = Vec::new();
        for _ in 0..6 {
            if state.is_over() {
                break;
            }
            log.push(format!("{:?}", planner.next_order(&reg, &state, 0)));
            play_round(&reg, &mut state);
        }
        log
    };
    let sharp = orders_from(5);
    assert!(!sharp.is_empty(), "the scene has to produce orders at all");
    assert_ne!(
        sharp,
        orders_from(3),
        "a blurred side must actually play differently, or this pins nothing"
    );
    // The real assertion: two difficulty-5 planners agree, and they agree
    // because neither of them drew anything, not because the rng happened to
    // land twice the same way.
    assert_eq!(sharp, orders_from(5));
}

// --- goals: an intention that outlives a round -----------------------------

#[test]
fn a_crew_keeps_the_goal_she_chose_until_it_is_finished() {
    // The point of the whole layer. A greedy planner re-decides where it is
    // going every round and therefore never gets anywhere; measured, that was
    // a medium tank driving 53 hexes over 19 rounds to end 10 hexes further
    // forward. A goal is kept, so the second round's plan is the first
    // round's plan continued.
    let reg = registry_wireless();
    let mut state =
        BattleState::from_map(&reg, "battle_plains", 101).expect("a shipped battle map");
    let mut planner = UtilityPlanner::with_difficulty(5, 11);

    let mut seen: Vec<(tactics_core::battle::Goal, tactics_core::Hex)> = Vec::new();
    for _ in 0..6 {
        if state.is_over() {
            break;
        }
        loop {
            let order = planner.next_order(&reg, &state, 0);
            if matches!(order, Order::Commit { .. }) {
                break;
            }
            let _ = state.apply(&reg, &order);
        }
        if let Some(me) = state.unit(UnitId(0))
            && let Some(goal) = me.goal
        {
            seen.push((goal, me.pos));
        }
        play_round(&reg, &mut state);
    }
    assert!(
        seen.len() >= 4,
        "she should be planning every round: {seen:?}"
    );
    assert!(
        seen.windows(2).any(|w| w[0].0 == w[1].0),
        "she has to carry an intention across a round at all: {seen:?}"
    );
    // The real rule, and the one worth pinning: she only ever changes her
    // mind by *finishing*. Anything else is the re-deciding this layer was
    // built to stop, and it would not show up as a goal that never persists —
    // it would show up as one that persists for a while and then wanders.
    for pair in seen.windows(2) {
        let ((was, _), (now, at)) = (pair[0], pair[1]);
        if was != now {
            assert_eq!(
                was,
                tactics_core::battle::Goal::Take(at),
                "she changed her goal without having arrived at the old one: {seen:?}"
            );
        }
    }
}

#[test]
fn an_order_replaces_her_own_ideas_rather_than_competing_with_them() {
    // The rule the direction memo exists to defend, restated where the goal
    // layer could most easily have undone it. A mission that names ground is
    // not one candidate among the objectives she likes the look of. Letting
    // it compete was the first draft, and it meant a crew under orders and a
    // crew with none chose the same ground, which is an order that has
    // stopped being one.
    //
    // Since subordinate initiative landed this is the **additivity contract**
    // for it, stated at zero: a doctrine with no initiative gets exactly the
    // two-entry list it always got. Not a list her own ideas are on and then
    // scored out of — on it, because the length of this list is how many
    // blurs the chooser draws from the rng, and a game that is the old game
    // must draw them at the old stream position too.
    let reg = registry_wireless();
    let state = BattleState::from_map(&reg, "battle_plains", 102).expect("a shipped map");
    let unit = UnitId(0);
    let free = tactics_core::ai::goal::candidates(&reg, &state, unit, None, None, 0.0);
    assert!(
        free.len() > 2,
        "with no orders she has the run of the map whatever her doctrine: {free:?}"
    );

    let told = tactics_core::offset_to_hex(30, 30);
    let under_orders = tactics_core::ai::goal::candidates(
        &reg,
        &state,
        unit,
        Some(&tactics_core::battle::Mission::Advance { to: told }),
        None,
        0.0,
    );
    assert_eq!(
        under_orders,
        vec![
            tactics_core::battle::Goal::Take(told),
            tactics_core::battle::Goal::Hold
        ],
        "told where to go, that is where she is going"
    );
}

#[test]
fn a_doctrine_with_initiative_puts_her_own_judgment_on_the_list_behind_the_order() {
    // The widening, and the two things about it that matter. The tile her own
    // sweep picked is *on* the list — that is the whole mechanism — and the
    // order is still at the head of it, because the chooser compares strictly
    // and this list's order is the tie-break. A widening that shuffled the
    // order into the middle would be the first draft all over again.
    //
    // What is deliberately *not* admitted is the map's other objectives.
    // Initiative is how she carries out an order, not whether she believes
    // it; deciding the far objective mattered more is `Unit::detached`.
    let reg = registry_wireless();
    let state = BattleState::from_map(&reg, "battle_plains", 102).expect("a shipped map");
    let unit = UnitId(0);
    let told = tactics_core::offset_to_hex(30, 30);
    let mine = tactics_core::offset_to_hex(3, 3);
    let mission = tactics_core::battle::Mission::Advance { to: told };
    let list = |initiative: f32| {
        tactics_core::ai::goal::candidates(
            &reg,
            &state,
            unit,
            Some(&mission),
            Some(mine),
            initiative,
        )
    };

    assert_eq!(
        list(0.0),
        vec![
            tactics_core::battle::Goal::Take(told),
            tactics_core::battle::Goal::Hold
        ],
        "no initiative is the two-entry list the goal layer always had"
    );
    assert_eq!(
        list(0.9),
        vec![
            tactics_core::battle::Goal::Take(told),
            tactics_core::battle::Goal::Take(mine),
            tactics_core::battle::Goal::Hold
        ],
        "and initiative adds her own answer, behind the order rather than in front of it"
    );
}

#[test]
fn two_crews_do_not_drive_for_the_same_hex() {
    // A section takes a piece of ground each. Said in the candidate list so
    // that it is said once and visibly, rather than as a tie-break buried in
    // whatever does the scoring — which is where the equivalent problem lived
    // before, as the plateau rule, and where it was very hard to see.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "battle_plains", 103).expect("a shipped map");
    let (first, second) = (UnitId(0), UnitId(1));
    let side = state.unit(first).expect("on the field").side;
    assert_eq!(
        state.unit(second).expect("on the field").side,
        side,
        "this test needs two crews of the same side"
    );

    let mine = tactics_core::ai::goal::candidates(&reg, &state, first, None, None, 0.0);
    let taken = mine
        .iter()
        .find_map(|g| match g {
            tactics_core::battle::Goal::Take(hex) => Some(*hex),
            tactics_core::battle::Goal::Hold => None,
        })
        .expect("there is ground worth having on this map");
    state.unit_mut(first).expect("on the field").goal =
        Some(tactics_core::battle::Goal::Take(taken));

    let hers = tactics_core::ai::goal::candidates(&reg, &state, second, None, None, 0.0);
    assert!(
        !hers.contains(&tactics_core::battle::Goal::Take(taken)),
        "somebody is already going there: {hers:?}"
    );
    assert!(
        hers.len() > 1,
        "and she still has somewhere of her own to go"
    );
}

/// The player has to be able to see a crew wavering *before* it costs them
/// something, or licence to disobey reads as the game cheating.
#[test]
fn crews_report_moving_up_the_ladder() {
    let mut reg = registry();
    // The scene needs an attritional fight: unsoftened, the first
    // penetration can find the ammunition rack and end the battle as a
    // brew-up before anyone's nerves are observable. Softened, pens wound
    // and frighten without destroying, which is the thing under test —
    // the ladder's reporting, not the gun's lethality.
    soften(&mut reg);
    let mut state = duel(&reg, 22);
    for side in state.living_sides() {
        state.apply(&reg, &Order::Commit { side }).unwrap();
    }

    // Fight until somebody has been hurt enough to move a rung.
    let mut said = false;
    for _ in 0..6 {
        if state.is_over() {
            break;
        }
        let events = play_round(&reg, &mut state);
        if events
            .iter()
            .any(|e| matches!(e, BattleEvent::MoraleChanged { .. }))
        {
            said = true;
            break;
        }
        for side in state.living_sides() {
            let _ = state.apply(&reg, &Order::Commit { side });
        }
    }
    assert!(said, "taking fire should eventually be reported as morale");
}

/// Difficulty is a mod. A one-rung ladder has to produce cadets who always do
/// as they are told, with nothing in Rust switched off to achieve it.
#[test]
fn a_gentle_mod_has_girls_who_never_refuse() {
    let mut reg = registry();
    reg.morale = tactics_core::data::MoraleRules {
        rungs: vec![tactics_core::data::MoraleRung {
            id: "steady".into(),
            name: "Steady".into(),
            at_pressure: 0,
            obeys: true,
            accuracy: 0,
            pinned: false,
        }],
        ..reg.morale.clone()
    };

    let mut state = duel(&reg, 23);
    state.units[0].pressure = 10_000;
    assert!(
        state.obeys(&reg, state.unit(UnitId(0)).unwrap()),
        "under a one-rung ladder no amount of pressure stops her"
    );

    // Open ground toward the enemy, not the enemy's own tile, which is
    // occupied and therefore unpathable.
    let forward = tactics_core::offset_to_hex(1, 1);
    let start = state.unit(UnitId(0)).unwrap().pos;
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: forward,
            },
        )
        .expect("ordered forward");
    let events = play_round(&reg, &mut state);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::Defied { .. })),
        "nobody refuses in the gentle game: {events:?}"
    );
    assert_ne!(
        state.unit(UnitId(0)).map(|u| u.pos),
        Some(start),
        "and she actually advances"
    );
}

// --- missions steering units -----------------------------------------------

#[test]
fn a_map_with_formations_but_no_missions_fights_exactly_as_the_flat_pool_did() {
    // The additivity hinge for the whole command system: formations that
    // have been given nothing to do must change nothing. Same seed, same
    // planners, one battle with its command state stripped — the event
    // streams must match to the byte, or the mission machinery is leaking
    // into battles that never asked for it.
    //
    // Succession narrowed this from "identical" to "identical in deeds", the
    // same way the command block did in chunk 5. A formation whose commander
    // burns hands over to the next cadet whether or not anybody priced a
    // radio, and says so — so `CommandPassed` is set aside here as words.
    // What it *costs* is `morale.leader_lost`, and at zero, which is what a
    // mod that never mentions the field gets, it costs nothing: the rest of
    // the stream has to match byte for byte.
    //
    // `recovery_near_leader` has to be zeroed for exactly the same reason and
    // was not, which cadence found. It is `leader_lost`'s mirror: a crew who
    // can see her formation's leader sheds extra pressure, and a *stripped*
    // battle has no formations for anybody to be near — so the two runs
    // genuinely differ in what a crew is carrying, whatever the currency
    // says. It stayed green for as long as nobody's pressure happened to
    // cross a rung inside the eight-round window, which is the definition of
    // a knife edge. Found the first time the AI drove anywhere different: at
    // event 214, unit 0 brews up, everybody who saw it takes `ally_destroyed`,
    // and unit 2 crosses to Wavering in the flat run and not in the other,
    // two points of rallying short. Zeroing both fields is what "a mod that
    // never mentions the chain of command" actually means.
    let reg = {
        let mut reg = registry_wireless();
        reg.morale.leader_lost = 0;
        reg.morale.recovery_near_leader = 0;
        reg
    };
    let run = |strip: bool| -> Vec<String> {
        let mut state = BattleState::from_map(&reg, "river_crossing", 21).unwrap();
        if strip {
            state.command = Default::default();
        }
        let mut ai = AiDriver::new();
        ai.insert(0, sharp_planner(&reg, 21, "massed_armor"));
        ai.insert(1, sharp_planner(&reg, 22, "elastic_defense"));
        let mut log = Vec::new();
        for _ in 0..8 {
            if state.is_over() {
                break;
            }
            ai.plan_round(&reg, &mut state);
            log.extend(state.resolve_round(&reg).iter().map(|e| format!("{e:?}")));
        }
        log
    };
    let with_formations = run(false);
    assert!(
        !with_formations.is_empty(),
        "the battle should do something"
    );
    let (spoken, deeds): (Vec<String>, Vec<String>) = with_formations
        .into_iter()
        .partition(|e| e.starts_with("CommandPassed"));
    assert!(
        !spoken.is_empty(),
        "and somebody's commander should be lost in it, or this proves nothing"
    );
    assert_eq!(
        deeds,
        run(true),
        "unmissioned formations must fight exactly as the flat pool did"
    );
}

#[test]
fn a_formation_advances_on_the_ground_its_mission_names() {
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 5).unwrap();
    let bridge = state
        .map
        .objectives()
        .iter()
        .find(|o| o.id == "bridge")
        .expect("river_crossing has a bridge")
        .anchor();
    let formation = formation_named(&state, "kuhlmann_armor");
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation,
                mission: Mission::Advance { to: bridge },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();

    let mut ai = AiDriver::new();
    ai.insert(0, sharp_planner(&reg, 5, "massed_armor"));
    ai.plan_round(&reg, &mut state);

    let members = state.formations()[formation.index()].members.clone();
    for id in members {
        let unit = state.unit(id).expect("nobody has died in planning");
        let before = unit.pos.distance_to(bridge);
        let after = unit.planned_destination().distance_to(bridge);
        assert!(
            after < before,
            "{} was ordered to the bridge and planned from {} to {} hexes away",
            unit.name,
            before,
            after
        );
    }
}

#[test]
fn an_ordered_withdrawal_needs_no_wounds() {
    // The evaluator's own exit pull is gated on damage, because an exit
    // nobody was ordered to take must not tempt an intact crew. A withdraw
    // *mission* is that order, so it pulls at full strength on full health —
    // which is what makes withdrawal a command decision rather than a
    // symptom.
    //
    // The pull, not the road: a withdrawal's route is priced for dead ground
    // (`balance.route_exposure.withdraw`), and a covered way out may begin
    // with a sidestep that brings her no nearer the lane this round. That is
    // the route rule's business and its own test's; here the road is the
    // straight one so what is read is whether the order pulls at all.
    let mut reg = registry_wireless();
    reg.balance.route_exposure.withdraw = 0;
    let mut state = BattleState::from_map(&reg, "river_crossing", 13).unwrap();
    let lane = state
        .map
        .objectives()
        .iter()
        .find(|o| o.id == "west_road")
        .expect("river_crossing has a western retreat lane")
        .hexes
        .clone();
    let formation = formation_named(&state, "kuhlmann_armor");
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation,
                mission: Mission::Withdraw {
                    via: "west_road".into(),
                },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();

    let mut ai = AiDriver::new();
    ai.insert(0, sharp_planner(&reg, 13, "massed_armor"));
    ai.plan_round(&reg, &mut state);

    let toward = |hex: tactics_core::Hex| lane.iter().map(|h| h.distance_to(hex)).min().unwrap();
    let members = state.formations()[formation.index()].members.clone();
    for id in members {
        let unit = state.unit(id).expect("planning harms nobody");
        assert!(
            (state.condition(&reg, unit) - 1.0).abs() < f32::EPSILON,
            "intact"
        );
        assert!(
            toward(unit.planned_destination()) < toward(unit.pos),
            "{} is unhurt and was still ordered out, so she heads for the lane",
            unit.name
        );
    }
}

#[test]
fn the_command_planner_assigns_missions_once_and_units_follow_them() {
    // A centralized doctrine (low delegation), because a devolved one
    // deliberately assigns no ground at all — see
    // a_devolved_commander_issues_no_ground_missions.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 9).unwrap();
    let mut ai = AiDriver::new();
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: Some("massed_armor".into()),
            },
            9,
            &reg,
        ),
    );

    // Round one: the commander divides the ground among her formations and
    // the driver carries the announcements out of the planning phase.
    let mut assigned = 0;
    ai.plan_round_with(&reg, &mut state, |d| {
        assigned += d
            .events
            .iter()
            .filter(|e| matches!(e, BattleEvent::MissionAssigned { .. }))
            .count();
    });
    let valkyrie_formations: Vec<_> = state.formations().iter().filter(|f| f.side == 1).collect();
    assert_eq!(
        assigned,
        valkyrie_formations.len(),
        "every formation gets a mission and each is said once"
    );
    assert!(
        valkyrie_formations.iter().all(|f| f.mission.is_some()),
        "the missions are standing on the formations"
    );
    assert!(state.has_committed(1), "and the side finishes its planning");

    // Round two: standing orders stand. The brain reviews and finds nothing
    // to change, so the log hears nothing.
    let _ = state.apply(&reg, &Order::Commit { side: 0 });
    state.resolve_round(&reg);
    assert!(state.is_planning(), "a new round has opened");
    let mut reassigned = 0;
    ai.plan_round_with(&reg, &mut state, |d| {
        reassigned += d
            .events
            .iter()
            .filter(|e| matches!(e, BattleEvent::MissionAssigned { .. }))
            .count();
    });
    assert_eq!(
        reassigned, 0,
        "an unchanged mission is not news, and re-announcing it every round would be"
    );
}

#[test]
fn a_commander_sends_her_grenadiers_to_hold_the_covered_ground() {
    // Composition-aware tasking. The brain divides the ground among its
    // formations, and until it could tell them apart it divided it *evenly*:
    // the grenadier section — a rifle platoon and the taxi carrying her —
    // drew a slot in the same rotation as three medium tanks and was ordered
    // to march at whatever objective came up next, beside armour that could
    // survive the trip. That is how a harness run buries most of its
    // infantry in the back of an APC.
    //
    // Two things are asserted, and neither of them names a chassis, because
    // neither does the brain: the section is told to *hold* rather than to
    // advance or assault, whatever the doctrine's appetite, and the ground
    // it is told to hold is the ground with cover in it. Both are read off
    // the hardware — somebody in the formation walks — the same way a base
    // of fire is recognised by somebody in it laying an indirect weapon.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "battle_plains", 17).expect("battle");
    let mut ai = AiDriver::new();
    ai.insert(
        0,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                // Aggression 0.85, so without the infantry branch the
                // grenadiers would be ordered to *assault* — the most
                // expensive thing a rifle platoon in a battle taxi can be
                // told to do.
                doctrine: Some("massed_armor".into()),
            },
            17,
            &reg,
        ),
    );

    let mut given: Vec<(String, Mission)> = Vec::new();
    ai.plan_round_with(&reg, &mut state, |d| {
        for event in &d.events {
            if let BattleEvent::MissionAssigned { formation, mission } = event {
                given.push((formation.clone(), mission.clone()));
            }
        }
    });

    let grenadiers = given
        .iter()
        .find(|(f, _)| f == "kuhlmann_grenadiers")
        .map(|(_, m)| m.clone())
        .expect("the grenadier section is given a mission like everybody else");
    let armor = given
        .iter()
        .find(|(f, _)| f == "kuhlmann_line")
        .map(|(_, m)| m.clone())
        .expect("so is the armoured line");

    let at = match grenadiers {
        Mission::Hold { at: Some(at) } => at,
        other => panic!("the grenadiers should be holding ground, not {other:?}"),
    };
    assert!(
        matches!(armor, Mission::Assault { .. } | Mission::Advance { .. }),
        "while the tanks are still sent to take it: {armor:?}"
    );

    // And the ground they were given is the covered ground. Scored the way
    // the brain scores it — mean cover over the objective's own hexes — so
    // a map edit that moves the trees moves this assertion with it rather
    // than pinning an objective by name.
    let cover = |objective: &tactics_core::map::Objective| -> i32 {
        let hexes: Vec<i32> = objective
            .hexes
            .iter()
            .filter_map(|h| state.map.get(*h))
            .filter_map(|t| reg.terrain(&t.terrain))
            .map(|def| def.cover)
            .collect();
        if hexes.is_empty() {
            return 0;
        }
        hexes.iter().sum::<i32>() / hexes.len() as i32
    };
    let held = state
        .map
        .objectives()
        .iter()
        .find(|o| o.anchor() == at)
        .expect("the anchor is an objective's");
    let best = state
        .map
        .objectives()
        .iter()
        .filter(|o| o.kind == tactics_core::map::ObjectiveKind::Hold)
        .map(cover)
        .max()
        .expect("the map has ground to hold");
    assert_eq!(
        cover(held),
        best,
        "the section on foot gets the ground it can hide in, not the next \
         slot in the rotation"
    );
}

#[test]
fn a_beaten_formation_is_ordered_out_by_its_commander() {
    // Withdrawal as a command decision: nobody in this formation consults
    // her own damage — the commander weighs the formation against her
    // doctrine's threshold and orders it out by the nearest lane.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 31).unwrap();
    let formation = formation_named(&state, "valkyrie_line");
    for id in state.formations()[formation.index()].members.clone() {
        maul(&reg, &mut state, id);
    }

    let mut ai = AiDriver::new();
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: Some("elastic_defense".into()),
            },
            31,
            &reg,
        ),
    );
    ai.plan_round(&reg, &mut state);

    match &state.formations()[formation.index()].mission {
        Some(Mission::Withdraw { via }) => assert_eq!(
            via, "east_road",
            "the lane is the nearest exit her side may use"
        ),
        other => panic!("a formation at a tenth strength should be ordered out, got {other:?}"),
    }
    // The intact formation hears nothing: elastic defence devolves command,
    // so its formations fight their own ground and are only ever *ordered*
    // to leave it.
    let screen = formation_named(&state, "valkyrie_screen");
    assert!(
        state.formations()[screen.index()].mission.is_none(),
        "a devolved commander does not micro-assign ground to a formation that is fighting well"
    );
}

#[test]
fn an_executor_only_command_fills_gaps_without_issuing_missions() {
    // The human hybrid, driven headlessly. The player's side has no brain —
    // she is the brain — so the object standing behind it must issue no
    // missions at all, plan the units of a formation she has given orders to,
    // and leave everyone else alone. That last part is the one worth pinning:
    // an AI side hands an unformationed unit to its fallback planner, which
    // sends her off to fight on her own judgment. Doing that on the player's
    // behalf would be inventing an order she never gave, so a unit outside
    // any mission gets today's "planned, watching" default instead.
    let reg = registry_wireless();
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "delegation_field",
        "palette": { "g": "grass" },
        "rows": ["gggggggggggggg", "gggggggggggggg", "gggggggggggggg"],
        "shape": "free",
        "sides": [{ "name": "Kuhlmann" }],
        "formations": [{ "id": "first", "name": "1st Platoon", "side": 0 }],
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let placement =
        |col: i32, row: i32, name: &str, formation: Option<&str>, leads: bool| UnitPlacement {
            aboard_at: None,
            at: [col, row],
            side: 0,
            vehicle: "medium_tank".into(),
            crew: Vec::new(),
            name: Some(name.into()),
            facing: None,
            formation: formation.map(str::to_string),
            leads,
        };
    let placements = vec![
        placement(0, 0, "Leader", Some("first"), true),
        placement(0, 1, "Follower", Some("first"), false),
        placement(0, 2, "Nobody's", None, false),
    ];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(&reg, &placements);
    let mut state = BattleState::from_placements(
        &reg,
        map,
        vec![SideState {
            name: "Kuhlmann".into(),
            ai: None,
        }],
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        17,
    )
    .expect("the staged placements are content the base mod ships");

    // The human's order, issued exactly as the UI issues it.
    let target = tactics_core::offset_to_hex(12, 1);
    let formation = FormationId(0);
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation,
                mission: Mission::Advance { to: target },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("the player may order her own formation");

    let mut ai = AiDriver::new();
    ai.insert(
        0,
        Box::new(tactics_core::ai::SideCommand::executor_only(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            17,
            &reg,
        )),
    );
    let mut spoken = 0;
    ai.plan_round_with(&reg, &mut state, |d| {
        assert!(d.rejected.is_none(), "the executors issued {:?}", d.order);
        spoken += d
            .events
            .iter()
            .filter(|e| matches!(e, BattleEvent::MissionAssigned { .. }))
            .count();
    });

    assert_eq!(
        spoken, 0,
        "an executor-only side has no commander and must never issue a mission"
    );
    assert!(state.has_committed(0), "and it closes the side's planning");

    for id in state.formations()[formation.index()].members.clone() {
        let unit = state.unit(id).expect("planning harms nobody");
        assert!(
            unit.planned_destination().distance_to(target) < unit.pos.distance_to(target),
            "{} is under a mission she can hear, so her executor drives her at it",
            unit.name
        );
    }

    let loose = state
        .side_units(0)
        .find(|u| state.formation_of(u.id).is_none())
        .expect("one unit answers to nobody");
    assert!(
        loose.planned,
        "she is accounted for, so the round can start"
    );
    assert!(
        loose.intent.path.is_empty() && loose.intent.fire == FireIntent::Hold,
        "but nobody ordered her anywhere, so she holds and watches: {:?}",
        loose.intent
    );
}

#[test]
fn initiative_moves_a_commander_on_and_obedience_does_not() {
    let reg = registry_wireless();

    // The balanced doctrine carries initiative 0.5: with the bridge already
    // hers, her formations are re-aimed at ground she does not hold.
    let mut state = BattleState::from_map(&reg, "river_crossing", 33).unwrap();
    state.objective_held[0] = Some(1);
    let mut ai = AiDriver::new();
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            33,
            &reg,
        ),
    );
    ai.plan_round(&reg, &mut state);
    let line = formation_named(&state, "valkyrie_line");
    let ford = state
        .map
        .objectives()
        .iter()
        .find(|o| o.id == "north_ford")
        .unwrap()
        .anchor();
    assert_eq!(
        state.formations()[line.index()].mission,
        Some(Mission::Advance { to: ford }),
        "initiative moves her off ground already taken"
    );

    // Massed armour carries initiative 0.3: the plan said the bridge, so
    // the bridge it is, held or not. (It says so as an *assault* rather than
    // an advance — that is the aggression split, pinned next door; what this
    // test is about is the hex it names.)
    let mut state = BattleState::from_map(&reg, "river_crossing", 33).unwrap();
    state.objective_held[0] = Some(0);
    let mut ai = AiDriver::new();
    ai.insert(
        0,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: Some("massed_armor".into()),
            },
            33,
            &reg,
        ),
    );
    ai.plan_round(&reg, &mut state);
    let armor = formation_named(&state, "kuhlmann_armor");
    let bridge = state.map.objectives()[0].anchor();
    assert_eq!(
        state.formations()[armor.index()].mission,
        Some(Mission::Assault { to: bridge }),
        "an obedient doctrine follows the letter of the plan"
    );
}

#[test]
fn an_aggressive_commander_orders_assaults_and_a_balanced_one_advances() {
    // Two orders for ground, and which one a commander gives is her
    // doctrine's answer to what the ground is worth. Massed armour (0.85)
    // will spend vehicles for it and says so: an assault presses through
    // whatever is firing. The balanced default (0.6) — which is what the
    // player's own delegated formations run under — orders a movement to
    // contact instead: take the bridge, but fight what you meet on the way.
    // Both name the same hex; the difference is entirely in what they will
    // pay for it.
    let reg = registry_wireless();
    let mission = |doctrine: Option<&str>| {
        let mut state = BattleState::from_map(&reg, "river_crossing", 33).unwrap();
        let mut ai = AiDriver::new();
        ai.insert(
            0,
            make_battle_planner(
                &AiConfig {
                    planner: "command".into(),
                    difficulty: 5,
                    doctrine: doctrine.map(str::to_string),
                },
                33,
                &reg,
            ),
        );
        ai.plan_round(&reg, &mut state);
        let armor = formation_named(&state, "kuhlmann_armor");
        state.formations()[armor.index()].mission.clone()
    };
    let bridge = BattleState::from_map(&reg, "river_crossing", 33)
        .unwrap()
        .map
        .objectives()[0]
        .anchor();
    assert_eq!(
        mission(Some("massed_armor")),
        Some(Mission::Assault { to: bridge }),
        "a doctrine that trades vehicles for ground orders the deliberate attack"
    );
    assert_eq!(
        mission(None),
        Some(Mission::Advance { to: bridge }),
        "and the balanced default moves to contact for the same hex"
    );
}

#[test]
fn a_devolved_commander_issues_no_ground_missions() {
    // Elastic defence devolves command (delegation 0.7): its formations
    // keep the whole-map judgment that is the doctrine's strength, and the
    // commander's only order is the one that is never devolved — leaving.
    // Measured before believed: pinning this doctrine to anchor hexes cost
    // it 16 of 24 wins against an unchanged opponent.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 17).unwrap();
    let mut ai = AiDriver::new();
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: Some("elastic_defense".into()),
            },
            17,
            &reg,
        ),
    );
    ai.plan_round(&reg, &mut state);
    assert!(
        state
            .formations()
            .iter()
            .filter(|f| f.side == 1)
            .all(|f| f.mission.is_none()),
        "her formations fight their own ground"
    );
    assert!(
        state.has_committed(1),
        "and the side still finishes planning"
    );
}

/// A doctrine can decide to break off, and until one did the engine's
/// withdrawal machinery was measured by nothing.
///
/// `wants_out` has been in `ai/command.rs` since chain-of-command shipped, and
/// no doctrine the base mod ships ever reached it: `beaten` asks whether a
/// formation is under `1 - withdraw_threshold` of the substance it started
/// with, and the shipped values put that at 15% for massed armour and 40% for
/// bounding overwatch — by which point the battle has usually decided itself.
/// A `grep -ci withdraw` over the four-seed determinism baseline returned
/// zero, which is why `planner.withdrawn_attack` swept bit-identical across
/// 180 battles: the term exists and nothing ever reads it.
///
/// `delaying_action` is a doctrine that breaks off after losing a fifth of
/// itself, which is what a delaying action is. The contrast with massed
/// armour at the same damage is the whole test — this is a decision the
/// content makes, not a threshold the engine imposes.
#[test]
fn a_doctrine_that_trades_ground_for_time_orders_its_own_withdrawal() {
    let reg = registry_wireless();
    let withdrawals = |doctrine: &str| {
        let mut state = BattleState::from_map(&reg, "river_crossing", 13).unwrap();
        let formation = formation_named(&state, "kuhlmann_armor");
        // A fifth of herself is the line `delaying_action` draws. One vehicle
        // of three is a third, which is past it and nowhere near massed
        // armour's fifteen percent.
        let first = state.formations()[formation.index()].members[0];
        strike_down(&mut state, first);

        let mut ai = AiDriver::new();
        // The *commander*, not the executor: `wants_out` lives in
        // `SideCommand`, which is the planner that reviews missions once a
        // round. `sharp_planner` builds the utility executor, which is why
        // every other withdrawal test in this file issues the mission by
        // hand.
        ai.insert(
            0,
            tactics_core::ai::make_battle_planner(
                &AiConfig {
                    planner: "command".into(),
                    difficulty: 5,
                    doctrine: Some(doctrine.into()),
                },
                13,
                &reg,
            ),
        );
        let mut out = Vec::new();
        ai.plan_round_with(&reg, &mut state, |d| {
            if let Order::SetMission {
                mission: Mission::Withdraw { via },
                ..
            } = &d.order
            {
                out.push(via.clone());
            }
        });
        out
    };

    assert!(
        !withdrawals("delaying_action").is_empty(),
        "a company that has lost a third of itself and was written to trade \
         ground for time is ordered out"
    );
    assert!(
        withdrawals("massed_armor").is_empty(),
        "and the same company under a doctrine that breaks off at fifteen \
         percent is not, which is the content making the decision"
    );
}

/// Eyes get sent to look, and the chassis is what says who they are.
///
/// `Mission::Recon` was code the commander could not issue: the executor
/// drove it, `planner.pull_under_fire` gated on it and
/// `Formation::latitude_for` handled it, and the mission chooser had
/// `Assault`, `Advance`, `Hold`, a base of fire and a withdrawal and nothing
/// that produced one. The chassis decides who *could* — a vehicle that sees
/// `planner.eyes_ratio_percent` further than her longest direct weapon
/// reaches — and the doctrine's `screening` decides whether this commander
/// *would*, which is the two-part answer the designer asked for.
#[test]
fn a_commander_who_screens_sends_her_eyes_to_look() {
    let reg = registry_wireless();
    let recons = |screening: f32| {
        let mut reg = reg.clone();
        for doctrine in reg.doctrines.values_mut() {
            doctrine.screening = screening;
            // Low enough that she issues ground missions at all: a doctrine
            // that devolves hands the whole question to her subordinates and
            // never reaches this branch, which is what hid the rule the first
            // time it was measured.
            doctrine.delegation = 0.2;
        }
        // `battle_hills`, because `river_crossing` has no formation whose
        // vehicles are only eyes: its recon car rides with the battery, and
        // the base-of-fire branch claims that formation first and rightly so.
        let mut state = BattleState::from_map(&reg, "battle_hills", 13).unwrap();
        let mut ai = AiDriver::new();
        ai.insert(
            0,
            tactics_core::ai::make_battle_planner(
                &AiConfig {
                    planner: "command".into(),
                    difficulty: 5,
                    doctrine: Some("massed_armor".into()),
                },
                13,
                &reg,
            ),
        );
        let mut out = Vec::new();
        ai.plan_round_with(&reg, &mut state, |d| {
            if let Order::SetMission {
                mission: Mission::Recon { .. },
                formation,
                ..
            } = &d.order
            {
                out.push(*formation);
            }
        });
        out
    };

    assert!(
        recons(0.0).is_empty(),
        "a commander who does not screen puts everybody in the line, which is \
         the game before this branch existed"
    );
    assert!(
        !recons(0.9).is_empty(),
        "and one who does sends the formation with the eyes in it to look"
    );
}
