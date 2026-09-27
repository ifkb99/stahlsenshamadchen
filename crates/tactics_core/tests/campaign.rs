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
use tactics_core::battle::{Destruction, Fate};
use tactics_core::data::{DataRegistry, ValidationReport};
use tactics_core::map::CampaignVictory;
use tactics_core::overworld::{
    ArmyMission, ArmyUnit, BattleReport, CampaignEnd, ElementId, OverworldEvent, OverworldOrder,
    OverworldState, Place, make_overworld_planner,
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
fn bloodless(state: &OverworldState, attacker: ElementId, defender: ElementId) -> BattleReport {
    let roster = |id: ElementId| state.army(id).map(|a| a.units.clone()).unwrap_or_default();
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
    winner: ElementId,
    loser: ElementId,
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
fn extra_army(state: &mut OverworldState, side: u8, at: Hex) -> ElementId {
    let name = format!("Extra {}", state.elements.len());
    state.add_company(
        side,
        &name,
        Place {
            pos: at,
            movement: 3,
            moved: false,
            tile: None,
            march: None,
            marched_ticks: 0,
            fighting: false,
        },
        Vec::new(),
        false,
    )
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
        Some(ElementId(0)),
        "1st Company carries headquarters"
    );
    assert_eq!(
        state.headquarters(1),
        Some(ElementId(2)),
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
    let events = wipe(&mut state, &reg, ElementId(0), ElementId(2));
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
    let events = wipe(&mut state, &reg, ElementId(0), ElementId(3));
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
    let events = wipe(&mut state, &reg, ElementId(0), ElementId(2));
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
    let events = wipe(&mut state, &reg, ElementId(0), ElementId(3));
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
    assert_eq!(state.senior_army(0), Some(ElementId(0)));

    // Flag the 2nd Company instead: the net follows the flag.
    state.element_mut(ElementId(0)).unwrap().headquarters = false;
    state.element_mut(ElementId(1)).unwrap().headquarters = true;
    assert_eq!(state.senior_army(0), Some(ElementId(1)));
    assert_eq!(state.headquarters(0), Some(ElementId(1)));
    assert!(!state.decapitated(0));

    // She dies: somebody still has to give the orders.
    state.element_mut(ElementId(1)).unwrap().alive = false;
    assert_eq!(
        state.senior_army(0),
        Some(ElementId(0)),
        "seniority succeeds her"
    );
    assert_eq!(state.headquarters(0), None);
    assert!(
        state.decapitated(0),
        "...but the headquarters is still lost"
    );

    // A side that flagged nobody roots at seniority and can never be
    // decapitated, which is every map written before the flag existed.
    for e in state.elements.iter_mut().filter(|e| e.side == 1) {
        e.headquarters = false;
    }
    assert_eq!(state.senior_army(1), Some(ElementId(2)));
    assert!(!state.decapitated(1));
    state.element_mut(ElementId(2)).unwrap().alive = false;
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
    let (attacker, defender) = (ElementId(0), ElementId(2));
    let contested = tactics_core::offset_to_hex(6, 4);
    let from = tactics_core::offset_to_hex(5, 4);
    state.place_mut(defender).unwrap().pos = contested;
    state.place_mut(attacker).unwrap().pos = from;

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
    let moved: Vec<ElementId> = events
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
    let (attacker, defender) = (ElementId(0), ElementId(2));
    let contested = tactics_core::offset_to_hex(6, 4);
    // The enemy stands *east* of her, so "away" would be west; her orders
    // say east, back toward her own end of the map.
    state.place_mut(defender).unwrap().pos = contested;
    state.place_mut(attacker).unwrap().pos = tactics_core::offset_to_hex(5, 4);
    let home = tactics_core::offset_to_hex(12, 4);
    state.element_mut(defender).unwrap().mission = Some(ArmyMission::Withdraw { to: home });

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
    let (attacker, defender) = (ElementId(0), ElementId(2));
    let contested = tactics_core::offset_to_hex(6, 4);
    let from = tactics_core::offset_to_hex(5, 4);
    state.place_mut(defender).unwrap().pos = contested;
    state.place_mut(attacker).unwrap().pos = from;

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
    let (attacker, defender) = (ElementId(0), ElementId(2));
    let corner = tactics_core::offset_to_hex(0, 0);
    let neighbours: Vec<Hex> = corner
        .all_neighbors()
        .into_iter()
        .filter(|h| state.map.get(*h).is_some())
        .collect();
    assert!(neighbours.len() <= 3, "a corner of the map");
    state.place_mut(defender).unwrap().pos = corner;
    state.place_mut(attacker).unwrap().pos = neighbours[0];
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
    let (attacker, defender) = (ElementId(0), ElementId(2));
    let factory = tiles_of(&state, "factory")[1];
    let beside = factory
        .all_neighbors()
        .into_iter()
        .find(|h| state.map.get(*h).is_some() && state.army_at(*h).is_none())
        .expect("a factory has a neighbour");
    state.place_mut(defender).unwrap().pos = factory;
    state.place_mut(attacker).unwrap().pos = beside;
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
    state.place_mut(ElementId(2)).unwrap().moved = true;
    let hunter = tactics_core::offset_to_hex(7, 4);
    state.place_mut(ElementId(3)).unwrap().pos = hunter;
    // Equal strength, so nothing but the flag separates them.
    let four = state.army(ElementId(1)).unwrap().units.clone();
    state.set_vehicles(ElementId(0), four.clone());
    state.set_vehicles(ElementId(3), four);
    let hq_at = tactics_core::offset_to_hex(3, 4);
    let other_at = tactics_core::offset_to_hex(4, 4);
    state.place_mut(ElementId(0)).unwrap().pos = hq_at;
    state.place_mut(ElementId(1)).unwrap().pos = other_at;
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
            army: ElementId(3),
            to: hq_at
        },
        "the war is won on the headquarters"
    );

    state.victory.decapitation = false;
    let order = planner(5).next_order(&reg, &state, 1);
    assert_eq!(
        order,
        OverworldOrder::MoveArmy {
            army: ElementId(3),
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
    let hq = ElementId(2);
    let hq_at = tactics_core::offset_to_hex(7, 4);
    state.place_mut(hq).unwrap().pos = hq_at;
    let threat = tactics_core::offset_to_hex(5, 4);
    state.place_mut(ElementId(0)).unwrap().pos = threat;
    assert!(
        hq_at.distance_to(threat) <= state.army(ElementId(0)).unwrap().movement as i32 + 1,
        "in reach"
    );
    assert_eq!(
        state.army(ElementId(0)).unwrap().units.len(),
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
    let extra = state.army(ElementId(0)).unwrap().units[0].clone();
    state.add_vehicle(ElementId(0), extra);
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
    let hq = ElementId(2);
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
    for e in state
        .elements
        .iter_mut()
        .filter(|e| e.side == 0 && e.place.is_some())
    {
        e.alive = false;
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
    let (giver, taker) = (ElementId(0), ElementId(1));
    let beside = state.army(giver).unwrap().pos.all_neighbors()[0];
    assert!(state.map.get(beside).is_some());
    state.place_mut(taker).unwrap().pos = beside;
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
    let (giver, taker) = (ElementId(0), ElementId(1));
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
    state.place_mut(taker).unwrap().pos = beside;
    state.place_mut(taker).unwrap().moved = true;
    assert_eq!(
        state.apply(&reg, &order(0)),
        Err(OverworldError::AlreadyMoved)
    );
    state.place_mut(taker).unwrap().moved = false;

    // The enemy's company, or the enemy's turn.
    let enemy = ElementId(2);
    state.place_mut(enemy).unwrap().pos = beside;
    state.place_mut(taker).unwrap().pos = beside.all_neighbors()[3];
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
    state.place_mut(taker).unwrap().pos = beside;
    state.place_mut(enemy).unwrap().pos = tactics_core::offset_to_hex(12, 1);
    assert_eq!(
        state.apply(&reg, &order(99)),
        Err(OverworldError::NoTransfer)
    );
    let keep = state.army(giver).unwrap().units[0].clone();
    state.set_vehicles(giver, vec![keep]);
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

// --- a campaign on a generated world (WORLD.md W2) ---

fn generated(reg: &DataRegistry) -> OverworldState {
    OverworldState::from_map(reg, "frontier_world", 3).expect("the generated campaign builds")
}

#[test]
fn a_generated_campaign_map_is_what_its_world_adds_up_to() {
    // Bottom up: the campaign hexes are the chunks' summaries, at the
    // chunks' own coordinates, and nothing else.
    let reg = registry();
    let state = generated(&reg);
    let world = state
        .world
        .as_ref()
        .expect("a generated campaign keeps its world");
    assert_eq!(*state.map, world.campaign_map());
    assert_eq!(state.map.len(), world.chunks().count());
    let mut stands: Vec<Hex> = state.columns().iter().map(|a| a.pos).collect();
    for a in &state.columns() {
        assert!(
            state.map.contains(a.pos),
            "`{}` stands off the campaign map",
            a.name
        );
    }
    stands.sort_by_key(|h| (h.x, h.y));
    stands.dedup();
    assert_eq!(
        stands.len(),
        state.columns().len(),
        "two armies were placed on one hex"
    );
}

#[test]
fn each_side_of_a_generated_campaign_begins_toward_its_own_edge() {
    // `frontier_world` places Kuhlmann toward the west and the Valkyries
    // toward the east; on the plane, not in chunk coordinates, which are
    // turned against it.
    let reg = registry();
    let state = generated(&reg);
    let radius = reg.scale.battle_map_radius();
    let x = |h: Hex| {
        let c = tactics_core::world::chunk_centre(h, radius);
        c.x as f64 + c.y as f64 * 0.5
    };
    let mean = |side: u8| {
        let xs: Vec<f64> = state
            .columns()
            .iter()
            .filter(|a| a.side == side)
            .map(|a| x(a.pos))
            .collect();
        xs.iter().sum::<f64>() / xs.len() as f64
    };
    assert!(
        mean(0) < mean(1),
        "west {} is not west of east {}",
        mean(0),
        mean(1)
    );
}

#[test]
fn a_generated_campaign_has_the_ground_its_ending_names() {
    let reg = registry();
    let state = generated(&reg);
    for terrain in &state.victory.hold {
        assert!(
            !tiles_of(&state, terrain).is_empty(),
            "the ending asks for `{terrain}` and the world made none"
        );
    }
}

#[test]
fn a_generated_campaign_saves_as_how_to_make_its_world() {
    let reg = registry();
    let state = generated(&reg);
    let text = tactics_core::save::SaveGame::<tactics_core::battle::BattleState>::new(
        &reg,
        Some(state.clone()),
        None,
    )
    .to_json()
    .unwrap();
    let back = tactics_core::save::SaveGame::from_json(&reg, &text)
        .unwrap()
        .0
        .overworld
        .unwrap();
    assert_eq!(back.map, state.map);
    assert_eq!(
        back.world.as_ref().unwrap().skeleton,
        state.world.as_ref().unwrap().skeleton
    );
}

#[test]
fn a_campaign_on_a_world_made_to_order_stands_on_that_world_and_saves_as_it() {
    // The setup screen's choices reach the world the campaign stands on,
    // her seed replaces the map's, and the save carries the rules the
    // choices made — so a loaded campaign is on the ground she chose, not
    // the mod's default ground under her armies.
    let reg = registry();
    let setup = tactics_core::data::WorldSetup::parse("size=small,woodland=heavy,seed=9").unwrap();
    let state = OverworldState::from_map_setup(&reg, "frontier_world", 3, &setup)
        .expect("a world made to order builds");
    let world = state.world.as_ref().unwrap();
    assert_eq!(world.seed, 9, "her seed, not the map's");
    assert_eq!(world.rules.radius, 7);
    assert_eq!(world.rules.cover.wood_percent, 45);
    assert_eq!(state.map.iter().count() as u32, world.rules.hexes());
    let plain = generated(&reg);
    assert_ne!(state.map, plain.map, "a different world from the default");

    let text = tactics_core::save::SaveGame::<tactics_core::battle::BattleState>::new(
        &reg,
        Some(state.clone()),
        None,
    )
    .to_json()
    .unwrap();
    let back = tactics_core::save::SaveGame::from_json(&reg, &text)
        .unwrap()
        .0
        .overworld
        .unwrap();
    assert_eq!(back.map, state.map);
    assert_eq!(back.world.as_ref().unwrap().rules, world.rules);
    assert_eq!(back.world.as_ref().unwrap().skeleton, world.skeleton);
}

#[test]
fn a_war_on_a_small_world_is_fought_to_an_end() {
    let reg = registry();
    let options = tactics_core::harness::campaign::CampaignOptions {
        world: tactics_core::data::WorldSetup::parse("size=small").unwrap(),
        ..Default::default()
    };
    let run = tactics_core::harness::campaign::play(&reg, "frontier_world", 0, &options).unwrap();
    assert!(run.end.is_some(), "never ended");
    assert_eq!(run.declined, 0);
}

#[test]
fn a_generated_campaign_is_fought_to_an_end_without_declining_a_fight() {
    let reg = registry();
    for seed in 0..3 {
        let run = tactics_core::harness::campaign::play(
            &reg,
            "frontier_world",
            seed,
            &tactics_core::harness::campaign::CampaignOptions::default(),
        )
        .expect("it builds");
        assert!(run.end.is_some(), "seed {seed} never ended");
        assert_eq!(run.declined, 0, "seed {seed} declined a fight");
    }
}

// --- marching on the ground (WORLD.md W2.1–W2.3), on the clock (W3.1) ---

/// On the clock, an order sets a march and the day walks it: finish every
/// side's orders and run the day, returning what it did — stopping at the
/// first fight, as the clock does, and at the end of the war, which a fight
/// on the ground can bring before the day is out.
fn run_the_day(reg: &DataRegistry, state: &mut OverworldState) -> Vec<OverworldEvent> {
    let day = state.turn;
    let mut all = Vec::new();
    for _ in 0..8 {
        let events = state
            .apply(reg, &OverworldOrder::EndTurn)
            .expect("end of turn");
        let fight = events
            .iter()
            .any(|e| matches!(e, OverworldEvent::BattleTriggered { .. }));
        all.extend(events);
        if fight || state.turn > day || state.over.is_some() {
            break;
        }
    }
    all
}

#[test]
fn an_army_on_a_generated_world_stands_on_a_tile_inside_its_campaign_hex() {
    let reg = registry();
    let state = generated(&reg);
    let radius = reg.scale.battle_map_radius();
    for a in &state.columns() {
        let tile = a
            .tile
            .expect("an army on a generated world stands on a tile");
        assert_eq!(
            tactics_core::world::chunk_of(tile, radius),
            a.pos,
            "`{}`",
            a.name
        );
    }
    let drawn = frontier(&reg);
    assert!(
        drawn.columns().iter().all(|a| a.tile.is_none()),
        "a drawn map has no tiles to stand on"
    );
}

#[test]
fn a_columns_day_is_its_slowest_vehicle_at_the_marchs_share() {
    // The ruling is that a column is not a tank: it keeps the pace of the
    // slowest thing that carries it, and a share of that, for the hours it is
    // on the road.
    let reg = registry();
    let state = generated(&reg);
    let army = &state.army(ElementId(0)).unwrap();
    let slowest = army
        .units
        .iter()
        .filter_map(|u| reg.vehicle(&u.vehicle))
        .filter(|v| v.movement.class != tactics_core::data::MovementClass::Foot)
        .map(|v| v.movement.points)
        .min()
        .unwrap();
    let rounds_per_hour = (3600.0 / reg.scale.round_seconds).round() as u32;
    assert_eq!(
        state.day_budget(&reg, army.id),
        reg.march.day_budget(slowest, rounds_per_hour)
    );
    assert!(state.day_budget(&reg, army.id) > 0);
}

#[test]
fn an_army_sent_anywhere_its_reach_offers_arrives_there_that_day() {
    // `reachable` is the coarse graph's estimate: tile-level passages between
    // neighbouring campaign hexes' standing tiles. The march is the real
    // route, which can only be as cheap or cheaper — so what `reachable`
    // offers, a march delivers. This is what keeps the range the player is
    // shown and the ground the planner picks honest.
    let reg = registry();
    let state = generated(&reg);
    let radius = reg.scale.battle_map_radius();
    let army = ElementId(0);
    let mut offered: Vec<Hex> = state
        .reachable(&reg, army)
        .into_keys()
        .filter(|h| *h != state.army(army).unwrap().pos)
        .collect();
    offered.sort_by_key(|h| (h.x, h.y));
    assert!(offered.len() > 3, "a day's march reaches somewhere");
    for to in offered.into_iter().step_by(3) {
        let mut trial = state.clone();
        trial
            .apply(&reg, &OverworldOrder::MoveArmy { army, to })
            .unwrap_or_else(|e| panic!("the march to {to:?} was refused: {e:?}"));
        run_the_day(&reg, &mut trial);
        let moved = trial.army(army).unwrap();
        assert_eq!(moved.pos, to, "offered {to:?}, stopped at {:?}", moved.pos);
        assert_eq!(
            tactics_core::world::chunk_of(moved.tile.unwrap(), radius),
            moved.pos
        );
    }
}

#[test]
fn a_march_into_an_enemy_halts_on_the_border_of_his_ground_and_fights() {
    // Contact on a generated world, read at the tile: the column's route
    // goes up to the enemy's campaign hex and stops at its edge, and the
    // fight is there — the same rule the drawn campaign keeps hex by hex.
    let reg = registry();
    let mut state = generated(&reg);
    let radius = reg.scale.battle_map_radius();
    let army = ElementId(0);
    let here = state.army(army).unwrap().pos;
    let enemy_at = here + Hex::new(2, 0);
    let enemy = state
        .columns()
        .iter()
        .find(|a| a.side != state.army(army).unwrap().side)
        .unwrap()
        .id;
    {
        let e = state.place_mut(enemy).unwrap();
        e.pos = enemy_at;
        e.tile = Some(tactics_core::world::chunk_centre(enemy_at, radius));
    }
    state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: enemy_at })
        .expect("an attack is an order like any other");
    let events = run_the_day(&reg, &mut state);
    // Where she was when the fight began: the last hex her march reached
    // before it. (Afterwards she may well stand on his ground — she came to
    // take it, and the day goes on.)
    let began = events
        .iter()
        .position(|e| matches!(e, OverworldEvent::ArmyEngaged { .. }))
        .expect("the fight began");
    let halted = events[..began]
        .iter()
        .rev()
        .find_map(|e| match e {
            OverworldEvent::ArmyMoved { army: a, path } if *a == army => path.last().copied(),
            _ => None,
        })
        .unwrap_or(here);
    assert_eq!(
        halted.unsigned_distance_to(enemy_at),
        1,
        "it stops next to him"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            OverworldEvent::ArmyEngaged { army: a, against: Some(d), at }
                if *a == army && *d == enemy && *at == enemy_at
        )),
        "and the fight is on, on his ground: {events:?}"
    );
}

#[test]
fn a_campaign_on_the_clock_is_the_same_war_from_the_same_seed() {
    let reg = registry();
    let play = |seed| {
        tactics_core::harness::campaign::play(
            &reg,
            "frontier_world",
            seed,
            &tactics_core::harness::campaign::CampaignOptions::default(),
        )
        .unwrap()
    };
    let (a, b) = (play(4), play(4));
    assert_eq!(a.days, b.days);
    assert_eq!(a.end, b.end);
    assert_eq!(a.battles.len(), b.battles.len());
}

#[test]
fn a_day_on_the_clock_goes_through_a_save_and_runs_on_the_same() {
    // Mid-march, mid-day: the clock, every column's leg and what it has
    // banked toward its next step are saved, and the rest of the day runs
    // exactly as it would have.
    let reg = registry();
    let mut state = generated(&reg);
    let army = ElementId(0);
    let far = state
        .reachable(&reg, army)
        .into_iter()
        .max_by_key(|(h, c)| (*c, h.x, h.y))
        .map(|(h, _)| h)
        .unwrap();
    state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: far })
        .unwrap();
    for _ in 0..4 {
        let events = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
        if events
            .iter()
            .any(|e| matches!(e, OverworldEvent::TurnStarted { .. }))
            && state.active_side == 1
        {
            break;
        }
    }
    // Everybody has ordered; run the clock part of the way into the day, and
    // on to a tick with movement banked toward the next step — saving on a
    // step's boundary would not tell a file that kept it from one that lost
    // it.
    state.advance_clock(&reg, 1_500);
    for _ in 0..24 {
        if state
            .army(army)
            .unwrap()
            .march
            .as_ref()
            .is_some_and(|m| m.banked > 0)
        {
            break;
        }
        state.advance_clock(&reg, 1);
    }
    assert!(
        state
            .army(army)
            .unwrap()
            .march
            .as_ref()
            .is_some_and(|m| m.banked > 0)
    );
    assert!(
        state.army(army).unwrap().march.is_some(),
        "the test wants a column still on the road"
    );
    let text = tactics_core::save::SaveGame::<tactics_core::battle::BattleState>::new(
        &reg,
        Some(state.clone()),
        None,
    )
    .to_json()
    .unwrap();
    let mut back = tactics_core::save::SaveGame::from_json(&reg, &text)
        .unwrap()
        .0
        .overworld
        .unwrap();
    assert_eq!(back.clock, state.clock);
    // Tick by tick while the column is still on the road, so a fraction of a
    // step lost in the file shows as a column a tile behind — compared every
    // tick, because a tick's worth lost makes her one tick late, and a
    // comparison every 25 ticks never landed on the one tick that showed it.
    for _ in 0..1_000 {
        let expected = state.advance_clock(&reg, 1);
        let actual = back.advance_clock(&reg, 1);
        assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
        assert_eq!(
            back.army(army).unwrap().tile,
            state.army(army).unwrap().tile
        );
    }
}

