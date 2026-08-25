//! Fog of war: per-side visibility, exploration memory, and unit spotting.
//!
//! Tiles have three knowledge states from a side's point of view:
//! - unseen: never observed (not in `explored`)
//! - explored: terrain remembered, current occupants unknown
//! - visible: currently observed (in `visible`)
//!
//! Enemy units are *spotted* while they stand on a visible tile, or while
//! `revealed` (they fired recently and haven't moved since).

use super::{BattleState, Event, UnitId, stats};
use crate::data::DataRegistry;
use crate::map::HexMap;
use hexx::Hex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// LoS runs in metres. Only the vertical unit matters: the sight line is
// interpolated by its position along the hex line, which is already the
// horizontal parameter, so how wide a hex is never enters the geometry.

/// Height of one elevation level, in metres. One map elevation digit.
const ELEVATION_STEP: f32 = 10.0;
/// Observer eye height above their own tile surface: a commander's cupola.
const EYE_HEIGHT: f32 = 2.5;
/// How far above the target tile surface we must see to "see" the target.
/// Lower than [`EYE_HEIGHT`], so looking out is fractionally easier than
/// being looked at — which is what makes a reverse slope worth taking.
const TARGET_HEIGHT: f32 = 2.0;

/// What one side knows about the battlefield.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SideFog {
    /// Tiles currently observed by this side's units.
    pub visible: HashSet<Hex>,
    /// Every tile ever observed (terrain memory).
    pub explored: HashSet<Hex>,
    /// Enemy units currently spotted.
    pub spotted: HashSet<UnitId>,
    /// When each currently spotted enemy was FIRST seen, as an absolute tick
    /// (`round * ticks_per_round + tick`), kept for as long as the spot holds
    /// and dropped the moment it lapses.
    ///
    /// This is what makes reaction time a response to *new information*
    /// rather than a tax on watching: the crew's clock for an enemy starts
    /// when he appears, not when the round does. Before this existed the
    /// opportunity-fire gate reset every round — a target watched for five
    /// rounds was paid for five times, and an ambush sprung at tick seven
    /// was answered instantly because seven beats any delay. Both are
    /// exactly backwards, and the post-mortem in cadets.md is emphatic about
    /// which way the model goes: she notices at tick four and acts at tick
    /// six.
    ///
    /// A `BTreeMap`, never a hash map: it is iterated nowhere today, but the
    /// first person to iterate it must not be handed an iteration-order bug.
    #[serde(default)]
    pub spotted_since: std::collections::BTreeMap<UnitId, u64>,
    /// Enemy units that gave away their position by firing. Cleared for a
    /// unit when it moves.
    pub revealed: HashSet<UnitId>,
}

/// One unit's vision, and the inputs it was computed from.
///
/// Vision is a pure function of where a unit stands, how far it can see, and
/// the map — and the map is immutable for the whole battle. So a cache keyed
/// on position and range is not an approximation of the real answer, it *is*
/// the real answer, which is what lets this be an optimisation with no
/// behavioural cost at all.
#[derive(Debug, Clone)]
struct VisionCache {
    pos: Hex,
    range: u32,
    /// `Arc` because search planners clone `BattleState` constantly and a
    /// scout's vision is two thousand hexes; branching should be a refcount
    /// bump, not a deep copy.
    tiles: Arc<HashSet<Hex>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FogMap {
    sides: Vec<SideFog>,
    /// Per-unit vision, indexed by unit id.
    ///
    /// Not saved: it is a pure function of the map and each unit's position
    /// and range, so it rebuilds itself on the first recompute after a load.
    /// Writing it out would bloat a save with thousands of hexes that carry no
    /// information.
    #[serde(skip)]
    vision: Vec<Option<VisionCache>>,
    /// The `(unit, pos, range)` list that produced each side's `visible` set.
    /// A side's visible set is a pure function of this list, so when the list
    /// is unchanged the union can be skipped outright — which is what makes
    /// the recompute after every shot nearly free, since firing moves nobody.
    ///
    /// Also not saved, for the same reason. Coming back empty costs one extra
    /// recompute after a load, which is the correct answer anyway.
    #[serde(skip)]
    visible_key: Vec<Vec<(UnitId, Hex, u32)>>,
}

impl FogMap {
    pub fn new(side_count: usize) -> Self {
        Self {
            sides: vec![SideFog::default(); side_count],
            vision: Vec::new(),
            visible_key: vec![Vec::new(); side_count],
        }
    }

