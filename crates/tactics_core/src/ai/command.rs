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

use super::{
    AiConfig, AiPlanner, Evaluator, best_weapon_against, difficulty_noise, next_unplanned_unit,
    visible_enemies,
};
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

/// The posture an unordered crew under fire falls back on. Mod data first —
/// the base game ships a `drill` doctrine and a mod may retune what drilled
/// self-preservation looks like — with a built-in equivalent if a mod
/// removes it, because a missing id must degrade to sensible behaviour
/// rather than to standing in the open.
fn drill_doctrine(data: &DataRegistry) -> DoctrineDef {
    data.doctrine("drill")
        .cloned()
        .unwrap_or_else(|| DoctrineDef {
            id: "drill".into(),
            name: "Battle Drill".into(),
            description: String::new(),
            aggression: 0.0,
            cover_value: 2.0,
            elevation_value: 1.2,
            concentration: 0.8,
            scouting: 0.0,
            objective_value: 0.0,
            indirect_appetite: 1.0,
            withdraw_threshold: 0.5,
            initiative: 0.5,
            delegation: 0.5,
        })
}

/// Whether anything the side can see could put fire on this unit where she
/// stands. The drill's trigger: fog-honest (spotted enemies only, through
/// the same [`best_weapon_against`] every planner prices shots with) and
/// deterministic, because "was she in danger" must answer the same on every
/// machine.
fn threatened(registry: &DataRegistry, state: &BattleState, unit: crate::battle::UnitId) -> bool {
    let Some(me) = state.unit(unit) else {
        return false;
    };
    visible_enemies(state, me.side)
        .iter()
        .any(|enemy| best_weapon_against(registry, state, enemy.id, enemy.pos, me).is_some())
}

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
    /// Plans the battle drill: the self-preservation move of an unordered
    /// unit under fire, when the commander is a human who has said nothing.
    /// Runs the `drill` posture from mod data — return fire, seek cover,
    /// want nothing else on the map — so "she moved without orders" is
    /// always survival and never campaigning.
    drill: UtilityPlanner,
    /// The side's doctrine, resolved once at build so the brain can consult
    /// it for formations that declare none of their own.
    side_doctrine: Option<DoctrineDef>,
    /// Built lazily on the first order, because planners are constructed
    /// before the battle exists and the formations live on it.
    built: bool,
    /// Whether this side has a brain of its own. False for the human hybrid
    /// (see [`Self::executor_only`]), where the commander is a person and
    /// this object is only her staff.
    reviews_missions: bool,
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
            drill: UtilityPlanner::new(
                Evaluator::new(drill_doctrine(data)),
                difficulty_noise(config.difficulty),
                seed ^ 0xD811_0000,
            ),
            side_doctrine: None,
            built: false,
            reviews_missions: true,
            reviewed_round: None,
            pending: VecDeque::new(),
        }
    }

    /// The same side, without a brain: per-formation executors and nothing
    /// else. This is the human hybrid the design doc describes — the player
    /// *is* the commander, so nothing here may issue a mission, and no
    /// [`Event::MissionAssigned`](crate::battle::Event::MissionAssigned) can
    /// come out of it.
    ///
    /// What it does instead is fill the gaps she left: a unit she did not
    /// order herself is planned by her formation's executor, under that
    /// formation's doctrine, in service of the mission she gave it. Units she
    /// *did* order are never touched, because the driver only ever asks about
    /// [`next_unplanned_unit`] and a hand-ordered unit is already planned.
    ///
    /// One rule is different from an AI side's, and deliberately so: **a unit
    /// in no formation, or in a formation under no mission, is not planned at
    /// all** — she is given a bare hold-fire, today's "planned, watching"
    /// default. An AI side would hand her to the fallback planner, which
    /// would send her off to fight on her own judgment; doing that for the
    /// human would be inventing a purpose she never gave. Delegation fills in
    /// the *how* of an order, never the *whether*. A mission still in the air
    /// counts as given ([`Formation::latest_mission`]): she has ordered it,
    /// and her subordinates plan on what they currently know, which for one
    /// transit window is still the last thing they heard.
    pub fn executor_only(config: &AiConfig, seed: u64, data: &DataRegistry) -> Self {
        Self {
            reviews_missions: false,
            ..Self::from_config(config, seed, data)
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

    /// Whether this member stands overwatch this round instead of moving:
    /// the executor half of bounding overwatch, with WEGO rounds as the
    /// bounds. A formation under a movement mission and in contact splits
    /// into two elements by member parity; the elements alternate by round,
    /// the bounding one advancing on the mission gradient while this one
    /// goes firm with guns up — hold-fire has been overwatch since the day
    /// it was named. Out of contact, everyone travels; a stand-fast has
    /// nothing to bound toward and a withdrawal values speed over ceremony
    /// (alternate bounds rearward is a real technique, and a refinement for
    /// later); a formation of one has nobody to cover her.
    fn overwatch_this_round(
        &self,
        state: &BattleState,
        unit: crate::battle::UnitId,
        index: usize,
    ) -> bool {
        let Some(formation) = state.command.formations().get(index) else {
            return false;
        };
        if !matches!(
            formation.mission_for(unit),
            Some(Mission::Advance { .. }) | Some(Mission::Recon { .. })
        ) {
            return false;
        }
        // Traveling versus bounding is a doctrine's call, and the harness
        // agreed with the textbooks before this gate existed: universal
        // bounding cost massed armour half its wins (10 to 5 in 36),
        // because its identity is trading security for tempo. An
        // aggressive doctrine travels in overwatch — spread, guns ready,
        // still moving — and only the cautious ones pay the round for the
        // covered bound.
        if self.doctrine_for(index).aggression >= 0.7 {
            return false;
        }
        // Contact worth the ceremony: a spotted enemy near enough to the
        // formation to matter, not one across the map.
        let relevant = state
            .fog
            .side(formation.side)
            .spotted
            .iter()
            .filter_map(|id| state.unit(*id))
            .any(|enemy| {
                formation
                    .members
                    .iter()
                    .filter_map(|id| state.unit(*id))
                    .any(|member| member.pos.distance_to(enemy.pos) <= 15)
            });
        if !relevant {
            return false;
        }
        let living: Vec<crate::battle::UnitId> = formation
            .members
            .iter()
            .copied()
            .filter(|id| state.unit(*id).is_some())
            .collect();
        if living.len() < 2 {
            return false;
        }
        // Element by position among the living (id order), bounding element
        // by round parity — deterministic, and it swaps every round.
        let Some(position) = living.iter().position(|id| *id == unit) else {
            return false;
        };
        (position % 2) as u32 != (state.round.wrapping_add(index as u32)) % 2
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
        // The lane itself is not this brain's rule to have: the player and the
        // campaign choose a withdrawal route the same way, so they choose it in
        // the same function.
        crate::battle::nearest_exit(state, formation.side, from)
    }
}

impl AiPlanner<BattleState, Order> for SideCommand {
    fn next_order(&mut self, registry: &DataRegistry, state: &BattleState, side: u8) -> Order {
        self.ensure_built(registry, state, side);

        // The commander speaks first, once per round: missions before unit
        // orders, so the executors already know what the ground is worth by
        // the time they plan the first vehicle. A brainless side skips this
        // entirely — somebody else has already spoken, or nobody has.
        if self.reviews_missions && self.reviewed_round != Some(state.round) {
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
        let formation = state
            .command
            .formations()
            .iter()
            .position(|f| f.contains(unit));
        // Without a brain, an unmissioned unit is one nobody has decided
        // about, and deciding for her is not this object's job — with one
        // exception every army since 1918 has drilled: nobody under fire
        // waits for permission to survive. Threatened, she executes the
        // battle drill (return fire, seek cover, want nothing else on the
        // map); safe, she watches her arc exactly as before, so the
        // parking lot stays parked. Any explicit order — including the
        // deliberate "hold and watch" — outranks the drill, because it
        // marks her planned before this is ever consulted.
        if !self.reviews_missions
            && !formation
                .and_then(|index| state.command.formations().get(index))
                .is_some_and(|f| f.latest_mission().is_some())
        {
            if threatened(registry, state, unit) {
                self.pending = self.drill.plan_unit(registry, state, unit).into();
                if let Some(order) = self.pending.pop_front() {
                    return order;
                }
            }
            return Order::SetFire {
                unit,
                fire: FireIntent::Hold,
            };
        }
        // Bounding overwatch: her element stands firm this round while the
        // other advances. Before the executor, because standing firm IS her
        // plan — guns up on her arc, covering the bound.
        if let Some(index) = formation
            && self.overwatch_this_round(state, unit, index)
        {
            return Order::SetFire {
                unit,
                fire: FireIntent::Hold,
            };
        }
        let executor = formation
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
