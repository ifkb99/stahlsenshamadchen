//! What a learned goal chooser is fed, and the guarantee that feeding it
//! changes nothing (REIMAGINED.md, L0).
//!
//! Three rules about the record. Recording a decision is a reading of the
//! state and takes no dice, so a battle fought with its decisions recorded is
//! the battle fought without. Every record is the shape its schema says, with
//! the choice on its own list. And a record holds no coordinate: a crew and
//! her mirror twin on a mirrored field are described identically, because a
//! network shown which way is west would learn it.
//!
//! And four about the network (L0.5). It is refused if it was trained on
//! other features, or if the engine's arithmetic disagrees with the
//! trainer's on its own probe; it scores by exactly the arithmetic it was
//! trained with; the chooser takes what it scores highest; and a `learned`
//! planner with no network is the utility planner, battle for battle.

mod common;

use std::sync::{Arc, Mutex};
use tactics_core::ai::features::{CANDIDATE, GoalTap, context_names, describe};
use tactics_core::ai::goal::GoalChooser;
use tactics_core::ai::goal::candidates;
use tactics_core::ai::learned::LearnedChooser;
use tactics_core::ai::make_battle_planner;
use tactics_core::ai::{AiConfig, AiDriver, Evaluator, SideCommand, UtilityPlanner};
use tactics_core::battle::Goal;
use tactics_core::battle::{BattleState, SideState};
use tactics_core::data::{DataRegistry, GoalNet, Layer, NetError, Probe};
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
///
/// `closer` moves every crew that many columns toward the axis, each twin by
/// the reflected amount, so that the two sides can be staged in sight of each
/// other and the relational features have somebody to be about.
fn mirrored_hills(reg: &DataRegistry, closer: i32) -> (BattleState, Vec<(usize, usize)>) {
    let file = reg.map("battle_hills").expect("battle_hills ships");
    let map = Battlefield::from_map_file(file).expect("the map builds");
    let span = 2 * reg.scale.battle_map_radius() as i32;
    let mut placements = file.units.clone();
    let toward = |[c, r]: [i32; 2]| {
        let axis2 = span - r.rem_euclid(2);
        if 2 * c < axis2 {
            [c + closer, r]
        } else {
            [c - closer, r]
        }
    };
    for p in &mut placements {
        p.crew.clear();
        p.at = toward(p.at);
        p.aboard_at = p.aboard_at.map(toward);
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

/// Every crew and her twin, described, compared row for row once each list is
/// sorted. Returns how many enemies the side-0 crews knew of, summed, so a
/// caller can require that the stage was about somebody.
fn twins_agree(reg: &DataRegistry, closer: i32) -> usize {
    let (state, pairs) = mirrored_hills(reg, closer);
    assert!(!pairs.is_empty());
    let evaluator = Evaluator::new(reg.doctrine("massed_armor").cloned().expect("doctrine"));
    let mut known = 0;
    for (a, b) in pairs {
        let (ua, ub) = (&state.units[a], &state.units[b]);
        if ua.aboard.is_some() {
            continue;
        }
        known += state
            .known_enemies(reg, tactics_core::battle::Knower::Crew(ua.id))
            .len();
        let describe_one = |unit: &tactics_core::battle::Unit| {
            let options = candidates(reg, &state, unit.id, None, None, 0.0);
            describe(reg, &state, &evaluator, unit.id, &options)
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
    known
}

#[test]
fn a_crew_and_her_mirror_twin_are_described_identically() {
    // Detection dice taken out, or the twins' first looks are different
    // throws and so is what each of them knows.
    let reg = common::seen(common::registry());
    twins_agree(&reg, 0);

    // And with places of her own on the list and the two sides staged in
    // sight of each other, which is where the sampler and the relational
    // features (aspect, crossfire, who sees the place) have something to say.
    let mut reg = reg;
    reg.planner.goal_places = 16;
    let known = twins_agree(&reg, 7);
    assert!(known > 0, "the close stage put nobody in anybody's sight");
}

#[test]
fn places_widen_the_list_spaced_apart_and_zero_is_the_list_before_them() {
    let mut reg = common::registry();
    let state = BattleState::from_map(&reg, "river_crossing", 7).expect("baseline map");
    let unit = state
        .units
        .iter()
        .find(|u| u.aboard.is_none())
        .expect("a crew")
        .id;
    let before = candidates(&reg, &state, unit, None, None, 0.0);
    reg.planner.goal_places = 12;
    reg.planner.place_spacing = 3;
    let after = candidates(&reg, &state, unit, None, None, 0.0);
    assert!(
        after.len() > before.len(),
        "{} candidates with places, {} without",
        after.len(),
        before.len()
    );
    // Every candidate the list had, it still has, in the same order.
    let kept: Vec<_> = after.iter().filter(|g| before.contains(g)).collect();
    assert_eq!(kept, before.iter().collect::<Vec<_>>());
    let added: Vec<_> = after
        .iter()
        .filter_map(|g| match g {
            Goal::Take(hex) if !before.contains(g) => Some(*hex),
            _ => None,
        })
        .collect();
    // A tie group is taken whole (see `goal::places`), so the list may run a
    // little past the number asked for, never far.
    assert!(
        !added.is_empty() && added.len() <= 18,
        "{} places",
        added.len()
    );
    // Representatives may sit nearer each other than the spacing, but never
    // on top of one another.
    for (i, a) in added.iter().enumerate() {
        assert!(added[i + 1..].iter().all(|b| b != a), "a place drawn twice");
    }
    reg.planner.goal_places = 0;
    assert_eq!(candidates(&reg, &state, unit, None, None, 0.0), before);
}

// --- the network ------------------------------------------------------------

/// A one-layer network over the engine's own feature list: a single score,
/// `weight` times the named candidate feature.
fn one_feature_net(feature: &str, weight: f32) -> GoalNet {
    let context = context_names();
    let width = context.len() + CANDIDATE.len();
    let at = context.len()
        + CANDIDATE
            .iter()
            .position(|f| *f == feature)
            .expect("a feature");
    let mut w = vec![vec![0.0]; width];
    w[at][0] = weight;
    GoalNet {
        context,
        candidate: CANDIDATE.iter().map(|s| s.to_string()).collect(),
        mean: vec![0.0; width],
        std: vec![1.0; width],
        layers: vec![Layer { w, b: vec![0.0] }],
        trained_on: Default::default(),
        probe: Vec::new(),
    }
}

#[test]
fn a_network_trained_on_other_features_is_refused() {
    let mut net = one_feature_net("hold", 1.0);
    net.context[0] = "something_else".into();
    let err = net.check(&context_names(), CANDIDATE).unwrap_err();
    assert!(
        matches!(
            err,
            NetError::Features {
                which: "context",
                ..
            }
        ),
        "{err}"
    );

    let mut net = one_feature_net("hold", 1.0);
    net.candidate.pop();
    let err = net.check(&context_names(), CANDIDATE).unwrap_err();
    assert!(
        matches!(
            err,
            NetError::Features {
                which: "candidate",
                ..
            }
        ),
        "{err}"
    );

    assert!(
        one_feature_net("hold", 1.0)
            .check(&context_names(), CANDIDATE)
            .is_ok()
    );
}

#[test]
fn a_network_scores_by_the_arithmetic_it_was_trained_with() {
    // Two inputs, two hidden units, one output, standardised: hand-computed.
    // x = (3, 1) standardised by mean (1, 1) and std (2, 1) is (1, 0).
    // hidden = relu(x · w1 + b1) = relu((1·2 + 0, 1·-3 + 0) + (0.5, 0.5))
    //        = relu(2.5, -2.5) = (2.5, 0) — the ReLU is what the second unit is
    //   there to test. output = 2.5·4 + 0·10 + 1 = 11.
    let net = GoalNet {
        context: vec!["a".into()],
        candidate: vec!["b".into()],
        mean: vec![1.0, 1.0],
        std: vec![2.0, 1.0],
        layers: vec![
            Layer {
                w: vec![vec![2.0, -3.0], vec![7.0, 7.0]],
                b: vec![0.5, 0.5],
            },
            Layer {
                w: vec![vec![4.0], vec![10.0]],
                b: vec![1.0],
            },
        ],
        trained_on: Default::default(),
        probe: Vec::new(),
    };
    assert!(net.check(&["a".to_string()], &["b"]).is_ok());
    assert!((net.score(&[3.0], &[1.0]) - 11.0).abs() < 1e-5);
}

#[test]
fn a_network_whose_arithmetic_disagrees_with_its_trainer_is_refused() {
    let context = context_names();
    let mut row = vec![0.0; CANDIDATE.len()];
    row[0] = 1.0; // `hold`
    let probe = |score| Probe {
        context: vec![0.0; context.len()],
        candidate: row.clone(),
        score,
    };
    let mut net = one_feature_net("hold", 2.0);
    net.probe = vec![probe(2.0)];
    assert!(net.check(&context, CANDIDATE).is_ok());
    net.probe = vec![probe(2.1)];
    let err = net.check(&context, CANDIDATE).unwrap_err();
    assert!(matches!(err, NetError::Probe { .. }), "{err}");
}

#[test]
fn a_learned_chooser_takes_the_candidate_its_network_scores_highest() {
    let reg = common::registry();
    let state = BattleState::from_map(&reg, "river_crossing", 7).expect("baseline map");
    let unit = state
        .units
        .iter()
        .find(|u| u.aboard.is_none())
        .expect("a crew")
        .id;
    let options = candidates(&reg, &state, unit, None, None, 0.0);
    assert!(options.contains(&Goal::Hold) && options.len() > 1);
    let evaluator = Evaluator::new(Default::default());
    for (weight, wants_hold) in [(1.0, true), (-1.0, false)] {
        let net = one_feature_net("hold", weight);
        let chosen = LearnedChooser {
            net: &net,
            evaluator: &evaluator,
        }
        .choose(&reg, &state, unit, &options);
        assert_eq!(
            chosen == Goal::Hold,
            wants_hold,
            "weight {weight} chose {chosen:?}"
        );
    }
}

#[test]
fn a_learned_planner_with_no_network_is_the_utility_planner() {
    let reg = common::registry();
    assert!(reg.goal_net.is_none(), "the base mod ships no network");
    let fight_with = |planner: &str| {
        let mut state = BattleState::from_map(&reg, "river_crossing", 7).expect("baseline map");
        let mut ai = AiDriver::new();
        for side in 0..2u8 {
            let config = AiConfig {
                planner: planner.into(),
                difficulty: 3,
                doctrine: Some("massed_armor".into()),
            };
            ai.insert(side, make_battle_planner(&config, 40 + side as u64, &reg));
        }
        let mut events = Vec::new();
        for _ in 0..6 {
            ai.plan_round(&reg, &mut state);
            events.extend(state.resolve_round(&reg).iter().map(|e| format!("{e:?}")));
        }
        events
    };
    assert_eq!(fight_with("learned"), fight_with("utility"));
    assert_eq!(fight_with("learned_command"), fight_with("command"));
}

// --- exploring --------------------------------------------------------------

/// A few rounds of `river_crossing` with side 0's goals chosen by `brain`,
/// tapped, and the records it took.
fn explored(
    reg: &DataRegistry,
    brain: tactics_core::ai::learned::GoalBrain,
) -> Vec<tactics_core::ai::features::GoalRecord> {
    let tap: GoalTap = Arc::new(Mutex::new(Vec::new()));
    let mut state = BattleState::from_map(reg, "river_crossing", 7).expect("baseline map");
    let mut ai = AiDriver::new();
    let learner = UtilityPlanner::from_config(&config("utility", "massed_armor"), 11, reg)
        .with_brain(brain)
        .tapped(tap.clone());
    ai.insert(0, Box::new(learner));
    ai.insert(
        1,
        Box::new(UtilityPlanner::from_config(
            &config("utility", "elastic_defense"),
            12,
            reg,
        )),
    );
    for _ in 0..6 {
        if state.is_over() {
            break;
        }
        ai.plan_round(reg, &mut state);
        state.resolve_round(reg);
    }
    tap.lock().expect("tap").clone()
}

#[test]
fn an_exploring_network_samples_from_its_own_softmax_and_says_how_likely_its_choice_was() {
    use tactics_core::ai::learned::{GoalBrain, log_softmax};
    let probs: f32 = log_softmax(&[1.0, 2.0, -3.0, 0.5], 0.7)
        .iter()
        .map(|l| l.exp())
        .sum();
    assert!((probs - 1.0).abs() < 1e-5, "a softmax that sums to {probs}");

    let reg = common::registry();
    let net = Arc::new(one_feature_net("hold", 0.0));
    let brain = GoalBrain::Explore {
        net,
        temperature: 1.0,
    };
    let first = explored(&reg, brain.clone());
    assert!(!first.is_empty());
    for record in &first {
        let logp = record
            .logp
            .expect("an exploring choice carries its log-probability");
        // A network that scores everything zero is uniform over the list.
        let uniform = -(record.candidates.len() as f32).ln();
        assert!(
            (logp - uniform).abs() < 1e-4,
            "logp {logp} against uniform {uniform}"
        );
        assert!(record.chosen.is_some_and(|i| i < record.candidates.len()));
        assert_eq!(record.side, 0);
    }
    // Uniform over several candidates, it does not always take the first.
    assert!(
        first.iter().any(|r| r.chosen != Some(0)),
        "a sampling chooser that never strayed from the head of the list"
    );
    // And the dice are the planner's, so the same seed explores the same way.
    assert_eq!(first, explored(&reg, brain));
}

// --- goal events ------------------------------------------------------------

#[test]
fn a_witness_sees_contact_nerve_and_orders_change_and_nothing_when_nothing_has() {
    use tactics_core::battle::{GoalTriggers, GoalWitness, Knower};
    let reg = common::seen(common::registry());
    let (mut state, pairs) = mirrored_hills(&reg, 7);
    let unit = pairs
        .iter()
        .map(|(a, _)| state.units[*a].id)
        .find(|id| !state.known_enemies(&reg, Knower::Crew(*id)).is_empty())
        .expect("the close stage puts somebody in sight");
    let goal = Goal::Take(state.unit(unit).unwrap().pos);
    let witness = GoalWitness::now(&reg, &state, unit, goal).expect("a witness");
    assert_eq!(
        witness.triggers(&reg, &state, unit),
        GoalTriggers::default(),
        "nothing has happened since she chose"
    );

    // An enemy she did not know of then.
    let mut blind = witness.clone();
    blind.known.clear();
    assert!(blind.triggers(&reg, &state, unit).contact);

    // Pinned now and not then.
    let pinned_at = reg
        .morale
        .rungs
        .iter()
        .find(|r| r.pinned)
        .map(|r| r.at_pressure)
        .expect("the base mod has a pinned rung");
    state
        .units
        .iter_mut()
        .find(|u| u.id == unit)
        .unwrap()
        .pressure = pinned_at;
    let t = witness.triggers(&reg, &state, unit);
    assert!(t.pinned && !t.contact);

    // A listener hears only what it listens for.
    let listen = tactics_core::data::GoalEvents {
        contact: true,
        ..Default::default()
    };
    assert!(!t.any(&listen));
    assert!(t.any(&tactics_core::data::GoalEvents::all()));
}

#[test]
fn an_interrupted_goal_goes_back_on_the_list_and_a_finished_one_does_not() {
    use tactics_core::ai::goal::with_interrupted;
    use tactics_core::battle::GoalWitness;
    let reg = common::registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 7).expect("baseline map");
    let unit = state
        .units
        .iter()
        .find(|u| u.aboard.is_none())
        .expect("a crew")
        .id;
    let options = candidates(&reg, &state, unit, None, None, 0.0);
    // Somewhere she is not, and not already on her list.
    let far = state
        .units
        .iter()
        .filter(|u| u.side != state.unit(unit).unwrap().side)
        .map(|u| u.pos)
        .find(|h| !options.contains(&Goal::Take(*h)))
        .expect("an enemy's hex");
    let mut witness = GoalWitness::now(&reg, &state, unit, Goal::Take(far)).unwrap();
    state
        .units
        .iter_mut()
        .find(|u| u.id == unit)
        .unwrap()
        .goal_witness = Some(witness.clone());
    assert_eq!(
        with_interrupted(&reg, &state, unit, options.clone()),
        options,
        "a goal that was not interrupted is not put back"
    );
    witness.interrupted = true;
    state
        .units
        .iter_mut()
        .find(|u| u.id == unit)
        .unwrap()
        .goal_witness = Some(witness);
    let back = with_interrupted(&reg, &state, unit, options.clone());
    assert_eq!(back.len(), options.len() + 1);
    let at = back
        .iter()
        .position(|g| *g == Goal::Take(far))
        .expect("put back");
    assert_eq!(back[at + 1], Goal::Hold, "just ahead of holding");
}

#[test]
fn crews_listening_for_events_decide_more_often_and_listening_for_none_is_the_game_before() {
    let mut reg = common::registry();
    let count = |reg: &DataRegistry| {
        let tap: GoalTap = Arc::new(Mutex::new(Vec::new()));
        let _ = fight(reg, Some(tap.clone()));
        tap.lock().map(|r| r.len()).unwrap_or(0)
    };
    let (quiet_events, quiet) = (fight(&reg, None).0, count(&reg));
    reg.planner.goal_events = tactics_core::data::GoalEvents::all();
    let listening = count(&reg);
    assert!(
        listening > quiet,
        "{listening} decisions listening for every event, {quiet} for none"
    );
    reg.planner.goal_events = Default::default();
    assert_eq!(fight(&reg, None).0, quiet_events);
}
