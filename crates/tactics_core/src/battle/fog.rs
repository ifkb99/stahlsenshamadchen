//! Fog of war: per-side visibility, exploration memory, and unit spotting.
//!
//! Tiles have three knowledge states from a side's point of view:
//! - unseen: never observed (not in `explored`)
//! - explored: terrain remembered, current occupants unknown
//! - visible: currently observed (in `visible`)
//!
//! Enemy units are *spotted* while they stand on a visible tile, or while
//! `revealed` (they fired recently and haven't moved since).

use super::{BattleState, Event, Unit, UnitId, stats};
use crate::data::{DataRegistry, Scale};
use crate::map::HexMap;
use hexx::Hex;
use rand::RngExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// LoS runs in metres. Only the vertical unit matters: the sight line is
// interpolated by its position along the hex line, which is already the
// horizontal parameter, so how wide a hex is never enters the geometry.

// How tall one elevation digit is belongs to the scale contract and is
// therefore data: `Scale::elevation_meters`, read through
// [`crate::data::Scale::elevation`]. It used to be a `const ELEVATION_STEP =
// 10.0` here, which agreed with the mod only because both said ten — a mod
// that raised `elevation_meters` got a steeper climb and a skyline that had
// not moved.

/// How high an observer's eye and a target's silhouette sit above their own
/// tiles, in metres, read off `balance.eye_height_cm` / `target_height_cm`.
///
/// Resolved once and carried by [`SightGrid`] for the same reason the tile
/// heights are: `SightGrid::clear` has no registry to ask, and threading one
/// in would mean every line-of-sight query carried a lookup for an answer no
/// battle can change.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Eyes {
    /// Observer eye height above her own tile: a commander's cupola.
    eye: f32,
    /// How far above the target tile we must see to "see" her. Lower than
    /// `eye`, so looking out is fractionally easier than being looked at,
    /// which is what makes a reverse slope worth taking.
    target: f32,
}

impl Eyes {
    fn of(balance: &crate::data::Balance) -> Self {
        Self {
            eye: balance.eye_height_cm as f32 / 100.0,
            target: balance.target_height_cm as f32 / 100.0,
        }
    }
}

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
    /// The absolute tick at which this side last searched for each enemy it
    /// could see the ground of but had not yet picked out of it.
    ///
    /// A search is a die roll, and the fog is recomputed several times a tick
    /// — after movement, after fire, and again after every individual shot —
    /// so without a clock the chance of finding somebody would be a function
    /// of how much shooting happened to be going on around her. One roll per
    /// enemy per tick, whoever asks and however often.
    ///
    /// Entries are overwritten rather than cleaned up: there is one per enemy
    /// unit at most, and what it holds is meaningless the moment `now` moves
    /// past it. A `BTreeMap` for the same reason [`Self::spotted_since`] is —
    /// nothing iterates it today and nobody should be handed an
    /// iteration-order bug the day something does.
    #[serde(default)]
    pub searched: std::collections::BTreeMap<UnitId, u64>,
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
///
/// **Adding a fact to the grid is meant to be one field here and one line in
/// [`Heights::of`]**, and nothing else — the same bargain
/// `movement::TileMove` makes, and for the same reason: these two grids are
/// the shape a streamed world resolves terrain in, and the next per-tile fact
/// worth resolving once should cost a line rather than a redesign.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Heights {
    /// The ground an observer stands on or a target is measured from.
    surface: f32,
    /// The top of whatever blocks sight here — the ground plus the terrain's
    /// own `vision_block`, so a forest stands above the hill it grows on.
    obstacle: f32,
}

