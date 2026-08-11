//! Monte Carlo tree search planner.
//!
//! Runs UCT over cloned battle states -- this is why the sim core is plain
//! data. Fog honesty is enforced by *determinization*: the search runs on a
//! copy of the battle where every enemy this side has not spotted simply does
//! not exist, so the planner cannot exploit hidden information.
//!
//! Simultaneous turns make a round a matrix game: both sides move at once, so
//! there is no "opponent to move" for the tree to alternate on. Rather than
//! search that properly, the enemy's intents are filled in by an ordinary
//! [`UtilityPlanner`] before each round resolves, and the tree searches only
//! this side's choices against that fixed policy. It is a deliberate
//! approximation: the search may be optimistic against an opponent who plays
//! very differently from the policy, which is the price of keeping the branch
//! factor to one side's decisions.

use super::{
    AiConfig, AiPlanner, Evaluator, PlannerRegistry, UtilityPlanner, difficulty_noise,
    next_unplanned_unit, resolve_doctrine,
};
use crate::battle::{BattleState, FireIntent, Order, Phase, UnitId, UnitIntent, reachable};
use crate::data::DataRegistry;
use hexx::Hex;
use rand::seq::IndexedRandom;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::VecDeque;

pub struct MctsPlanner {
    pub iterations: u32,
    /// How many further decisions to play out past a leaf.
    pub rollout_depth: u32,
    pub exploration: f32,
    /// Cap on children per node to keep branching sane.
    pub max_actions: usize,
    evaluator: Evaluator,
    /// Stands in for the enemy while searching. Doctrine-neutral: we do not
    /// know how the other side thinks, so guessing would be worse than not.
    policy: UtilityPlanner,
    /// Used when there is nothing to search: no contact means no tree worth
    /// building, and the greedy planner already knows how to go looking.
    fallback: UtilityPlanner,
    rng: ChaCha8Rng,
    /// Orders decided for a unit but not yet handed out.
    pending: VecDeque<Order>,
}

impl MctsPlanner {
    pub fn from_config(
        config: &AiConfig,
        seed: u64,
        data: &DataRegistry,
        _planners: &PlannerRegistry,
    ) -> Self {
        let difficulty = config.difficulty.clamp(1, 5);
        let doctrine = resolve_doctrine(config, data);
        Self {
            iterations: match difficulty {
                1 => 60,
                2 => 150,
                3 => 400,
                4 => 900,
                _ => 2000,
            },
            rollout_depth: 20,
            exploration: 1.2,
            max_actions: 16,
            evaluator: Evaluator::new(doctrine.clone()),
            policy: UtilityPlanner::with_difficulty(3, seed ^ 0x9E37_79B9),
            fallback: UtilityPlanner::new(
                Evaluator::new(doctrine),
                difficulty_noise(difficulty),
                seed ^ 0x85EB_CA6B,
            ),
            rng: ChaCha8Rng::seed_from_u64(seed),
            pending: VecDeque::new(),
        }
    }

    pub fn with_difficulty(difficulty: u8, seed: u64) -> Self {
        Self::from_config(
            &AiConfig {
                planner: "mcts".into(),
                difficulty,
                doctrine: None,
            },
            seed,
            &DataRegistry::default(),
            &PlannerRegistry::empty(),
        )
    }

    /// Candidate decisions for the next unit that needs orders, or the commit
    /// that ends this side's planning.
    fn candidate_steps(
        &mut self,
        registry: &DataRegistry,
        state: &BattleState,
        side: u8,
    ) -> Vec<Step> {
        let Some(unit) = next_unplanned_unit(state, side) else {
            return vec![Step::Commit];
        };
        let Some(pos) = state.unit(unit).map(|u| u.pos) else {
            return vec![Step::Commit];
        };

        let mut tiles: Vec<Hex> = reachable(registry, state, unit)
            .into_keys()
            .filter(|h| *h != pos)
            .collect();
        tiles.sort_unstable_by_key(|h| (h.x, h.y));
        // Sample destinations when there are too many to expand.
        let room = self.max_actions.saturating_sub(1).max(2);
        while tiles.len() > room {
            let i = self.rng.random_range(0..tiles.len());
            tiles.swap_remove(i);
        }
        // Holding position is always an option worth considering.
        tiles.push(pos);

        tiles
            .into_iter()
            .map(|tile| {
                let attack = self
                    .evaluator
                    .score_tile(registry, state, unit, tile)
                    .attack;
                Step::Plan {
                    unit,
                    dest: (tile != pos).then_some(tile),
                    fire: match attack {
                        Some((target, weapon)) => FireIntent::Target { target, weapon },
                        None => FireIntent::Hold,
                    },
                }
            })
            .collect()
    }

