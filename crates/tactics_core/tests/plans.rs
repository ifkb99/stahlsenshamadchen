//! Plans (`tactics_core::ai::plan`): a commander matching a template she
//! knows onto the ground, PLANNING.md step 4.
//!
//! Contents: planning strength is the commander's own stats; a commander who
//! knows the play plans it and one who does not, does not; the word to go and
//! the three things that give it; a plan in flight is carried by the battle,
//! through a save; a play nobody declared is refused; and the playout that
//! chooses between plans scores what is left rather than who "won", and never
//! plans inside itself. That it is fought only on what the commander has
//! been told is in `net.rs`, beside the stage that separates the two.
//!
//! Staged on the ridge arena with two formations a side. No shipped doctrine
//! teaches a play yet — the instrument (`examples/plans`) has not shown one
//! winning — so each test teaches its own.

use tactics_core::ai::plan::{self, Strength};
use tactics_core::ai::{AiConfig, AiDriver, make_battle_planner};
use tactics_core::battle::{
    BattleState, Event, FormationId, Mission, Order, Plan, PlanPhase, SideState, UnitId,
    struck_facing,
};
use tactics_core::data::{ArmorFacing, DataRegistry};
use tactics_core::harness::arena::RIDGE_ARENA;
use tactics_core::map::UnitPlacement;
use tactics_core::roster::Roster;
use tactics_core::save::SaveGame;

mod common;
use common::{registry, seen};

/// The base mod, plus a doctrine that teaches fix and flank and one that
/// teaches nothing, both otherwise bounding overwatch.
///
/// **With no playouts**: the tests here that are about what a plan *is* — who
/// nominates it, when it goes, what the battle keeps — read the cheap model,
/// which decides alone at `playout_samples: 0`. Whether a playout then
/// prefers the plan to holding is a judgement about this stage's dice, not
/// about the machinery, and the playout's own tests are at the bottom.
fn schooled() -> DataRegistry {
    let mut reg = seen(registry());
    reg.planner.playout_samples = 0;
    let base = reg.doctrine("bounding_overwatch").unwrap().clone();
    for (id, teaches) in [
        ("taught", vec!["fix_and_flank".to_string()]),
        ("untaught", vec![]),
    ] {
        let mut d = base.clone();
        d.id = id.into();
        d.teaches = teaches;
        reg.doctrines.insert(id.into(), d);
    }
    reg
}

/// Two formations a side on the ridge: west's four mediums at `west_x`, east's
/// heavies-led pairs at `east_x`, in the arena's own lateral coordinates.
/// West's first leader is crewed by `commander`, her second by `second`.
fn ridge_stage(
    reg: &DataRegistry,
    west_x: i32,
    east_x: i32,
    commander: &str,
    second: &str,
) -> BattleState {
    let arena = &RIDGE_ARENA;
    let map = arena
        .map_with_formations(&[("w1", 0), ("w2", 0), ("e1", 1), ("e2", 1)])
        .unwrap();
    let place =
        |side: u8, x: i32, y: i32, vehicle: &str, formation: &str, leads: bool, crew: &str| {
            UnitPlacement {
                aboard_at: None,
                at: tactics_core::hex_to_offset(arena.rel(x, y)),
                side,
                vehicle: vehicle.into(),
                crew: if crew.is_empty() {
                    Vec::new()
                } else {
                    vec![crew.into()]
                },
                name: None,
                facing: None,
                formation: Some(formation.into()),
                leads,
            }
        };
    let placements = vec![
        place(0, west_x, 1, "medium_tank", "w1", true, commander),
        place(0, west_x, 3, "medium_tank", "w1", false, ""),
        place(0, west_x, -1, "medium_tank", "w2", true, second),
        place(0, west_x, -3, "medium_tank", "w2", false, ""),
        place(1, east_x, 1, "heavy_tank", "e1", true, ""),
        place(1, east_x, 3, "medium_tank", "e1", false, ""),
        place(1, east_x, -1, "heavy_tank", "e2", true, ""),
        place(1, east_x, -3, "medium_tank", "e2", false, ""),
    ];
    let (roster, crews) = Roster::stamp_for(reg, &placements);
    BattleState::from_placements(
        reg,
        map,
        ["West", "East"]
            .map(|name| SideState {
                name: name.into(),
                ai: None,
            })
            .to_vec(),
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        5,
    )
    .expect("the staged placements are content the base mod ships")
}

