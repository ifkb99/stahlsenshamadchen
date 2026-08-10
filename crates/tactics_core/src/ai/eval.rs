//! Doctrine-weighted scoring, shared by every planner.
//!
//! The numbers a planner cares about — is this tile worth holding, is this
//! position winning — live here rather than inside one planner, so the
//! utility planner's per-unit choice and MCTS's search heuristic can never
//! drift apart, and so a doctrine file changes both at once.

use super::{best_weapon_against, visible_enemies};
use crate::battle::{BattleState, UnitId};
use crate::data::{DataRegistry, DoctrineDef};
use crate::map::ObjectiveKind;
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

        // Objectives: the one thing on the map worth something with no enemy
        // attached to it, and the reason this evaluator will leave good cover
        // at all. Measured against the old behaviour it is the whole fix —
        // without it, holding the best ground in sight is unbeatable play,
        // and two sides doing that never meet.
        let objective = self.objective_value(state, me.side, tile, hp_fraction);

        // Advance: with something to shoot, close on it. With no contact and
        // no objectives, push toward the middle of the map to find some —
        // which is all this could do before objectives existed, and is still
        // what a map that names none gets.
        let advance = match enemies.iter().map(|e| e.pos.distance_to(tile)).min() {
            Some(nearest) => -(nearest as f32) * 0.3 * doctrine.aggression,
            None if state.map.objectives().is_empty() => {
                -(state.map.center().distance_to(tile) as f32) * 0.15 * doctrine.scouting
            }
            // The objective term is already saying which way to walk, and far
            // more specifically than "inwards" ever did.
            None => 0.0,
        };

        TileScore {
            score: attack_value * 2.0 * (0.5 + doctrine.aggression) - threat * caution
                + terrain_value
                + mass
                + objective
                + advance,
            attack: best_attack.map(|(t, w, _)| (t, w)),
        }
    }

    /// What holding `tile` is worth in objective terms to `side`.
    ///
    /// Every objective is scored and the best taken, so a unit walks toward
    /// the one it can most usefully affect rather than being pulled apart by
    /// all of them at once. Two things fall out of the shape:
    ///
    /// - The reward is for *standing* on it, and the distance term is a slope
    ///   leading there, so a unit twenty hexes away still knows which way to
    ///   drive. That slope is what a greedy one-round planner needs: it can
    ///   only see the tiles it can reach this round, so a gradient that is
    ///   flat until you arrive is a gradient it cannot follow.
    /// - Ground already held is worth half. It is still worth sitting on —
    ///   walking away hands it back for free — but not worth marching across
    ///   the map for when there is unclaimed ground somewhere else.
    ///
    /// The two coefficients were swept over 24 battles. Both halves of the
    /// curve are bad in different ways: at zero this is the old game, 11 of 24
    /// decided and 302 shots fired in 16.5 rounds because nobody advances; at
    /// double these values it is 21 of 24 decided but only 517 shots in 8.1
    /// rounds, because units drive at the objective through fire and are
    /// destroyed before a firefight develops. The peak of *fighting* — 652
    /// shots, 23 of 24 decided, 9.3 rounds — is here, which is why the
    /// engine-side numbers are half what they first were rather than the
    /// doctrine values being odd fractions.
    /// Exits are the exception, and the reason this is not one formula. Ground
    /// worth *leaving* by cannot be worth walking to in general — a unit that
    /// valued the exit the way it values the bridge would drive off the map on
    /// the first round and call it a victory. The pull toward an exit is
    /// therefore gated on the doctrine's `withdraw_threshold` and the
    /// vehicle's own damage: intact crews cannot see the lane at all, and a
    /// crew that is nearly finished under a doctrine that expects to fall back
    /// will run for it. That is the whole of withdrawal for now; who is
    /// *permitted* to leave belongs to the chain of command, not to the
    /// evaluator.
    fn objective_value(&self, state: &BattleState, side: u8, tile: Hex, hp_fraction: f32) -> f32 {
        // How badly this crew wants out: nothing at all until the doctrine's
        // withdrawal threshold is crossed, then rising as the vehicle is shot
        // to pieces. An intact tank under any doctrine scores every exit at
        // zero, which is what stops the lane being a free win.
        let flight = ((self.doctrine.withdraw_threshold - hp_fraction)
            / self.doctrine.withdraw_threshold.max(0.01))
        .clamp(0.0, 1.0);

        let mut best: Option<f32> = None;
        for (objective, held) in state.objectives() {
            // Ground reserved to the other side is somebody else's business.
            if !objective.open_to(side) {
                continue;
            }
            let appetite = match objective.kind {
                ObjectiveKind::Hold => {
                    // Ground already held is worth half: still worth sitting
                    // on, not worth marching across the map for.
                    if held == Some(side) { 0.5 } else { 1.0 }
                }
                ObjectiveKind::Exit => flight,
            };
            if appetite <= 0.0 {
                continue;
            }
            let weight = objective.value as f32 * self.doctrine.objective_value * appetite;
            let distance = objective
                .hexes
                .iter()
                .map(|h| h.distance_to(tile))
                .min()
                .unwrap_or(0);
            let reward = if objective.contains(tile) { 1.5 } else { 0.0 };
            let score = weight * (reward - 0.15 * distance as f32);
            if best.is_none_or(|b| score > b) {
                best = Some(score);
            }
        }
        best.unwrap_or(0.0)
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
        let material = (ours * weight_ours - theirs * weight_theirs) / total;

        // Ground counts too, or a search planner would happily trade away
        // every objective on the map for a favourable exchange of tanks and
        // then lose on points. Weighted against material rather than added to
        // it so the result stays inside [-1, 1] and the two are comparable.
        // Only ground that can be *held* counts here. An exit is scored by
        // driving off it, which shows up in the score rather than in who
        // stands where, and folding it in would dilute the fraction by ground
        // nobody can ever hold.
        let ground: f32 = state
            .objectives()
            .filter(|(o, _)| o.kind == ObjectiveKind::Hold)
            .map(|(objective, held)| match held {
                Some(s) if s == side => objective.value as f32,
                Some(_) => -(objective.value as f32),
                None => 0.0,
            })
            .sum();
        let at_stake: f32 = state
            .map
            .objectives()
            .iter()
            .filter(|o| o.kind == ObjectiveKind::Hold)
            .map(|o| o.value as f32)
            .sum::<f32>();
        if at_stake <= 0.0 {
            return material;
        }
        0.7 * material + 0.3 * (ground / at_stake)
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