// --- fights on the ground (WORLD.md W3.2–W3.3) ---

/// A generated campaign with the first Valkyrie army moved two campaign
/// hexes from Kuhlmann's first, and Kuhlmann ordered onto it.
fn about_to_meet(reg: &DataRegistry) -> (OverworldState, ElementId, ElementId) {
    let mut state = generated(reg);
    let radius = reg.scale.battle_map_radius();
    let army = ElementId(0);
    let here = state.army(army).unwrap().pos;
    let enemy_at = here + Hex::new(2, 0);
    let enemy = state
        .columns()
        .iter()
        .find(|a| a.side != state.army(army).unwrap().side)
        .unwrap()
        .id;
    let world = state.world.clone().unwrap();
    {
        let e = state.place_mut(enemy).unwrap();
        e.pos = enemy_at;
        e.tile = Some(world.stand_tile(reg, enemy_at));
    }
    let _ = radius;
    state
        .apply(reg, &OverworldOrder::MoveArmy { army, to: enemy_at })
        .unwrap();
    (state, army, enemy)
}

#[test]
fn contact_on_a_generated_world_is_a_fight_on_the_ground_there() {
    // No battle is sent anywhere: the two armies' vehicles are placed on the
    // tiles around where they stand, and the fight is fought there, on the
    // world, while the clock runs.
    let reg = registry();
    let (mut state, army, enemy) = about_to_meet(&reg);
    let events = run_the_day(&reg, &mut state);
    assert!(
        events.iter().any(|e| matches!(
            e,
            OverworldEvent::ArmyEngaged { army: a, against: Some(d), .. }
                if *a == army && *d == enemy
        )) || state.front.is_some(),
        "contact opened no fight: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OverworldEvent::BattleTriggered { .. })),
        "a clocked campaign sends no battle anywhere"
    );
}

