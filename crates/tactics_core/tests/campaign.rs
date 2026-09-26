//! What ends a campaign, and what a battle does to the map afterwards.
//!
//! The overworld map declares its own ending (`victory`: ground to hold,
//! whether losing headquarters loses) and flags the army carrying
//! headquarters; a battle hands back a [`BattleReport`] the campaign reads
//! for who won ground, who fell back and who is gone. Both are additive —
//! a map that declares nothing is fought to elimination, exactly as every
//! campaign was — and the first test here is that pin.
//!
//! Contents: the frontier's own declaration; the additivity pin; the three
//! endings (elimination, decapitation, ground held at dawn); the
//! headquarters as the root of the net and the succession when it dies;
//! where a withdrawn army arrives; and the campaign planner's reading of the
//! rule — the enemy's headquarters worth the war, its own kept out of reach.

use hexx::Hex;
use tactics_core::ai::{AiConfig, AiPlanner};
use tactics_core::data::{DataRegistry, ValidationReport};
use tactics_core::map::CampaignVictory;
use tactics_core::overworld::{
    Army, ArmyId, ArmyMission, BattleReport, CampaignEnd, OverworldEvent, OverworldOrder,
    OverworldState, make_overworld_planner,
};

mod common;
use common::{registry, registry_wireless};

/// The shipped campaign, day one, with the radio switched off so a test about
/// the ending is not also a test about who can hear headquarters.
fn frontier(reg: &DataRegistry) -> OverworldState {
    OverworldState::from_map(reg, "frontier", 1).expect("the shipped campaign builds")
}

/// Every tile of the given terrain, in a fixed order.
fn tiles_of(state: &OverworldState, terrain: &str) -> Vec<Hex> {
    let mut hexes: Vec<Hex> = state
        .map
        .iter()
        .filter(|(_, t)| t.terrain == terrain)
        .map(|(h, _)| h)
        .collect();
    hexes.sort_by_key(|h| (h.x, h.y));
    hexes
}

/// A battle nobody was hurt in, reported with everybody's roster intact.
fn bloodless(state: &OverworldState, attacker: ArmyId, defender: ArmyId) -> BattleReport {
    let roster = |id: ArmyId| state.army(id).map(|a| a.units.clone()).unwrap_or_default();
    BattleReport::of(
        attacker,
        defender,
        vec![(attacker, roster(attacker)), (defender, roster(defender))],
        Vec::new(),
    )
}

/// A battle that wiped `loser` out, reported through the real path.
fn wipe(
    state: &mut OverworldState,
    reg: &DataRegistry,
    winner: ArmyId,
    loser: ArmyId,
) -> Vec<OverworldEvent> {
    let survivors = state
        .army(winner)
        .map(|a| a.units.clone())
        .unwrap_or_default();
    state.apply_battle_result(
        reg,
        &BattleReport::of(
            winner,
            loser,
            vec![(winner, survivors), (loser, Vec::new())],
            Vec::new(),
        ),
    )
}

fn ended(events: &[OverworldEvent]) -> Option<(Option<u8>, CampaignEnd)> {
    events.iter().find_map(|e| match e {
        OverworldEvent::GameEnded { winner, reason } => Some((*winner, *reason)),
        _ => None,
    })
}

/// End the active side's turn; the day turns over when the last side does.
fn end_turn(reg: &DataRegistry, state: &mut OverworldState) -> Vec<OverworldEvent> {
    state
        .apply(reg, &OverworldOrder::EndTurn)
        .expect("a turn can always be ended while the campaign runs")
}

/// A third army for a side, fielding nothing, standing somewhere. Used to
/// hem an army in; what these tests weigh is where an army *is*.
fn extra_army(state: &mut OverworldState, side: u8, at: Hex) -> ArmyId {
    let id = ArmyId(state.armies.len() as u32);
    state.armies.push(Army {
        id,
        side,
        name: format!("Extra {}", id.0),
        pos: at,
        movement: 3,
        moved: false,
        units: Vec::new(),
        alive: true,
        mission: None,
        headquarters: false,
    });
    id
}

// --- what the frontier says ------------------------------------------------

/// The shipped campaign declares what winning it is, and the declaration
/// reaches the state a screen reads.
#[test]
fn the_frontier_declares_what_winning_it_is() {
    let reg = registry_wireless();
    let state = frontier(&reg);
    assert_eq!(state.victory.hold, vec!["factory".to_string()]);
    assert!(state.victory.decapitation);
    assert_eq!(
        state.headquarters(0),
        Some(ArmyId(0)),
        "1st Company carries headquarters"
    );
    assert_eq!(
        state.headquarters(1),
        Some(ArmyId(2)),
        "the Vanguard carries the Valkyries'"
    );
    assert_eq!(state.victory.hold_days, 3);
    assert_eq!(
        state.hold_objective(&reg).as_deref(),
        Some("hold every Factory for 3 nights"),
        "the rule in words, for the banner"
    );
    assert_eq!(
        state.hold_progress(&reg, 0),
        Some((0, 2)),
        "two factories on the map and nobody holds either on day one"
    );
}

