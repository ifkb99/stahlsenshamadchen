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

/// The battlefield `seed` makes, with `arena`'s deployment moved onto it.
/// `None` when the registry has no world generator, or the world cannot be
/// made.
pub fn field(reg: &DataRegistry, arena: &Arena, seed: u64) -> Option<GeneratedField> {
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
    let tracked = |id: &str| {
        reg.terrain(id)
            .and_then(|t| t.cost_for(MovementClass::Tracked))
            .is_some()
    };
    let level_at = |hex: Hex| {
        tiles
            .iter()
            .find(|(h, ..)| *h == hex)
            .map(|(_, id, level)| (id.as_str(), *level))
    };

    // Deploy each pair on the nearest ground a tank can stand on.
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
                .find(|h| !taken.contains(h) && level_at(*h).is_some_and(|(id, _)| tracked(id)))
            {
                taken.push(hex);
                return Some(hex);
            }
        }
        None
    };
    let mut deployment = Vec::new();
    for (west, east) in arena.deployment() {
        deployment.push((place(west)?, place(east)?));
    }

    // The ground worth having: the highest within reach of the centre, then
    // the highest on each flank (either side of the line the two forces face
    // each other along).
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
        tiles
            .iter()
            .filter(|(h, id, _)| pick(*h) && tracked(id))
            .max_by_key(|(h, _, level)| {
                (
                    *level,
                    std::cmp::Reverse(h.distance_to(centre)),
                    std::cmp::Reverse((h.x, h.y)),
                )
            })
            .map(|(h, ..)| *h)
    };
    let crest = highest(&|h| h.distance_to(centre) as u32 <= CREST_WITHIN)?;
    let flank = |sign: i32| {
        highest(&|h| {
            across(h).signum() == sign
                && h.distance_to(crest) >= 4
                && (h.distance_to(centre) as u32) <= arena.radius
        })
    };
    let objective = |id: &str, name: &str, at: Hex, value: u32| Objective {
        id: id.into(),
        name: name.into(),
        hexes: std::iter::once(at)
            .chain(
                at.all_neighbors()
                    .into_iter()
                    .filter(|h| level_at(*h).is_some_and(|(id, _)| tracked(id))),
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
    Some(GeneratedField {
        field: Battlefield { terrain, scenario },
        deployment,
    })
}