    /// Play one decision out on a cloned battle. Committing also fills in the
    /// enemy's intents and resolves the round, which is where simultaneity is
    /// approximated.
    fn advance(&mut self, registry: &DataRegistry, sim: &mut BattleState, side: u8, step: &Step) {
        for order in step.orders(side) {
            let _ = sim.apply(registry, &order);
        }
        if matches!(step, Step::Commit) {
            self.fill_other_sides(registry, sim, side);
            sim.resolve_round(registry);
        }
    }

    fn fill_other_sides(&mut self, registry: &DataRegistry, sim: &mut BattleState, side: u8) {
        let others: Vec<u8> = sim
            .living_sides()
            .into_iter()
            .filter(|s| *s != side)
            .collect();
        // Bounded so a planner that somehow never commits cannot hang the
        // search.
        let budget = sim.units.len() * 2 + 4;
        for other in others {
            for _ in 0..budget {
                if !sim.is_planning() || sim.has_committed(other) {
                    break;
                }
                let order = self.policy.next_order(registry, sim, other);
                let commits = matches!(order, Order::Commit { .. });
                let _ = sim.apply(registry, &order);
                if commits {
                    break;
                }
            }
        }
    }

    fn search(&mut self, registry: &DataRegistry, state: &BattleState, side: u8) -> Option<Step> {
        let root_seed: u64 = self.rng.random();
        let known = determinize(state, side, root_seed);

        let root_actions = self.candidate_steps(registry, &known, side);
        if root_actions.len() <= 1 {
            return root_actions.into_iter().next();
        }

        let mut nodes = vec![Node {
            parent: None,
            action: None,
            children: Vec::new(),
            untried: root_actions,
            visits: 0.0,
            value: 0.0,
        }];

        for _ in 0..self.iterations {
            let mut sim = known.clone();
            let mut current = 0usize;

            // Selection: descend fully-expanded nodes by UCB1.
            while nodes[current].untried.is_empty() && !nodes[current].children.is_empty() {
                let parent_visits = nodes[current].visits.max(1.0);
                let c = self.exploration;
                let best = *nodes[current]
                    .children
                    .iter()
                    .max_by(|&&a, &&b| {
                        let ucb = |i: usize| {
                            let n = &nodes[i];
                            if n.visits == 0.0 {
                                f32::INFINITY
                            } else {
                                n.value / n.visits + c * (parent_visits.ln() / n.visits).sqrt()
                            }
                        };
                        ucb(a).total_cmp(&ucb(b))
                    })
                    .expect("children not empty");
                if let Some(action) = nodes[best].action.clone() {
                    self.advance(registry, &mut sim, side, &action);
                }
                current = best;
            }

            // Expansion.
            if !nodes[current].untried.is_empty() && sim.over.is_none() {
                let i = self.rng.random_range(0..nodes[current].untried.len());
                let action = nodes[current].untried.swap_remove(i);
                self.advance(registry, &mut sim, side, &action);
                let untried = if sim.over.is_none() {
                    self.candidate_steps(registry, &sim, side)
                } else {
                    Vec::new()
                };
                let child = nodes.len();
                nodes.push(Node {
                    parent: Some(current),
                    action: Some(action),
                    children: Vec::new(),
                    untried,
                    visits: 0.0,
                    value: 0.0,
                });
                nodes[current].children.push(child);
                current = child;
            }

            // Rollout: random-ish play to a shallow horizon.
            for _ in 0..self.rollout_depth {
                if sim.over.is_some() {
                    break;
                }
                let options = self.candidate_steps(registry, &sim, side);
                let Some(action) = options.choose(&mut self.rng).cloned() else {
                    break;
                };
                self.advance(registry, &mut sim, side, &action);
            }

            // Backpropagation. Every node is scored from the searching side's
            // view, because the opponent is a fixed policy rather than a
            // player in the tree.
            let score = self.evaluator.position_value(&sim, side);
            let mut at = Some(current);
            while let Some(i) = at {
                nodes[i].visits += 1.0;
                nodes[i].value += score;
                at = nodes[i].parent;
            }
        }

        nodes[0]
            .children
            .iter()
            .max_by(|&&a, &&b| nodes[a].visits.total_cmp(&nodes[b].visits))
            .and_then(|&i| nodes[i].action.clone())
    }
}

