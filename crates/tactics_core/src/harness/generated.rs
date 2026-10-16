//! Battlefields the world generator makes, for training on ground nobody has
//! seen twice (REIMAGINED.md L1.1).
//!
//! Every number the learned chooser had earned was on one ridge, so nobody
//! could say whether it had learned to fight or learned the ridge. A seed
//! here is a world (`worldgen`, at a small radius so it is quick to make), and
//! the battlefield is its centre chunk — which is exactly one battle map,
//! radius 20, with the relief, woods, hedges, streams, rivers and villages
//! the campaign stands on. The forces and their deployment pattern are an
//! [`Arena`]'s, moved onto this ground, so a battle here differs from the
//! arena's battle by the ground and nothing else.
//!
//! The ground is **not** mirrored, and does not need to be: a rollout fights
//! each field twice with the learner in each seat, so whatever one end is
//! worth cancels across the pair, which is how the `ground` table already
//! reads the hand-drawn maps.
//!
//! What is worth fighting for is read off the ground rather than declared:
//! the highest ground near the centre, worth 3, and the highest ground on
//! each flank, worth 2 each — the ridge arena's crest and spurs, found
//! instead of drawn. Ties go to the nearer the centre, then to the
//! coordinate, which on ground that is not mirrored decides nothing a
//! reflection could see.

use super::arena::Arena;
use crate::Hex;
use crate::data::{DataRegistry, MovementClass};
use crate::map::{Battlefield, HexMap, Objective, ObjectiveKind, Scenario};
use crate::world::chunk_centre;
use crate::worldgen::GeneratedWorld;

/// The world's radius in campaign hexes: enough that its centre chunk has
/// neighbours on every side for rivers and towns to come from, small enough
/// to make in a few milliseconds.
const WORLD_RADIUS: u32 = 2;

/// How far from the centre the crest is looked for, in hexes.
const CREST_WITHIN: u32 = 4;

/// The score that wins outright: the ridge arena's.
const VICTORY_SCORE: u32 = 40;

/// A generated battlefield and where the two forces deploy on it.
pub struct GeneratedField {
    pub field: Battlefield,
    /// Pairs of (side 0's hex, side 1's hex), in the arena's order.
    pub deployment: Vec<(Hex, Hex)>,
}

/// How the arena's deployment is laid on the ground: how far apart, and
/// along which axis. The default is the arena's own, unchanged.
///
/// The question it exists for (REIMAGINED.md, *Flanks*): the arena puts the
/// two forces 18 hexes apart, inside the heavy gun's reach and one round's
/// drive from the 75's, so a battle is decided by the shooting before a
/// manoeuvre could be worth anything. Spreading them across the map gives
/// the approach a phase of its own; turning the axis stops every battle
/// being fought along the same line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    /// The deployment's distance from the centre, in percent of the arena's.
    pub spread_percent: u32,
    /// Turn the axis by a sixth of a circle a step, chosen per world.
    pub turned: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            spread_percent: 100,
            turned: false,
        }
    }
}

impl Layout {
    /// Where `want` (an arena deployment hex) lands under this layout, on
    /// the world `seed` makes. The turn is drawn from the seed, so both
    /// seatings of a world share it.
    fn lay(&self, centre: Hex, want: Hex, seed: u64) -> Hex {
        let d = want - centre;
        let scale = |v: i32| (v * self.spread_percent as i32 + 50).div_euclid(100);
        let spread = Hex::new(scale(d.x), scale(d.y));
        let turns = if self.turned {
            ((seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 32) % 6) as u32
        } else {
            0
        };
        centre + spread.rotate_cw(turns)
    }
}

/// The battlefield `seed` makes, with `arena`'s deployment moved onto it.
/// `None` when the registry has no world generator, or the world cannot be
/// made.
pub fn field(reg: &DataRegistry, arena: &Arena, seed: u64) -> Option<GeneratedField> {
    field_laid_out(reg, arena, seed, Layout::default())
}

/// [`field`], with the deployment laid out by `layout`.
pub fn field_laid_out(
    reg: &DataRegistry,
    arena: &Arena,
    seed: u64,
    layout: Layout,
) -> Option<GeneratedField> {
    let rules = crate::data::WorldGen {
        radius: WORLD_RADIUS,
        ..reg.worldgen.clone()?
    };
    let chunk_radius = reg.scale.battle_map_radius();
    let world = GeneratedWorld::with_rules(rules, chunk_radius, seed).ok()?;
    // The chunk's tiles, moved so its centre is the arena's centre: the
    // arena's deployment then lands on this ground unchanged.
    let shift = arena.centre_hex() - chunk_centre(Hex::ZERO, chunk_radius);
    let mut terrain = HexMap::default();
    let mut tiles: Vec<(Hex, String, i32)> = Vec::new();
    for (hex, id, level) in world.chunk_tiles(Hex::ZERO) {
        terrain.insert(hex + shift, id, level);
        tiles.push((hex + shift, id.to_string(), level));
    }
    let centre = arena.centre_hex();
    // Looked up, never iterated, so the map's order reaches nothing.
    let index: std::collections::HashMap<Hex, (&str, i32)> = tiles
        .iter()
        .map(|(h, id, level)| (*h, (id.as_str(), *level)))
        .collect();
    let level_at = |hex: Hex| index.get(&hex).copied();
    let wants = arena.deployment().into_iter().map(|(west, east)| {
        let lay = |want: Hex| layout.lay(centre, want, seed);
        (lay(west), lay(east))
    });
    let deployment = deploy(reg, &level_at, centre, wants)?;
    let candidates: Vec<Hex> = tiles.iter().map(|(h, ..)| *h).collect();
    let scenario = scenario_between(
        reg,
        &level_at,
        &candidates,
        centre,
        &deployment,
        CREST_WITHIN,
        &|h| (h.distance_to(centre) as u32) <= arena.radius,
    )?;
    Some(GeneratedField {
        field: Battlefield { terrain, scenario },
        deployment,
    })
}