fn plan_round_for(reg: &DataRegistry, state: &mut BattleState, doctrine: &str) -> Vec<Event> {
    let mut ai = AiDriver::new();
    ai.insert(
        0,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 3,
                doctrine: Some(doctrine.into()),
            },
            9,
            reg,
        ),
    );
    let mut events = Vec::new();
    ai.plan_round_with(reg, state, |d| events.extend(d.events.iter().cloned()));
    events
}

/// Planning strength is the commander's own stats, not the side's
/// difficulty — the designer's ruling. Irma Krieger (`command` 14, `will` 13)
/// weighs more options, misjudges them less and holds a plan harder than a
/// cadet with no command training at all.
#[test]
fn a_commanders_planning_strength_is_her_own_stats() {
    let reg = schooled();
    let state = ridge_stage(&reg, -9, 9, "irma", "mina");
    let (strong, weak) = (
        Strength::of(&reg, &state, UnitId(0)),
        Strength::of(&reg, &state, UnitId(2)),
    );
    assert!(
        strong.breadth > weak.breadth
            && strong.misjudge < weak.misjudge
            && strong.commitment > weak.commitment,
        "Irma {strong:?} should out-plan an untrained cadet {weak:?} in every respect"
    );
    assert!(strong.level > weak.level);
}

/// A commander who knows fix and flank plans it — one formation fixing, the
/// other going round to ground off the enemy's frontal arc — and her side,
/// and only her side, hears her say so. The same commander from an academy
/// that never taught it divides the ground as commanders always have.
#[test]
fn a_commander_who_knows_the_play_plans_it_and_one_who_does_not_does_not() {
    let reg = schooled();
    let mut taught = ridge_stage(&reg, -6, 3, "irma", "");
    let events = plan_round_for(&reg, &mut taught, "taught");
    let plans: Vec<Plan> = taught.plans(0).cloned().collect();
    assert_eq!(plans.len(), 1, "one command, one plan: {events:?}");
    let p = &plans[0];
    assert_eq!(p.commander, UnitId(0), "the captain plans for the side");
    assert_ne!(p.fix, p.manoeuvre);
    let target = taught.unit(p.target).expect("aimed at somebody alive");
    assert_eq!(target.side, 1);
    assert_ne!(
        struck_facing(target.pos, target.facing, p.flank),
        ArmorFacing::Front,
        "a flank is off his front"
    );
    let adopted = events
        .iter()
        .find(|e| matches!(e, Event::PlanAdopted { .. }))
        .expect("and she says so");
    assert!(adopted.heard_by(&taught, 0) && !adopted.heard_by(&taught, 1));

    let mut untaught = ridge_stage(&reg, -6, 3, "irma", "");
    plan_round_for(&reg, &mut untaught, "untaught");
    assert_eq!(untaught.plans(0).count(), 0, "nobody taught her the play");
}

/// The word to go is given when the flankers are gathered and the fix is in
/// place — or at once if the flankers are found, or when the deadline the plan
/// set itself passes. Otherwise nothing: a plan waits for its moment.
#[test]
fn the_word_to_go_waits_for_its_moment_and_no_longer() {
    let reg = schooled();
    // Out of each other's sight, so nobody is found and nobody is engaged.
    let state = ridge_stage(&reg, -9, 9, "irma", "");
    let (fix_at, gather_at) = (
        state.unit(UnitId(0)).unwrap().pos,
        state.unit(UnitId(2)).unwrap().pos,
    );
    let far = RIDGE_ARENA.rel(0, 11);
    let plan = |fire, assembly, go_by| Plan {
        side: 0,
        commander: UnitId(0),
        template: "fix_and_flank".into(),
        fix: FormationId(0),
        manoeuvre: FormationId(1),
        target: UnitId(4),
        fire_position: fire,
        assembly,
        flank: RIDGE_ARENA.rel(6, 5),
        phase: PlanPhase::Forming,
        score: 0,
        go_by,
    };
    let goes = |p: &Plan| {
        let orders = plan::trigger(&reg, &state, p);
        orders.iter().any(|o| {
            matches!(o, Order::SetMission { mission: Mission::Assault { .. }, formation, .. } if *formation == FormationId(1))
        })
    };
    assert!(goes(&plan(fix_at, gather_at, 99)), "both in place: go");
    assert!(
        !goes(&plan(far, gather_at, 99)),
        "the fix is not in place: wait"
    );
    assert!(
        !goes(&plan(fix_at, far, 99)),
        "the flankers are not gathered: wait"
    );
    assert!(
        goes(&plan(far, far, state.round)),
        "the deadline has come: go anyway"
    );
}