/// A map that says nothing about its ending is fought to elimination and
/// nothing else: the ground can be held for ever and the headquarters can
/// burn, and the campaign runs on.
#[test]
fn a_campaign_that_declares_no_ending_is_fought_to_elimination() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    state.victory = CampaignVictory::default();
    assert!(state.victory.is_empty());
    assert_eq!(state.hold_progress(&reg, 0), None);
    assert_eq!(state.hold_objective(&reg), None);

    // Every factory held through a night: nothing.
    for hex in tiles_of(&state, "factory") {
        state.owners.insert(hex, 0);
    }
    let mut events = end_turn(&reg, &mut state);
    events.extend(end_turn(&reg, &mut state));
    assert_eq!(state.turn, 2, "a day went by");
    assert_eq!(
        ended(&events),
        None,
        "held ground ends nothing here: {events:?}"
    );

    // The enemy headquarters wiped out: nothing, while the Reserve lives.
    let events = wipe(&mut state, &reg, ArmyId(0), ArmyId(2));
    assert_eq!(
        ended(&events),
        None,
        "a decapitation ends nothing here: {events:?}"
    );
    assert!(state.over.is_none());
    assert!(
        state.decapitated(1),
        "...though the fact itself is still readable"
    );

    // The last army: the ending every campaign has always had.
    let events = wipe(&mut state, &reg, ArmyId(0), ArmyId(3));
    assert_eq!(ended(&events), Some((Some(0), CampaignEnd::Elimination)));
    assert_eq!(state.over, Some(Some(0)));
}

// --- the three endings -----------------------------------------------------

/// Under `decapitation`, losing the army flagged as headquarters loses the
/// campaign on the spot, however many other armies the side still has.
#[test]
fn losing_the_headquarters_army_loses_the_campaign() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    let events = wipe(&mut state, &reg, ArmyId(0), ArmyId(2));
    assert_eq!(
        ended(&events),
        Some((Some(0), CampaignEnd::Decapitation)),
        "{events:?}"
    );
    assert!(
        state.side_armies(1).next().is_some(),
        "the Reserve is still on the map; it just has nobody to take orders from"
    );
    assert!(state.defeated(1));
    assert!(!state.defeated(0));
    assert_eq!(state.over, Some(Some(0)));
    assert_eq!(
        state.apply(&reg, &OverworldOrder::EndTurn),
        Err(tactics_core::overworld::OverworldError::GameOver),
        "nothing more can be ordered"
    );
}

/// Losing an army that is *not* headquarters loses nothing but the army.
#[test]
fn losing_an_ordinary_army_is_only_that() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    let events = wipe(&mut state, &reg, ArmyId(0), ArmyId(3));
    assert_eq!(ended(&events), None, "{events:?}");
    assert!(!state.defeated(1));
    assert!(state.over.is_none());
}

/// The ground rule fires at dawn, not at the moment of capture: taking the
/// last factory on your turn hands the enemy one turn to take it back, and
/// only a set still complete when the day turns wins.
#[test]
fn holding_every_factory_through_the_night_wins_the_campaign() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    state.victory.hold_days = 1;
    let factories = tiles_of(&state, "factory");
    assert_eq!(factories.len(), 2);

    // One of two: nothing, however many nights.
    state.owners.insert(factories[0], 0);
    let mut events = end_turn(&reg, &mut state);
    events.extend(end_turn(&reg, &mut state));
    assert_eq!(state.turn, 2);
    assert_eq!(
        ended(&events),
        None,
        "half the set is not the set: {events:?}"
    );
    assert_eq!(state.hold_progress(&reg, 0), Some((1, 2)));

    // The second, on side 0's turn: still nothing until the day turns.
    state.owners.insert(factories[1], 0);
    assert_eq!(state.active_side, 0);
    let events = end_turn(&reg, &mut state);
    assert_eq!(state.active_side, 1, "the Valkyries get their turn");
    assert_eq!(
        ended(&events),
        None,
        "the capture is not the win: {events:?}"
    );

    // The enemy takes one back on her turn: the set is broken at dawn.
    state.owners.insert(factories[1], 1);
    let events = end_turn(&reg, &mut state);
    assert_eq!(state.turn, 3);
    assert_eq!(
        ended(&events),
        None,
        "a factory retaken in time: {events:?}"
    );

    // Held through a whole night: won, and the ending is the day's last word.
    state.owners.insert(factories[1], 0);
    let _ = end_turn(&reg, &mut state);
    let events = end_turn(&reg, &mut state);
    assert_eq!(
        ended(&events),
        Some((Some(0), CampaignEnd::Held)),
        "{events:?}"
    );
    assert!(
        matches!(events.last(), Some(OverworldEvent::GameEnded { .. })),
        "the ending comes after the morning's traffic, not before it: {events:?}"
    );
    assert_eq!(state.over, Some(Some(0)));
}