/// Whether a tank can stand on `id`.
fn tracked(reg: &DataRegistry, id: &str) -> bool {
    reg.terrain(id)
        .and_then(|t| t.cost_for(MovementClass::Tracked))
        .is_some()
}

/// Each pair of `wants` (side 0's hex, side 1's) moved onto the nearest
/// ground a tank can stand on and nobody has taken, nearest `centre` first
/// and the coordinate last. `None` when a pair finds nothing within five
/// hexes. Shared with [`super::open`], so both read deployment the same way.
pub(crate) fn deploy<'a>(
    reg: &DataRegistry,
    ground: &dyn Fn(Hex) -> Option<(&'a str, i32)>,
    centre: Hex,
    wants: impl IntoIterator<Item = (Hex, Hex)>,
) -> Option<Vec<(Hex, Hex)>> {
    let mut taken: Vec<Hex> = Vec::new();
    let mut place = |want: Hex| -> Option<Hex> {
        for radius in 0..6u32 {
            let mut ring: Vec<Hex> = if radius == 0 {
                vec![want]
            } else {
                want.ring(radius).collect()
            };
            ring.sort_by_key(|h| (h.distance_to(centre), h.x, h.y));
            if let Some(hex) = ring
                .into_iter()
                .find(|h| !taken.contains(h) && ground(*h).is_some_and(|(id, _)| tracked(reg, id)))
            {
                taken.push(hex);
                return Some(hex);
            }
        }
        None
    };
    let mut deployment = Vec::new();
    for (west, east) in wants {
        deployment.push((place(west)?, place(east)?));
    }
    Some(deployment)
}

/// The ground worth having between two deployed forces, read off the ground
/// rather than declared: the highest within `crest_within` of `centre`,
/// worth 3, then the highest on each side of the line the two forces face
/// each other along, worth 2 each, at least four hexes from the crest and
/// wherever `flank_within` allows. Only `candidates` are looked at. Ties go
/// to the nearer the centre, then to the coordinate, which on ground that
/// is not mirrored decides nothing a reflection could see.
pub(crate) fn scenario_between<'a>(
    reg: &DataRegistry,
    ground: &dyn Fn(Hex) -> Option<(&'a str, i32)>,
    candidates: &[Hex],
    centre: Hex,
    deployment: &[(Hex, Hex)],
    crest_within: u32,
    flank_within: &dyn Fn(Hex) -> bool,
) -> Option<Scenario> {
    let mean = |side: fn(&(Hex, Hex)) -> Hex| -> Hex {
        let n = deployment.len().max(1) as i32;
        let sum = deployment.iter().map(side).fold(Hex::ZERO, |a, b| a + b);
        Hex::new(sum.x / n, sum.y / n)
    };
    let axis = mean(|p| p.1) - mean(|p| p.0);
    let across = |hex: Hex| {
        // The sign of the cross product: which side of the axis `hex` is on.
        let d = hex - centre;
        axis.x * d.y - axis.y * d.x
    };
    let highest = |pick: &dyn Fn(Hex) -> bool| -> Option<Hex> {
        candidates
            .iter()
            .filter_map(|h| ground(*h).map(|(id, level)| (*h, id, level)))
            .filter(|(h, id, _)| pick(*h) && tracked(reg, id))
            .max_by_key(|(h, _, level)| {
                (
                    *level,
                    std::cmp::Reverse(h.distance_to(centre)),
                    std::cmp::Reverse((h.x, h.y)),
                )
            })
            .map(|(h, ..)| h)
    };
    let crest = highest(&|h| h.distance_to(centre) as u32 <= crest_within)?;
    let flank = |sign: i32| {
        highest(&|h| across(h).signum() == sign && h.distance_to(crest) >= 4 && flank_within(h))
    };
    let objective = |id: &str, name: &str, at: Hex, value: u32| Objective {
        id: id.into(),
        name: name.into(),
        hexes: std::iter::once(at)
            .chain(
                at.all_neighbors()
                    .into_iter()
                    .filter(|h| ground(*h).is_some_and(|(id, _)| tracked(reg, id))),
            )
            .collect(),
        value,
        kind: ObjectiveKind::Hold,
        side: None,
    };
    let mut scenario = Scenario::default()
        .with_victory_score(VICTORY_SCORE)
        .with_objective(objective("crest", "the crest", crest, 3));
    if let Some(at) = flank(1) {
        scenario.push_objective(objective("left", "the left spur", at, 2));
    }
    if let Some(at) = flank(-1) {
        scenario.push_objective(objective("right", "the right spur", at, 2));
    }
    Some(scenario)
}
