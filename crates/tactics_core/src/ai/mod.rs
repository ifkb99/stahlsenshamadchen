//! Swappable AI planners.
//!
//! [`AiPlanner`] is generic over the state and order types, so the same trait
//! drives battle units and overworld armies. Planners are handed the full
//! state but must only act on what their side's fog allows -- the provided
//! implementations go through [`BattleState::known_enemies`] rather than
//! peeking at hidden units.
//!
//! Three things are deliberately independent, because they answer different
//! questions:
//!
//! - **Planner** — *how* a side thinks. [`UtilityPlanner`] scores candidates,
//!   [`MctsPlanner`] searches. Chosen per side, and extensible through
//!   [`PlannerRegistry`].
//! - **Doctrine** — *what* it values. Mod data ([`DoctrineDef`]) read by the
//!   shared [`Evaluator`], so two academies running the same algorithm still
//!   fight differently.
//! - **Difficulty** — *how well* it executes: scoring noise for the utility
//!   planner, search budget for MCTS. Nothing else.
//!
//! That separation is the point: a weak opponent running massed-armour
//! doctrine should still recognisably fight like massed armour, just badly.

mod command;
mod driver;
mod eval;
pub mod goal;
mod mcts;
mod utility;

pub use command::SideCommand;
pub use driver::{AiDriver, Decision};
pub use eval::{Evaluator, TileScore};
pub use mcts::{MctsPlanner, determinize};
pub use utility::UtilityPlanner;

use crate::battle::{BattleState, Order, Unit, UnitId};
use crate::data::{DataRegistry, DoctrineDef};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A decision maker for one side. Called repeatedly while that side has
/// something to say; each call returns the next order to apply. Returning
/// [`Order::Commit`] (or the overworld equivalent) yields control.
pub trait AiPlanner<S, O>: Send + Sync {
    fn next_order(&mut self, registry: &DataRegistry, state: &S, side: u8) -> O;

    /// Whether the order just returned came from the battle drill — a crew
    /// acting to keep herself alive — rather than from anything anybody told
    /// her to do.
    ///
    /// Presentation, not simulation: nothing in the engine branches on it,
    /// and a planner that never overrides an order can ignore it, which is
    /// why it defaults to `false` rather than becoming another thing every
    /// planner has to implement. What it is *for* is the bargain this game
    /// makes everywhere else and broke here: a vehicle that moves with no
    /// visible order behind it is indistinguishable from a bug, so every
    /// deviation says so out loud. The case that motivated it is a personal
    /// march broken off for cover, which the game crate previously filtered
    /// out of its own announcement and left silent.
    fn last_was_drill(&self) -> bool {
        false
    }
}

/// JSON-configurable AI assignment, e.g.
/// `{"planner": "mcts", "difficulty": 4, "doctrine": "massed_armor"}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiConfig {
    pub planner: String,
    #[serde(default = "default_difficulty")]
    pub difficulty: u8,
    /// Doctrine id from mod data. Absent means the balanced default, so map
    /// files written before doctrine existed keep working.
    #[serde(default)]
    pub doctrine: Option<String>,
}

fn default_difficulty() -> u8 {
    3
}

/// Planner names the engine ships. Map validation warns about anything else,
/// since planners are Rust rather than mod data.
pub const BUILTIN_PLANNERS: &[&str] = &["utility", "mcts", "command"];

/// How a planner is built.
///
/// The registry is passed in so a planner can construct subordinates: a
/// hierarchical commander needs child planners with their own doctrines.
/// Threading it through now costs one parameter; adding it later would mean
/// touching every planner.
pub type BattlePlannerCtor =
    fn(&AiConfig, u64, &DataRegistry, &PlannerRegistry) -> Box<dyn AiPlanner<BattleState, Order>>;

/// Planner name to constructor. Built-ins are registered by
/// [`Self::with_builtins`]; anything else can be added at startup.
pub struct PlannerRegistry {
    ctors: HashMap<String, BattlePlannerCtor>,
}

impl Default for PlannerRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

impl PlannerRegistry {
    pub fn empty() -> Self {
        Self {
            ctors: HashMap::new(),
        }
    }

