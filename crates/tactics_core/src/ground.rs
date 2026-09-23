//! Reading ground: what a commander sees when she looks at a map before
//! anybody has moved — where the high ground is, what cannot be seen from
//! where, which way she could go without being watched.
//!
//! The first layer of the planning design in `PLANNING.md`: a plan is a
//! template matched onto terrain, and this is the terrain half. Nothing in the
//! AI reads it yet.
//!
//! **Built for a world that does not end at the map edge.** The design has
//! battle tiles flowing into one continuous world, so nothing here may assume a
//! small, closed, fully known map:
//!
//! - The reader asks the world four questions through [`Ground`] and nothing
//!   else. A battle answers them today; [`Patch`] answers them for any number
//!   of maps folded in at any offset, which is what a streamed world will be.
//! - Readings are computed **lazily, a region at a time** ([`region_of`]),
//!   cached per region and dropped per region ([`TerrainReader::forget_around`])
//!   when a tile changes or its neighbours stream in. No whole-map pass is on
//!   any path.
//! - Every question is asked **inside an [`Area`]** — the commander's area of
//!   interest — so its cost scales with the fight rather than the world.
//! - Ground is named **by feature** ([`Feature`]): "the crest" is a set of
//!   hexes and an anchor, found from the ground itself, and survives a map
//!   that grows around it.
//! - A tile that is not known is **not counted**, never counted as seen or
//!   unseen: how much a hex sees is a share of the tiles that exist within
//!   range, so a map edge does not read as a hill and a neighbouring tile
//!   streaming in changes the answer the way it should.
//!
//! Every key is geometric — a count, a share, an elevation — with a
//! coordinate only ever last, so a reading is mirror-symmetric wherever the
//! ground is. `tests/ground.rs` holds it to that on the ridge arena.

use crate::battle::{BattleState, MoveGrid, SightGrid};
use crate::data::{DataRegistry, MovementClass};
use crate::map::{HexMap, Tile};
use hexx::Hex;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

/// What the reader needs to know about the world, and nothing more.
///
/// Four questions, so that the thing answering them can be a battle, a stitch
/// of several battle maps, or one day a world streamed a region at a time.
pub trait Ground {
    /// The terrain id of the tile at `hex`, or `None` where no tile is known.
    fn terrain_id(&self, hex: Hex) -> Option<&str>;
    /// The elevation level of the tile at `hex`, or `None` where no tile is
    /// known.
    fn elevation(&self, hex: Hex) -> Option<i32>;
    /// Whether a crew at `from` could see a hull at `to`, geometry only: no
    /// range, no concealment, no fog. Mirror-symmetric, as the engine's own
    /// sight line is.
    fn sight_clear(&self, from: Hex, to: Hex) -> bool;
    /// What one step costs a vehicle of `class`, or `None` where she cannot
    /// make it.
    fn step_cost(&self, class: MovementClass, max_climb: i32, from: Hex, to: Hex) -> Option<u32>;
}

impl Ground for BattleState {
    fn terrain_id(&self, hex: Hex) -> Option<&str> {
        self.map.get(hex).map(|t| t.terrain.as_str())
    }
    fn elevation(&self, hex: Hex) -> Option<i32> {
        self.map.get(hex).map(|t| t.elevation)
    }
    fn sight_clear(&self, from: Hex, to: Hex) -> bool {
        self.sight.clear(from, to)
    }
    fn step_cost(&self, class: MovementClass, max_climb: i32, from: Hex, to: Hex) -> Option<u32> {
        self.moves.cost(class, max_climb, from, to)
    }
}

/// Ground assembled from maps: one, or several folded in side by side.
///
/// The shape a streamed world will have — tiles arrive a map at a time, at an
/// offset, and the sight and movement grids fold them in through the same
/// `insert` the battle's own grids are built with — available today, so the
/// reader can be held to working across a seam before there is a world to
/// seam.
#[derive(Default)]
pub struct Patch {
    tiles: HashMap<Hex, Tile>,
    sight: SightGrid,
    moves: MoveGrid,
}

impl Patch {
    /// Ground made of one map, where it lies.
    pub fn of(registry: &DataRegistry, map: &HexMap) -> Self {
        let mut patch = Self::default();
        patch.add(registry, map, Hex::ZERO);
        patch
    }

