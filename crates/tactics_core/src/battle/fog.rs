//! Fog of war: per-side visibility, exploration memory, and unit spotting.
//!
//! Tiles have three knowledge states from a side's point of view:
//! - unseen: never observed (not in `explored`)
//! - explored: terrain remembered, current occupants unknown
//! - visible: currently observed (in `visible`)
//!
//! Enemy units are *spotted* while they stand on a visible tile, or while
//! `revealed` (they fired recently and haven't moved since).

use super::{stats, BattleState, Event, UnitId};
use crate::data::DataRegistry;
use crate::map::HexMap;
use hexx::Hex;
use std::collections::HashSet;

/// Height of one elevation step, in the abstract units LoS math runs in.
const ELEVATION_STEP: f32 = 2.0;
/// Observer eye height above their own tile surface.
const EYE_HEIGHT: f32 = 1.5;
/// How far above the target tile surface we must see to "see" the target.
const TARGET_HEIGHT: f32 = 1.0;

/// What one side knows about the battlefield.
#[derive(Debug, Clone, Default)]
pub struct SideFog {
    /// Tiles currently observed by this side's units.
    pub visible: HashSet<Hex>,
    /// Every tile ever observed (terrain memory).
    pub explored: HashSet<Hex>,
    /// Enemy units currently spotted.
    pub spotted: HashSet<UnitId>,
    /// Enemy units that gave away their position by firing. Cleared for a
    /// unit when it moves.
    pub revealed: HashSet<UnitId>,
}

#[derive(Debug, Clone, Default)]
pub struct FogMap {
    sides: Vec<SideFog>,
}

impl FogMap {
    pub fn new(side_count: usize) -> Self {
        Self {
            sides: vec![SideFog::default(); side_count],
        }
    }

    pub fn side(&self, side: u8) -> &SideFog {
        &self.sides[side as usize]
    }

    pub fn side_mut(&mut self, side: u8) -> &mut SideFog {
        &mut self.sides[side as usize]
    }
}

/// Terrain-aware line of sight with elevation: the sight line from the
/// observer's eye to the target point must clear every intermediate tile's
/// obstacle height (tile elevation plus terrain `vision_block`). Higher
/// ground therefore sees over lower obstacles.
pub fn los_clear(registry: &DataRegistry, map: &HexMap, from: Hex, to: Hex) -> bool {
    if from == to {
        return true;
    }
    let (Some(from_tile), Some(to_tile)) = (map.get(from), map.get(to)) else {
        return false;
    };
    let eye = from_tile.elevation as f32 * ELEVATION_STEP + EYE_HEIGHT;
    let target = to_tile.elevation as f32 * ELEVATION_STEP + TARGET_HEIGHT;

    let line: Vec<Hex> = from.line_to(to).collect();
    let last = (line.len() - 1) as f32;
    for (i, hex) in line.iter().enumerate() {
        if i == 0 || i == line.len() - 1 {
            continue;
        }
        let Some(tile) = map.get(*hex) else {
            // Off-map gaps don't block sight.
            continue;
        };
        let block = registry
            .terrain(&tile.terrain)
            .map(|t| t.vision_block)
            .unwrap_or(0);
        let obstacle = (tile.elevation + block) as f32 * ELEVATION_STEP;
        let ray = eye + (target - eye) * (i as f32 / last);
        if obstacle >= ray {
            return false;
        }
    }
    true
}

/// Every tile a single unit can currently see.
pub fn unit_vision(registry: &DataRegistry, state: &BattleState, id: UnitId) -> HashSet<Hex> {
    let Some(unit) = state.unit(id) else {
        return HashSet::new();
    };
    let range = stats::vision_range(registry, unit);
    unit.pos
        .range(range)
        .filter(|hex| state.map.contains(*hex) && los_clear(registry, &state.map, unit.pos, *hex))
        .collect()
}

/// Recompute all sides' fog from scratch. Returns `UnitSpotted` events for
/// enemies that just became visible.
pub fn recompute(registry: &DataRegistry, state: &mut BattleState) -> Vec<Event> {
    let mut events = Vec::new();
    for side in 0..state.sides.len() as u8 {
        let mut visible = HashSet::new();
        let unit_ids: Vec<UnitId> = state.side_units(side).map(|u| u.id).collect();
        for id in unit_ids {
            visible.extend(unit_vision(registry, state, id));
        }

        let previously_spotted = state.fog.side(side).spotted.clone();
        let revealed = state.fog.side(side).revealed.clone();
        let spotted: HashSet<UnitId> = state
            .alive_units()
            .filter(|u| u.side != side)
            .filter(|u| visible.contains(&u.pos) || revealed.contains(&u.id))
            .map(|u| u.id)
            .collect();

        for &id in &spotted {
            if !previously_spotted.contains(&id) {
                if let Some(unit) = state.unit(id) {
                    events.push(Event::UnitSpotted {
                        unit: id,
                        by_side: side,
                        at: unit.pos,
                    });
                }
            }
        }

        let fog = state.fog.side_mut(side);
        fog.explored.extend(visible.iter().copied());
        fog.visible = visible;
        fog.spotted = spotted;
    }
    events
}

/// A unit fired: everyone now knows where it is until it moves.
pub fn reveal_to_all(state: &mut BattleState, unit: UnitId) {
    let Some(side) = state.unit(unit).map(|u| u.side) else {
        return;
    };
    for other in 0..state.sides.len() as u8 {
        if other != side {
            state.fog.side_mut(other).revealed.insert(unit);
        }
    }
}

/// A unit moved: it is no longer revealed by its old muzzle flashes.
pub fn clear_reveal(state: &mut BattleState, unit: UnitId) {
    for side in 0..state.sides.len() as u8 {
        state.fog.side_mut(side).revealed.remove(&unit);
    }
}