#[test]
fn a_fight_on_the_ground_ends_with_its_survivors_armies_again_where_they_stand() {
    let reg = registry();
    let (mut state, army, enemy) = about_to_meet(&reg);
    let before: usize = [army, enemy]
        .iter()
        .map(|a| state.army(*a).unwrap().units.len())
        .sum();
    // Orders in, then the clock a tick at a time, keeping where each army's
    // vehicles stood in the fight on the last tick before it ended.
    state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    let mut last_seen: Vec<(ElementId, Vec<Hex>)> = Vec::new();
    let mut ended = false;
    for _ in 0..60_000 {
        if let Some(e) = state.front.as_ref() {
            last_seen = e
                .armies()
                .into_iter()
                .map(|a| {
                    let at = e
                        .battle()
                        .surviving_units()
                        .filter(|u| e.origin(u.id) == Some(a))
                        .map(|u| u.pos)
                        .collect();
                    (a, at)
                })
                .collect();
        }
        let events = state.advance_clock(&reg, 1);
        if events
            .iter()
            .any(|e| matches!(e, OverworldEvent::FightingOver { .. }))
        {
            ended = true;
            break;
        }
    }
    assert!(ended, "the fight never ended");
    for (id, stood) in &last_seen {
        let Some(a) = state.army(*id) else {
            continue;
        };
        let tile = a.tile.unwrap();
        assert!(
            stood.iter().any(|h| h.unsigned_distance_to(tile) <= 2),
            "`{}` is at {tile:?}, and its vehicles were at {stood:?}",
            a.name
        );
    }
    assert!(state.front.is_none());
    let radius = reg.scale.battle_map_radius();
    let mut after = 0;
    for id in [army, enemy] {
        let Some(a) = state.army(id) else { continue };
        assert!(!a.fighting, "`{}` is still fighting", a.name);
        let tile = a.tile.unwrap();
        assert_eq!(tactics_core::world::chunk_of(tile, radius), a.pos);
        after += a.units.len();
    }
    assert!(after <= before, "a fight added vehicles");
}

