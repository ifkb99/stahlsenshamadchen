//! What a learned goal chooser is fed, and the guarantee that feeding it
//! changes nothing (REIMAGINED.md, L0).
//!
//! Three rules. Recording a decision is a reading of the state and takes no
//! dice, so a battle fought with its decisions recorded is the battle fought
//! without. Every record is the shape its schema says, with the choice on its
//! own list. And a record holds no coordinate: a crew and her mirror twin on
//! a mirrored field are described identically, because a network shown which
//! way is west would learn it.

mod common;

use std::sync::{Arc, Mutex};
use tactics_core::ai::features::{CANDIDATE, GoalTap, context_names, describe};
use tactics_core::ai::goal::candidates;
use tactics_core::ai::{AiConfig, AiDriver, Evaluator, SideCommand, UtilityPlanner};
use tactics_core::battle::{BattleState, SideState};
use tactics_core::data::DataRegistry;
use tactics_core::map::Battlefield;
use tactics_core::roster::Roster;

fn config(planner: &str, doctrine: &str) -> AiConfig {
    AiConfig {
        planner: planner.into(),
        difficulty: 3,
        doctrine: Some(doctrine.into()),
    }
}

/// Fight `river_crossing` — the determinism baseline — for a few rounds under
/// the command planner on one side and the bare executor on the other, with
/// the decisions recorded or not, and hand back the event stream and the
/// records.
fn fight(reg: &DataRegistry, tap: Option<GoalTap>) -> (Vec<String>, usize) {
    let mut state = BattleState::from_map(reg, "river_crossing", 7).expect("baseline map");
    let mut ai = AiDriver::new();
    let command = SideCommand::from_config(&config("command", "massed_armor"), 11, reg);
    let utility = UtilityPlanner::from_config(&config("utility", "elastic_defense"), 12, reg);
    match &tap {
        Some(tap) => {
            ai.insert(0, Box::new(command.tapped(tap.clone())));
            ai.insert(1, Box::new(utility.tapped(tap.clone())));
        }
        None => {
            ai.insert(0, Box::new(command));
            ai.insert(1, Box::new(utility));
        }
    }
    let mut events = Vec::new();
    for _ in 0..8 {
        if state.is_over() {
            break;
        }
        ai.plan_round(reg, &mut state);
        events.extend(state.resolve_round(reg).iter().map(|e| format!("{e:?}")));
    }
    let recorded = tap.map_or(0, |t| t.lock().map(|r| r.len()).unwrap_or(0));
    (events, recorded)
}

#[test]
fn a_battle_whose_decisions_are_recorded_is_the_battle_fought_without() {
    let reg = common::registry();
    let tap: GoalTap = Arc::new(Mutex::new(Vec::new()));
    let (plain, _) = fight(&reg, None);
    let (tapped, recorded) = fight(&reg, Some(tap));
    assert!(recorded > 0, "the tap recorded no decisions at all");
    assert_eq!(
        plain, tapped,
        "recording the goal decisions changed the battle"
    );
}

#[test]
fn every_record_is_the_width_its_schema_names_with_the_choice_on_its_own_list() {
    let reg = common::registry();
    let tap: GoalTap = Arc::new(Mutex::new(Vec::new()));
    let _ = fight(&reg, Some(tap.clone()));
    let records = tap.lock().expect("tap").clone();
    let width = context_names().len();
    assert!(!records.is_empty());
    for record in &records {
        assert_eq!(record.context.len(), width, "context width");
        assert!(
            !record.candidates.is_empty(),
            "a decision with nothing to choose from"
        );
        for row in &record.candidates {
            assert_eq!(row.len(), CANDIDATE.len(), "candidate width");
            assert!(
                row.iter().all(|x| x.is_finite()),
                "a feature is not finite: {row:?}"
            );
        }
        assert!(
            record.chosen.is_some_and(|i| i < record.candidates.len()),
            "the chosen goal is not on the list it was chosen from"
        );
        assert!(record.context.iter().all(|x| x.is_finite()));
    }
}

/// Stage `battle_hills` — exactly mirror-symmetric, generated — with every
/// crew stamped anonymous, and pair each side-0 crew with the side-1 crew on
/// her reflected hex. The reflection is the generator's own: the row kept,
/// column `c` sent to `2·radius − row % 2 − c`.
fn mirrored_hills(reg: &DataRegistry) -> (BattleState, Vec<(usize, usize)>) {
    let file = reg.map("battle_hills").expect("battle_hills ships");
    let map = Battlefield::from_map_file(file).expect("the map builds");
    let mut placements = file.units.clone();
    for p in &mut placements {
        p.crew.clear();
    }
    let sides = file
        .sides
        .iter()
        .map(|s| SideState {
            name: s.name.clone(),
            ai: None,
        })
        .collect();
    let (roster, crews) = Roster::stamp_for(reg, &placements);
    let state =
        BattleState::from_placements(reg, map, sides, &placements, &crews, Arc::new(roster), 3)
            .expect("battle_hills stages");
    let span = 2 * reg.scale.battle_map_radius() as i32;
    let mirror = |h| {
        let [c, r] = tactics_core::hex_to_offset(h);
        tactics_core::offset_to_hex(span - r.rem_euclid(2) - c, r)
    };
    let mut pairs = Vec::new();
    for (i, a) in state.units.iter().enumerate().filter(|(_, u)| u.side == 0) {
        let twin = state.units.iter().position(|b| {
            b.side == 1
                && b.vehicle == a.vehicle
                && b.pos == mirror(a.pos)
                && b.aboard.is_some() == a.aboard.is_some()
        });
        pairs.push((i, twin.expect("battle_hills is mirrored crew for crew")));
    }
    (state, pairs)
}

fn sorted_rows(mut rows: Vec<Vec<f32>>) -> Vec<Vec<f32>> {
    rows.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    rows
}

fn close(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| (x - y).abs() <= 1e-3 * (1.0 + x.abs().max(y.abs())))
}

#[test]
fn a_crew_and_her_mirror_twin_are_described_identically() {
    // Detection dice taken out, or the twins' first looks are different
    // throws and so is what each of them knows.
    let reg = common::seen(common::registry());
    let (state, pairs) = mirrored_hills(&reg);
    assert!(!pairs.is_empty());
    let evaluator = Evaluator::new(reg.doctrine("massed_armor").cloned().expect("doctrine"));
    for (a, b) in pairs {
        let (ua, ub) = (&state.units[a], &state.units[b]);
        if ua.aboard.is_some() {
            continue;
        }
        let describe_one = |unit: &tactics_core::battle::Unit| {
            let options = candidates(&reg, &state, unit.id, None, None, 0.0);
            describe(&reg, &state, &evaluator, unit.id, &options)
        };
        let (ra, rb) = (describe_one(ua), describe_one(ub));
        assert!(
            close(&ra.context, &rb.context),
            "{} and her twin read different contexts:\n{:?}\n{:?}",
            ua.vehicle,
            ra.context,
            rb.context
        );
        let (ca, cb) = (sorted_rows(ra.candidates), sorted_rows(rb.candidates));
        assert_eq!(
            ca.len(),
            cb.len(),
            "{}: different candidate counts",
            ua.vehicle
        );
        for (x, y) in ca.iter().zip(&cb) {
            assert!(
                close(x, y),
                "{} and her twin read a candidate differently:\n{x:?}\n{y:?}",
                ua.vehicle
            );
        }
    }
}