/// `hold_days` is nights *in a row*: the set held at three dawns running
/// wins, a dawn without it starts the count again, and the count survives
/// a change of hands the other way. The frontier asks for three.
#[test]
fn the_ground_must_be_held_for_as_many_nights_running_as_the_map_asks() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    assert_eq!(state.victory.hold_days, 3, "the frontier asks for three");
    let factories = tiles_of(&state, "factory");
    for hex in &factories {
        state.owners.insert(*hex, 1);
    }
    let dawn = |state: &mut OverworldState| {
        let mut events = end_turn(&reg, state);
        events.extend(end_turn(&reg, state));
        events
    };

    // Night one, night two: held, counted, not yet won.
    for night in 1..=2 {
        let events = dawn(&mut state);
        assert_eq!(ended(&events), None, "night {night}: {events:?}");
        assert_eq!(state.nights_held(1), night);
        assert_eq!(state.nights_held(0), 0);
    }

    // A factory lost before the third dawn: the count starts over.
    state.owners.insert(factories[0], 0);
    let events = dawn(&mut state);
    assert_eq!(ended(&events), None, "{events:?}");
    assert_eq!(state.nights_held(1), 0, "a broken streak is no streak");
    assert_eq!(state.hold_streak, None);

    // Retaken and held three nights running: won on the third.
    state.owners.insert(factories[0], 1);
    for night in 1..=2 {
        let events = dawn(&mut state);
        assert_eq!(ended(&events), None, "night {night} again: {events:?}");
    }
    let events = dawn(&mut state);
    assert_eq!(
        ended(&events),
        Some((Some(1), CampaignEnd::Held)),
        "{events:?}"
    );
    assert_eq!(state.nights_held(1), 3);
}

// --- headquarters and the net ---------------------------------------------

/// The signals net roots at the flagged army rather than the first-declared
/// one, and when the flagged army dies seniority takes over — without the
/// death being forgotten.
#[test]
fn the_net_roots_at_the_flagged_headquarters_and_seniority_succeeds_it() {
    let reg = registry();
    let mut state = frontier(&reg);
    assert_eq!(state.senior_army(0), Some(ArmyId(0)));

    // Flag the 2nd Company instead: the net follows the flag.
    state.armies[0].headquarters = false;
    state.armies[1].headquarters = true;
    assert_eq!(state.senior_army(0), Some(ArmyId(1)));
    assert_eq!(state.headquarters(0), Some(ArmyId(1)));
    assert!(!state.decapitated(0));

    // She dies: somebody still has to give the orders.
    state.armies[1].alive = false;
    assert_eq!(
        state.senior_army(0),
        Some(ArmyId(0)),
        "seniority succeeds her"
    );
    assert_eq!(state.headquarters(0), None);
    assert!(
        state.decapitated(0),
        "...but the headquarters is still lost"
    );

    // A side that flagged nobody roots at seniority and can never be
    // decapitated, which is every map written before the flag existed.
    for army in state.armies.iter_mut().filter(|a| a.side == 1) {
        army.headquarters = false;
    }
    assert_eq!(state.senior_army(1), Some(ArmyId(2)));
    assert!(!state.decapitated(1));
    state.armies[2].alive = false;
    assert!(!state.decapitated(1), "nothing flagged, nothing to lose");
}

// --- where a withdrawn army arrives -----------------------------------------

/// An army whose every surviving vehicle left the field by an exit is not
/// beaten and is not where the battle was: it falls back a hex, away from
/// the enemy, and the victor advances onto the ground it gave up.
#[test]
fn an_army_that_withdrew_falls_back_a_hex_and_the_victor_takes_the_ground() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    let (attacker, defender) = (ArmyId(0), ArmyId(2));
    let contested = tactics_core::offset_to_hex(6, 4);
    let from = tactics_core::offset_to_hex(5, 4);
    state.army_mut(defender).unwrap().pos = contested;
    state.army_mut(attacker).unwrap().pos = from;

    let mut report = bloodless(&state, attacker, defender);
    report.withdrew = vec![defender];
    let events = state.apply_battle_result(&reg, &report);

    let fell_back_to = state.army(defender).unwrap().pos;
    assert_ne!(fell_back_to, contested, "she is not where the battle was");
    assert_eq!(fell_back_to.distance_to(contested), 1, "one hex back");
    assert!(
        fell_back_to.distance_to(from) > contested.distance_to(from),
        "away from the enemy, not toward her: {fell_back_to:?}"
    );
    assert_eq!(
        state.army(attacker).unwrap().pos,
        contested,
        "the ground she gave up is the victor's"
    );
    let moved: Vec<ArmyId> = events
        .iter()
        .filter_map(|e| match e {
            OverworldEvent::ArmyMoved { army, path } if path.len() == 2 => Some(*army),
            _ => None,
        })
        .collect();
    assert_eq!(
        moved,
        vec![defender, attacker],
        "both steps are said out loud"
    );
    assert!(
        state.side_armies(1).any(|a| a.id == defender),
        "not destroyed"
    );
    assert_eq!(ended(&events), None);
}

/// Under a withdrawal order, falling back goes the way she was told to go.
#[test]
fn an_army_falling_back_under_orders_falls_back_along_them() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    let (attacker, defender) = (ArmyId(0), ArmyId(2));
    let contested = tactics_core::offset_to_hex(6, 4);
    // The enemy stands *east* of her, so "away" would be west; her orders
    // say east, back toward her own end of the map.
    state.army_mut(defender).unwrap().pos = contested;
    state.army_mut(attacker).unwrap().pos = tactics_core::offset_to_hex(5, 4);
    let home = tactics_core::offset_to_hex(12, 4);
    state.army_mut(defender).unwrap().mission = Some(ArmyMission::Withdraw { to: home });

    let mut report = bloodless(&state, attacker, defender);
    report.withdrew = vec![defender];
    state.apply_battle_result(&reg, &report);

    let fell_back_to = state.army(defender).unwrap().pos;
    assert_eq!(fell_back_to.distance_to(contested), 1);
    assert!(
        fell_back_to.distance_to(home) < contested.distance_to(home),
        "the first step of the road home, not the hex furthest from the enemy: {fell_back_to:?}"
    );
}