    pub fn side(&self, side: u8) -> &SideFog {
        &self.sides[side as usize]
    }

    pub fn side_mut(&mut self, side: u8) -> &mut SideFog {
        &mut self.sides[side as usize]
    }

    /// Put back the caches a save leaves out.
    ///
    /// `vision` and `visible_key` are `#[serde(skip)]`, so a deserialized fog
    /// map carries empty ones — and `visible_key` is *indexed* by side during
    /// recompute, so an empty one panics rather than merely recomputing. The
    /// sizes cannot come from serde because they depend on the side count.
    pub fn rehydrate(&mut self) {
        self.vision.clear();
        self.visible_key = vec![Vec::new(); self.sides.len()];
    }

    /// This unit's cached vision, if it was computed for exactly this
    /// position and range.
    fn cached(&self, id: UnitId, pos: Hex, range: u32) -> Option<&Arc<HashSet<Hex>>> {
        match self.vision.get(id.index()) {
            Some(Some(c)) if c.pos == pos && c.range == range => Some(&c.tiles),
            _ => None,
        }
    }

    fn store(&mut self, id: UnitId, pos: Hex, range: u32, tiles: HashSet<Hex>) {
        if self.vision.len() <= id.index() {
            self.vision.resize(id.index() + 1, None);
        }
        self.vision[id.index()] = Some(VisionCache {
            pos,
            range,
            tiles: Arc::new(tiles),
        });
    }
}

/// The two heights line of sight needs from a tile, in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Heights {
    /// The ground an observer stands on or a target is measured from.
    surface: f32,
    /// The top of whatever blocks sight here — the ground plus the terrain's
    /// own `vision_block`, so a forest stands above the hill it grows on.
    obstacle: f32,
}

/// Every tile's sight heights, resolved once.
///
/// [`los_clear`] needs a terrain definition for every step of every ray, and
/// terrain is keyed by `String` in the registry. A scout with 2 km of vision
/// tests nearly two thousand tiles per look and each test walks up to twenty
/// steps, which put that string hash on the order of a million times per
/// round — for an answer that cannot change, because no battle alters its
/// own terrain.
///
/// Shared through [`BattleState`] behind an `Arc`, so cloning a state for
/// search branching does not copy it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SightGrid {
    /// Not saved: derived entirely from the map and the terrain definitions,
    /// so a loaded game rebuilds it rather than carrying a copy. It is the
    /// single biggest structure in a battle — one entry per tile — and it
    /// holds no state a player could have changed.
    ///
    /// The consequence is that [`crate::save`] *must* rebuild it after
    /// deserializing; an empty grid answers every sight question wrongly
    /// rather than loudly.
    #[serde(skip)]
    tiles: HashMap<Hex, Heights>,
}

impl SightGrid {
    /// Whether this grid has been built. A deserialized battle carries an
    /// empty one until [`crate::save`] refills it.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

impl SightGrid {
    pub fn build(registry: &DataRegistry, map: &HexMap) -> Self {
        let tiles = map
            .iter()
            .map(|(hex, tile)| {
                let block = registry
                    .terrain(&tile.terrain)
                    .map(|t| t.vision_block)
                    .unwrap_or(0);
                (
                    hex,
                    Heights {
                        surface: tile.elevation as f32 * ELEVATION_STEP,
                        obstacle: (tile.elevation + block) as f32 * ELEVATION_STEP,
                    },
                )
            })
            .collect();
        Self { tiles }
    }