#[test]
fn a_campaign_in_the_middle_of_a_fight_goes_through_a_save_and_fights_on_the_same() {
    let reg = registry();
    let (mut state, _, _) = about_to_meet(&reg);
    // Everybody's orders in, then the clock tick by tick to the contact — a
    // day's end-of-turn would fight the whole fight inside one call — and a
    // few minutes into it.
    state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    for _ in 0..20_000 {
        if state.front.is_some() {
            break;
        }
        state.advance_clock(&reg, 1);
    }
    assert!(state.front.is_some(), "the test wants a fight in progress");
    state.advance_clock(&reg, 40);
    let text = tactics_core::save::SaveGame::<tactics_core::battle::BattleState>::new(
        &reg,
        Some(state.clone()),
        None,
    )
    .to_json()
    .unwrap();
    let mut back = tactics_core::save::SaveGame::from_json(&reg, &text)
        .unwrap()
        .0
        .overworld
        .unwrap();
    for _ in 0..120 {
        let expected = state.advance_clock(&reg, 1);
        let actual = back.advance_clock(&reg, 1);
        assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    }
    let positions = |s: &OverworldState| -> Vec<(u32, Hex)> {
        s.front
            .iter()
            .flat_map(|e| e.battle().units.iter().map(|u| (u.id.0, u.pos)))
            .collect()
    };
    assert_eq!(positions(&back), positions(&state));
}

#[test]
fn a_campaign_that_has_ended_goes_no_further() {
    // The fighting on the ground can end the war in the middle of a tick,
    // and the clock used to go on past it inside the same call: columns
    // marched, towns changed hands, the next dawn ran, and on seed 1 a whole
    // new fight was joined and fought after the campaign's winner had been
    // declared. What may follow `GameEnded` is the fight that ended it
    // winding down — its armies leaving it, the ground they stood on — and
    // nothing that is the world going on.
    let reg = registry();
    let options = tactics_core::harness::campaign::CampaignOptions {
        trace: true,
        ..Default::default()
    };
    let mut ended = 0;
    for seed in 0..4 {
        let run = tactics_core::harness::campaign::play(&reg, "frontier_world", seed, &options)
            .expect("it builds");
        let Some(at) = run
            .events
            .iter()
            .position(|e| matches!(e, OverworldEvent::GameEnded { .. }))
        else {
            continue;
        };
        ended += 1;
        let after: Vec<_> = run.events[at + 1..]
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    OverworldEvent::ArmyMoved { .. }
                        | OverworldEvent::TurnStarted { .. }
                        | OverworldEvent::ArmyEngaged { .. }
                        | OverworldEvent::BattleTriggered { .. }
                        | OverworldEvent::FightAwaitsOrders { .. }
                        | OverworldEvent::GameEnded { .. }
                )
            })
            .collect();
        assert!(
            after.is_empty(),
            "seed {seed}: the world went on after the campaign ended: {after:?}"
        );
    }
    assert!(ended > 0, "no seed ended, so nothing was tested");
}