/// An attacker who withdrew has yielded her claim: she falls back and the
/// defender keeps the ground.
#[test]
fn an_attacker_who_withdrew_yields_her_claim() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    let (attacker, defender) = (ArmyId(0), ArmyId(2));
    let contested = tactics_core::offset_to_hex(6, 4);
    let from = tactics_core::offset_to_hex(5, 4);
    state.army_mut(defender).unwrap().pos = contested;
    state.army_mut(attacker).unwrap().pos = from;

    let mut report = bloodless(&state, attacker, defender);
    report.withdrew = vec![attacker];
    state.apply_battle_result(&reg, &report);

    assert_eq!(
        state.army(defender).unwrap().pos,
        contested,
        "the defender holds"
    );
    let pos = state.army(attacker).unwrap().pos;
    assert_eq!(pos.distance_to(from), 1);
    assert!(
        pos.distance_to(contested) > 1,
        "and the attacker is further off than she was"
    );
}

/// Nowhere to fall back to is nowhere: hemmed in by armies and the map
/// edge, a withdrawn army stands where it was, and the victor — the tile
/// still being occupied — does not advance.
#[test]
fn a_withdrawn_army_with_nowhere_to_go_stands_where_it_was() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    let (attacker, defender) = (ArmyId(0), ArmyId(2));
    let corner = tactics_core::offset_to_hex(0, 0);
    let neighbours: Vec<Hex> = corner
        .all_neighbors()
        .into_iter()
        .filter(|h| state.map.get(*h).is_some())
        .collect();
    assert!(neighbours.len() <= 3, "a corner of the map");
    state.army_mut(defender).unwrap().pos = corner;
    state.army_mut(attacker).unwrap().pos = neighbours[0];
    for hex in &neighbours[1..] {
        extra_army(&mut state, 0, *hex);
    }

    let mut report = bloodless(&state, attacker, defender);
    report.withdrew = vec![defender];
    let events = state.apply_battle_result(&reg, &report);

    assert_eq!(state.army(defender).unwrap().pos, corner, "nowhere to go");
    assert_eq!(
        state.army(attacker).unwrap().pos,
        neighbours[0],
        "and the tile is still hers"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyMoved { .. })),
        "nobody moved: {events:?}"
    );
}

/// A victor advancing onto a factory holds the factory. Ground changes hands
/// by standing on it however the standing came about.
#[test]
fn the_victor_advancing_onto_a_factory_captures_it() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    let (attacker, defender) = (ArmyId(0), ArmyId(2));
    let factory = tiles_of(&state, "factory")[1];
    let beside = factory
        .all_neighbors()
        .into_iter()
        .find(|h| state.map.get(*h).is_some() && state.army_at(*h).is_none())
        .expect("a factory has a neighbour");
    state.army_mut(defender).unwrap().pos = factory;
    state.army_mut(attacker).unwrap().pos = beside;
    state.owners.insert(factory, 1);

    let events = wipe(&mut state, &reg, attacker, defender);
    assert_eq!(state.army(attacker).unwrap().pos, factory);
    assert_eq!(
        state.owners.get(&factory),
        Some(&0),
        "captured by standing on it"
    );
    assert!(
        events.iter().any(
            |e| matches!(e, OverworldEvent::ObjectiveCaptured { at, side: 0 } if *at == factory)
        ),
        "{events:?}"
    );
}

// --- the campaign planner reads the rule ------------------------------------

fn planner(difficulty: u8) -> Box<dyn AiPlanner<OverworldState, OverworldOrder>> {
    make_overworld_planner(
        &AiConfig {
            planner: "simple".into(),
            difficulty,
            doctrine: None,
        },
        7,
    )
}

