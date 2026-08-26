//! Swappable AI planners.
//!
//! [`AiPlanner`] is generic over the state and order types, so the same trait
//! drives battle units and overworld armies. Planners are handed the full
//! state but must only act on what their side's fog allows -- the provided
//! implementations go through [`visible_enemies`] and friends rather than
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

/// Enemies of `side` that its fog currently allows it to target.
pub fn visible_enemies(state: &BattleState, side: u8) -> Vec<&Unit> {
    let fog = state.fog.side(side);
    state
        .alive_units()
        .filter(|u| u.side != side && fog.spotted.contains(&u.id))
        .collect()
}

/// The next unit of `side` that has not been given orders this round.
pub fn next_unplanned_unit(state: &BattleState, side: u8) -> Option<UnitId> {
    state.unplanned_units(side).map(|u| u.id).min()
}

/// Whether anything the side can see could put fire on this unit where she
/// stands. Fog-honest (spotted enemies only, through the same
/// [`best_weapon_against`] every planner prices shots with) and
/// deterministic, because "was she in danger" must answer the same on every
/// machine.
///
/// Three very different things ask it, and they must ask it the same way or
/// the game contradicts itself: the battle drill at the planning table, which
/// is what an unordered crew does when nobody has told her anything; the
/// evaluator, where being under fire is what suspends a movement to contact;
/// and the engine's mid-round drill, where the same danger noticed at tick
/// four is what sends an idle crew scrambling for the trees. All three are
/// the same sentence — *is somebody shooting at me* — so all three read the
/// same predicate rather than formulas that can drift apart. The engine's
/// consumer needs to know *who* so it can ask the crew's clock whether she
/// has caught up with each of them yet, hence [`threats`] underneath.
pub(crate) fn threatened(registry: &DataRegistry, state: &BattleState, unit: UnitId) -> bool {
    !threats(registry, state, unit).is_empty()
}

/// The spotted enemies that could put fire on this unit where she stands, in
/// id order. The list form of [`threatened`], for the one caller — the
/// mid-round drill — that must weigh each threat against when she first laid
/// eyes on it.
pub(crate) fn threats(registry: &DataRegistry, state: &BattleState, unit: UnitId) -> Vec<UnitId> {
    let Some(me) = state.unit(unit) else {
        return Vec::new();
    };
    visible_enemies(state, me.side)
        .iter()
        .filter(|enemy| best_weapon_against(registry, state, enemy.id, enemy.pos, me).is_some())
        .map(|enemy| enemy.id)
        .collect()
}

/// The best (weapon index, expected damage, would-kill) attack `unit` could
/// make against `target` if it were standing at `from`.
pub fn best_weapon_against(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    from: hexx::Hex,
    target: &Unit,
) -> Option<(usize, f32, bool)> {
    let u = state.unit(unit)?;
    let vehicle = registry.vehicle(&u.vehicle)?;
    let dist = from.distance_to(target.pos);
    let mut best: Option<(usize, f32, bool)> = None;
    for (i, weapon_id) in vehicle.weapons.iter().enumerate() {
        let Some(weapon) = registry.weapon(weapon_id) else {
            continue;
        };
        if !(weapon.range[0] as i32..=weapon.range[1] as i32).contains(&dist) {
            continue;
        }
        if !weapon.indirect && !state.sight.clear(from, target.pos) {
            continue;
        }
        let dmg =
            crate::battle::expected_damage(registry, state, unit, from, weapon, target.id, false);
        // A gun that expects nothing — racks empty, or a round that cannot
        // beat the plate it would strike — is not a weapon against this
        // target at all. This is the line that makes `threatened` honest
        // now that the damage floor is gone: a machine gun in range of a
        // heavy tank no longer counts as somebody shooting at her, so the
        // drill stops breaking cover for it and a movement to contact
        // stops pausing for it.
        if dmg <= 0.0 {
            continue;
        }
        // "Could this plausibly finish her": the expected outcome against
        // what is actually left aboard. Same shape as the old hit-point
        // comparison, with substance as the pool.
        let kill = dmg >= state.substance(registry, target).0 as f32 * 0.9;
        if best.is_none_or(|(_, d, _)| dmg > d) {
            best = Some((i, dmg, kill));
        }
    }
    best
}