    /// Fold a map in, shifted by `offset`. A tile already known at a hex is
    /// replaced, as a streamed region replaces what was known of it.
    pub fn add(&mut self, registry: &DataRegistry, map: &HexMap, offset: Hex) {
        for (hex, tile) in map.iter() {
            let at = hex + offset;
            self.sight.insert(registry, at, tile);
            self.moves.insert(registry, at, tile);
            self.tiles.insert(at, tile.clone());
        }
    }

    /// How many tiles are known.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// Whether nothing is known yet.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }
}

impl Ground for Patch {
    fn terrain_id(&self, hex: Hex) -> Option<&str> {
        self.tiles.get(&hex).map(|t| t.terrain.as_str())
    }
    fn elevation(&self, hex: Hex) -> Option<i32> {
        self.tiles.get(&hex).map(|t| t.elevation)
    }
    fn sight_clear(&self, from: Hex, to: Hex) -> bool {
        self.sight.clear(from, to)
    }
    fn step_cost(&self, class: MovementClass, max_climb: i32, from: Hex, to: Hex) -> Option<u32> {
        self.moves.cost(class, max_climb, from, to)
    }
}

/// Radius of a reading region, in hexes: 61 tiles to a region.
///
/// The unit of laziness and of invalidation, not a rule of the game. Small
/// enough that asking about one hex reads little it was not asked about;
/// large enough that an area of interest is a handful of regions rather than
/// hundreds of cache entries.
pub const REGION_RADIUS: u32 = 4;

/// How far down from a hex the ground must fall for it to be high ground
/// rather than part of a plateau's middle, in hexes. Geometry, not a
/// preference: three hexes is 300 m, the length of a slope a crew on top can
/// see the foot of.
pub const RELIEF_RADIUS: u32 = 3;

/// Whether `hex` is high ground: known, not lower than any known neighbour,
/// and higher than some known hex within [`RELIEF_RADIUS`].
pub fn high_ground(ground: &impl Ground, hex: Hex) -> bool {
    let Some(here) = ground.elevation(hex) else {
        return false;
    };
    if hex
        .all_neighbors()
        .iter()
        .any(|n| ground.elevation(*n).is_some_and(|e| e > here))
    {
        return false;
    }
    hex.range(RELIEF_RADIUS)
        .any(|n| ground.elevation(n).is_some_and(|e| e < here))
}

/// The region a hex's reading is computed and cached with. A hexagonal tiling
/// of the whole plane (`hexx`'s own), so it is defined for any coordinate a
/// world will ever have, not only for the hexes of one map.
pub fn region_of(hex: Hex) -> Hex {
    hex.to_lower_res(REGION_RADIUS)
}

/// What one tile is, to somebody deciding where to stand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileReading {
    /// Known tiles within [`TerrainReader::outlook_range`] a crew here could
    /// see, not counting her own.
    pub seen: u32,
    /// Known tiles within that range, seen or not.
    pub in_range: u32,
    /// The terrain's cover and concealment, as the resolver reads them.
    pub cover: i32,
    pub concealment: i32,
    pub elevation: i32,
}

impl TileReading {
    /// How much of what exists around her she can see, per mille. A share,
    /// not a count, so a hex at the edge of the known world is not a worse
    /// lookout merely for having less world around it.
    pub fn share(&self) -> i32 {
        (self.seen * 1000 / self.in_range.max(1)) as i32
    }
}

/// A commander's area of interest: the hexes a question is asked inside.
///
/// Sorted and deduplicated, so membership is a binary search and nothing that
/// walks it can depend on the order it was built in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Area {
    hexes: Vec<Hex>,
}

impl Area {
    /// Every hex within `radius` of any of `centres`.
    pub fn around(centres: impl IntoIterator<Item = Hex>, radius: u32) -> Self {
        Self::of(centres.into_iter().flat_map(|c| c.range(radius)))
    }

    /// Exactly these hexes.
    pub fn of(hexes: impl IntoIterator<Item = Hex>) -> Self {
        let mut hexes: Vec<Hex> = hexes.into_iter().collect();
        hexes.sort_unstable_by_key(|h| (h.x, h.y));
        hexes.dedup();
        Self { hexes }
    }

    pub fn hexes(&self) -> &[Hex] {
        &self.hexes
    }

    pub fn contains(&self, hex: Hex) -> bool {
        self.hexes
            .binary_search_by_key(&(hex.x, hex.y), |h| (h.x, h.y))
            .is_ok()
    }

