//! What the enemy could do to a crew standing on a given piece of ground.
//!
//! This is one function and one struct, and the point of both is that there
//! is exactly one of them. "How dangerous is that hex" is asked from at
//! least three directions — the evaluator deciding where to drive, the
//! player's overlay asking why a tile is red, and anything that wants to
//! explain a decision after the fact — and every one of them must get the
//! resolver's own arithmetic rather than a model of it. The previous
//! arrangement is the argument for this one: the AI priced danger as
//! `expected_damage` against the hex she was *already* standing on, scaled
//! by `1/distance` to the tile it was actually deciding about, so cover,
//! elevation, facing and range never reached the decision at all.
//!
//! What is deliberately *not* here: doctrine weights, planner numbers,
//! distance falloffs, and any opinion about what a crew ought to do with
//! the answer. Those are judgment and belong to whoever is asking. This
//! module answers a question about the rules.

use super::{BattleState, UnitId, combat};
use crate::data::DataRegistry;
use hexx::Hex;

/// One enemy's best shot at a crew standing on a particular hex.
///
/// `hit_percent` and `expected` are the two halves a reader wants kept
/// apart: a near-certain scratch and an unlikely killing blow are both
/// "some expected damage", and a player owed an explanation is owed both
/// numbers. `expected` already has the hit chance in it — it is
/// [`combat::expected_damage`], the same currency the offense half of the
/// evaluator has always spent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bearing {
    /// Who could shoot.
    pub enemy: UnitId,
    /// Which of her weapons, as an index into her chassis' weapon list.
    pub weapon: usize,
    /// Her chance of hitting, as the resolver would roll it.
    pub hit_percent: i32,
    /// Expected damage in substance points: hit chance times what the round
    /// is worth against the plate it would strike.
    pub expected: f32,
}

/// Every spotted enemy that could put fire on `unit` if she stood at `at`,
/// with her best weapon for the job, in enemy id order.
///
/// Fog-honest: only enemies `unit`'s own side has found are listed, so a
/// planner reading this cannot flinch away from a tank nobody has seen and
/// thereby tell the player it is there. That makes the answer a statement
/// about the side's *picture* rather than about the board, which is the
/// only kind of statement an AI is allowed to act on.
///
/// Deterministic twice over: the enemies come in id order and each one's
/// weapon is chosen by [`combat::best_weapon_from`]'s first-wins tie-break,
/// so the list is a pure function of the state and may be summed, printed
/// or compared without sorting.
///
/// `at` is hypothetical; the enemies are where they really are. That is the
/// asymmetry the question has — she is deciding where to stand, and they
/// have already parked.
pub fn fire_on(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    at: Hex,
) -> Vec<Bearing> {
    let Some(me) = state.unit(unit) else {
        return Vec::new();
    };
    let mut bearings = Vec::new();
    for enemy in crate::ai::visible_enemies(state, me.side) {
        let Some((weapon, expected)) =
            combat::best_weapon_from(registry, state, enemy.id, enemy.pos, unit, at)
        else {
            continue;
        };
        // The gun is known to exist by the line above; asking the chassis
        // for it again only to name it would be a second lookup for a
        // number we would then have to keep in step.
        let hit_percent = registry
            .vehicle(&enemy.vehicle)
            .and_then(|v| v.weapons.get(weapon))
            .and_then(|w| registry.weapon(w))
            .map(|w| combat::hit_chance(registry, state, enemy.id, enemy.pos, w, unit, at, false))
            .unwrap_or(0);
        bearings.push(Bearing {
            enemy: enemy.id,
            weapon,
            hit_percent,
            expected,
        });
    }
    bearings
}
