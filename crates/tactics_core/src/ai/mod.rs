//! Swappable AI planners.
//!
//! [`AiPlanner`] is generic over the state and order types, so the same
//! trait drives battle units and overworld armies. Planners are handed the
//! full state but must only act on what their side's fog allows -- the
//! provided implementations go through [`visible_enemies`] and friends
//! rather than peeking at hidden units.

mod mcts;
mod utility;

pub use mcts::MctsPlanner;
pub use utility::UtilityPlanner;

use crate::battle::{BattleState, Order, Unit, UnitId};
use crate::data::DataRegistry;
use serde::{Deserialize, Serialize};

/// A decision maker for one side. Called repeatedly during that side's
/// phase; each call returns the next order to apply. Returning
/// [`Order::EndTurn`] (or the overworld equivalent) yields control.
pub trait AiPlanner<S, O>: Send + Sync {
    fn next_order(&mut self, registry: &DataRegistry, state: &S, side: u8) -> O;
}

/// JSON-configurable AI assignment, e.g. `{"planner": "utility", "difficulty": 3}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiConfig {
    pub planner: String,
    #[serde(default = "default_difficulty")]
    pub difficulty: u8,
}

fn default_difficulty() -> u8 {
    3
}

/// Build a battle planner from a config. Unknown planner names fall back to
/// the utility planner so a typo in a mod degrades instead of crashing.
pub fn make_battle_planner(
    config: &AiConfig,
    seed: u64,
) -> Box<dyn AiPlanner<BattleState, Order>> {
    let difficulty = config.difficulty.clamp(1, 5);
    match config.planner.as_str() {
        "mcts" => Box::new(MctsPlanner::with_difficulty(difficulty, seed)),
        _ => Box::new(UtilityPlanner::with_difficulty(difficulty, seed)),
    }
}

/// Enemies of `side` that its fog currently allows it to target.
pub fn visible_enemies<'s>(state: &'s BattleState, side: u8) -> Vec<&'s Unit> {
    let fog = state.fog.side(side);
    state
        .alive_units()
        .filter(|u| u.side != side && fog.spotted.contains(&u.id))
        .collect()
}

/// The next unit of `side` that can still receive orders.
pub fn next_idle_unit(state: &BattleState, side: u8) -> Option<UnitId> {
    state
        .side_units(side)
        .filter(|u| !u.acted)
        .map(|u| u.id)
        .min()
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
        if !weapon.indirect
            && !crate::battle::los_clear(registry, &state.map, from, target.pos)
        {
            continue;
        }
        let dmg =
            crate::battle::expected_damage(registry, state, unit, from, weapon, target.id, false);
        let kill = dmg >= target.hp as f32 * 0.9;
        if best.is_none_or(|(_, d, _)| dmg > d) {
            best = Some((i, dmg, kill));
        }
    }
    best
}