    pub fn len(&self) -> usize {
        self.hexes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hexes.is_empty()
    }
}

/// What kind of ground a [`Feature`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureKind {
    /// High ground: a crest, a spur, a tor, a knoll.
    Vantage,
}

/// A piece of ground with a name a plan can use: its hexes and the one that
/// best stands for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feature {
    pub kind: FeatureKind,
    /// The hex that best stands for it: for a vantage, the one that sees most.
    pub anchor: Hex,
    /// Every hex of it, sorted.
    pub hexes: Vec<Hex>,
    /// What the anchor sees, in tiles.
    pub seen: u32,
}

/// A route and what it cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// Every hex from the start to the end, both included.
    pub path: Vec<Hex>,
    /// Movement points the route costs, without the exposure price.
    pub cost: u32,
    /// Hexes along it (the start excluded) somebody in the watching set can
    /// see.
    pub exposed: u32,
}

/// Reads ground lazily and remembers what it has read.
///
/// Owned by whoever plans, not by the battle: a reading is a pure function of
/// the ground, so each planner may keep its own, nothing about it is saved,
/// and nothing in a battle's state has to be rebuilt for it on load.
pub struct TerrainReader {
    outlook_range: u32,
    regions: HashMap<Hex, HashMap<Hex, TileReading>>,
}

impl TerrainReader {
    /// A reader with the mod's own idea of how far a lookout's view is worth
    /// counting.
    pub fn new(registry: &DataRegistry) -> Self {
        Self {
            outlook_range: registry.planner.outlook_range,
            regions: HashMap::new(),
        }
    }

    /// How far a tile's view is counted, in hexes.
    pub fn outlook_range(&self) -> u32 {
        self.outlook_range
    }

    /// How many regions have been read. What a test of laziness watches, and
    /// what a streamed world's memory budget will.
    pub fn regions_read(&self) -> usize {
        self.regions.len()
    }

    /// Drop every cached region whose readings a change at `hex` could reach:
    /// a tile's view extends [`Self::outlook_range`] hexes, so a tile changing
    /// — or a neighbouring map streaming in beside it — is felt that far.
    pub fn forget_around(&mut self, hex: Hex) {
        let reach = (self.outlook_range + 2 * REGION_RADIUS + 1) as i32;
        self.regions
            .retain(|region, _| region.to_higher_res(REGION_RADIUS).distance_to(hex) > reach);
    }

    /// The reading of the tile at `hex`, reading its region first if nobody
    /// has asked about it yet. `None` where no tile is known.
    pub fn tile(
        &mut self,
        registry: &DataRegistry,
        ground: &impl Ground,
        hex: Hex,
    ) -> Option<TileReading> {
        ground.terrain_id(hex)?;
        let key = region_of(hex);
        if !self.regions.contains_key(&key) {
            let read = self.read_region(registry, ground, key);
            self.regions.insert(key, read);
        }
        self.regions.get(&key).and_then(|r| r.get(&hex)).copied()
    }

    fn read_region(
        &self,
        registry: &DataRegistry,
        ground: &impl Ground,
        key: Hex,
    ) -> HashMap<Hex, TileReading> {
        // The region's own hexes, whatever the tiling's shape: every hex in
        // reach of its centre that maps back to it.
        let centre = key.to_higher_res(REGION_RADIUS);
        let mut out = HashMap::new();
        for hex in centre.range(2 * REGION_RADIUS) {
            if region_of(hex) != key {
                continue;
            }
            let Some(id) = ground.terrain_id(hex) else {
                continue;
            };
            let def = registry.terrain(id);
            let (mut seen, mut in_range) = (0, 0);
            for other in hex.range(self.outlook_range) {
                if other == hex || ground.terrain_id(other).is_none() {
                    continue;
                }
                in_range += 1;
                if ground.sight_clear(hex, other) {
                    seen += 1;
                }
            }
            out.insert(
                hex,
                TileReading {
                    seen,
                    in_range,
                    cover: def.map(|d| d.cover).unwrap_or(0),
                    concealment: def.map(|d| d.concealment).unwrap_or(0),
                    elevation: ground.elevation(hex).unwrap_or(0),
                },
            );
        }
        out
    }

