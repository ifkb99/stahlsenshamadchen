//! Doctrine-weighted scoring, shared by every planner.
//!
//! The numbers a planner cares about — is this tile worth holding, is this
//! position winning — live here rather than inside one planner, so the
//! utility planner's per-unit choice and MCTS's search heuristic can never
//! drift apart, and so a doctrine file changes both at once.

use super::{best_weapon_against, visible_enemies};
use crate::battle::{BattleState, UnitId};
use crate::data::{DataRegistry, DoctrineDef};
use hexx::Hex;

/// What holding a tile is worth, and the shot that comes with it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileScore {
    pub score: f32,
    /// The best attack available from that tile: target and weapon index.
    pub attack: Option<(UnitId, usize)>,
}

/// Scores positions the way one doctrine sees them. Deterministic: any
/// difficulty noise is the planner's business, added on top.
pub struct Evaluator {
    pub doctrine: DoctrineDef,
}

impl Evaluator {
    pub fn new(doctrine: DoctrineDef) -> Self {
        Self { doctrine }
    }

    /// Score `unit` standing on `tile`, together with the best attack it
    /// could make from there.
    pub fn score_tile(
        &self,
        registry: &DataRegistry,
        state: &BattleState,
        unit: UnitId,
        tile: Hex,
    ) -> TileScore {
        let Some(me) = state.unit(unit) else {
            return TileScore {
                score: 0.0,
                attack: None,
            };
        };
        let doctrine = &self.doctrine;
        let enemies = visible_enemies(state, me.side);

        // Offense: the best shot available from this tile. Indirect appetite
        // scales what artillery is worth, so a doctrine that hoards shells
        // values a howitzer opportunity less than one that spends them.
        let mut best_attack: Option<(UnitId, usize, f32)> = None;
        for enemy in &enemies {
            let Some((weapon, dmg, kill)) = best_weapon_against(registry, state, unit, tile, enemy)
            else {
                continue;
            };
            let mut value = dmg + if kill { 4.0 } else { 0.0 };
            if self.is_indirect(registry, state, unit, weapon) {
                value *= doctrine.indirect_appetite;
            }
            if best_attack.is_none_or(|(_, _, v)| value > v) {
                best_attack = Some((enemy.id, weapon, value));
            }
        }
        let attack_value = best_attack.map(|(_, _, v)| v).unwrap_or(0.0);

        // Threat: how hard the visible enemies could hit us there. A damaged
        // unit under a doctrine that expects to withdraw weighs this more.
        let mut threat = 0.0;
        for enemy in &enemies {
            if let Some((_, dmg, _)) = best_weapon_against(registry, state, enemy.id, enemy.pos, me)
            {
                // Cheap positional check: could they reach/see this tile?
                let dist = enemy.pos.distance_to(tile);
                if dist <= 6 {
                    threat += dmg * (1.0 / dist.max(1) as f32);
                }
            }
        }
        let hp_fraction = registry
            .vehicle(&me.vehicle)
            .map(|v| me.hp as f32 / v.max_hp.max(1) as f32)
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        let caution =
            (1.5 - doctrine.aggression) * (1.0 + doctrine.withdraw_threshold * (1.0 - hp_fraction));

        // Terrain: cover and high ground, worth as much as doctrine says.
        let mut terrain_value = 0.0;
        if let Some(t) = state.map.get(tile) {
            terrain_value += t.elevation as f32 * 0.4 * doctrine.elevation_value;
            if let Some(def) = registry.terrain(&t.terrain) {
                terrain_value += def.cover as f32 * 0.03 * doctrine.cover_value;
            }
        }

        // Mass: stay within supporting distance of the rest of the force.
        // Friends already under orders count from where they are heading, not
        // where they stand, so a formation converges instead of chasing.
        let mass = state
            .side_units(me.side)
            .filter(|other| other.id != unit)
            .map(|other| other.planned_destination().distance_to(tile))
            .min()
            .map(|dist| -(dist as f32) * 0.1 * doctrine.concentration)
            .unwrap_or(0.0);

        // Advance: with something to shoot, close on it; with no contact at
        // all, push toward the middle of the map to find some.
        let advance = match enemies.iter().map(|e| e.pos.distance_to(tile)).min() {
            Some(nearest) => -(nearest as f32) * 0.3 * doctrine.aggression,
            None => -(state.map.center().distance_to(tile) as f32) * 0.15 * doctrine.scouting,
        };

        TileScore {
            score: attack_value * 2.0 * (0.5 + doctrine.aggression) - threat * caution
                + terrain_value
                + mass
                + advance,
            attack: best_attack.map(|(t, w, _)| (t, w)),
        }
    }

    /// How the battle stands for `side`, in [-1, 1]. Aggressive doctrines
    /// count damage dealt for more than damage avoided, so search under those
    /// weights accepts trades a cautious one would refuse.
    pub fn position_value(&self, state: &BattleState, side: u8) -> f32 {
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
            return 0.0;
        }
        let weight_theirs = 0.5 + self.doctrine.aggression * 0.5;
        let weight_ours = 1.5 - weight_theirs;
        (ours * weight_ours - theirs * weight_theirs) / total
    }

    fn is_indirect(
        &self,
        registry: &DataRegistry,
        state: &BattleState,
        unit: UnitId,
        weapon: usize,
    ) -> bool {
        state
            .unit(unit)
            .and_then(|u| registry.vehicle(&u.vehicle))
            .and_then(|v| v.weapons.get(weapon))
            .and_then(|w| registry.weapon(w))
            .is_some_and(|w| w.indirect)
    }
}