impl Heights {
    /// Resolve one tile against the registry. **The one place this module
    /// turns a tile into heights** — [`los_clear`] comes through here too, so
    /// the reference path and the cached one cannot disagree about what an
    /// elevation digit is worth any more than they can about a sight line.
    fn of(
        tile: &crate::map::Tile,
        terrain: Option<&crate::data::TerrainDef>,
        scale: &Scale,
    ) -> Self {
        let block = terrain.map(|t| t.vision_block).unwrap_or(0);
        Self {
            surface: scale.elevation(tile.elevation),
            obstacle: scale.elevation(tile.elevation + block),
        }
    }
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
///
/// # Built for a world that streams
///
/// The twin of [`crate::battle::MoveGrid`] in this too: tiles are folded in a
/// region at a time through [`Self::extend`] rather than resolved once for a
/// map that is the whole world, because the design has the overworld and the
/// battlefield converging into one continuous world at two zoom levels.
/// Deriving a whole 1261-tile battle map costs about 25 microseconds, which
/// is why these are built on load and never shipped as map assets: a grid
/// written into content would be a second copy of `vision_block`, and a mod
/// that retuned the terrain would silently get the old answer.
///
/// **The thing that will bite:** see [`sight_line_clear`], which treats a tile
/// it cannot find as transparent. That is exactly right while the map is the
/// whole world and a wrong answer the moment it is a window onto one.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SightGrid {
    /// The two observer heights, resolved with the tiles. `#[serde(skip)]`
    /// like the rest of the grid, and put back by [`crate::battle::SavedBattle::rehydrate`].
    #[serde(skip)]
    eyes: Option<Eyes>,
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
    /// Resolve a whole map.
    pub fn build(registry: &DataRegistry, map: &HexMap) -> Self {
        let mut grid = Self::default();
        grid.extend(registry, map);
        grid
    }

    /// Fold a region's tiles in, replacing anything already known about them.
    ///
    /// `build` is this on an empty grid, deliberately: the streaming path and
    /// the whole-map path are one piece of code, so the one used every day is
    /// the one keeping the other honest.
    pub fn extend(&mut self, registry: &DataRegistry, map: &HexMap) {
        for (hex, tile) in map.iter() {
            self.insert(registry, hex, tile);
        }
    }

    /// Resolve one tile.
    pub fn insert(&mut self, registry: &DataRegistry, hex: Hex, tile: &crate::map::Tile) {
        self.eyes = Some(Eyes::of(&registry.balance));
        self.tiles.insert(
            hex,
            Heights::of(tile, registry.terrain(&tile.terrain), &registry.scale),
        );
    }

    /// Whether this grid has been built. A deserialized battle carries an
    /// empty one until [`crate::save`] refills it.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// How many tiles are resolved. Uninteresting today and the thing a
    /// streamed world watches.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// Line of sight, using the precomputed heights. Identical in result to
    /// [`los_clear`] — they share [`sight_line_clear`] precisely so the fast
    /// path cannot drift away from the reference one.
    pub fn clear(&self, from: Hex, to: Hex) -> bool {
        let eyes = self.eyes.unwrap_or(Eyes {
            eye: 2.5,
            target: 2.0,
        });
        sight_line_clear(|hex| self.tiles.get(&hex).copied(), eyes, from, to)
    }
}

/// How far a residual may be from another before the two count as tied, when
/// deciding which hex a sight ray passes through.
///
/// Four orders of magnitude above `f32`'s resolution at these magnitudes and
/// four below a hex, so it catches every ray that genuinely runs along a hex
/// boundary and no ray that merely passes near one. It has to be far above
/// the noise: the whole point is that two arithmetically identical queries
/// which differ by an ulp must land on the *same* side of it.
const RAY_TIE: f32 = 1e-3;