/// One decision in the tree: everything a single unit will do this round, or
/// the end of this side's planning.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    Plan {
        unit: UnitId,
        dest: Option<Hex>,
        fire: FireIntent,
    },
    Commit,
}

impl Step {
    fn orders(&self, side: u8) -> Vec<Order> {
        match self {
            Step::Commit => vec![Order::Commit { side }],
            Step::Plan { unit, dest, fire } => {
                let mut orders = Vec::new();
                if let Some(to) = dest {
                    orders.push(Order::SetMove {
                        unit: *unit,
                        to: *to,
                    });
                }
                orders.push(Order::SetFire {
                    unit: *unit,
                    fire: *fire,
                });
                orders
            }
        }
    }
}

struct Node {
    parent: Option<usize>,
    action: Option<Step>,
    children: Vec<usize>,
    untried: Vec<Step>,
    visits: f32,
    value: f32,
}

/// Remove every enemy unit `side` has not spotted, and forget what the other
/// sides have been told to do: the search may only plan against what it knows
/// about. Since planning is simultaneous, nobody's orders are knowable while
/// they are being written, so the policy opponent has to guess them again.
pub fn determinize(state: &BattleState, side: u8, seed: u64) -> BattleState {
    let mut known = state.clone();
    let spotted = known.fog.side(side).spotted.clone();
    for unit in &mut known.units {
        if unit.side == side {
            continue;
        }
        if unit.alive && !spotted.contains(&unit.id) {
            // Off the board, and *not destroyed* — which is the whole point of
            // `exited` and the only honest thing the search can say about a
            // vehicle it has never seen. Clearing `alive` alone made her read
            // as a wreck, and one reader takes that literally:
            // `check_victory`'s decapitation pass classifies a loss by
            // `!alive && !exited`, so on a map that declared
            // `loss_conditions` the search opened on a world where the enemy's
            // commanding officer was already dead and this side had already
            // won. Every branch then scores the same and the tree is worth
            // nothing. Nobody else inside a rollout reads `exited`.
            unit.alive = false;
            unit.exited = true;
        }
        unit.intent = UnitIntent::default();
        unit.planned = false;
    }
    if let Phase::Planning { committed } = &mut known.phase {
        for (other, done) in committed.iter_mut().enumerate() {
            if other as u8 != side {
                *done = false;
            }
        }
    }
    // Fresh RNG: the planner must not be able to predict the real battle's
    // future dice.
    known.rng = ChaCha8Rng::seed_from_u64(seed);
    known
}

impl AiPlanner<BattleState, Order> for MctsPlanner {
    fn next_order(&mut self, registry: &DataRegistry, state: &BattleState, side: u8) -> Order {
        while let Some(order) = self.pending.pop_front() {
            let unit = match &order {
                Order::SetMove { unit, .. } | Order::SetFire { unit, .. } => Some(*unit),
                _ => None,
            };
            if unit.is_none_or(|u| state.unit(u).is_some()) {
                return order;
            }
        }
        if next_unplanned_unit(state, side).is_none() {
            return Order::Commit { side };
        }
        // With nobody in sight, determinization leaves an empty battlefield
        // and every branch looks like a win. Scouting is the greedy planner's
        // job anyway.
        if super::visible_enemies(state, side).is_empty() {
            return self.fallback.next_order(registry, state, side);
        }

        match self.search(registry, state, side) {
            Some(step) => {
                self.pending = step.orders(side).into();
                self.pending.pop_front().unwrap_or(Order::Commit { side })
            }
            None => Order::Commit { side },
        }
    }
}