/// Once the word is given the attack goes in: a going plan is carried
/// through while it stands, never swapped for a better-looking one. The first
/// measurement caught a commander recalling her own assault in the review
/// that ordered it.
///
/// Staged so a better-looking plan certainly exists: the going plan fixes from
/// the hex beside the enemy, under every gun he has, which any fresh reading
/// of the ground beats by far more than her commitment. It must stand anyway.
#[test]
fn once_the_word_is_given_the_plan_is_carried_through() {
    let reg = schooled();
    // What she would plan here, to borrow a flank that is really a flank.
    let mut first = ridge_stage(&reg, -6, 3, "irma", "");
    plan_round_for(&reg, &mut first, "taught");
    let good = first
        .plans(0)
        .next()
        .cloned()
        .expect("a plan to borrow from");

    let mut state = ridge_stage(&reg, -6, 3, "irma", "");
    let target = state.unit(good.target).unwrap().pos;
    let exposed = target
        .all_neighbors()
        .into_iter()
        .find(|h| state.map.contains(*h) && state.unit_at(*h).is_none())
        .expect("somewhere beside him");
    let going = Plan {
        fire_position: exposed,
        phase: PlanPhase::Going,
        ..good.clone()
    };
    state
        .apply(
            &reg,
            &Order::SetPlan {
                side: 0,
                commander: going.commander,
                plan: Some(going.clone()),
            },
        )
        .unwrap();
    plan_round_for(&reg, &mut state, "taught");
    assert_eq!(
        state.plan_of(going.commander),
        Some(&going),
        "a going plan is not replaced while it stands"
    );
}

/// A plan in flight lives on the battle, so a saved battle remembers what
/// its commander meant — a planner's private memory would not survive the
/// save that `tests/save.rs` forks battles through.
#[test]
fn a_plan_in_flight_is_saved_with_the_battle() {
    let reg = schooled();
    let mut state = ridge_stage(&reg, -6, 3, "irma", "");
    plan_round_for(&reg, &mut state, "taught");
    let before: Vec<Plan> = state.plans(0).cloned().collect();
    assert!(!before.is_empty());
    let text = SaveGame::new(&reg, None, Some(state)).to_json().unwrap();
    let restored = SaveGame::from_json(&reg, &text).unwrap().0.battle.unwrap();
    assert_eq!(restored.plans(0).cloned().collect::<Vec<_>>(), before);
}

/// A play nobody declared is a play nobody can run: refused by validation
/// rather than quietly never chosen.
#[test]
fn a_play_nobody_declared_is_refused() {
    let mut reg = schooled();
    reg.doctrines
        .get_mut("taught")
        .unwrap()
        .teaches
        .push("nonesuch".into());
    let mut report = tactics_core::data::ValidationReport::default();
    reg.validate_into(&mut report);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("taught") && e.contains("nonesuch")),
        "{:?}",
        report.errors
    );
}

// --- the playout ---

/// A playout scores what the options leave on the board, never the copy's
/// verdict. A board that holds only the enemies she knows can "end" the
/// moment they are gone — here, at once, since she knows of nobody — and the
/// first draft scored almost every option as a won battle, 1.0, which could
/// tell none of them apart.
#[test]
fn a_playout_scores_what_is_left_rather_than_who_won_the_copy() {
    let reg = schooled();
    let state = ridge_stage(&reg, -9, 9, "irma", "");
    let ours = AiConfig {
        planner: "command".into(),
        difficulty: 3,
        doctrine: Some("taught".into()),
    };
    let value = plan::playout(&reg, &state, 0, &[], &ours, 11);
    assert!(
        value < 1.0,
        "a board with nobody on it but her is not a won battle: {value}"
    );
    assert_eq!(
        value,
        plan::playout(&reg, &state, 0, &[], &ours, 11),
        "and the same dice read the same"
    );
}

/// The commander a playout runs her side under carries out the plan she was
/// handed and makes none of her own — or every playout would play out
/// playouts. The same stage under an ordinary commander plans at once
/// (`a_commander_who_knows_the_play_plans_it_...`).
#[test]
fn a_commander_inside_a_playout_makes_no_plans_of_her_own() {
    let reg = schooled();
    let mut state = ridge_stage(&reg, -6, 3, "irma", "");
    let mut ai = AiDriver::new();
    ai.insert(
        0,
        Box::new(tactics_core::ai::SideCommand::for_playout(
            &AiConfig {
                planner: "command".into(),
                difficulty: 3,
                doctrine: Some("taught".into()),
            },
            9,
            &reg,
        )),
    );
    ai.plan_round(&reg, &mut state);
    assert_eq!(state.plans(0).count(), 0);
}
