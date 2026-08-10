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
use crate::battle::{BattleState, FireIntent, Formation, FormationId, Mission, Order};
use crate::data::{DataRegistry, DoctrineDef};
use crate::map::{Objective, ObjectiveKind};
use crate::roster::GirlId;
use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque};

use super::UtilityPlanner;

/// The delegation level at or beyond which a commander stops assigning
/// ground and trusts her formations' own judgment — directive command in
/// the Auftragstaktik tradition, as opposed to the detailed orders a
/// centralized doctrine writes. Withdrawal is exempt: whether to keep
/// fighting is never devolved.
const DEVOLVED: f32 = 0.6;

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
    /// The side's doctrine, resolved once at build so the brain can consult
    /// it for formations that declare none of their own.
    side_doctrine: Option<DoctrineDef>,
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
            side_doctrine: None,
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
        self.side_doctrine = Some(side_doctrine.clone());
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

    /// The doctrine a formation fights under: its own, else the side's.
    fn doctrine_for(&self, index: usize) -> &DoctrineDef {
        self.executors
            .get(&index)
            .map(|e| &e.evaluator.doctrine)
            .or(self.side_doctrine.as_ref())
            .expect("built before any review")
    }

    /// What each formation should be doing, compared against what it is
    /// doing.
    ///
    /// Whether to keep fighting outranks where: a formation beaten past its
    /// doctrine's `withdraw_threshold` is ordered out by the nearest lane,
    /// and that order is never rescinded — this is where withdrawal stops
    /// being a symptom each vehicle shows and becomes a decision a commander
    /// makes, which is what the design doc means by willingness belonging to
    /// the commander.
    ///
    /// Ground worth holding is divided round-robin, most valuable first,
    /// among the formations in declaration order — the map author ordered
    /// both lists, so the pairing is theirs to control. Doctrine colours the
    /// mission: an aggressive one is sent to *take* the ground, a cautious
    /// one to stand on it, which is the difference between marching at a
    /// bridge and defending it. `initiative` is read here too: a commander
    /// with it moves her people off ground already taken toward ground that
    /// is not, while one without it follows the letter of the original plan.
    /// A map with no ground to hold gets no missions at all, which keeps a
    /// no-objective battle exactly the fight it was before commanders
    /// existed.
    fn mission_review(&self, registry: &DataRegistry, state: &BattleState, side: u8) -> Vec<Order> {
        let mut ground: Vec<(&Objective, Option<u8>)> = state
            .objectives()
            .filter(|(o, _)| o.kind == ObjectiveKind::Hold)
            .collect();
        // Stable, so equal values keep declaration order.
        ground.sort_by_key(|(o, _)| Reverse(o.value));

        let mut orders = Vec::new();
        let mut next = 0usize;
        for (index, formation) in state.command.formations().iter().enumerate() {
            if formation.side != side {
                continue;
            }
            // An ordered withdrawal stands whatever happens next. Strength
            // is measured over the roster the formation went in with, so it
            // cannot "recover" when its weakest vehicle dies — but the rule
            // is stated as well as computed, because a retreat that
            // un-happens is the kind of flicker that makes an AI look
            // broken.
            // What she has *said*, not what has arrived: an order still
            // travelling is one she has already given, and a commander who
            // forgot that would re-send it every round of the transit window
            // — burying the log and resetting the clock each time, so a
            // delayed mission would never land at all.
            let ordered = formation.latest_mission();
            if matches!(ordered, Some(Mission::Withdraw { .. })) {
                continue;
            }
            let doctrine = self.doctrine_for(index);
            if let Some(via) = self.wants_out(registry, state, formation, doctrine) {
                orders.push(Order::SetMission {
                    formation: FormationId(index as u32),
                    mission: Mission::Withdraw { via },
                });
                continue;
            }

            // A commander who devolves does not micro-assign ground at all:
            // her formations keep their native judgment — the whole-map
            // objective weighing that is exactly what a defensive doctrine
            // is good at — and hear from her only when it is time to leave.
            // This is not a tuning dodge, it is what the delegation knob
            // *means*, and it was measured before it was believed: pinning
            // elastic defence to anchor hexes cost it 16 of 24 wins against
            // a flat opponent, because choosing its own ground is its game.
            if doctrine.delegation >= DEVOLVED {
                continue;
            }
            if ground.is_empty() {
                continue;
            }
            let mut pick = next % ground.len();
            next += 1;
            if doctrine.initiative >= 0.5
                && ground[pick].1 == Some(side)
                && let Some(open) = ground.iter().position(|(_, held)| *held != Some(side))
            {
                pick = open;
            }
            let anchor = ground[pick].0.anchor();
            let desired = if doctrine.aggression >= 0.5 {
                Mission::Advance { to: anchor }
            } else {
                Mission::Hold { at: Some(anchor) }
            };
            // Standing orders stand. Re-issuing an identical mission every
            // round would bury the log in news that nothing changed.
            if ordered != Some(&desired) {
                orders.push(Order::SetMission {
                    formation: FormationId(index as u32),
                    mission: desired,
                });
            }
        }
        orders
    }

    /// Whether this formation is beaten badly enough that its commander
    /// orders it out, and by which lane. `None` while it fights on — which
    /// includes having nowhere to go: a side with no exit of its own holds,
    /// because driving for a lane that does not exist is not a retreat.
    fn wants_out(
        &self,
        registry: &DataRegistry,
        state: &BattleState,
        formation: &Formation,
        doctrine: &DoctrineDef,
    ) -> Option<String> {
        let (mut hp, mut max) = (0i64, 0i64);
        for id in &formation.members {
            // `state.units` rather than `state.unit()`, because the dead and
            // the exited still count against what the formation went in
            // with: strength is monotonic, so a retreat cannot un-happen
            // when the weakest vehicle stops dragging the average down.
            let Some(unit) = state.units.get(id.index()) else {
                continue;
            };
            if unit.alive {
                hp += i64::from(unit.hp.max(0));
            }
            max += i64::from(
                registry
                    .vehicle(&unit.vehicle)
                    .map(|v| v.max_hp)
                    .unwrap_or(0),
            );
        }
        let strength = hp as f32 / max.max(1) as f32;
        // `withdraw_threshold` is the fraction of strength *lost* before a
        // doctrine looks for the way out, so the stubborn 0.85 fights to a
        // remnant and the elastic 0.45 leaves with something to rebuild.
        if strength >= (1.0 - doctrine.withdraw_threshold).clamp(0.0, 1.0) {
            return None;
        }
        let from = formation
            .leader
            .and_then(|id| state.unit(id))
            .or_else(|| formation.members.iter().find_map(|id| state.unit(*id)))
            .map(|u| u.pos)?;
        let mut best: Option<(i32, &Objective)> = None;
        for objective in state.map.objectives() {
            if objective.kind != ObjectiveKind::Exit || !objective.open_to(formation.side) {
                continue;
            }
            let dist = objective
                .hexes
                .iter()
                .map(|h| h.distance_to(from))
                .min()
                .unwrap_or(i32::MAX);
            // Strict less-than: the first-declared lane wins a tie, so this
            // cannot flap between two equally distant exits.
            if best.is_none_or(|(b, _)| dist < b) {
                best = Some((dist, objective));
            }
        }
        best.map(|(_, o)| o.id.clone())
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
            self.pending
                .extend(self.mission_review(registry, state, side));
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
