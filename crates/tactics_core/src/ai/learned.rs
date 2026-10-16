//! The learned goal chooser: a trained network behind [`GoalChooser`].
//!
//! REIMAGINED.md L0.5. It sees exactly what [`super::features::describe`]
//! writes down for the dataset — so what it was trained on and what it is
//! shown in play are one function — scores every candidate, and takes the
//! highest. Everything after the choice (the road, the shot, the drill,
//! defiance) is the executor's, as it is for the hand-written chooser.
//!
//! It draws no dice. Difficulty's blur is the hand-written chooser's model
//! of a commander who misjudges the ground; a learned commander's
//! misjudgments are her own, and a weaker one is a different network (or,
//! later, a different trait vector), not noise on this one.

use super::Evaluator;
use super::features::{CANDIDATE, context_names, describe_surveyed};
use super::goal::{GoalChooser, Survey};
use crate::battle::{BattleState, Goal, UnitId};
use crate::data::{DataRegistry, GoalNet, NetError};
use std::path::Path;
use std::sync::Arc;

/// Who chooses a crew's goals: the hand-written chooser, a trained network,
/// or a coin.
///
/// `Random` is an instrument, not an opponent. It takes a candidate uniformly
/// from the list, and what it loses by against the utility chooser is how
/// much the goal decision is *worth* on a battlefield — the headroom any
/// learned chooser has to find, and the signal reinforcement learning has to
/// learn from. A goal layer a coin can run as well as the teacher is a layer
/// whose decisions do not decide anything.
///
/// `Explore` is the network *sampling* from its own softmax at a temperature
/// instead of taking its best: how a policy in training tries things. It
/// records the log-probability of what it chose (`GoalRecord::logp`), which is
/// the half of a policy-gradient ratio only the actor knows. It draws from the
/// planner's rng, so an exploring battle is still reproducible from its seed.
#[derive(Debug, Clone, Default)]
pub enum GoalBrain {
    #[default]
    Utility,
    Learned(Arc<GoalNet>),
    Explore {
        net: Arc<GoalNet>,
        temperature: f32,
    },
    Random,
}

/// Every candidate's score under `net`, beside the description that was
/// scored. One description per decision, shared by the chooser and the tap.
pub fn score_candidates(
    net: &GoalNet,
    registry: &DataRegistry,
    state: &BattleState,
    evaluator: &Evaluator,
    unit: UnitId,
    candidates: &[Goal],
) -> (crate::ai::features::GoalRecord, Vec<f32>) {
    score_candidates_surveyed(
        net,
        registry,
        state,
        evaluator,
        unit,
        candidates,
        &Survey::new(),
    )
}

/// [`score_candidates`], reading her roads from `survey`.
pub fn score_candidates_surveyed(
    net: &GoalNet,
    registry: &DataRegistry,
    state: &BattleState,
    evaluator: &Evaluator,
    unit: UnitId,
    candidates: &[Goal],
    survey: &Survey,
) -> (crate::ai::features::GoalRecord, Vec<f32>) {
    let record = describe_surveyed(registry, state, evaluator, unit, candidates, survey);
    let scores = record
        .candidates
        .iter()
        .map(|row| net.score(&record.context, row))
        .collect();
    (record, scores)
}

/// Log-softmax of `scores / temperature`, stable.
pub fn log_softmax(scores: &[f32], temperature: f32) -> Vec<f32> {
    let t = temperature.max(1e-6);
    let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let sum: f32 = scores.iter().map(|s| ((s - max) / t).exp()).sum();
    let log_sum = sum.ln();
    scores.iter().map(|s| (s - max) / t - log_sum).collect()
}

/// Chooses a goal by the network's score.
pub struct LearnedChooser<'a> {
    pub net: &'a GoalNet,
    /// Her doctrine's evaluator, because the teacher's reading of each
    /// candidate is part of what the network is shown.
    pub evaluator: &'a Evaluator,
}

impl GoalChooser for LearnedChooser<'_> {
    fn choose(
        &mut self,
        registry: &DataRegistry,
        state: &BattleState,
        unit: UnitId,
        candidates: &[Goal],
    ) -> Goal {
        self.choose_surveyed(registry, state, unit, candidates, &Survey::new())
    }
}

impl LearnedChooser<'_> {
    /// [`GoalChooser::choose`], reading her roads from `survey`.
    pub fn choose_surveyed(
        &mut self,
        registry: &DataRegistry,
        state: &BattleState,
        unit: UnitId,
        candidates: &[Goal],
        survey: &Survey,
    ) -> Goal {
        let record = describe_surveyed(registry, state, self.evaluator, unit, candidates, survey);
        let mut best: Option<(f32, Goal)> = None;
        for (row, goal) in record.candidates.iter().zip(candidates) {
            let score = self.net.score(&record.context, row);
            // Strictly greater, so a tie goes to the earlier candidate — the
            // same list-order tie-break the hand-written chooser keeps, and
            // the reason `candidates` puts her orders first.
            if best.is_none_or(|(s, _)| score > s) {
                best = Some((score, *goal));
            }
        }
        best.map(|(_, goal)| goal).unwrap_or(Goal::Hold)
    }
}

/// Read a trained network and check it against the features this engine
/// describes, ready for [`DataRegistry::goal_net`].
pub fn load_goal_net(path: &Path) -> Result<Arc<GoalNet>, NetError> {
    let net = GoalNet::from_path(path)?;
    net.check(&context_names(), CANDIDATE)?;
    Ok(Arc::new(net))
}