/// Two enemy armies of the same size, the headquarters one hex further off
/// than the other: with the map saying that losing it loses, the planner
/// goes for the headquarters; without, for the nearer army.
#[test]
fn the_campaign_planner_hunts_the_enemy_headquarters_when_that_ends_the_war() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    // Side 1 to move; its own headquarters has already spent its turn so the
    // Reserve is the army asked about.
    state.active_side = 1;
    state.army_mut(ArmyId(2)).unwrap().moved = true;
    let hunter = tactics_core::offset_to_hex(7, 4);
    state.army_mut(ArmyId(3)).unwrap().pos = hunter;
    // Equal strength, so nothing but the flag separates them.
    let four = state.army(ArmyId(1)).unwrap().units.clone();
    state.army_mut(ArmyId(0)).unwrap().units = four.clone();
    state.army_mut(ArmyId(3)).unwrap().units = four;
    let hq_at = tactics_core::offset_to_hex(3, 4);
    let other_at = tactics_core::offset_to_hex(4, 4);
    state.army_mut(ArmyId(0)).unwrap().pos = hq_at;
    state.army_mut(ArmyId(1)).unwrap().pos = other_at;
    assert!(hunter.distance_to(hq_at) > hunter.distance_to(other_at));
    // Nothing left to capture, so the choice is between the two armies and
    // not between an army and a factory.
    let prizes: Vec<Hex> = state
        .map
        .iter()
        .filter(|(_, t)| reg.terrain(t.terrain).is_some_and(|d| d.capturable))
        .map(|(h, _)| h)
        .collect();
    for hex in prizes {
        state.owners.insert(hex, 1);
    }

    let order = planner(5).next_order(&reg, &state, 1);
    assert_eq!(
        order,
        OverworldOrder::MoveArmy {
            army: ArmyId(3),
            to: hq_at
        },
        "the war is won on the headquarters"
    );

    state.victory.decapitation = false;
    let order = planner(5).next_order(&reg, &state, 1);
    assert_eq!(
        order,
        OverworldOrder::MoveArmy {
            army: ArmyId(3),
            to: other_at
        },
        "with nothing riding on it, the nearer army"
    );
}

/// The headquarters, under a rule that losing it loses, backs away from a
/// stronger force that could reach it next turn — and from nothing else:
/// at parity it is an army like any other, or one company could chase it
/// off every objective on the map.
#[test]
fn the_campaign_headquarters_backs_away_from_a_stronger_force_that_can_reach_it() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    state.active_side = 1;
    let hq = ArmyId(2);
    let hq_at = tactics_core::offset_to_hex(7, 4);
    state.army_mut(hq).unwrap().pos = hq_at;
    let threat = tactics_core::offset_to_hex(5, 4);
    state.army_mut(ArmyId(0)).unwrap().pos = threat;
    assert!(
        hq_at.distance_to(threat) <= state.army(ArmyId(0)).unwrap().movement as i32 + 1,
        "in reach"
    );
    assert_eq!(
        state.army(ArmyId(0)).unwrap().units.len(),
        state.army(hq).unwrap().units.len(),
        "the frontier's two headquarters are matched"
    );
    // Nothing left to capture, so the only thing on the map to go toward
    // is the enemy, and any move away from her is a move away.
    let prizes: Vec<Hex> = state
        .map
        .iter()
        .filter(|(_, t)| reg.terrain(t.terrain).is_some_and(|d| d.capturable))
        .map(|(h, _)| h)
        .collect();
    for hex in prizes {
        state.owners.insert(hex, 1);
    }

    // At parity she does not run.
    let order = planner(5).next_order(&reg, &state, 1);
    let OverworldOrder::MoveArmy { army, to } = order else {
        panic!("{order:?}");
    };
    assert_eq!(army, hq);
    assert!(
        to.distance_to(threat) < hq_at.distance_to(threat),
        "an equal force is not a reason to leave; it is a fight: {to:?}"
    );

    // One more tank on the other side and she does.
    let extra = state.army(ArmyId(0)).unwrap().units[0].clone();
    state.army_mut(ArmyId(0)).unwrap().units.push(extra);
    let order = planner(5).next_order(&reg, &state, 1);
    let OverworldOrder::MoveArmy { army, to } = order else {
        panic!("the headquarters moves first and moves away: {order:?}");
    };
    assert_eq!(army, hq);
    assert!(
        to.distance_to(threat) > hq_at.distance_to(threat),
        "further from the threat than she was: {to:?}"
    );

    // And the same planner with the flag meaning nothing plays her as any
    // other army, which on this map is toward the nearest prize.
    state.victory.decapitation = false;
    let order = planner(5).next_order(&reg, &state, 1);
    let OverworldOrder::MoveArmy { to, .. } = order else {
        panic!("{order:?}");
    };
    assert!(
        to.distance_to(threat) < hq_at.distance_to(threat),
        "an ordinary army does not run from a fight it could lose: {to:?}"
    );
}

/// The shelter rule can find nothing better than where she stands, and
/// says so as a move to her own hex. That has to be a legal order that
/// spends the turn, or the one-order-per-call loop asks about her for ever;
/// and with nothing at all to do she ends the turn like any other army.
#[test]
fn a_headquarters_with_nowhere_better_to_be_spends_its_turn_standing() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    state.active_side = 1;
    let hq = ArmyId(2);
    let hq_at = state.army(hq).unwrap().pos;
    state
        .apply(
            &reg,
            &OverworldOrder::MoveArmy {
                army: hq,
                to: hq_at,
            },
        )
        .expect("standing still is a legal order");
    assert!(state.army(hq).unwrap().moved, "and it spends the turn");
    assert_eq!(state.army(hq).unwrap().pos, hq_at);

    // Everything worth taking is already hers, and the enemy is nowhere:
    // nothing to do, and the turn ends as it would for any army.
    let mut state = frontier(&reg);
    state.active_side = 1;
    let prizes: Vec<Hex> = state
        .map
        .iter()
        .filter(|(_, t)| reg.terrain(t.terrain).is_some_and(|d| d.capturable))
        .map(|(h, _)| h)
        .collect();
    for hex in prizes {
        state.owners.insert(hex, 1);
    }
    for army in state.armies.iter_mut().filter(|a| a.side == 0) {
        army.alive = false;
    }
    assert_eq!(
        planner(5).next_order(&reg, &state, 1),
        OverworldOrder::EndTurn
    );
}