/// The hexes a sight ray is passing through at one step, and why there can be
/// more than one.
///
/// Rounding a point on the ray to a hex is a nearest-centre question, and a
/// ray that runs exactly along the boundary between two hexes has no nearest
/// centre — the answer is a coin toss decided by the last bit of a float.
/// `hexx::Hex::round` has to make that choice, and it makes it consistently
/// **in absolute terms**, which is precisely the thing the invariants forbid:
/// the mirror image of a ray is a ray travelling the other way, its
/// arithmetic differs by an ulp, and the coin lands the other way up.
///
/// Measured before this existed, on the mirror-symmetric skill arena:
/// **1.27% of the 109,230 ordered hex pairs disagreed with their own mirror
/// image** about whether one could see the other, and side B won 46 of 72
/// equal battles at difficulty 5, where nothing else can break a tie.
///
/// So an ambiguous step resolves to *every* hex it could be in, and the
/// caller blocks if any of them blocks. That set is what a reflection
/// preserves — reflecting the ray reflects the whole set rather than picking
/// its other member — and so is reversing the ray, which is the second
/// symmetry sight ought to have and mostly did.
///
/// It is a small rule change as well as a symmetry fix, and in the honest
/// direction: a ray grazing the boundary of a wood is now stopped by the
/// wood. The alternative reading — clear if *any* candidate is clear — is
/// equally symmetric and says that a crew can see down the crack between two
/// blocks of timber, which is not a thing.
fn ray_hexes(from: Hex, to: Hex, i: usize, steps: f32) -> ([Hex; 3], usize) {
    let t = i as f32 / steps;
    let fx = from.x as f32 + (to.x - from.x) as f32 * t;
    let fy = from.y as f32 + (to.y - from.y) as f32 * t;
    // Cube coordinates sum to zero, so the third is not an independent number.
    // Deriving it rather than rounding it is what makes the three distances
    // below comparable, and cube coordinates are why a plain squared distance
    // is the right metric at all: they are a projection of 3-space onto the
    // plane `x + y + z == 0`, on which Euclidean distance is
    // `sqrt((dx² + dy² + dz²) / 2)` — and the constant does not survive a
    // comparison, so it is not computed.
    let fz = -fx - fy;
    let dist2 = |h: Hex| {
        let (dx, dy, dz) = (h.x as f32 - fx, h.y as f32 - fy, h.z() as f32 - fz);
        dx * dx + dy * dy + dz * dz
    };

    // Nearest centre, and everything tied with it. Asking the question this
    // way rather than by inspecting the rounding is what makes it *complete*:
    // a first attempt tested the residuals for a tie between the two
    // coordinates cube rounding chooses between, which halved the disagreement
    // and did not remove it, because `f32::round` breaks its own halfway cases
    // away from zero — a rule that is symmetric about the origin and not about
    // anywhere else, so a map whose centre is not the origin still leaked.
    let start = Hex::round([fx, fy]);

    // Almost every step of almost every ray is nowhere near a boundary, and
    // this is the hottest arithmetic in the simulation, so the ties are found
    // rather than searched for. Writing `d` for the offset from the point to
    // the nearest centre and `e` for a neighbour direction, the neighbour is
    // `2(d·e) + 2` further away, so it ties exactly when `d·e` is `-1`; the
    // six neighbour directions make `d·e` one of `±(dx - dy)`, `±(dx - dz)`,
    // `±(dy - dz)`. So the widest of those three differences reaching 1 *is*
    // the point being on a boundary, exactly, for three subtractions — and it
    // can never exceed 1, because `start` is the nearest centre.
    let [dx, dy, dz] = [
        start.x as f32 - fx,
        start.y as f32 - fy,
        start.z() as f32 - fz,
    ];
    let edge = (dx - dy).abs().max((dx - dz).abs()).max((dy - dz).abs());
    if edge < 1.0 - RAY_TIE {
        return ([start, Hex::ZERO, Hex::ZERO], 1);
    }

    // On a boundary, the true nearest is `start` or one of its six
    // neighbours, so seven distances settle it — and every one of them is a
    // quantity a reflection preserves.
    let mut best = dist2(start);
    for hex in start.all_neighbors() {
        best = best.min(dist2(hex));
    }
    let mut out = [Hex::ZERO; 3];
    let mut n = 0;
    for hex in std::iter::once(start).chain(start.all_neighbors()) {
        if dist2(hex) - best > RAY_TIE || out[..n].contains(&hex) {
            continue;
        }
        out[n] = hex;
        n += 1;
        // Three is the geometric maximum — a point equidistant from four hex
        // centres does not exist — and stopping is cheaper than proving it
        // again every step.
        if n == out.len() {
            break;
        }
    }
    (out, n)
}

