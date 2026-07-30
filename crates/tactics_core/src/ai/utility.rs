//! Greedy utility planner: score every tile one unit could hold, take the
//! best, and engage whatever that tile can reach. Cheap, tunable, and honest
//! about fog.
//!
//! A unit needs two orders — where to drive and what to shoot — so the
//! planner buffers the pair and hands them over one call at a time, keeping
//! [`AiPlanner::next_order`] the only entry point the callers need.

use super::{difficulty_noise, next_unplanned_unit, resolve_doctrine, AiConfig, AiPlanner, Evaluator};
use crate::battle::{reachable, BattleState, FireIntent, Order, UnitId};
use crate::data::DataRegistry;
use hexx::Hex;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::VecDeque;

/// The best tile found for one unit, and the shot that came with it.
struct Choice {
    score: f32,
    dest: Hex,
    attack: Option<(UnitId, usize)>,
}

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
    fn plan_unit(
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

        let mut best: Option<Choice> = None;
        for tile in options {
            let scored = self.evaluator.score_tile(registry, state, unit, tile);
            let score = self.noisy_score(scored.score);
            if best.as_ref().is_none_or(|b| score > b.score) {
                best = Some(Choice {
                    score,
                    dest: tile,
                    attack: scored.attack,
                });
            }
        }

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