#[test]
fn every_seed_of_the_generated_campaign_is_fought_to_an_end() {
    // Before fights were on the ground, a headquarters reduced to one hidden
    // crew was attacked every day in a battle that stalemated without
    // contact, and three seeds in sixteen never ended. On the ground the
    // attacker advances on where he stands, and finds him.
    let reg = registry();
    for seed in 0..4 {
        let run = tactics_core::harness::campaign::play(
            &reg,
            "frontier_world",
            seed,
            &tactics_core::harness::campaign::CampaignOptions::default(),
        )
        .unwrap();
        assert!(run.end.is_some(), "seed {seed} never ended");
    }
}

#[test]
fn a_column_that_reaches_a_fight_in_progress_joins_it_on_its_own_side() {
    // WORLD.md W3.5: the fight is on the ground, so an army marching into
    // it arrives in it — vehicles on tiles beside the ones already there, a
    // formation of its own, fighting for its own side — rather than a second
    // battle being staged somewhere or the column waiting on the border.
    let reg = registry();
    let (mut state, army, enemy) = about_to_meet(&reg);
    state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    for _ in 0..20_000 {
        if state.front.is_some() {
            break;
        }
        state.advance_clock(&reg, 1);
    }
    assert!(state.front.is_some(), "the test wants a fight in progress");
    let before = state.front.as_ref().unwrap().battle().units.len();
    let enemy_side = state.army(enemy).unwrap().side;
    // The other Valkyrie army, set down on the border of the fight's hex —
    // a column moves a tile a minute and a fight here lasts about nine, so
    // one set down further off arrives after it is over — and sent in.
    let reserve = state
        .columns()
        .iter()
        .find(|a| a.side == enemy_side && a.id != enemy)
        .unwrap()
        .id;
    let at = state.army(enemy).unwrap().pos;
    let radius = reg.scale.battle_map_radius();
    let (start, border) = at
        .all_neighbors()
        .into_iter()
        .filter(|n| state.army_at(*n).is_none())
        .find_map(|n| {
            tactics_core::world::chunk_hexes(n, radius)
                .find(|t| {
                    t.all_neighbors()
                        .iter()
                        .any(|x| tactics_core::world::chunk_of(*x, radius) == at)
                })
                .map(|t| (n, t))
        })
        .expect("a free neighbouring hex");
    {
        let r = state.place_mut(reserve).unwrap();
        r.pos = start;
        r.tile = Some(border);
        r.moved = false;
    }
    // Orders are given at dawn in turn; set the march directly, as the
    // Valkyries' planner would have.
    let leg_order = OverworldOrder::MoveArmy {
        army: reserve,
        to: at,
    };
    state.active_side = enemy_side;
    state
        .apply(&reg, &leg_order)
        .expect("the reserve is sent to the fight");
    let mut joined = false;
    for _ in 0..20_000 {
        let events = state.advance_clock(&reg, 1);
        if events
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyEngaged { army: a, .. } if *a == reserve))
        {
            joined = true;
            break;
        }
        if state.front.is_none() {
            break;
        }
    }
    assert!(
        joined,
        "the reserve never reached the fight, or the fight ended first"
    );
    let e = state.front.as_ref().unwrap();
    assert!(state.army(reserve).unwrap().fighting);
    assert!(e.battle().units.len() > before, "nobody arrived");
    let theirs: Vec<_> = e
        .battle()
        .units
        .iter()
        .filter(|u| e.origin(u.id) == Some(reserve))
        .collect();
    assert!(!theirs.is_empty());
    assert!(
        theirs.iter().all(|u| u.side == enemy_side),
        "they fight for their own side"
    );
    let _ = army;
}

// --- the player in the chain of command (WORLD.md W4) ---

#[test]
fn every_side_knows_its_commander_from_the_vehicle_its_map_flags() {
    // `command` on a vehicle names the side's commander: its senior cadet.
    // Kuhlmann's flagged Panther is Anka Weiss's; the Valkyries', Irma's.
    let reg = registry();
    for state in [frontier(&reg), generated(&reg)] {
        for side in &state.sides {
            let c = side
                .commander
                .expect("every side's map flags a command vehicle");
            let cadet = state.roster.get(c).unwrap();
            let in_a_flagged_crew = state
                .columns()
                .iter()
                .filter(|a| {
                    a.side as usize
                        == state
                            .sides
                            .iter()
                            .position(|s| s.name == side.name)
                            .unwrap()
                })
                .any(|a| a.units.iter().any(|u| u.crew.contains(&c)));
            assert!(
                in_a_flagged_crew,
                "{} commands {} from outside it",
                cadet.name, side.name
            );
        }
    }
}

#[test]
fn her_death_ends_the_campaign_and_her_wound_hands_command_to_the_next_senior() {
    // The designer's rulings: her death ends the campaign; a wound is not a
    // death, and while she is in the infirmary the next senior commands.
    let reg = registry();
    let mut state = frontier(&reg);
    let commander = state.sides[0].commander.unwrap();
    assert_eq!(state.acting_commander(&reg, 0), Some(commander));

    state.roster.get_mut(commander).unwrap().status =
        tactics_core::roster::CadetStatus::Wounded { days: 3 };
    assert!(!state.defeated(0), "a wound is not a death");
    let acting = state.acting_commander(&reg, 0).expect("somebody commands");
    assert_ne!(
        acting, commander,
        "the next senior commands while she is out"
    );

    state.roster.get_mut(commander).unwrap().status = tactics_core::roster::CadetStatus::Dead;
    assert!(state.defeated(0));
    let events = end_turn(&reg, &mut state);
    assert!(
        events.iter().any(|e| matches!(
            e,
            OverworldEvent::GameEnded {
                winner: Some(1),
                reason: tactics_core::overworld::CampaignEnd::CommanderKilled,
            }
        )),
        "{events:?}"
    );
}

#[test]
fn on_the_clock_an_order_is_given_on_any_tick_and_a_newer_one_replaces_it() {
    // No turn to be out of: the player can order a company in the middle of
    // the day, while it is another side's phase, and a second order is the
    // order — the march in progress is replaced, not the new one refused.
    let reg = registry();
    let mut state = generated(&reg);
    let army = ElementId(0);
    state.active_side = 1;
    let reach: Vec<Hex> = {
        let mut r: Vec<Hex> = state.reachable(&reg, army).into_keys().collect();
        r.sort_by_key(|h| (h.x, h.y));
        r.into_iter()
            .filter(|h| *h != state.army(army).unwrap().pos)
            .take(2)
            .collect()
    };
    state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: reach[0] })
        .expect("an order out of turn, on the clock");
    state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: reach[1] })
        .expect("and a second one");
    assert_eq!(
        state.army(army).unwrap().march.as_ref().unwrap().to,
        reach[1]
    );
}

