//! A side that fights through its chain of command.
//!
//! [`SideCommand`] is what stands behind a side when its map names the
//! `command` planner: a commander brain that issues missions to formations,
//! and a utility-planner executor per formation that plans each member unit
//! in service of its standing mission. From the outside it is an ordinary
//! [`AiPlanner`] — the driver asks it for one order at a time until it
//! commits — which is the point: the hierarchy is an implementation detail
//! of the side, not a new contract for every caller.
//!
//! The brain deliberately speaks only through [`Order::SetMission`]. It could
//! reach into the executors and tell them things privately, and it must
//! never: a mission that exists only inside a planner is one the log cannot
//! report, the save cannot carry, and a replaced brain (a human, an LLM)
//! cannot see. Everything the commander decides goes through the order
//! stream like everybody else's decisions.
//!
//! The first brain is modest on purpose: it divides the map's ground worth
//! holding among its formations, biggest objective to the first-declared
//! formation, and holds each mission until it has a different one. Whether
//! that is *enough* brain is what the balance harness's delegation-tax table
//! exists to say; making it clever belongs to the willingness work, not to
//! the plumbing.

use super::{AiConfig, AiPlanner, Evaluator, difficulty_noise, next_unplanned_unit};
use crate::battle::{BattleState, FireIntent, FormationId, Mission, Order};
use crate::data::{DataRegistry, DoctrineDef};
use crate::map::ObjectiveKind;
use crate::roster::GirlId;
use std::collections::{HashMap, VecDeque};

use super::UtilityPlanner;

pub struct SideCommand {
    config: AiConfig,
    seed: u64,
    /// The girl in command of the side: the leader of its first-declared
    /// formation. Nothing reads her yet — this is the seam the design doc
    /// promises ("the brain is constructed for the side's commanding girl
    /// from the start"), filled in when the state is first seen so her
    /// traits and command skill can steer the brain without a rework.
    #[allow(dead_code)]
    commander: Option<GirlId>,
    /// One executor per formation this side owns, keyed by the formation's
    /// index in [`crate::battle::CommandState`]. A map, but never iterated —
    /// units are routed through it by direct lookup, so its order can leak
    /// into nothing.
    executors: HashMap<usize, UtilityPlanner>,
    /// Plans units in no formation, under the side's own doctrine — which is
    /// exactly what the whole side was before formations existed.
    fallback: UtilityPlanner,
    /// Built lazily on the first order, because planners are constructed
    /// before the battle exists and the formations live on it.
    built: bool,
    /// The round missions were last reviewed, so the brain speaks once per
    /// round rather than once per order.
    reviewed_round: Option<u32>,
    pending: VecDeque<Order>,
}

impl SideCommand {
    pub fn from_config(config: &AiConfig, seed: u64, data: &DataRegistry) -> Self {
        Self {
            config: config.clone(),
            seed,
            commander: None,
            executors: HashMap::new(),
            fallback: UtilityPlanner::from_config(config, seed ^ 0xC0FF_EE00, data),
            built: false,
            reviewed_round: None,
            pending: VecDeque::new(),
        }
    }

    /// Resolve the formations of `side` into executors, once. A formation
    /// declaring its own doctrine gets an evaluator that believes it; the
    /// rest inherit the side's.
    fn ensure_built(&mut self, registry: &DataRegistry, state: &BattleState, side: u8) {
        if self.built {
            return;
        }
        self.built = true;
        let side_doctrine = super::resolve_doctrine(&self.config, registry);
        for (index, formation) in state.command.formations().iter().enumerate() {
            if formation.side != side {
                continue;
            }
            let doctrine: DoctrineDef = formation
                .doctrine
                .as_deref()
                .and_then(|id| registry.doctrine(id).cloned())
                .unwrap_or_else(|| side_doctrine.clone());
            self.executors.insert(
                index,
                UtilityPlanner::new(
                    Evaluator::new(doctrine),
                    difficulty_noise(self.config.difficulty),
                    // Distinct stream per formation, or two platoons under
                    // the same noise level would blunder identically.
                    self.seed ^ ((index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
                ),
            );
        }
        self.commander = state
            .command
            .formations()
            .iter()
            .find(|f| f.side == side)
            .and_then(|f| f.leader)
            .and_then(|id| state.unit(id))
            .and_then(|u| u.crew.first().copied());
    }

    /// What each formation should be doing, compared against what it is
    /// doing. Ground worth holding is divided round-robin, most valuable
    /// first, among the formations in declaration order — the map author
    /// ordered both lists, so the pairing is theirs to control. A map with
    /// no ground to hold gets no missions at all, which keeps a
    /// no-objective battle exactly the fight it was before commanders
    /// existed.
    fn mission_review(&self, state: &BattleState, side: u8) -> Vec<Order> {
        let mut ground: Vec<&crate::map::Objective> = state
            .map
            .objectives()
            .iter()
            .filter(|o| o.kind == ObjectiveKind::Hold)
            .collect();
        if ground.is_empty() {
            return Vec::new();
        }
        // Stable, so equal values keep declaration order.
        ground.sort_by_key(|o| std::cmp::Reverse(o.value));

        let mut orders = Vec::new();
        let mut next = 0usize;
        for (index, formation) in state.command.formations().iter().enumerate() {
            if formation.side != side {
                continue;
            }
            let target = ground[next % ground.len()];
            next += 1;
            let desired = Mission::Advance {
                to: target.anchor(),
            };
            // Standing orders stand. Re-issuing an identical mission every
            // round would bury the log in news that nothing changed.
            if formation.mission.as_ref() != Some(&desired) {
                orders.push(Order::SetMission {
                    formation: FormationId(index as u32),
                    mission: desired,
                });
            }
        }
        orders
    }
}

impl AiPlanner<BattleState, Order> for SideCommand {
    fn next_order(&mut self, registry: &DataRegistry, state: &BattleState, side: u8) -> Order {
        self.ensure_built(registry, state, side);

        // The commander speaks first, once per round: missions before unit
        // orders, so the executors already know what the ground is worth by
        // the time they plan the first vehicle.
        if self.reviewed_round != Some(state.round) {
            self.reviewed_round = Some(state.round);
            self.pending.extend(self.mission_review(state, side));
        }

        while let Some(order) = self.pending.pop_front() {
            // A unit destroyed since its orders were queued has nothing to
            // say; a mission cannot go stale the same way.
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
        let executor = state
            .command
            .formations()
            .iter()
            .position(|f| f.contains(unit))
            .and_then(|index| self.executors.get_mut(&index))
            .unwrap_or(&mut self.fallback);
        self.pending = executor.plan_unit(registry, state, unit).into();
        match self.pending.pop_front() {
            Some(order) => order,
            // Nothing to say about this unit: hold fire so it counts as
            // planned and the round can proceed.
            None => Order::SetFire {
                unit,
                fire: FireIntent::Hold,
            },
        }
    }
}