    pub fn with_builtins() -> Self {
        let mut registry = Self::empty();
        registry.register("utility", |config, seed, data, _planners| {
            Box::new(UtilityPlanner::from_config(config, seed, data))
        });
        registry.register("mcts", |config, seed, data, planners| {
            Box::new(MctsPlanner::from_config(config, seed, data, planners))
        });
        registry.register("command", |config, seed, data, _planners| {
            Box::new(SideCommand::from_config(config, seed, data))
        });
        registry
    }

    pub fn register(&mut self, name: impl Into<String>, ctor: BattlePlannerCtor) {
        self.ctors.insert(name.into(), ctor);
    }

    pub fn contains(&self, name: &str) -> bool {
        self.ctors.contains_key(name)
    }

    /// Build the planner a side asked for. An unknown name degrades to the
    /// utility planner: a typo in a mod should not crash a battle. Map
    /// validation is where the typo gets reported.
    pub fn build(
        &self,
        config: &AiConfig,
        seed: u64,
        data: &DataRegistry,
    ) -> Box<dyn AiPlanner<BattleState, Order>> {
        let ctor = self
            .ctors
            .get(config.planner.as_str())
            .or_else(|| self.ctors.get("utility"));
        match ctor {
            Some(ctor) => ctor(config, seed, data, self),
            None => Box::new(UtilityPlanner::from_config(config, seed, data)),
        }
    }
}

/// Build a battle planner from a config using the built-in registry.
pub fn make_battle_planner(
    config: &AiConfig,
    seed: u64,
    data: &DataRegistry,
) -> Box<dyn AiPlanner<BattleState, Order>> {
    PlannerRegistry::with_builtins().build(config, seed, data)
}

/// The doctrine a side fights by. An absent or unknown id falls back to the
/// balanced default rather than refusing to field the side.
pub fn resolve_doctrine(config: &AiConfig, data: &DataRegistry) -> DoctrineDef {
    config
        .doctrine
        .as_deref()
        .and_then(|id| data.doctrine(id).cloned())
        .unwrap_or_default()
}

/// Scoring noise for a difficulty level. Low difficulty sees the same
/// candidates through a blurrier lens, which plays badly in a humanlike way
/// rather than following visibly dumb rules.
pub fn difficulty_noise(difficulty: u8) -> f32 {
    match difficulty.clamp(1, 5) {
        1 => 6.0,
        2 => 3.0,
        3 => 1.5,
        4 => 0.5,
        _ => 0.0,
    }
}

/// How much of the goal chooser's deeper reasoning a commander at this
/// difficulty actually does, 0..=1.
///
/// The twin of [`difficulty_noise`] and the answer to a real problem with it:
/// blur alone gets *less* discriminating as the thing being blurred gets
/// better, because a value function that separates a good goal from a bad one
/// more sharply is one a blurred commander still ranks correctly. Deepening
/// the chooser while leaving difficulty as noise would therefore have made
/// difficulty matter less.
///
/// So the two axes say different things about a bad commander. Noise is
/// misjudging what she has read; foresight is not having read it — she sees
/// the objective and how far off it is as the crow flies, and not the river
/// in between, nor the gun covering the open ground, nor that the enemy is
/// nearer to the bridge than she is. Both kinds of error are ones a player
/// can watch happen and punish, which is the whole standard difficulty is
/// held to here.
///
/// Zero at difficulty 1 is exactly the chooser as it stood before it could
/// price a road, so the weakest commander plays the game the AI has always
/// played and every level above her is an addition.
pub fn difficulty_foresight(difficulty: u8) -> f32 {
    match difficulty.clamp(1, 5) {
        1 => 0.0,
        2 => 0.25,
        3 => 0.5,
        4 => 0.8,
        _ => 1.0,
    }
}

/// The next unit of `side` that has not been given orders this round.
pub fn next_unplanned_unit(state: &BattleState, side: u8) -> Option<UnitId> {
    state.unplanned_units(side).map(|u| u.id).min()
}

