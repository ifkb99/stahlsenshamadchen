//! Greedy utility planner: score every tile one unit could hold, take the
//! best, and engage whatever that tile can reach. Cheap, tunable, and honest
//! about fog.
//!
//! A unit needs two orders — where to drive and what to shoot — so the
//! planner buffers the pair and hands them over one call at a time, keeping
//! [`AiPlanner::next_order`] the only entry point the callers need.

use super::{
    AiConfig, AiPlanner, Evaluator, difficulty_noise, next_unplanned_unit, resolve_doctrine,
};
use crate::battle::{BattleState, FireIntent, Order, UnitId, reachable};
use crate::data::DataRegistry;
use hexx::Hex;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::VecDeque;

/// The best tile found for one unit, and the shot that came with it.
struct Choice {
    dest: Hex,
    attack: Option<(UnitId, usize)>,
}

/// One scored candidate: the tile, its (possibly noisy) score, and the
/// attack the evaluator found from it.
type Candidate = (Hex, f32, Option<(UnitId, usize)>);

pub struct UtilityPlanner {
    /// What this side values. Doctrine, not difficulty.
    pub evaluator: Evaluator,
    /// Uniform noise amplitude added to every candidate score. Difficulty,
    /// not doctrine.
    pub noise: f32,
    rng: ChaCha8Rng,
    /// Orders decided for a unit but not yet handed out.
    pending: VecDeque<Order>,
}

impl UtilityPlanner {
    pub fn new(evaluator: Evaluator, noise: f32, seed: u64) -> Self {
        Self {
            evaluator,
            noise,
            rng: ChaCha8Rng::seed_from_u64(seed),
            pending: VecDeque::new(),
        }
    }

    pub fn from_config(config: &AiConfig, seed: u64, data: &DataRegistry) -> Self {
        Self::new(
            Evaluator::new(resolve_doctrine(config, data)),
            difficulty_noise(config.difficulty),
            seed,
        )
    }

    /// A planner with the balanced default doctrine, for callers that only
    /// care about skill: MCTS uses this for its policy opponent.
    pub fn with_difficulty(difficulty: u8, seed: u64) -> Self {
        Self::new(
            Evaluator::new(Default::default()),
            difficulty_noise(difficulty),
            seed,
        )
    }

    fn noisy_score(&mut self, score: f32) -> f32 {
        if self.noise > 0.0 {
            score + self.rng.random_range(-self.noise..self.noise)
        } else {
            score
        }
    }

    /// Decide everything one unit will do this round.
    ///
    /// Crate-visible because [`super::SideCommand`] routes each unit to a
    /// per-formation instance of this planner: the executor half of chain of
    /// command *is* this planner, aimed by whatever mission the evaluator
    /// finds on the unit's formation.
    pub(crate) fn plan_unit(
        &mut self,
        registry: &DataRegistry,
        state: &BattleState,
        unit: UnitId,
    ) -> Vec<Order> {
        let Some(pos) = state.unit(unit).map(|u| u.pos) else {
            return Vec::new();
        };
        let mut options: Vec<Hex> = reachable(registry, state, unit).into_keys().collect();
        options.sort_unstable_by_key(|h| (h.x, h.y));

        let mut scored: Vec<Candidate> = Vec::with_capacity(options.len());
        for tile in options {
            let tile_score = self.evaluator.score_tile(registry, state, unit, tile);
            scored.push((tile, self.noisy_score(tile_score.score), tile_score.attack));
        }
        let top = scored
            .iter()
            .map(|(_, s, _)| *s)
            .fold(f32::NEG_INFINITY, f32::max);

        // Among tiles the evaluator cannot meaningfully tell apart, take the
        // one closest to where she already stands. This is the fix for the
        // skillgap instrument's inversion finding, and it is deterministic
        // rather than another kind of noise. Open ground scores in broad
        // plateaus, and the old rule — strictly-better-or-keep-the-first,
        // over a fixed (x, y) sweep — sent every unit on a side to the SAME
        // corner of every plateau: identical crews made identical choices
        // and arrived as a queue, which is why a noiseless side clumped,
        // burned its movement crossing its own plateau, and lost to any
        // opponent scattered by randomness. Preferring the nearest
        // equivalent tile keeps a dispersed side dispersed (units standing
        // apart stay apart when the ground between is all the same),
        // conserves movement for ground that is actually better, and is
        // what a crew would do: nobody drives across a field to park on
        // identical grass.
        const PLATEAU: f32 = 0.3;
        let best = scored
            .into_iter()
            .filter(|(_, s, _)| *s >= top - PLATEAU)
            .min_by_key(|(tile, _, _)| (pos.distance_to(*tile), tile.x, tile.y))
            .map(|(tile, _, attack)| Choice { dest: tile, attack });

        let (dest, attack) = match best {
            Some(choice) => (choice.dest, choice.attack),
            None => (pos, None),
        };
        let mut orders = Vec::new();
        if dest != pos {
            orders.push(Order::SetMove { unit, to: dest });
        }
        match attack {
            Some((target, weapon)) => orders.push(Order::SetFire {
                unit,
                fire: FireIntent::Target { target, weapon },
            }),
            // Nothing worth engaging: watch the ground instead. Said out
            // loud so the unit counts as planned rather than forgotten.
            None if orders.is_empty() => orders.push(Order::SetFire {
                unit,
                fire: FireIntent::Hold,
            }),
            None => {}
        }
        orders
    }
}

impl AiPlanner<BattleState, Order> for UtilityPlanner {
    fn next_order(&mut self, registry: &DataRegistry, state: &BattleState, side: u8) -> Order {
        while let Some(order) = self.pending.pop_front() {
            // A unit destroyed since the order was queued has nothing to say.
            let unit = match &order {
                Order::SetMove { unit, .. } | Order::SetFire { unit, .. } => Some(*unit),
                _ => None,
            };
            if unit.is_none_or(|u| state.unit(u).is_some()) {
                return order;
            }
        }
        let Some(unit) = next_unplanned_unit(state, side) else {
            return Order::Commit { side };
        };
        self.pending = self.plan_unit(registry, state, unit).into();
        match self.pending.pop_front() {
            Some(order) => order,
            // Nothing to say about a unit that cannot be planned; hold fire
            // so it is marked planned and the round can proceed.
            None => Order::SetFire {
                unit,
                fire: FireIntent::Hold,
            },
        }
    }
}