/// Terrain-aware line of sight with elevation: the sight line from the
/// observer's eye to the target point must clear every intermediate tile's
/// obstacle height. Higher ground therefore sees over lower obstacles.
///
/// `heights` returns `None` for a tile that is not on the map.
fn sight_line_clear(
    heights: impl Fn(Hex) -> Option<Heights>,
    eyes: Eyes,
    from: Hex,
    to: Hex,
) -> bool {
    if from == to {
        return true;
    }
    let (Some(from_h), Some(to_h)) = (heights(from), heights(to)) else {
        return false;
    };
    let eye = from_h.surface + eyes.eye;
    let target = to_h.surface + eyes.target;

    // Walked by index rather than through `Hex::line_to`, and that is the
    // symmetry fix rather than a micro-optimisation: `line_to` resolves a ray
    // running along a hex boundary to one of the two hexes by an absolute
    // rule, so the mirror image of a sight query gets the other one. See
    // [`ray_hexes`], which returns every hex the step could be in.
    //
    // Still no `Vec`: the line is as long as the sight range, so a scout with
    // 2 km of vision runs this over a thousand times per look.
    let steps = from.distance_to(to) as usize;
    let last = steps as f32;
    for i in 1..steps {
        let (candidates, n) = ray_hexes(from, to, i, last);
        for hex in &candidates[..n] {
            let Some(h) = heights(*hex) else {
                // A tile nobody knows about does not block sight.
                //
                // Correct while the map is the whole world — "not on the map"
                // then means there is genuinely nothing there — and a **wrong
                // answer the day the map is a window onto a streamed one**, where
                // it would mean "not loaded". A ridge in an unloaded chunk would
                // stop blocking, so whether a crew can see a tank would depend on
                // what the renderer happened to have paged in, which is the one
                // kind of non-determinism this engine cannot survive: replays,
                // the search AI and the committed event stream all rest on it.
                //
                // The fix is not here. It is the loading rule: the resolved
                // region has to cover everything any unit can see or shoot, not
                // merely where the units are standing. See TODO under hex
                // streaming.
                continue;
            };
            let ray = eye + (target - eye) * (i as f32 / last);
            if h.obstacle >= ray {
                return false;
            }
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
            map.get(hex)
                .map(|tile| Heights::of(tile, registry.terrain(&tile.terrain), &registry.scale))
        },
        Eyes::of(&registry.balance),
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

/// What it takes for one side to find one enemy this tick.
enum Search {
    /// Found without a die: she fired and gave herself away, this side
    /// already has her, or the data gives a crew looking straight at her no
    /// reason to miss her. The last of those is what makes the whole
    /// detection rule additive — a mod that declares nothing never rolls.
    Found,
    /// One roll in 100, against the best-placed crew who can see her ground.
    /// Zero means nobody can see it, or that nobody who can has any chance at
    /// all.
    ///
    /// The best crew rather than a roll each, and this was the second draft.
    /// A roll per spotter reads well — more eyes, more chances — but it makes
    /// the printed chance a lie by however many crews happen to be looking:
    /// at four spotters a nominal 2% is 8%, so the whole usable range of the
    /// knob collapsed into single digits and the instrument's "ticks to find"
    /// column was wrong by a factor nobody could see. More eyes are still
    /// worth having, through the thing they actually buy — more *ground*
    /// watched, and someone closer to it — rather than through multiplying
    /// dice against the one crew who was already the likeliest to find her.
    Best(i32),
}

/// What each of `side`'s crews would have to roll to pick `unit` out of the
/// ground she is standing on.
///
/// `key` is that side's `(unit, pos, range)` list, built by walking
/// `state.units` in id order — which is what makes the list of rolls, and so
/// the rng draws that consume it, reproducible.
///
/// Two different concealments meet here and they do different jobs.
/// [`crate::data::VehicleDef::concealment`] is hers, and it shortens the
/// range at which she can be found at all — a platoon lying in a ditch is
/// hard to see from anywhere rather than hard to see from one direction, and
/// standing in real cover counts it twice. The terrain's is the ground's, and
/// it buys her *time* inside whatever range is left. Firing bypasses both,
/// upstream: an ambush is spent by springing it.
fn search(
    registry: &DataRegistry,
    state: &BattleState,
    key: &[(UnitId, Hex, u32)],
    unit: &Unit,
) -> Search {
    let concealment = registry
        .vehicle(&unit.vehicle)
        .map(|v| v.concealment)
        .unwrap_or(0);
    let terrain = state
        .map
        .get(unit.pos)
        .and_then(|t| registry.terrain(&t.terrain));
    let covered = terrain.is_some_and(|t| t.cover >= 30);
    let hidden = if covered {
        concealment.saturating_mul(2).min(95)
    } else {
        concealment.min(95)
    };
    let ground = terrain.map(|t| t.concealment).unwrap_or(0);

    // The fast path, and the reason a mod that declares no detection rules
    // pays nothing for this pass existing. A crew hiding behind no chassis
    // concealment (`hidden` zero) is inside somebody's reach precisely when
    // her hex is in the side's `visible` union — which the caller has already
    // checked — so if the numbers make the look certain there is nothing left
    // to establish. It is asked at the *worst* distance, one hex of a one-hex
    // reach, so a `true` here means no distance could have made it uncertain.
    //
    // It matters because this runs for every enemy of every side on every
    // recompute, and a recompute happens after movement, after fire and after
    // every individual shot.
    let at_the_far_edge = registry.balance.detection_chance(ground, 1, 1, unit.moved);
    if hidden == 0 && at_the_far_edge >= 100 {
        return Search::Found;
    }

    let mut best = 0;
    for (spotter, at, range) in key {
        let effective = range * (100 - hidden) / 100;
        let distance = at.distance_to(unit.pos);
        if distance > effective as i32 {
            continue;
        }
        // Her own reach, not the concealment-shortened one. The two terms
        // answer different questions and compounding them was measurably
        // wrong: `VehicleDef::concealment` has already priced how close
        // somebody has to be to find *her* at all, by cutting `effective`
        // down, while `detection_at_range_percent` is about how much detail
        // this crew resolves at a distance — a fact about her eyes and the
        // air. Feeding the shortened reach in made a platoon at three hexes
        // sit at 75% of "reach" and take the full far-band penalty on top of
        // the shortening, which is the same fact charged twice.
        let chance = registry
            .balance
            .detection_chance(ground, distance, *range, unit.moved);
        // The arithmetic before the set lookup, deliberately. `best` is a
        // maximum, so a crew who cannot improve on it need not be asked
        // whether she can see the hex at all — and that question is a hash
        // probe into a two-thousand-hex field of view, which is the only
        // expensive thing in here.
        if chance <= best {
            continue;
        }
        if !state
            .fog
            .cached(*spotter, *at, *range)
            .is_some_and(|tiles| tiles.contains(&unit.pos))
        {
            continue;
        }
        if chance >= 100 {
            return Search::Found;
        }
        best = chance;
    }
    Search::Best(best)
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

        // The absolute tick "now": what the reaction clocks are stamped
        // with, and the clock a search is rate-limited against. During
        // planning the round has not begun moving, so a spot made then is
        // stamped at the round's first tick — a target visible while orders
        // were being written is not a surprise when they execute.
        let now = state.round as u64 * registry.scale.ticks_per_round as u64
            + state.resolving_tick().unwrap_or(0) as u64;

        // Units are visited in id order and events pushed in that order, so
        // the same seed always yields the same event stream. Iterating the
        // spotted set instead would not: a HashSet's order varies run to
        // run, and a replay cannot survive that. It only stayed hidden while
        // vision was short enough that two enemies rarely appeared at once.
        //
        // The pass is in two halves and the seam is the rng: a die cannot be
        // thrown while a borrow of `state.fog` is alive. Only the *uncertain*
        // looks cross that seam, which is what keeps the common case free —
        // a `Vec` that is never pushed to never allocates, and with nothing
        // left to decide nothing is ever pushed.
        let fog = state.fog.side(side);
        let mut spotted = HashSet::new();
        let mut spotted_now = Vec::new();
        let mut uncertain: Vec<(UnitId, Hex, i32)> = Vec::new();
        for unit in state
            .units
            .iter()
            .filter(|u| u.alive && u.side != side && u.aboard.is_none())
        {
            let revealed = fog.revealed.contains(&unit.id);
            if !(fog.visible.contains(&unit.pos) || revealed) {
                continue;
            }
            // A crew who fired is found outright, and so is one this side
            // already has in its sights. A search is what it costs to
            // *acquire* a contact, never what it costs to keep watching one:
            // without that second clause every found enemy would be rolled
            // for again every tick and flicker in and out of the picture.
            if revealed || fog.spotted.contains(&unit.id) {
                spotted.insert(unit.id);
                if !fog.spotted.contains(&unit.id) {
                    spotted_now.push((unit.id, unit.pos));
                }
                continue;
            }
            // Already looked for this tick. The fog is recomputed after
            // movement, after fire and after every individual shot, and the
            // inputs to a search do not change in between — so the other
            // passes of a tick neither price the look again nor throw a
            // second die at it.
            if fog.searched.get(&unit.id) == Some(&now) {
                continue;
            }
            match search(registry, state, &key, unit) {
                Search::Found => {
                    spotted.insert(unit.id);
                    spotted_now.push((unit.id, unit.pos));
                }
                Search::Best(chance) => uncertain.push((unit.id, unit.pos, chance)),
            }
        }

        for (id, at, chance) in uncertain {
            // The look is spent whether or not there was anything worth
            // rolling for: a target must not be easier to find because a
            // firefight is going on somewhere else, and a chance of nothing
            // is a look that failed rather than a look nobody took. The
            // short circuit is what keeps a game with no detection rules
            // from drawing a die it does not need.
            state.fog.side_mut(side).searched.insert(id, now);
            if chance > 0 && state.rng.random_range(0..100) < chance {
                spotted.insert(id);
                spotted_now.push((id, at));
            }
        }
        // Back into id order. The certain looks were collected in it and the
        // uncertain ones were appended after, which is deterministic but is
        // not the order this module promises everywhere else — and the event
        // stream a replay reads comes straight off this list.
        spotted_now.sort_unstable_by_key(|(id, _)| id.index());

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
