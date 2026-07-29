//! Monte Carlo tree search planner.
//!
//! Runs UCT over cloned battle states -- this is why the sim core is plain
//! data. Fog honesty is enforced by *determinization*: the search runs on a
//! copy of the battle where every enemy this side has not spotted simply
//! does not exist, so the planner cannot exploit hidden information.

use super::{best_weapon_against, next_idle_unit, visible_enemies, AiPlanner};
use crate::battle::{reachable, BattleState, Order};
use crate::data::DataRegistry;
use rand::seq::IndexedRandom;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

pub struct MctsPlanner {
    pub iterations: u32,
    pub rollout_depth: u32,
    pub exploration: f32,
    /// Cap on children per node to keep branching sane.
    pub max_actions: usize,
    rng: ChaCha8Rng,
}

impl MctsPlanner {
    pub fn with_difficulty(difficulty: u8, seed: u64) -> Self {
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
            rng: ChaCha8Rng::seed_from_u64(seed),
        }
    }
}

struct Node {
    parent: Option<usize>,
    action: Option<Order>,
    /// Side that benefits from this node's action (the side to move at the
    /// parent).
    acting_side: u8,
    children: Vec<usize>,
    untried: Vec<Order>,
    visits: f32,
    /// Total value accumulated from `acting_side`'s perspective.
    value: f32,
}

/// Remove every enemy unit `side` has not spotted: the search may only plan
/// against what it knows about.
fn determinize(state: &BattleState, side: u8, seed: u64) -> BattleState {
    let mut known = state.clone();
    let spotted = known.fog.side(side).spotted.clone();
    for unit in &mut known.units {
        if unit.alive && unit.side != side && !spotted.contains(&unit.id) {
            unit.alive = false;
        }
    }
    // Fresh RNG: the planner must not be able to predict the real battle's
    // future dice.
    known.rng = ChaCha8Rng::seed_from_u64(seed);
    known
}

/// Candidate orders for the side to move, capped and roughly ordered.
fn candidate_orders(
    registry: &DataRegistry,
    state: &BattleState,
    side: u8,
    cap: usize,
    rng: &mut ChaCha8Rng,
) -> Vec<Order> {
    let Some(unit_id) = next_idle_unit(state, side) else {
        return vec![Order::EndTurn];
    };
    let unit = state.unit(unit_id).expect("idle unit is alive");
    let mut orders = Vec::new();

    // Attacks from where we stand.
    for enemy in visible_enemies(state, side) {
        if let Some((weapon, _, _)) = best_weapon_against(registry, state, unit_id, unit.pos, enemy)
        {
            orders.push(Order::Attack {
                unit: unit_id,
                target: enemy.id,
                weapon,
            });
        }
    }

    if !unit.moved {
        let mut tiles: Vec<_> = reachable(registry, state, unit_id)
            .into_keys()
            .filter(|h| *h != unit.pos)
            .collect();
        tiles.sort_unstable_by_key(|h| (h.x, h.y));
        // Sample destinations if there are too many.
        while tiles.len() > cap.saturating_sub(orders.len() + 1).max(3) {
            let i = rng.random_range(0..tiles.len());
            tiles.swap_remove(i);
        }
        for tile in tiles {
            orders.push(Order::Move {
                unit: unit_id,
                to: tile,
            });
        }
    }

    orders.push(Order::Wait { unit: unit_id });
    orders.truncate(cap.max(1));
    orders
}

/// Terminal/heuristic evaluation in [-1, 1] from `side`'s perspective.
fn evaluate(state: &BattleState, side: u8) -> f32 {
    if let Some(result) = state.over {
        return match result.winner {
            Some(w) if w == side => 1.0,
            Some(_) => -1.0,
            None => 0.0,
        };
    }
    let mut ours = 0.0;
    let mut theirs = 0.0;
    for unit in state.alive_units() {
        if unit.side == side {
            ours += unit.hp as f32;
        } else {
            theirs += unit.hp as f32;
        }
    }
    let total = ours + theirs;
    if total <= 0.0 {
        0.0
    } else {
        (ours - theirs) / total
    }
}

impl AiPlanner<BattleState, Order> for MctsPlanner {
    fn next_order(&mut self, registry: &DataRegistry, state: &BattleState, side: u8) -> Order {
        let root_seed: u64 = self.rng.random();
        let known = determinize(state, side, root_seed);

        let root_actions = candidate_orders(registry, &known, side, self.max_actions, &mut self.rng);
        if root_actions.len() == 1 {
            return root_actions.into_iter().next().unwrap();
        }

        let mut nodes = vec![Node {
            parent: None,
            action: None,
            acting_side: side,
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
                    let _ = sim.apply(registry, &action);
                }
                current = best;
            }

            // Expansion.
            if !nodes[current].untried.is_empty() && sim.over.is_none() {
                let i = self.rng.random_range(0..nodes[current].untried.len());
                let action = nodes[current].untried.swap_remove(i);
                let acting_side = sim.active_side;
                let _ = sim.apply(registry, &action);
                let untried = if sim.over.is_none() {
                    candidate_orders(registry, &sim, sim.active_side, self.max_actions, &mut self.rng)
                } else {
                    Vec::new()
                };
                let child = nodes.len();
                nodes.push(Node {
                    parent: Some(current),
                    action: Some(action),
                    acting_side,
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
                let mover = sim.active_side;
                let options = candidate_orders(registry, &sim, mover, 8, &mut self.rng);
                let Some(action) = options.choose(&mut self.rng).cloned() else {
                    break;
                };
                if sim.apply(registry, &action).is_err() {
                    let _ = sim.apply(registry, &Order::EndTurn);
                }
            }

            // Backpropagation: each node scores from its actor's view.
            let mut at = Some(current);
            while let Some(i) = at {
                let score = evaluate(&sim, nodes[i].acting_side);
                nodes[i].visits += 1.0;
                nodes[i].value += score;
                at = nodes[i].parent;
            }
        }

        // Most-visited root child is the answer.
        nodes[0]
            .children
            .iter()
            .max_by(|&&a, &&b| nodes[a].visits.total_cmp(&nodes[b].visits))
            .and_then(|&i| nodes[i].action.clone())
            .unwrap_or(Order::EndTurn)
    }
}