#[test]
fn a_fight_she_can_reach_waits_for_her_orders_and_one_she_cannot_is_fought_without_her() {
    // WORLD.md W4.4. With a person commanding Kuhlmann in person, the fight
    // her company walks into stops the clock at its planning phase until she
    // commits; the Valkyries' side is planned by the engine meanwhile. Her
    // company off the net fights under its own leader and the clock does
    // not wait.
    let reg = registry();
    let (mut state, army, _) = about_to_meet(&reg);
    state.human_command = true;
    state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    let mut waiting = None;
    for _ in 0..20_000 {
        let events = state.advance_clock(&reg, 1);
        if let Some(OverworldEvent::FightAwaitsOrders { side }) = events
            .iter()
            .find(|e| matches!(e, OverworldEvent::FightAwaitsOrders { .. }))
        {
            waiting = Some(*side);
            break;
        }
    }
    let side = waiting.expect("her fight waits for her");
    assert_eq!(side, 0);
    let clock = state.clock;
    let again = state.advance_clock(&reg, 50);
    assert_eq!(state.clock, clock, "the clock waits with it");
    assert!(
        again
            .iter()
            .any(|e| matches!(e, OverworldEvent::FightAwaitsOrders { .. }))
    );

    state
        .order_in_fight(&reg, &tactics_core::battle::Order::Commit { side: 0 })
        .expect("she commits");
    state.advance_clock(&reg, 5);
    assert!(state.clock > clock, "and the world goes on");

    // The same fight with her company off the net: nobody waits.
    let (mut off, army2, _) = about_to_meet(&reg);
    off.human_command = true;
    off.out_of_contact.push(army2);
    off.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    for _ in 0..20_000 {
        let events = off.advance_clock(&reg, 1);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, OverworldEvent::FightAwaitsOrders { .. })),
            "a company she cannot reach does not wait for her"
        );
        if events
            .iter()
            .any(|e| matches!(e, OverworldEvent::FightingOver { .. }))
        {
            break;
        }
    }
    let _ = army;
}

#[test]
fn on_the_clock_an_army_ordered_onto_its_own_hex_holds_there() {
    // The campaign planner says "stay" by ordering an army onto the hex it
    // stands on. On the clock that was refused as a march with no road,
    // which spent the army's day and left it on whatever march it had —
    // found when the Valkyries never moved in the game. It is a hold.
    let reg = registry();
    let mut state = generated(&reg);
    let army = ElementId(0);
    let here = state.army(army).unwrap().pos;
    let far = state
        .reachable(&reg, army)
        .into_keys()
        .find(|h| *h != here)
        .unwrap();
    state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: far })
        .unwrap();
    assert!(state.army(army).unwrap().march.is_some());
    state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: here })
        .expect("staying is an order");
    assert!(
        state.army(army).unwrap().march.is_none(),
        "and it ends the march"
    );
}

#[test]
fn a_fight_at_a_town_is_fought_over_the_town_and_whoever_holds_it_takes_it() {
    // WORLD.md W3.4 and W2.5: the towns near a fight are objectives on their
    // own tiles, and when the fight is over a town's campaign hex goes to
    // whoever holds those tiles — capture read from the ground.
    let reg = registry();
    let mut state = generated(&reg);
    let world = state.world.clone().unwrap();
    let radius = world.chunk_radius;
    let army = ElementId(0);
    let here = state.army(army).unwrap().pos;
    // The nearest town to Kuhlmann's first company that nobody stands in,
    // with a Valkyrie army set down on its square.
    let town = world
        .skeleton
        .towns
        .iter()
        .map(|t| (t, tactics_core::world::chunk_of(t.centre, radius)))
        .filter(|(_, hex)| *hex != here && state.army_at(*hex).is_none())
        .min_by_key(|(_, hex)| hex.unsigned_distance_to(here))
        .map(|(t, hex)| (*t, hex))
        .unwrap();
    let (town, at) = town;
    let enemy = state.columns().iter().find(|a| a.side == 1).unwrap().id;
    {
        let e = state.place_mut(enemy).unwrap();
        e.pos = at;
        e.tile = Some(town.centre);
    }
    state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: at })
        .unwrap();
    state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    let mut holder = None;
    let mut fought_over = false;
    for _ in 0..200_000 {
        if let Some(e) = state.front.as_ref() {
            for (o, held) in e.battle().objectives() {
                if o.id.starts_with("town-") && o.hexes.contains(&town.centre) {
                    fought_over = true;
                    holder = held;
                }
            }
        }
        let events = state.advance_clock(&reg, 1);
        if events
            .iter()
            .any(|e| matches!(e, OverworldEvent::FightingOver { .. }))
        {
            break;
        }
    }
    assert!(fought_over, "the fight at the town was not over the town");
    if let Some(side) = holder {
        assert_eq!(state.owners.get(&at), Some(&side), "its holder took it");
    }
    // Both sides' survivors often end in the town, the contest cancelling;
    // the campaign keeps one army to a hex, so one of them has fallen back.
    for a in state.columns().iter() {
        assert!(
            !state
                .columns()
                .iter()
                .any(|o| o.side != a.side && o.pos == a.pos),
            "`{}` shares a hex with the enemy after the fight",
            a.name
        );
    }
}

/// Two fights on a generated campaign, far enough apart that neither pair
/// can see the other, begun together: each Kuhlmann army set on the border
/// of its own hex, a Valkyrie army in the hex across it, and sent in, so
/// contact is its first step. (Set down further off they meet minutes apart,
/// and a fight here can be over before the other begins.) The clock is run
/// until all four are fighting. Returns the pairs, Kuhlmann first.
fn two_fights(reg: &DataRegistry) -> (OverworldState, Vec<(ElementId, ElementId)>) {
    let mut state = generated(reg);
    let world = state.world.clone().unwrap();
    let radius = world.chunk_radius;
    let kuhlmann: Vec<ElementId> = state
        .columns()
        .iter()
        .filter(|a| a.side == 0)
        .map(|a| a.id)
        .collect();
    let valkyries: Vec<ElementId> = state
        .columns()
        .iter()
        .filter(|a| a.side == 1)
        .map(|a| a.id)
        .collect();
    let mut pairs = Vec::new();
    for (ours, theirs) in kuhlmann.iter().zip(&valkyries) {
        let here = state.army(*ours).unwrap().pos;
        let there = here
            .all_neighbors()
            .into_iter()
            .find(|n| state.army_at(*n).is_none())
            .unwrap();
        let (from, to) = (world.stand_tile(reg, here), world.stand_tile(reg, there));
        let border = from
            .line_to(to)
            .take_while(|t| tactics_core::world::chunk_of(*t, radius) == here)
            .last()
            .unwrap();
        state.place_mut(*ours).unwrap().tile = Some(border);
        let e = state.place_mut(*theirs).unwrap();
        e.pos = there;
        e.tile = Some(to);
        state
            .apply(
                reg,
                &OverworldOrder::MoveArmy {
                    army: *ours,
                    to: there,
                },
            )
            .unwrap();
        pairs.push((*ours, *theirs));
    }
    state.apply(reg, &OverworldOrder::EndTurn).unwrap();
    for _ in 0..20_000 {
        if state.front.as_ref().is_some_and(|f| f.armies().len() == 4) {
            break;
        }
        state.advance_clock(reg, 1);
    }
    (state, pairs)
}