/// Whether anything the side can see could put fire on this unit where she
/// stands. Fog-honest (spotted enemies only) and deterministic, because "was
/// she in danger" must answer the same on every machine.
///
/// Six very different things ask it, and they must ask it the same way or the
/// game contradicts itself: the battle drill at the planning table, which is
/// what an unordered crew does when nobody has told her anything; the
/// evaluator, where being under fire is what suspends a movement to contact;
/// the taxi rules, where nobody mounts up or waits at a tailgate under fire;
/// the dismount reflex; and the engine's mid-round drill and its rout, where
/// the same danger noticed at tick four is what sends an idle crew scrambling.
/// All of them are the same sentence — *is somebody shooting at me* — so all
/// of them read the same predicate rather than formulas that can drift apart.
///
/// **It is [`crate::battle::incoming`] and nothing else**, which is the whole
/// of Wave 2's first part: this used to be a second walk over the visible
/// enemies asking [`best_weapon_against`] the same question
/// [`crate::battle::danger`] asks, with its own gate. Two walks are two
/// answers waiting to happen — a gun added to one and not the other, or a
/// gate reworded in one place — and the currency has exactly one answer to
/// *who can shoot her there*. The two are arithmetically identical today
/// (`best_weapon_from` admits a gun precisely when its `worth` is positive,
/// and the sum of positive terms is positive), so this is a refactor with a
/// test on it rather than a behaviour change; what it buys is that it stays
/// identical.
pub fn threatened(registry: &DataRegistry, state: &BattleState, unit: UnitId) -> bool {
    let Some(me) = state.unit(unit) else {
        return false;
    };
    crate::battle::incoming(registry, state, unit, me.pos).worth > 0.0
}

/// The spotted enemies that could put fire on this unit where she stands, in
/// id order. The list form of [`threatened`], for the two callers — the
/// mid-round drill and the rout — that must weigh each threat against when
/// she first laid eyes on it, or run away from where it is standing.
///
/// [`crate::battle::fire_on`]'s membership, and deliberately nothing more:
/// the bearings carry a weapon, a hit chance and a cadence that neither
/// caller wants, but taking the ids off the one walk is what keeps "who can
/// shoot her" from having two answers.
pub fn threats(registry: &DataRegistry, state: &BattleState, unit: UnitId) -> Vec<UnitId> {
    let Some(me) = state.unit(unit) else {
        return Vec::new();
    };
    crate::battle::fire_on(registry, state, unit, me.pos)
        .into_iter()
        .map(|bearing| bearing.enemy)
        .collect()
}

/// The best (weapon index, worth of a round of fire, would-kill) attack
/// `unit` could make against `target` if `unit` were standing at `from` and
/// `target` at `at`.
///
/// Both ends are hypothetical, because both ends of a shot are ground: the
/// evaluator varies `from` when it is deciding where a crew should drive,
/// and varies `at` when it is asking what could be done to her there. Every
/// caller that means "where they actually are" passes the real hexes.
///
/// The gun and the arithmetic are [`crate::battle::best_weapon_from`]'s;
/// what this adds is the *judgment* on top — whether the shot would
/// plausibly finish her, which is a comparison against what is left aboard
/// and therefore an AI question rather than a rule of the battlefield.
///
/// The number is [`crate::battle::ShotValue::worth_per_round`]: what a round
/// of fire from that gun is worth, damage and pressure together. Every caller
/// of this is scoring *ground* — where to drive, what she could do from there
/// — and ground is held for rounds, so a gun's rate of fire belongs in the
/// figure. The one place that prices a single trigger pull is
/// `combat::best_opportunity_shot`, which does not come through here.
pub fn best_weapon_against(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    from: hexx::Hex,
    target: &Unit,
    at: hexx::Hex,
) -> Option<(usize, f32, bool)> {
    let (weapon, value) =
        crate::battle::best_weapon_from(registry, state, unit, from, target.id, at)?;
    // "Could this plausibly finish her": the expected outcome against
    // what is actually left aboard. Same shape as the old hit-point
    // comparison, with substance as the pool.
    //
    // **Per shot, and pure damage**, while the value beside it is worth over
    // a whole round. That asymmetry is deliberate and is the conservative
    // reading of both halves. The +4 bonus the evaluator pays on this flag
    // is for a *decisive* shot — the one that ends her — and multiplying it
    // by cadence would have an autocannon at twelve shots a round believe it
    // finishes everything it can see, which is the machine-gun-grinds-a-tank
    // defect wearing a new hat. Pressure is excluded for the same reason:
    // frightening a crew is not killing her, however much of it you do.
    let kill = value.expected >= state.substance(registry, target).0 as f32 * 0.9;
    Some((weapon, value.worth_per_round(), kill))
}