    /// The vantages inside `area`: high ground, grouped into features and
    /// ranked by how much of the ground around it can be seen from it.
    ///
    /// A hex is high ground when nothing next to it is higher and something
    /// within [`RELIEF_RADIUS`] is lower — a crest, a spur, a tor, a knoll —
    /// which is geometry and nothing else. A flat plain has no vantage at
    /// all, and neither does the edge of the known world.
    ///
    /// The first draft asked for ground that *sees* markedly more than its
    /// neighbours, and it was noise: standing in a wood blinds a tile, so
    /// every hex beside a wood read as prominent, and a plain with a few
    /// copses came out with seventeen "vantages". How much a hex sees is how
    /// vantages are ranked, not how they are found.
    ///
    /// The hexes that qualify are grouped by adjacency alone, so which feature
    /// a hex belongs to does not depend on the order anything was asked in.
    /// Sorted by what the anchor sees, most first; the anchor's coordinate
    /// settles only exact ties.
    pub fn vantages(
        &mut self,
        registry: &DataRegistry,
        ground: &impl Ground,
        area: &Area,
    ) -> Vec<Feature> {
        let standing_out: Vec<Hex> = area
            .hexes()
            .iter()
            .copied()
            .filter(|h| high_ground(ground, *h))
            .collect();
        let member = Area::of(standing_out.iter().copied());

        let mut claimed = Area::default();
        let mut features = Vec::new();
        for &start in member.hexes() {
            if claimed.contains(start) {
                continue;
            }
            // Flood the component. Visiting order cannot change membership.
            let mut hexes = vec![start];
            let mut frontier = vec![start];
            while let Some(at) = frontier.pop() {
                for next in at.all_neighbors() {
                    if member.contains(next) && !hexes.contains(&next) {
                        hexes.push(next);
                        frontier.push(next);
                    }
                }
            }
            let hexes = Area::of(hexes);
            claimed = Area::of(claimed.hexes().iter().chain(hexes.hexes()).copied());
            let mut ranked: Vec<(Hex, TileReading)> = hexes
                .hexes()
                .iter()
                .filter_map(|h| self.tile(registry, ground, *h).map(|r| (*h, r)))
                .collect();
            ranked.sort_by_key(|(h, r)| (Reverse(r.seen), Reverse(r.elevation), h.x, h.y));
            let Some(&(anchor, best)) = ranked.first() else {
                continue;
            };
            features.push(Feature {
                kind: FeatureKind::Vantage,
                anchor,
                hexes: hexes.hexes().to_vec(),
                seen: best.seen,
            });
        }
        features.sort_by_key(|f| (Reverse(f.seen), f.anchor.x, f.anchor.y));
        features
    }
}

/// The hexes of `area` that nobody in `observers` — each a position and how
/// far she sees — can see: dead ground, relative to them.
///
/// Relative on purpose. "Dead ground" is not a property of a hex but of a hex
/// and somebody looking, which is why this is a query asked with the
/// observers of the moment rather than a reading cached with the map.
pub fn dead_ground(ground: &impl Ground, observers: &[(Hex, u32)], area: &Area) -> Vec<Hex> {
    area.hexes()
        .iter()
        .copied()
        .filter(|h| ground.terrain_id(*h).is_some())
        .filter(|h| !watched(ground, observers, *h))
        .collect()
}

/// Whether anybody in `observers` can see `hex`.
pub fn watched(ground: &impl Ground, observers: &[(Hex, u32)], hex: Hex) -> bool {
    observers
        .iter()
        .any(|(at, range)| at.distance_to(hex) <= *range as i32 && ground.sight_clear(*at, hex))
}

/// Firing positions in `area` onto the ground `onto`: every hex from which a
/// crew could see some of it within `range`, with how many of its hexes, most
/// first, better cover breaking ties and the coordinate last.
pub fn firing_positions(
    registry: &DataRegistry,
    ground: &impl Ground,
    area: &Area,
    onto: &Area,
    range: u32,
) -> Vec<(Hex, u32, i32)> {
    let mut out: Vec<(Hex, u32, i32)> = area
        .hexes()
        .iter()
        .copied()
        .filter_map(|h| {
            let cover = registry.terrain(ground.terrain_id(h)?).map(|d| d.cover)?;
            let sees = onto
                .hexes()
                .iter()
                .filter(|t| {
                    ground.terrain_id(**t).is_some()
                        && h.distance_to(**t) <= range as i32
                        && ground.sight_clear(h, **t)
                })
                .count() as u32;
            (sees > 0).then_some((h, sees, cover))
        })
        .collect();
    out.sort_by_key(|&(h, sees, cover)| (Reverse(sees), Reverse(cover), h.x, h.y));
    out
}