// --- cross-loading ---------------------------------------------------------

/// A vehicle changes companies through the order stream, with the rules the
/// campaign owns: beside each other, the giver's turn, neither marched
/// today, never the giver's last vehicle.
#[test]
fn a_vehicle_moves_between_companies_standing_side_by_side() {
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    let (giver, taker) = (ArmyId(0), ArmyId(1));
    let beside = state.army(giver).unwrap().pos.all_neighbors()[0];
    assert!(state.map.get(beside).is_some());
    state.army_mut(taker).unwrap().pos = beside;
    let before = (
        state.army(giver).unwrap().units.len(),
        state.army(taker).unwrap().units.len(),
    );
    let sent = state.army(giver).unwrap().units[1].clone();

    let events = state
        .apply(
            &reg,
            &OverworldOrder::TransferUnit {
                from: giver,
                to: taker,
                unit: 1,
            },
        )
        .expect("side by side, fresh, and not her last");
    assert_eq!(state.army(giver).unwrap().units.len(), before.0 - 1);
    assert_eq!(state.army(taker).unwrap().units.len(), before.1 + 1);
    assert_eq!(
        state.army(taker).unwrap().units.last(),
        Some(&sent),
        "crew and all"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            OverworldEvent::UnitTransferred { from, to, vehicle }
                if *from == giver && *to == taker && *vehicle == sent.vehicle
        )),
        "{events:?}"
    );
    assert!(
        !state.army(giver).unwrap().moved && !state.army(taker).unwrap().moved,
        "cross-loading is not a march; both can still move today"
    );
}

#[test]
fn a_transfer_is_refused_where_the_campaign_says_it_makes_no_sense() {
    use tactics_core::overworld::OverworldError;
    let reg = registry_wireless();
    let mut state = frontier(&reg);
    let (giver, taker) = (ArmyId(0), ArmyId(1));
    let order = |unit: usize| OverworldOrder::TransferUnit {
        from: giver,
        to: taker,
        unit,
    };

    // Not beside each other (the frontier starts them six hexes apart).
    assert_eq!(
        state.apply(&reg, &order(0)),
        Err(OverworldError::NoTransfer)
    );

    // Beside each other, but one of them has marched today.
    let beside = state.army(giver).unwrap().pos.all_neighbors()[0];
    state.army_mut(taker).unwrap().pos = beside;
    state.army_mut(taker).unwrap().moved = true;
    assert_eq!(
        state.apply(&reg, &order(0)),
        Err(OverworldError::AlreadyMoved)
    );
    state.army_mut(taker).unwrap().moved = false;

    // The enemy's company, or the enemy's turn.
    let enemy = ArmyId(2);
    state.army_mut(enemy).unwrap().pos = beside;
    state.army_mut(taker).unwrap().pos = beside.all_neighbors()[3];
    assert_eq!(
        state.apply(
            &reg,
            &OverworldOrder::TransferUnit {
                from: giver,
                to: enemy,
                unit: 0
            }
        ),
        Err(OverworldError::NoTransfer)
    );
    assert_eq!(
        state.apply(
            &reg,
            &OverworldOrder::TransferUnit {
                from: enemy,
                to: giver,
                unit: 0
            }
        ),
        Err(OverworldError::NotYourTurn)
    );

    // A vehicle she does not have, and her last one.
    state.army_mut(taker).unwrap().pos = beside;
    state.army_mut(enemy).unwrap().pos = tactics_core::offset_to_hex(12, 1);
    assert_eq!(
        state.apply(&reg, &order(99)),
        Err(OverworldError::NoTransfer)
    );
    let keep = state.army(giver).unwrap().units[0].clone();
    state.army_mut(giver).unwrap().units = vec![keep];
    assert_eq!(
        state.apply(&reg, &order(0)),
        Err(OverworldError::NoTransfer),
        "an empty army is a destroyed one, and nobody means that"
    );
    assert_eq!(state.army(giver).unwrap().units.len(), 1);
}

// --- validation ------------------------------------------------------------