#[test]
fn two_fights_in_different_places_are_one_battle() {
    // The designer's ruling: no real separation between engagements, only
    // different things happening on different parts of the map at once. Two
    // contacts far apart are fought in one battle — the front — so there is
    // nothing to merge when they drift together and nothing to split when
    // they part.
    let reg = registry();
    let (state, pairs) = two_fights(&reg);
    let front = state.front.as_ref().expect("the fighting began");
    assert_eq!(
        front.armies().len(),
        4,
        "both fights are in the one battle: {:?}",
        front.armies()
    );
    let (a, b) = (
        state.army(pairs[0].1).unwrap(),
        state.army(pairs[1].1).unwrap(),
    );
    let apart = a.pos.unsigned_distance_to(b.pos);
    assert!(
        apart >= 2,
        "the two fights are in different places ({apart} hexes apart)"
    );
    // One battle for the world's fighting is not a battle over the world's
    // towns: a town is fought over while the fighting is near it, or a crew
    // here would be pulled toward a factory a day's march off.
    let world = state.world.as_ref().unwrap();
    let within = world.rules.towns.contested_within;
    let battle = front.battle();
    let crews: Vec<Hex> = battle
        .units
        .iter()
        .filter(|u| u.alive())
        .map(|u| u.pos)
        .collect();
    let mut towns = 0;
    for (objective, _) in battle.objectives() {
        let Some(i) = objective.id.strip_prefix("town-") else {
            continue;
        };
        let centre = world.skeleton.towns[i.parse::<usize>().unwrap()].centre;
        assert!(
            crews
                .iter()
                .any(|h| h.unsigned_distance_to(centre) <= within),
            "{} is fought over with nobody within {within} tiles of it",
            objective.id
        );
        towns += 1;
    }
    assert!(
        towns < world.skeleton.towns.len(),
        "not every town in the world is fought over"
    );
}

#[test]
fn a_fight_that_is_over_in_one_place_lets_its_army_go_while_another_goes_on() {
    // The other half of the ruling: an army is not held in the fighting by
    // somebody else's fight. The second pair's Valkyries are struck off the
    // field, and the Kuhlmann company that was fighting them — nobody left
    // within her reach — is a column again once the battle's own stalemate
    // patience has run, while the first pair are still at it.
    let reg = registry();
    let (mut state, pairs) = two_fights(&reg);
    let (winner, gone) = pairs[1];
    let front = state.front.as_mut().expect("the fighting began");
    for u in front.crews_of(gone) {
        if let Some(unit) = front.battle_mut().units.iter_mut().find(|x| x.id == u) {
            unit.fate = Fate::Destroyed(Destruction::Abandoned);
        }
    }
    for _ in 0..20_000 {
        if !state.army(winner).unwrap().fighting {
            break;
        }
        state.advance_clock(&reg, 1);
    }
    assert!(
        !state.army(winner).unwrap().fighting,
        "she left the fighting"
    );
    let front = state
        .front
        .as_ref()
        .expect("the other fight was still going on when she left");
    assert!(
        front.armies().contains(&pairs[0].0) && front.armies().contains(&pairs[0].1),
        "and it is still in the battle: {:?}",
        front.armies()
    );
    assert!(!front.armies().contains(&winner));
}

#[test]
fn the_game_opens_on_the_generated_campaign_and_one_it_does_not_have_fails_validation() {
    // The fourth round of rulings: the generated campaign is the default.
    // Which war a mod is about is the mod's to say, so it is `campaign` in
    // `mod.json`, and naming anything but an overworld map is an error
    // rather than a game that opens on nothing.
    let reg = registry();
    assert_eq!(reg.campaign.as_deref(), Some("frontier_world"));
    let state = OverworldState::from_map(&reg, "frontier_world", 1).unwrap();
    assert!(
        state.clocked(),
        "the default campaign runs on the world clock"
    );
    assert!(reg.validate().is_ok());
    for wrong in ["river_crossing", "nowhere"] {
        let mut reg = registry();
        reg.campaign = Some(wrong.into());
        let report = reg.validate();
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("`campaign`") && e.contains(wrong)),
            "{wrong}: {:?}",
            report.errors
        );
    }
}

// --- the chain of command -----------------------------------------------------

/// The living vehicles answering directly to `under`, as `(id, vehicle)`.
fn vehicles_under(state: &OverworldState, under: ElementId) -> Vec<(ElementId, ArmyUnit)> {
    let mut leaves: Vec<_> = state
        .elements
        .iter()
        .filter(|e| e.alive && e.parent == Some(under))
        .filter_map(|e| Some((e.seq, e.id, e.vehicle.clone()?)))
        .collect();
    leaves.sort_by_key(|(seq, id, _)| (*seq, *id));
    leaves.into_iter().map(|(_, id, v)| (id, v)).collect()
}

#[test]
fn a_campaigns_order_of_battle_is_each_sides_chain_of_command() {
    // The designer's ruling (2026-09-26): no separate state for anything on
    // the map; it all flows from the chain of command. Each side is one
    // tree — the side, its companies, their vehicles — and what the map
    // shows as a column is a node of it that stands on its own, carrying
    // exactly the vehicles beneath it.
    let reg = registry();
    for state in [frontier(&reg), generated(&reg)] {
        for side in 0..state.sides.len() as u8 {
            let roots: Vec<ElementId> = state
                .elements
                .iter()
                .filter(|e| e.side == side && e.parent.is_none())
                .map(|e| e.id)
                .collect();
            assert_eq!(roots.len(), 1, "one root a side");
            assert_eq!(state.side_root(side), Some(roots[0]));
            let root = state.element(roots[0]).unwrap();
            assert!(root.place.is_none() && root.vehicle.is_none());
            for column in state.side_armies(side) {
                let node = state.element(column.id).unwrap();
                assert_eq!(node.parent, Some(roots[0]), "a company answers to its side");
                let beneath: Vec<ArmyUnit> = vehicles_under(&state, column.id)
                    .into_iter()
                    .map(|(_, v)| v)
                    .collect();
                assert!(!beneath.is_empty());
                assert_eq!(column.units, beneath, "a column is what is beneath it");
            }
        }
        // Every vehicle answers to somebody on the map, and every company
        // that stands on the map is a column.
        for e in state.elements.iter().filter(|e| e.vehicle.is_some()) {
            let parent = state.element(e.parent.unwrap()).unwrap();
            assert!(parent.place.is_some(), "{} answers to a company", e.name);
        }
        let placed = state.elements.iter().filter(|e| e.place.is_some()).count();
        assert_eq!(state.columns().len(), placed);
    }
}

#[test]
fn a_vehicle_keeps_her_place_in_the_chain_of_command_through_a_fight() {
    // A fight hands each company back a list of what came home. Each
    // survivor is the vehicle she was — the same node, under the same
    // company — not a new one standing in for her, or a vehicle sent out on
    // her own could never be recognised as the one that went.
    let reg = registry();
    let (mut state, army, enemy) = about_to_meet(&reg);
    let before: Vec<(ElementId, Vec<ElementId>)> = [army, enemy]
        .iter()
        .map(|a| {
            (
                *a,
                vehicles_under(&state, *a)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect(),
            )
        })
        .collect();
    state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    let mut fought = false;
    for _ in 0..60_000 {
        fought |= state.front.is_some();
        let events = state.advance_clock(&reg, 1);
        if events
            .iter()
            .any(|e| matches!(e, OverworldEvent::FightingOver { .. }))
        {
            break;
        }
    }
    assert!(fought && state.front.is_none(), "the fight was fought out");
    let mut survivors = 0;
    for (company, was) in before {
        let Some(column) = state.army(company) else {
            continue;
        };
        let now = vehicles_under(&state, company);
        assert_eq!(column.units.len(), now.len());
        for (id, _) in now {
            assert!(
                was.contains(&id),
                "{id:?} came home a stranger to {company:?}"
            );
            survivors += 1;
        }
    }
    assert!(survivors > 0, "somebody came home to be recognised");
}