/// Every covered route out of one start: one search, asked about as many
/// ends as the caller likes.
///
/// A plan weighs many possible destinations for one element — every hex off
/// the enemy's frontal arc within reach — and routing to each separately
/// would be a search per candidate. This is the search once.
pub struct CoveredRoutes {
    from: Hex,
    best: HashMap<Hex, u32>,
    came: HashMap<Hex, Hex>,
    exposed: HashMap<Hex, bool>,
    steps: HashMap<(Hex, Hex), u32>,
}

impl CoveredRoutes {
    /// Search out of `from` inside `area` for a vehicle of `class`, every hex
    /// `observers` can see costing `exposure_price` more movement points.
    #[allow(clippy::too_many_arguments)]
    pub fn search(
        ground: &impl Ground,
        class: MovementClass,
        max_climb: i32,
        from: Hex,
        observers: &[(Hex, u32)],
        area: &Area,
        exposure_price: u32,
    ) -> Self {
        let mut exposed: HashMap<Hex, bool> = HashMap::new();
        let mut steps: HashMap<(Hex, Hex), u32> = HashMap::new();
        let mut best: HashMap<Hex, u32> = HashMap::new();
        let mut came: HashMap<Hex, Hex> = HashMap::new();
        // Coordinates last in the heap key: they settle only exact ties.
        let mut heap = BinaryHeap::new();
        best.insert(from, 0);
        heap.push(Reverse((0u32, from.x, from.y)));
        while let Some(Reverse((cost, x, y))) = heap.pop() {
            let at = Hex::new(x, y);
            if best.get(&at).is_some_and(|b| cost > *b) {
                continue;
            }
            for next in at.all_neighbors() {
                if !area.contains(next) {
                    continue;
                }
                let Some(step) = ground.step_cost(class, max_climb, at, next) else {
                    continue;
                };
                steps.insert((at, next), step);
                let seen = *exposed
                    .entry(next)
                    .or_insert_with(|| watched(ground, observers, next));
                let total = cost + step + if seen { exposure_price } else { 0 };
                if best.get(&next).is_none_or(|b| total < *b) {
                    best.insert(next, total);
                    came.insert(next, at);
                    heap.push(Reverse((total, next.x, next.y)));
                }
            }
        }
        Self {
            from,
            best,
            came,
            exposed,
            steps,
        }
    }

    /// The route to `to`, or `None` if the search never reached it.
    pub fn to(&self, to: Hex) -> Option<Route> {
        self.best.get(&to)?;
        let mut path = vec![to];
        while let Some(prev) = self.came.get(path.last()?) {
            path.push(*prev);
        }
        path.reverse();
        if path.first() != Some(&self.from) {
            return None;
        }
        let mut cost = 0;
        let mut seen = 0;
        for pair in path.windows(2) {
            cost += self.steps.get(&(pair[0], pair[1]))?;
            if self.exposed.get(&pair[1]).copied().unwrap_or(false) {
                seen += 1;
            }
        }
        Some(Route {
            path,
            cost,
            exposed: seen,
        })
    }
}

/// The cheapest way from `from` to `to` inside `area` for a vehicle of
/// `class`, where every hex `observers` can see costs `exposure_price` more
/// movement points than it otherwise would.
///
/// At a price of zero it is the plain shortest route; as the price rises it
/// trades distance for dead ground, which is what a covered approach is. The
/// start is not charged — she is wherever she is. `None` when no route inside
/// the area exists. One destination of [`CoveredRoutes`].
#[allow(clippy::too_many_arguments)]
pub fn covered_route(
    ground: &impl Ground,
    class: MovementClass,
    max_climb: i32,
    from: Hex,
    to: Hex,
    observers: &[(Hex, u32)],
    area: &Area,
    exposure_price: u32,
) -> Option<Route> {
    let area = Area::of(area.hexes().iter().copied().chain([from, to]));
    CoveredRoutes::search(
        ground,
        class,
        max_climb,
        from,
        observers,
        &area,
        exposure_price,
    )
    .to(to)
}
