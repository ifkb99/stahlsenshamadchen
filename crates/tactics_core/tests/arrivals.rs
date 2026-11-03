//! Reinforcements: formations that arrive during a battle
//! (`map::ArrivalSpec`, SLICE.md *Reinforcements*).
//!
//! The rule in one line: a unit that arrives did not exist until it did.
//! Before its round nothing can see, shoot, count or lose it; at the top of
//! that round it is on the field, its side hears that it came, and the enemy
//! finds out by seeing it. A map that names no arrival is the battle it
//! always was, which the determinism snapshot defends unregenerated.
//!
//! Fought on `slice_hollerbach`, where the Valkyries' Wiesel is expected in
//! round 4, give or take one.

use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, make_battle_planner};
use tactics_core::battle::{BattleState, Event, Order, UnitId};
use tactics_core::data::DataRegistry;
use tactics_core::save::SaveGame;

mod common;
use common::{registry, strike_down};

const MAP: &str = "slice_hollerbach";

fn planner(reg: &DataRegistry, seed: u64) -> Box<dyn AiPlanner<BattleState, Order>> {
    make_battle_planner(
        &AiConfig {
            planner: "utility".into(),
            difficulty: 4,
            doctrine: Some("massed_armor".into()),
        },
        seed,
        reg,
    )
}

/// One round, both sides planned by the utility AI; its events.
fn round(reg: &DataRegistry, state: &mut BattleState, seed: u64) -> Vec<Event> {
    let mut ai = AiDriver::new();
    ai.insert(0, planner(reg, seed));
    ai.insert(1, planner(reg, seed + 1));
    ai.plan_round(reg, state);
    state.resolve_round(reg)
}

fn wiesel(state: &BattleState) -> Option<UnitId> {
    state
        .units
        .iter()
        .find(|u| u.vehicle == "light_tank")
        .map(|u| u.id)
}

#[test]
fn a_reinforcement_is_not_on_the_field_until_its_round() {
    let reg = registry();
    for seed in 1..=6 {
        let mut state = BattleState::from_map(&reg, MAP, seed).unwrap();
        assert_eq!(
            wiesel(&state),
            None,
            "nobody has a Wiesel to see or count yet"
        );
        let due = state.arrivals[0].round;
        assert!(
            (3..=5).contains(&due),
            "round 4 give or take one, rolled {due}"
        );
        assert_eq!(state.arrivals[0].expected, 4, "what her side was told");

        let mut heard = None;
        while !state.is_over() && state.round < due {
            assert_eq!(
                wiesel(&state),
                None,
                "still on the road in round {}",
                state.round
            );
            for ev in round(&reg, &mut state, seed) {
                if let Event::Arrived { unit, at } = ev {
                    heard = Some((unit, at));
                }
            }
        }
        if state.is_over() {
            continue;
        }
        let tank = wiesel(&state).expect("she came at the top of her round");
        let (unit, at) = heard.expect("and her coming was an event");
        assert_eq!(unit, tank);
        let u = state.unit(tank).expect("on the field, alive");
        assert_eq!(u.pos, at);
        assert_eq!(
            state.round, due,
            "she arrived as round {due} opened, before its planning"
        );
        assert!(
            state.units.iter().all(|other| other.id <= tank),
            "her id follows every id already on the field"
        );
        assert!(state.arrivals.is_empty(), "and nobody else is on the way");
        let arrived = Event::Arrived { unit, at };
        assert!(arrived.heard_by(&state, 1), "her side hears she has come");
        assert!(!arrived.heard_by(&state, 0), "the enemy has to see her");
    }
}

#[test]
fn a_side_with_help_on_the_way_is_not_beaten() {
    // Strike down every Valkyrie on the field before the Wiesel comes: the
    // side has nobody on the board, but it has somebody on the road, and a
    // battle that ended there would end on a fact nobody could see.
    let reg = registry();
    let mut state = BattleState::from_map(&reg, MAP, 2).unwrap();
    let theirs: Vec<UnitId> = state
        .units
        .iter()
        .filter(|u| u.side == 1 && u.alive())
        .map(|u| u.id)
        .collect();
    for id in theirs {
        strike_down(&mut state, id);
    }
    let due = state.arrivals[0].round;
    while state.round < due && !state.is_over() {
        round(&reg, &mut state, 2);
    }
    assert!(!state.is_over(), "nobody on the field is not nobody coming");
    assert!(wiesel(&state).is_some(), "and here she is");
}

#[test]
fn an_arrival_on_the_way_survives_a_save() {
    // The future must round-trip: a battle forked through a save before the
    // Wiesel comes must have her come in the same round, to the same hex,
    // and the same battle after.
    let reg = registry();
    let mut original = BattleState::from_map(&reg, MAP, 5).unwrap();
    round(&reg, &mut original, 5);
    assert!(!original.arrivals.is_empty(), "still on her way when saved");
    let text = SaveGame::new(&reg, None, Some(original.clone()))
        .to_json()
        .expect("serialises");
    let mut restored = SaveGame::from_json(&reg, &text)
        .expect("deserialises")
        .0
        .battle
        .expect("a battle");
    assert_eq!(restored.arrivals, original.arrivals);
    let play = |state: &mut BattleState| -> Vec<String> {
        (0..5)
            .flat_map(|_| round(&reg, state, 5))
            .filter(|e| !matches!(e, Event::TickStarted { .. }))
            .map(|e| format!("{e:?}"))
            .collect()
    };
    let expected = play(&mut original);
    assert!(
        expected.iter().any(|e| e.starts_with("Arrived")),
        "she came in the window"
    );
    assert_eq!(play(&mut restored), expected);
}