// --- sending a vehicle out ------------------------------------------------------

/// A Kuhlmann company with a vehicle to spare, and the node of one her
/// commander is not riding in.
fn spare_vehicle(state: &OverworldState) -> (ElementId, ElementId) {
    let commander = state.sides[0].commander;
    state
        .side_armies(0)
        .filter(|a| a.units.len() >= 2)
        .find_map(|a| {
            state
                .members(a.id)
                .into_iter()
                .find(|m| {
                    let crew = &state.element(*m).unwrap().vehicle.as_ref().unwrap().crew;
                    commander.is_none_or(|c| !crew.contains(&c))
                })
                .map(|m| (a.id, m))
        })
        .expect("a company with a vehicle to spare")
}

/// A campaign hex beside `army` that nobody stands on.
fn free_beside(state: &OverworldState, army: ElementId) -> Hex {
    let at = state.army(army).unwrap().pos;
    at.all_neighbors()
        .into_iter()
        .find(|n| state.map.contains(*n) && state.army_at(*n).is_none())
        .expect("a free hex beside her")
}

/// Run the clock until `done` holds, or give up.
fn run_until(
    reg: &DataRegistry,
    state: &mut OverworldState,
    done: impl Fn(&OverworldState) -> bool,
) {
    for _ in 0..40_000 {
        if done(state) {
            return;
        }
        state.advance_clock(reg, 1);
    }
}

#[test]
fn a_vehicle_sent_out_stands_on_her_own_and_holds_there_still_her_companys() {
    // WORLD.md W4.6b: an order to a vehicle below a company sends her out.
    // Nothing new is made — she is the same node under the same company,
    // with a place of her own and a standing order to hold — and when she
    // gets there she stays until she is recalled.
    let reg = registry();
    let mut state = generated(&reg);
    let (company, scout) = spare_vehicle(&state);
    let before = state.army(company).unwrap().units.len();
    let elements = state.elements.len();
    let to = free_beside(&state, company);
    let events = state
        .apply(&reg, &OverworldOrder::MoveArmy { army: scout, to })
        .unwrap();
    assert_eq!(
        events.first(),
        Some(&OverworldEvent::ArmyDetached {
            army: scout,
            from: company
        })
    );
    assert_eq!(state.elements.len(), elements, "nothing new was made");
    assert_eq!(state.element(scout).unwrap().parent, Some(company));
    let out = state.army(scout).expect("she stands on her own");
    assert_eq!(out.units.len(), 1);
    assert!(out.name.ends_with(&state.army(company).unwrap().name));
    assert_eq!(state.army(company).unwrap().units.len(), before - 1);

    run_until(&reg, &mut state, |s| s.army(scout).unwrap().march.is_none());
    assert_eq!(state.army(scout).unwrap().pos, to, "she got there");
    // Two days later she is still there: a hold, not a round trip.
    let dawn = state.ticks_to_dawn(&reg) + 2 * 17_280;
    state.advance_clock(&reg, dawn);
    assert_eq!(
        state.army(scout).map(|a| a.pos),
        Some(to),
        "holding until recalled"
    );
    assert_eq!(state.element(scout).unwrap().parent, Some(company));
}

#[test]
fn a_recalled_vehicle_goes_home_and_is_part_of_her_companys_column_again() {
    let reg = registry();
    let mut state = generated(&reg);
    let (company, scout) = spare_vehicle(&state);
    let before = state.army(company).unwrap().units.clone();
    let out = free_beside(&state, company);
    let further = out
        .all_neighbors()
        .into_iter()
        .find(|n| {
            state.map.contains(*n)
                && state.army_at(*n).is_none()
                && n.distance_to(state.army(company).unwrap().pos) == 2
        })
        .expect("a hex two away");
    state
        .apply(
            &reg,
            &OverworldOrder::MoveArmy {
                army: scout,
                to: further,
            },
        )
        .unwrap();
    run_until(&reg, &mut state, |s| s.army(scout).unwrap().march.is_none());
    assert_eq!(state.army(scout).unwrap().pos, further);

    let events = state
        .apply(&reg, &OverworldOrder::Recall { element: scout })
        .unwrap();
    assert_eq!(events, vec![OverworldEvent::ArmyRecalled { army: scout }]);
    let mut rejoined = false;
    for _ in 0..40_000 {
        rejoined |= state.advance_clock(&reg, 1).iter().any(|e| {
            *e == OverworldEvent::ArmyRejoined {
                army: scout,
                into: company,
            }
        });
        if rejoined {
            break;
        }
    }
    assert!(rejoined, "she came home");
    assert!(
        state.army(scout).is_none(),
        "she no longer stands on her own"
    );
    let element = state.element(scout).unwrap();
    assert!(element.alive && element.parent == Some(company));
    assert_eq!(
        state.army(company).unwrap().units,
        before,
        "and her company's column is what it was, in the order it was"
    );
}

#[test]
fn a_vehicle_that_cannot_be_sent_out_is_refused_and_nothing_changes() {
    use tactics_core::overworld::OverworldError;
    let reg = registry();
    let mut state = generated(&reg);
    let snapshot = format!("{:?}", state.elements);
    // The vehicle her commander rides in does not go to look.
    let commander = state.sides[0].commander.expect("the campaign names her");
    let hers = state
        .elements
        .iter()
        .find(|e| {
            e.vehicle
                .as_ref()
                .is_some_and(|v| v.crew.contains(&commander))
        })
        .unwrap()
        .id;
    let hq = state.element(hers).unwrap().parent.unwrap();
    let to = free_beside(&state, hq);
    assert_eq!(
        state.apply(&reg, &OverworldOrder::MoveArmy { army: hers, to }),
        Err(OverworldError::NoDetach)
    );
    // A company answers to its side: there is nowhere to recall it to.
    assert_eq!(
        state.apply(&reg, &OverworldOrder::Recall { element: hq }),
        Err(OverworldError::NoRecall)
    );
    assert_eq!(
        format!("{:?}", state.elements),
        snapshot,
        "nothing happened"
    );

    // A company's last vehicle is the company.
    let (company, _) = spare_vehicle(&state);
    let mut units = state.army(company).unwrap().units;
    units.truncate(1);
    state.set_vehicles(company, units);
    let last = state.members(company)[0];
    let to = free_beside(&state, company);
    assert_eq!(
        state.apply(&reg, &OverworldOrder::MoveArmy { army: last, to }),
        Err(OverworldError::NoDetach)
    );

    // A drawn campaign has no clock for her to march on.
    let mut drawn = frontier(&reg);
    let (company, pos) = drawn
        .side_armies(drawn.active_side)
        .find(|a| a.units.len() >= 2)
        .map(|a| (a.id, a.pos))
        .unwrap();
    let vehicle = drawn.members(company)[1];
    assert_eq!(
        drawn.apply(
            &reg,
            &OverworldOrder::MoveArmy {
                army: vehicle,
                to: pos.all_neighbors()[0]
            }
        ),
        Err(OverworldError::NoDetach)
    );
}
