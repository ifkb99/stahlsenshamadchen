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
use super::features::{CANDIDATE, context_names, describe};
use super::goal::GoalChooser;
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
#[derive(Debug, Clone, Default)]
pub enum GoalBrain {
    #[default]
    Utility,
    Learned(Arc<GoalNet>),
    Random,
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
        let record = describe(registry, state, self.evaluator, unit, candidates);
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
