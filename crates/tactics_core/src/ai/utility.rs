//! Greedy utility planner: score every (destination, attack) pair for one
//! unit at a time and take the best. Cheap, tunable, and honest about fog.
//!
//! Difficulty is expressed as scoring noise: low-difficulty planners see the
//! same candidates through a blurrier lens, which makes them play worse in a
//! humanlike way rather than following visibly dumb rules.

use super::{best_weapon_against, next_idle_unit, visible_enemies, AiPlanner};
use crate::battle::{reachable, BattleState, Order, UnitId};
use crate::data::DataRegistry;
use hexx::Hex;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

pub struct UtilityPlanner {
    /// 0..=1: how much expected damage outweighs self-preservation.
    pub aggression: f32,
    /// Uniform noise amplitude added to every candidate score.
    pub noise: f32,
    rng: ChaCha8Rng,
}

impl UtilityPlanner {
    pub fn new(aggression: f32, noise: f32, seed: u64) -> Self {
        Self {
            aggression,
            noise,
            rng: ChaCha8Rng::seed_from_u64(seed),
        }
    }

    pub fn with_difficulty(difficulty: u8, seed: u64) -> Self {
        let noise = match difficulty {
            1 => 6.0,
            2 => 3.0,
            3 => 1.5,
            4 => 0.5,
            _ => 0.0,
        };
        Self::new(0.6, noise, seed)
    }

    /// Score standing on `tile`, optionally attacking from there.
    fn score_tile(
        &mut self,
        registry: &DataRegistry,
        state: &BattleState,
        unit: UnitId,
        tile: Hex,
    ) -> (f32, Option<(UnitId, usize)>) {
        let enemies = visible_enemies(state, state.unit(unit).map(|u| u.side).unwrap_or(0));
        let me = state.unit(unit).expect("scored unit exists");

        // Offense: best attack available from this tile.
        let mut best_attack: Option<(UnitId, usize, f32)> = None;
        for enemy in &enemies {
            if let Some((weapon, dmg, kill)) = best_weapon_against(registry, state, unit, tile, enemy)
            {
                let value = dmg + if kill { 4.0 } else { 0.0 };
                if best_attack.is_none_or(|(_, _, v)| value > v) {
                    best_attack = Some((enemy.id, weapon, value));
                }
            }
        }
        let attack_value = best_attack.map(|(_, _, v)| v).unwrap_or(0.0);

        // Threat: how hard the visible enemies could hit us back there.
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

        // Terrain: cover and high ground are worth holding.
        let mut terrain_value = 0.0;
        if let Some(t) = state.map.get(tile) {
            terrain_value += t.elevation as f32 * 0.4;
            if let Some(def) = registry.terrain(&t.terrain) {
                terrain_value += def.cover as f32 * 0.03;
            }
        }

        // Advance: with nothing to shoot, close the distance to the nearest
        // spotted enemy; with no contact at all, push toward map center to
        // find one (scouting pressure).
        let advance = if let Some(nearest) = enemies.iter().map(|e| e.pos.distance_to(tile)).min()
        {
            -(nearest as f32) * 0.3 * self.aggression
        } else {
            -(state.map.center().distance_to(tile) as f32) * 0.15
        };

        let noise = if self.noise > 0.0 {
            self.rng.random_range(-self.noise..self.noise)
        } else {
            0.0
        };
        let score = attack_value * 2.0 * (0.5 + self.aggression) - threat * (1.5 - self.aggression)
            + terrain_value
            + advance
            + noise;
        (score, best_attack.map(|(t, w, _)| (t, w)))
    }
}

impl AiPlanner<BattleState, Order> for UtilityPlanner {
    fn next_order(&mut self, registry: &DataRegistry, state: &BattleState, side: u8) -> Order {
        let Some(unit_id) = next_idle_unit(state, side) else {
            return Order::EndTurn;
        };
        let unit = state.unit(unit_id).expect("idle unit is alive");

        if !unit.moved {
            // Choose a destination (possibly the current tile).
            let mut options: Vec<Hex> = reachable(registry, state, unit_id).into_keys().collect();
            options.sort_unstable_by_key(|h| (h.x, h.y));
            let mut best: Option<(f32, Hex)> = None;
            for tile in options {
                let (score, _) = self.score_tile(registry, state, unit_id, tile);
                if best.is_none_or(|(s, _)| score > s) {
                    best = Some((score, tile));
                }
            }
            let dest = best.map(|(_, h)| h).unwrap_or(unit.pos);
            if dest != unit.pos {
                return Order::Move {
                    unit: unit_id,
                    to: dest,
                };
            }
            // Standing still: fall through to the action decision.
        }

        // Already in position: attack if anything is worth shooting.
        let (_, attack) = self.score_tile(registry, state, unit_id, unit.pos);
        match attack {
            Some((target, weapon)) => Order::Attack {
                unit: unit_id,
                target,
                weapon,
            },
            None => Order::Wait { unit: unit_id },
        }
    }
}