    /// Line of sight, using the precomputed heights. Identical in result to
    /// [`los_clear`] — they share [`sight_line_clear`] precisely so the fast
    /// path cannot drift away from the reference one.
    pub fn clear(&self, from: Hex, to: Hex) -> bool {
        sight_line_clear(|hex| self.tiles.get(&hex).copied(), from, to)
    }
}

/// Terrain-aware line of sight with elevation: the sight line from the
/// observer's eye to the target point must clear every intermediate tile's
/// obstacle height. Higher ground therefore sees over lower obstacles.
///
/// `heights` returns `None` for a tile that is not on the map.
fn sight_line_clear(heights: impl Fn(Hex) -> Option<Heights>, from: Hex, to: Hex) -> bool {
    if from == to {
        return true;
    }
    let (Some(from_h), Some(to_h)) = (heights(from), heights(to)) else {
        return false;
    };
    let eye = from_h.surface + EYE_HEIGHT;
    let target = to_h.surface + TARGET_HEIGHT;

    // Walked as an iterator rather than collected. The line is as long as the
    // sight range, so a scout with 2 km of vision runs this over a thousand
    // times per look; there is no reason for each one to build a Vec first.
    let steps = from.distance_to(to) as usize;
    let last = steps as f32;
    for (i, hex) in from.line_to(to).enumerate() {
        if i == 0 || i == steps {
            continue;
        }
        let Some(h) = heights(hex) else {
            // Off-map gaps don't block sight.
            continue;
        };
        let ray = eye + (target - eye) * (i as f32 / last);
        if h.obstacle >= ray {
            return false;
        }
    }
    true
}

/// Terrain-aware line of sight, resolving terrain through the registry on
/// every step.
///
/// This is the reference implementation and the one to reach for in a test or
/// a one-off query. Anything inside the simulation loop should go through
/// [`BattleState::sight`] instead, which answers the same question without
/// the per-step lookups.
pub fn los_clear(registry: &DataRegistry, map: &HexMap, from: Hex, to: Hex) -> bool {
    sight_line_clear(
        |hex| {
            map.get(hex).map(|tile| {
                let block = registry
                    .terrain(&tile.terrain)
                    .map(|t| t.vision_block)
                    .unwrap_or(0);
                Heights {
                    surface: tile.elevation as f32 * ELEVATION_STEP,
                    obstacle: (tile.elevation + block) as f32 * ELEVATION_STEP,
                }
            })
        },
        from,
        to,
    )
}

/// Every tile visible from `pos` at `range`, ignoring any cache.
fn look(state: &BattleState, pos: Hex, range: u32) -> HashSet<Hex> {
    pos.range(range)
        .filter(|hex| state.map.contains(*hex) && state.sight.clear(pos, *hex))
        .collect()
}

/// Every tile a single unit can currently see.
pub fn unit_vision(registry: &DataRegistry, state: &BattleState, id: UnitId) -> HashSet<Hex> {
    let Some(unit) = state.unit(id) else {
        return HashSet::new();
    };
    look(
        state,
        unit.pos,
        stats::vision_range(registry, &state.roster, unit, state.terrain_at(unit.pos)),
    )
}

/// Whether `id` can currently see `target`, answered from the cache the last
/// recompute left warm.
///
/// This exists for the command picture, which asks "can this reporter see
/// that contact" for a handful of pairs every tick — the same question fog
/// just computed for every unit on the field. Recomputing a two-thousand-hex
/// field of view per pair took round resolution from ~1.1 ms to ~5.6 ms,
/// measured; reading the cache costs a key comparison. The cold path (a
/// cache that has not seen this position yet) falls back to the honest
/// computation, so the answer cannot depend on cache temperature.
pub(crate) fn sees(registry: &DataRegistry, state: &BattleState, id: UnitId, target: Hex) -> bool {
    let Some(unit) = state.unit(id) else {
        return false;
    };
    let range = stats::vision_range(registry, &state.roster, unit, state.terrain_at(unit.pos));
    if let Some(tiles) = state.fog.cached(id, unit.pos, range) {
        return tiles.contains(&target);
    }
    unit.pos.distance_to(target) <= range as i32
        && state.map.contains(target)
        && state.sight.clear(unit.pos, target)
}

/// Recompute all sides' fog. Returns `UnitSpotted` events for enemies that
/// just became visible.
///
/// Called after movement and after fire in every tick, and again after every
/// individual shot, so it has to be cheap when nothing has changed. Two
/// things make it so, and neither alters what the result is:
///
/// - a unit whose position and range are unchanged reuses its cached vision,
///   because the map cannot change under it;
/// - a side whose whole `(unit, pos, range)` list is unchanged skips the
///   union entirely.
///
/// Spotting is always re-evaluated, because `revealed` changes when a unit
/// fires without anybody having moved.
pub fn recompute(registry: &DataRegistry, state: &mut BattleState) -> Vec<Event> {
    let mut events = Vec::new();
    for side in 0..state.sides.len() as u8 {
        // Built by iterating `state.units` in id order, so the key — and the
        // event order that follows from it — never depends on hash ordering.
        let key: Vec<(UnitId, Hex, u32)> = state
            .side_units(side)
            // Passengers see nothing for their side: buttoned up in the
            // back of a carrier, her eyes are the carrier's problem.
            .filter(|u| u.aboard.is_none())
            .map(|u| {
                (
                    u.id,
                    u.pos,
                    stats::vision_range(registry, &state.roster, u, state.terrain_at(u.pos)),
                )
            })
            .collect();

        if state.fog.visible_key.get(side as usize) != Some(&key) {
            // Compute the misses first, against an immutable state, then
            // store them; the union afterwards reads only cache hits.
            let stale: Vec<(UnitId, Hex, u32)> = key
                .iter()
                .copied()
                .filter(|(id, pos, range)| state.fog.cached(*id, *pos, *range).is_none())
                .collect();
            let looked: Vec<(UnitId, Hex, u32, HashSet<Hex>)> = stale
                .into_iter()
                .map(|(id, pos, range)| (id, pos, range, look(state, pos, range)))
                .collect();
            for (id, pos, range, tiles) in looked {
                state.fog.store(id, pos, range, tiles);
            }

            let mut visible = HashSet::new();
            for (id, pos, range) in &key {
                if let Some(tiles) = state.fog.cached(*id, *pos, *range) {
                    visible.extend(tiles.iter().copied());
                }
            }
            let fog = state.fog.side_mut(side);
            fog.explored.extend(visible.iter().copied());
            fog.visible = visible;
            // Cloned rather than moved: the concealment pass below still
            // reads the spotter list.
            state.fog.visible_key[side as usize] = key.clone();
        }

        // Units are visited in id order and events pushed in that order, so
        // the same seed always yields the same event stream. Iterating the
        // spotted set instead would not: a HashSet's order varies run to
        // run, and a replay cannot survive that. It only stayed hidden while
        // vision was short enough that two enemies rarely appeared at once.
        let fog = state.fog.side(side);
        let mut spotted = HashSet::new();
        let mut spotted_now = Vec::new();
        for unit in state
            .units
            .iter()
            .filter(|u| u.alive && u.side != side && u.aboard.is_none())
        {
            let revealed = fog.revealed.contains(&unit.id);
            if !(fog.visible.contains(&unit.pos) || revealed) {
                continue;
            }
            // Concealment: standing on ground somebody can see is not the
            // same as being seen. A concealed unit on a visible tile is
            // spotted only by a spotter whose own sight reaches the tile
            // AND who stands inside her vision range scaled down by the
            // target's `concealment` — counted twice when the target is in
            // real cover, which is where a platoon in the treeline becomes
            // the ambush the design doc promises. Firing bypasses all of
            // it (`revealed`): an ambush is spent by springing it.
            // Vehicles default to concealment 0 and skip this entirely —
            // spotted exactly as they always were.
            if !revealed {
                let concealment = registry
                    .vehicle(&unit.vehicle)
                    .map(|v| v.concealment)
                    .unwrap_or(0);
                if concealment > 0 {
                    let covered = state
                        .map
                        .get(unit.pos)
                        .and_then(|t| registry.terrain(&t.terrain))
                        .is_some_and(|t| t.cover >= 30);
                    let hidden = if covered {
                        concealment.saturating_mul(2).min(95)
                    } else {
                        concealment.min(95)
                    };
                    let seen = key.iter().any(|(sid, spos, srange)| {
                        let effective = srange * (100 - hidden) / 100;
                        spos.distance_to(unit.pos) <= effective as i32
                            && state
                                .fog
                                .cached(*sid, *spos, *srange)
                                .is_some_and(|tiles| tiles.contains(&unit.pos))
                    });
                    if !seen {
                        continue;
                    }
                }
            }
            spotted.insert(unit.id);
            if !fog.spotted.contains(&unit.id) {
                spotted_now.push((unit.id, unit.pos));
            }
        }
        // The absolute tick "now": what the reaction clocks are stamped
        // with. During planning the round has not begun moving, so a spot
        // made then is stamped at the round's first tick — a target visible
        // while orders were being written is not a surprise when they
        // execute.
        let now = state.round as u64 * registry.scale.ticks_per_round as u64
            + state.resolving_tick().unwrap_or(0) as u64;
        for (unit, at) in spotted_now {
            events.push(Event::UnitSpotted {
                unit,
                by_side: side,
                at,
            });
        }
        let fog = state.fog.side_mut(side);
        fog.spotted_since.retain(|id, _| spotted.contains(id));
        for id in &spotted {
            fog.spotted_since.entry(*id).or_insert(now);
        }
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