/// A vehicle travelling with an army has no hex and no side of its own, and a
/// map that still gives it one is told so.
///
/// It used to be a `UnitPlacement` — scenario data, which carries a
/// battlefield coordinate, a facing and a formation, none of which is a
/// question about a vehicle inside an army. `frontier` shipped eighteen `at`
/// fields naming hexes the army was not on, and eighteen `side` fields that
/// could have contradicted the army's without anybody finding out, because
/// the validator read the *army's* hex when checking the unit and nothing
/// anywhere read either field. Both are retired rather than deleted outright,
/// so a map written against the old shape is told what is being ignored
/// instead of quietly having it ignored — the same courtesy
/// `planner.mission_weight` gets.
#[test]
fn a_vehicle_travelling_with_an_army_has_no_hex_of_its_own() {
    let reg = registry();
    let base = reg.map("frontier").expect("shipped campaign map").clone();

    let mut report = ValidationReport::default();
    base.validate_into(&reg, &mut report);
    assert!(
        report.warnings.is_empty(),
        "the shipped campaign declares nothing retired: {:?}",
        report.warnings
    );

    let mut file = base.clone();
    let unit = &mut file.armies[0].units[0];
    unit.at = Some([99, 99]);
    unit.side = Some(7);
    let chassis = unit.vehicle.clone();
    let army = file.armies[0].name.clone();
    let mut report = ValidationReport::default();
    file.validate_into(&reg, &mut report);

    assert!(
        report.is_ok(),
        "a retired field is a warning, not a broken map: {:?}",
        report.errors
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains(&army) && w.contains(&chassis) && w.contains("`at`/`side`")),
        "nothing named the retired fields: {:?}",
        report.warnings
    );

    // And the hex it named is nobody's business: an army off the map is an
    // error, a *vehicle* claiming a hex off the map is not, because it is
    // standing where its army stands.
    assert!(
        !report.errors.iter().any(|e| e.contains("outside the map")),
        "a retired coordinate was validated as though it meant something: {:?}",
        report.errors
    );
}

/// A campaign has no treasury, and content that still funds one is told so.
///
/// Funds were paid in by capturable terrain (`income`) and by campaign
/// scripts, seeded per side on the map (`funds`), shown on the banner and
/// spent by nothing at all. They went on 2026-09-23. `income` was also, and
/// only incidentally, how the campaign planner decided which ground to want;
/// that job kept its numbers as the terrain's `value`. Both old keys are
/// retired rather than deleted, so a mod written against them is told what
/// is being ignored instead of quietly getting a campaign planner that
/// wants nothing.
#[test]
fn a_campaign_has_no_treasury_and_content_that_funds_one_is_told() {
    let reg = registry();
    let mut report = ValidationReport::default();
    reg.validate_into(&mut report);
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.contains("income") || w.contains("funds")),
        "the base mod declares neither: {:?}",
        report.warnings
    );
    assert_eq!(
        (
            reg.terrain("city").map(|t| t.value),
            reg.terrain("factory").map(|t| t.value)
        ),
        (Some(3), Some(5)),
        "the campaign planner still wants what it wanted when this was income"
    );

    let mut funded = reg.clone();
    funded
        .terrain
        .get_mut("city")
        .expect("the base mod ships a city")
        .retired_income = Some(3);
    let mut file = funded
        .map("frontier")
        .expect("shipped campaign map")
        .clone();
    file.sides[0].retired_funds = Some(20);
    funded.maps.insert(file.id.clone(), file);
    let mut report = ValidationReport::default();
    funded.validate_into(&mut report);
    assert!(
        report.is_ok(),
        "a retired key is a warning, not a broken mod: {:?}",
        report.errors
    );
    for (what, needle) in [
        ("terrain", "city` declares income"),
        ("map", "declares funds"),
    ] {
        assert!(
            report.warnings.iter().any(|w| w.contains(needle)),
            "nothing told the {what} its key is no longer read: {:?}",
            report.warnings
        );
    }
}

/// The two ways a `victory` block can be written so that it never fires are
/// errors, not silences.
#[test]
fn a_victory_rule_nobody_could_ever_satisfy_fails_validation() {
    let reg = registry();
    let base = reg.map("frontier").expect("shipped campaign map").clone();

    let mut report = ValidationReport::default();
    base.validate_into(&reg, &mut report);
    assert!(
        report.is_ok(),
        "the shipped map validates: {:?}",
        report.errors
    );

    // Ground the map does not have: the factories erased from the rows.
    let mut file = base.clone();
    for row in &mut file.rows {
        *row = row.replace('I', "p");
    }
    let mut report = ValidationReport::default();
    file.validate_into(&reg, &mut report);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("victory.hold") && e.contains("none on the map")),
        "{:?}",
        report.errors
    );

    // Ground nobody can capture.
    let mut file = base.clone();
    file.victory.hold = vec!["plains".into()];
    let mut report = ValidationReport::default();
    file.validate_into(&reg, &mut report);
    assert!(
        report.errors.iter().any(|e| e.contains("not capturable")),
        "{:?}",
        report.errors
    );

    // Terrain no mod ships.
    let mut file = base.clone();
    file.victory.hold = vec!["moon".into()];
    let mut report = ValidationReport::default();
    file.validate_into(&reg, &mut report);
    assert!(
        report.errors.iter().any(|e| e.contains("no mod ships")),
        "{:?}",
        report.errors
    );

    // A hold over ground that need never be held.
    let mut file = base.clone();
    file.victory.hold_days = 0;
    let mut report = ValidationReport::default();
    file.validate_into(&reg, &mut report);
    assert!(
        report.errors.iter().any(|e| e.contains("hold_days")),
        "{:?}",
        report.errors
    );

    // Decapitation with a side that flagged no headquarters.
    let mut file = base.clone();
    for army in file.armies.iter_mut().filter(|a| a.side == 1) {
        army.headquarters = false;
    }
    let mut report = ValidationReport::default();
    file.validate_into(&reg, &mut report);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("side 1") && e.contains("never be decapitated")),
        "{:?}",
        report.errors
    );

    // Two flags on one side is a warning, and the first wins.
    let mut file = base.clone();
    for army in file.armies.iter_mut().filter(|a| a.side == 0) {
        army.headquarters = true;
    }
    let mut report = ValidationReport::default();
    file.validate_into(&reg, &mut report);
    assert!(report.is_ok(), "{:?}", report.errors);
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("flags 2 armies as headquarters")),
        "{:?}",
        report.warnings
    );
}

/// A cadet who rides out hurt does not come home healthier for it.
///
/// A battle's fate used to be written straight over what she was, which was
/// harmless while nobody who could be a casualty was anything but fit when it
/// started. The muster changed that: called up, she goes out carrying a
/// recovery, and the wreck roll that says `Unharmed` — she got out and
/// reached her own lines, which is a perfectly ordinary thing for it to
/// say — would have signed her fit on the strength of having been shot at.
/// `CadetStatus::worse_of` keeps whichever answer holds her out longer, and a
/// grave over everything.
#[test]
fn a_cadet_who_rides_out_hurt_does_not_come_home_healthier() {
    use tactics_core::overworld::CrewLoss;
    use tactics_core::roster::{CadetStatus, CasualtyRules};

    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 3).expect("the shipped campaign");
    // Gentle, so that nothing here can end in a grave and the test is about
    // the arithmetic of days rather than about the stakes.
    state.rules = CasualtyRules { permadeath: false };
    let attacker = state.side_armies(0).next().expect("she has an army").id;
    let defender = state.side_armies(1).next().expect("so has he").id;

    // A long recovery, and then a whole battle's worth of wreck rolls over
    // the same cadets: at safety 4 and no recorded cause most of them come
    // back `Unharmed`, so this is the case the old line got wrong.
    let called_up: Vec<_> = state.roster.of_side(0).map(|c| c.id).collect();
    for id in &called_up {
        state.roster.get_mut(*id).unwrap().status = CadetStatus::Wounded { days: 9 };
    }
    let losses: Vec<CrewLoss> = called_up
        .iter()
        .map(|cadet| CrewLoss {
            cadet: *cadet,
            vehicle: "recon_car".into(),
            killed_by: None,
            found: None,
            aid: tactics_core::data::AVERAGE,
        })
        .collect();
    state.apply_battle_result(
        &reg,
        &BattleReport::of(attacker, defender, Vec::new(), losses),
    );

    for id in &called_up {
        let status = state.roster.get(*id).unwrap().status;
        assert!(
            status.days_out().is_some_and(|days| days >= 9),
            "she rode out with nine days to go and came back on {status:?}"
        );
    }
}

/// The shipped campaign is willing to kill its characters, and a mod that
/// declines the rule is the game exactly as it was.
///
/// The stakes are content (`casualties.permadeath`), not a constant, and that
/// is the whole point of where they live. Permadeath is the intended default
/// and the base mod says so — the designer's ruling of 2026-09-10, taken off
/// the `attrition` table's numbers: 0.71 dead a battle per side in the
/// harness's sample, 0.13 per hull destroyed, so a campaign that costs
/// Kuhlmann her whole order of battle buries about 1.3 of her 24. But the
/// repo's rule is that every harsh system is an additive rule whose *absence*
/// is the gentle game, and a default flipped in Rust would have made the
/// gentle campaign the one that needed an edit. So the engine still defaults
/// to nobody dying, the base mod declares the stakes, and this test is both
/// halves of that.
#[test]
fn the_shipped_campaign_kills_and_a_mod_that_declines_the_rule_does_not() {
    use tactics_core::overworld::CrewLoss;
    use tactics_core::roster::CadetStatus;

    let reg = registry();
    assert!(
        reg.casualties.permadeath,
        "the base mod is the campaign that means it"
    );
    let mut gentle = registry();
    gentle.casualties.permadeath = false;

    // Everybody on side 0, pulled out of the least survivable chassis the
    // mod ships, by the worst thing that can happen to it. Eight campaigns'
    // worth, because one cadet's fate is a coin and the rule is about the
    // shape of a hundred of them.
    let buried = |reg: &DataRegistry| -> usize {
        let mut dead = 0;
        for seed in 0..8 {
            let mut state =
                OverworldState::from_map(reg, "frontier", seed).expect("the shipped campaign");
            let attacker = state.side_armies(0).next().expect("she has an army").id;
            let defender = state.side_armies(1).next().expect("so has he").id;
            let losses: Vec<CrewLoss> = state
                .roster
                .of_side(0)
                .map(|cadet| CrewLoss {
                    cadet: cadet.id,
                    vehicle: "heavy_tank".into(),
                    killed_by: Some(tactics_core::data::DamageType::Kinetic),
                    found: None,
                    aid: tactics_core::data::AVERAGE,
                })
                .collect();
            state.apply_battle_result(
                reg,
                &BattleReport::of(attacker, defender, Vec::new(), losses),
            );
            dead += state
                .roster
                .of_side(0)
                .filter(|c| c.status == CadetStatus::Dead)
                .count();
        }
        dead
    };

    assert!(
        buried(&reg) > 0,
        "the shipped campaign burned eight academies and buried nobody"
    );
    assert_eq!(
        buried(&gentle),
        0,
        "a campaign that declines the rule killed somebody anyway"
    );
}
