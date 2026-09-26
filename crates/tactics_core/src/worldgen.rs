//! Making a world: relief, cover, rivers, towns and roads, from a seed.
//!
//! WORLD.md W1.2–W1.4. The designer's ruling is bottom up: the tiles are the
//! truth, and a campaign hex is a summary of the tiles inside it. The shape
//! that makes that affordable is two stages:
//!
//! - **The skeleton**, made once when the world is: the features a tile
//!   cannot decide about itself because they are about *other* tiles —
//!   where a river runs, where the towns are, which way a road goes between
//!   them. Rivers run down the relief, towns sit on flat ground by water,
//!   roads are least-cost paths joining every town.
//! - **The tile function**, [`GeneratedWorld::tile`]: what one hex is, a
//!   pure function of the seed, the skeleton and the hex, and of nothing
//!   else — not of which chunk asked, what is loaded, or in what order. There
//!   is no chunk boundary anywhere inside it, which is what makes a seam
//!   impossible rather than merely unlikely.
//!
//! Relief and cover are value noise over the plane, hashed from integers
//! and blended with nothing but `+` and `*`, so a tile is the same tile on
//! every machine. The shares a mod asks for ("28% woodland") are made true by
//! calibrating against the world's own noise when it is made: thresholds are
//! quantiles of a fixed sample of the world, not constants guessed against a
//! distribution nobody wrote down.
//!
//! **A tie is broken by the seed, not the compass** ([`GeneratedWorld::lot`]).
//! The first draft broke ties on a site's score by its coordinates, which is
//! allowed in principle — the rule against it is about decisions in a battle
//! — and wrong in practice: a town's score is a small integer with many ties,
//! so on every seed the towns lined up along the western edge. The compass
//! makes bad maps as surely as it makes bad decisions.

use crate::data::{DataRegistry, SkeletonFeature, SummaryRule, WorldGen};
use crate::world::{chunk_hexes, chunk_of};
use hexx::Hex;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};

/// Independent streams of noise, so relief and cover do not share a lattice.
#[derive(Clone, Copy)]
enum Channel {
    Relief = 1,
    Cover = 2,
    Wet = 3,
    Lot = 4,
}

/// SplitMix64: the one mixing function the world is made with.
fn mix(state: u64, value: u64) -> u64 {
    let mut z = (state ^ value).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A value in `[0, 1)` for one lattice point.
fn lattice(seed: u64, channel: Channel, octave: u32, x: i64, y: i64) -> f64 {
    let h = [channel as u64, octave as u64, x as u64, y as u64]
        .into_iter()
        .fold(mix(0, seed), mix);
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Smoothstep: continuous slope across a lattice cell's edge.
fn fade(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// Where a hex sits on the plane, in tiles. Pointy-topped axial to
/// Cartesian; the irrational factor is a constant, and every operation on it
/// is a correctly-rounded IEEE one.
fn plane(hex: Hex) -> (f64, f64) {
    (
        hex.x as f64 + hex.y as f64 * 0.5,
        hex.y as f64 * 0.866_025_403_784_438_6,
    )
}

/// Fractal value noise at `hex`: `octaves` layers, each twice as fine and
/// half as strong as the last, normalised to `[0, 1)`.
fn noise(seed: u64, channel: Channel, hex: Hex, scale: u32, octaves: u32) -> f64 {
    let (px, py) = plane(hex);
    let mut total = 0.0;
    let mut weight = 0.0;
    let mut amplitude = 1.0;
    let mut cell = scale.max(1) as f64;
    for octave in 0..octaves.max(1) {
        let (x, y) = (px / cell, py / cell);
        let (ix, iy) = (x.floor(), y.floor());
        let (fx, fy) = (fade(x - ix), fade(y - iy));
        let (ix, iy) = (ix as i64, iy as i64);
        let v00 = lattice(seed, channel, octave, ix, iy);
        let v10 = lattice(seed, channel, octave, ix + 1, iy);
        let v01 = lattice(seed, channel, octave, ix, iy + 1);
        let v11 = lattice(seed, channel, octave, ix + 1, iy + 1);
        let top = v00 + (v10 - v00) * fx;
        let bottom = v01 + (v11 - v01) * fx;
        total += (top + (bottom - top) * fy) * amplitude;
        weight += amplitude;
        amplitude *= 0.5;
        cell = (cell * 0.5).max(1.0);
    }
    total / weight
}

/// A town: where, how far it reaches, and whether it has a factory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Town {
    pub centre: Hex,
    pub radius: u32,
    pub factory: bool,
}

/// The features made once, when the world is: every one a list of tiles in
/// order, so the skeleton is the same bytes whenever it is made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skeleton {
    /// Each river, source first; each step is a neighbour of the last.
    pub rivers: Vec<Vec<Hex>>,
    /// Each town, in the order they were chosen (best site first).
    pub towns: Vec<Town>,
    /// Each road, from one town's centre to another's; each step is a
    /// neighbour of the last.
    pub roads: Vec<Vec<Hex>>,
}

/// Which features touch one chunk, so a tile can ask without walking the
/// whole skeleton.
#[derive(Debug, Clone, Default)]
struct ChunkFeatures {
    water: HashMap<Hex, i32>,
    road: HashSet<Hex>,
    towns: Vec<usize>,
}

/// Thresholds that make the mod's shares true of this world.
#[derive(Debug, Clone, Default, PartialEq)]
struct Calibration {
    /// Relief noise at or above `levels[i]` is at least level `i + 1`.
    levels: Vec<f64>,
    /// Cover noise at or above this is wood.
    wood: f64,
    /// Cover noise at or above this (and below `wood`) is hedged.
    hedge: f64,
    /// Wet noise below this, on level-0 ground, is wet.
    wet: f64,
}

/// What a campaign hex is, summarised from its tiles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// The campaign terrain, by the first summary rule that holds.
    pub terrain: String,
    /// The mean elevation of its land, rounded.
    pub elevation: i32,
}

/// A world, made: its seed and rules, the skeleton, and what it takes to
/// answer for any tile.
#[derive(Debug, Clone)]
pub struct GeneratedWorld {
    pub seed: u64,
    pub rules: WorldGen,
    /// The radius of a chunk in tiles: one campaign hex.
    pub chunk_radius: u32,
    pub skeleton: Skeleton,
    calibration: Calibration,
    index: HashMap<(i32, i32), ChunkFeatures>,
}

/// What a generated world is saved as: what it takes to make it again. The
/// skeleton and every tile follow from these three, so a save carries a
/// world of 160,000 tiles in a few hundred bytes (WORLD.md W1.6). The rules
/// travel in the file rather than being read off the mods at load, so a
/// retuned `worldgen` block cannot quietly give a loaded campaign different
/// ground from the one it was fought on.
#[derive(Serialize, Deserialize)]
struct SavedGeneration {
    seed: u64,
    rules: WorldGen,
    chunk_radius: u32,
}

impl Serialize for GeneratedWorld {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        SavedGeneration {
            seed: self.seed,
            rules: self.rules.clone(),
            chunk_radius: self.chunk_radius,
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for GeneratedWorld {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let saved = SavedGeneration::deserialize(d)?;
        Self::with_rules(saved.rules, saved.chunk_radius, saved.seed)
            .map_err(serde::de::Error::custom)
    }
}

/// Why a world could not be made.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WorldGenError {
    #[error("no mod declares a `worldgen` block")]
    NoRules,
    #[error("worldgen: {0}")]
    Invalid(String),
}

impl GeneratedWorld {
    /// Make the world the registry's rules describe, from `seed`.
    pub fn new(registry: &DataRegistry, seed: u64) -> Result<Self, WorldGenError> {
        let rules = registry.worldgen.clone().ok_or(WorldGenError::NoRules)?;
        Self::with_rules(rules, registry.scale.battle_map_radius(), seed)
    }

    /// Make a world from these rules, with chunks of `chunk_radius` tiles.
    pub fn with_rules(
        rules: WorldGen,
        chunk_radius: u32,
        seed: u64,
    ) -> Result<Self, WorldGenError> {
        if rules.relief.shares.is_empty() || rules.relief.shares.len() > 10 {
            return Err(WorldGenError::Invalid(format!(
                "relief.shares names {} levels; a world has 1 to 10",
                rules.relief.shares.len()
            )));
        }
        let mut world = Self {
            seed,
            rules,
            chunk_radius,
            skeleton: Skeleton::default(),
            calibration: Calibration::default(),
            index: HashMap::new(),
        };
        world.calibration = world.calibrate();
        world.skeleton.rivers = world.lay_rivers();
        world.index_rivers();
        world.skeleton.towns = world.site_towns();
        world.skeleton.roads = world.lay_roads();
        world.index_rest();
        Ok(world)
    }

    /// Every chunk in the world, ring by ring from the centre.
    pub fn chunks(&self) -> impl Iterator<Item = Hex> {
        Hex::ZERO.spiral_range(0..=self.rules.radius)
    }

    /// Whether `hex` is part of the world at all.
    pub fn contains(&self, hex: Hex) -> bool {
        chunk_of(hex, self.chunk_radius).unsigned_distance_to(Hex::ZERO) <= self.rules.radius
    }

    // --- the tile function -------------------------------------------------

    fn relief_noise(&self, hex: Hex) -> f64 {
        let r = &self.rules.relief;
        noise(self.seed, Channel::Relief, hex, r.scale, r.octaves)
    }

    fn cover_noise(&self, hex: Hex) -> f64 {
        noise(self.seed, Channel::Cover, hex, self.rules.cover.scale, 2)
    }

    fn wet_noise(&self, hex: Hex) -> f64 {
        noise(self.seed, Channel::Wet, hex, self.rules.cover.scale, 1)
    }

    /// The level relief noise `v` stands at.
    /// A seeded lot drawn for `hex`: what a tie between two sites is broken
    /// by. Uniform over the map, so no direction is favoured.
    fn lot(&self, hex: Hex) -> u64 {
        [Channel::Lot as u64, hex.x as u64, hex.y as u64]
            .into_iter()
            .fold(mix(0, self.seed), mix)
    }

    fn level_of(&self, v: f64) -> i32 {
        self.calibration.levels.iter().filter(|t| v >= **t).count() as i32
    }

    /// What the ground is before anybody laid a road or dug a town: relief
    /// and cover alone. Roads are routed over this.
    fn natural(&self, hex: Hex) -> (&str, i32) {
        let palette = &self.rules.terrain;
        let level = self.level_of(self.relief_noise(hex));
        let cover = self.cover_noise(hex);
        let c = &self.calibration;
        let terrain = if cover >= c.wood {
            &palette.wood
        } else if cover >= c.hedge {
            &palette.hedge
        } else if level == 0 && self.wet_noise(hex) < c.wet {
            &palette.wet
        } else {
            &palette.open
        };
        (terrain, level)
    }

    /// What `hex` is: its terrain id and elevation level, or `None` past the
    /// edge of the world.
    ///
    /// A pure function of the seed, the rules, the skeleton and the hex. A
    /// town is built over everything; a road over a river is its bridge; a
    /// river runs at the level it has fallen to.
    pub fn tile(&self, hex: Hex) -> Option<(&str, i32)> {
        if !self.contains(hex) {
            return None;
        }
        let palette = &self.rules.terrain;
        let (natural, level) = self.natural(hex);
        let chunk = chunk_of(hex, self.chunk_radius);
        let Some(features) = self.index.get(&(chunk.x, chunk.y)) else {
            return Some((natural, level));
        };
        if features.towns.iter().any(|&t| {
            let town = &self.skeleton.towns[t];
            hex.unsigned_distance_to(town.centre) <= town.radius
        }) {
            return Some((&palette.town, level));
        }
        if features.road.contains(&hex) {
            return Some((&palette.road, level));
        }
        if let Some(&water) = features.water.get(&hex) {
            return Some((&palette.water, water));
        }
        Some((natural, level))
    }

    /// Every tile of `chunk`, centre outward, ready for
    /// [`crate::world::World::load_chunk`]. Empty for a chunk past the edge.
    pub fn chunk_tiles(&self, chunk: Hex) -> Vec<(Hex, &str, i32)> {
        chunk_hexes(chunk, self.chunk_radius)
            .filter_map(|hex| self.tile(hex).map(|(t, e)| (hex, t, e)))
            .collect()
    }

    /// Whether `chunk` holds `feature` of the skeleton.
    pub fn holds(&self, chunk: Hex, feature: SkeletonFeature) -> bool {
        let Some(f) = self.index.get(&(chunk.x, chunk.y)) else {
            return false;
        };
        match feature {
            SkeletonFeature::Factory => f.towns.iter().any(|&t| {
                let town = &self.skeleton.towns[t];
                town.factory && chunk_of(town.centre, self.chunk_radius) == chunk
            }),
            SkeletonFeature::Town => f
                .towns
                .iter()
                .any(|&t| chunk_of(self.skeleton.towns[t].centre, self.chunk_radius) == chunk),
            SkeletonFeature::Road => !f.road.is_empty(),
            SkeletonFeature::River => !f.water.is_empty(),
        }
    }

    /// What a campaign hex is, read off its tiles by the mod's summary
    /// rules: `R` in WORLD.md's terms. `None` past the edge of the world.
    pub fn summary(&self, chunk: Hex) -> Option<Summary> {
        let tiles = self.chunk_tiles(chunk);
        if tiles.is_empty() {
            return None;
        }
        let mut counts: HashMap<&str, u32> = HashMap::new();
        let mut land = (0i64, 0i64);
        for (_, terrain, level) in &tiles {
            *counts.entry(terrain).or_default() += 1;
            if *terrain != self.rules.terrain.water {
                land = (land.0 + *level as i64, land.1 + 1);
            }
        }
        let n = tiles.len() as u32;
        let mean_tenths = if land.1 == 0 {
            0
        } else {
            (land.0 * 10 / land.1) as u32
        };
        let holds = |rule: &SummaryRule| {
            rule.feature.is_none_or(|f| self.holds(chunk, f))
                && rule.mean_elevation_tenths.is_none_or(|t| mean_tenths >= t)
                && rule.share.as_ref().is_none_or(|s| {
                    counts.get(s.terrain.as_str()).copied().unwrap_or(0) * 100 >= s.percent * n
                })
        };
        let terrain = self
            .rules
            .summary
            .iter()
            .find(|r| holds(r))
            .map_or_else(|| self.rules.terrain.open.clone(), |r| r.terrain.clone());
        Some(Summary {
            terrain,
            elevation: ((mean_tenths + 5) / 10) as i32,
        })
    }

    // --- making it -----------------------------------------------------------

    /// A fixed sample of the world: every seventh tile of every chunk. Big
    /// enough that the quantiles are the world's, small enough to be quick.
    fn sample(&self) -> Vec<Hex> {
        self.chunks()
            .flat_map(|c| chunk_hexes(c, self.chunk_radius).step_by(7))
            .collect()
    }

    fn calibrate(&self) -> Calibration {
        let sample = self.sample();
        let quantile = |mut values: Vec<f64>, share: f64| -> f64 {
            values.sort_by(f64::total_cmp);
            let i = ((values.len() as f64) * share.clamp(0.0, 1.0)) as usize;
            values
                .get(i.min(values.len().saturating_sub(1)))
                .copied()
                .unwrap_or(1.0)
        };
        let relief: Vec<f64> = sample.iter().map(|h| self.relief_noise(*h)).collect();
        let mut levels = Vec::new();
        let mut below = 0u32;
        let shares = &self.rules.relief.shares;
        for share in &shares[..shares.len() - 1] {
            below += share;
            levels.push(quantile(relief.clone(), below as f64 / 100.0));
        }
        let cover: Vec<f64> = sample.iter().map(|h| self.cover_noise(*h)).collect();
        let c = &self.rules.cover;
        let wood = quantile(cover.clone(), 1.0 - c.wood_percent as f64 / 100.0);
        let hedge = quantile(
            cover,
            1.0 - (c.wood_percent + c.hedge_percent) as f64 / 100.0,
        );
        let wet_sample: Vec<f64> = sample.iter().map(|h| self.wet_noise(*h)).collect();
        let wet = quantile(wet_sample, c.wet_percent as f64 / 100.0);
        Calibration {
            levels,
            wood,
            hedge,
            wet,
        }
    }

    /// Chunks in the world, best `score` first, each at least `spacing`
    /// chunks from every one taken before it, `count` of them.
    fn pick_spaced<F: Fn(Hex) -> Option<(i64, Hex)>>(
        &self,
        count: u32,
        spacing: u32,
        score: F,
    ) -> Vec<Hex> {
        let mut candidates: Vec<(i64, Hex)> = self.chunks().filter_map(&score).collect();
        candidates.sort_by_key(|(s, h)| (Reverse(*s), self.lot(*h)));
        let mut taken: Vec<Hex> = Vec::new();
        for (_, hex) in candidates {
            if taken.len() as u32 >= count {
                break;
            }
            let chunk = chunk_of(hex, self.chunk_radius);
            if taken
                .iter()
                .all(|t| chunk_of(*t, self.chunk_radius).unsigned_distance_to(chunk) >= spacing)
            {
                taken.push(hex);
            }
        }
        taken
    }

    /// Each river's source is the highest ground near a chunk's centre,
    /// highest first. From there it takes the path to the edge of the world
    /// that climbs least: a step costs one, and a step uphill costs in
    /// proportion to the rise besides. So it runs down wherever it can and,
    /// caught in a hollow, spills over the lowest point of its rim — rather
    /// than filling the hollow tile by tile, which is what "the lowest
    /// neighbour not yet visited" does and what the first draft did (a
    /// 2,244-tile river in a world 530 across). A river that reaches one
    /// laid before it ends there: a confluence.
    fn lay_rivers(&self) -> Vec<Vec<Hex>> {
        let r = self.chunk_radius as i32;
        let sources = self.pick_spaced(self.rules.rivers.count, self.rules.rivers.spacing, |c| {
            let centre = chunk_hexes(c, self.chunk_radius).next()?;
            let best = centre
                .range((r / 2) as u32)
                .step_by(5)
                .max_by_key(|h| ((self.relief_noise(*h) * 1e9) as i64, self.lot(*h)))?;
            // Rivers rise in high ground but not on the world's rim, or they
            // leave it at once.
            let rim = self.rules.radius.saturating_sub(1);
            (c.unsigned_distance_to(Hex::ZERO) < rim.max(1))
                .then(|| ((self.relief_noise(best) * 1e9) as i64, best))
        });
        let mut rivers: Vec<Vec<Hex>> = Vec::new();
        let mut water: HashSet<Hex> = HashSet::new();
        for source in sources {
            if water.contains(&source) {
                continue;
            }
            let path = self.run_down(source, &water);
            water.extend(path.iter().copied());
            rivers.push(path);
        }
        rivers
    }

    /// The least-climbing path from `source` to the rim of the world or to
    /// water already laid, by Dijkstra.
    fn run_down(&self, source: Hex, water: &HashSet<Hex>) -> Vec<Hex> {
        /// What a thousandth of the relief's range costs to climb, in steps.
        const UPHILL: f64 = 2.0;
        let mut height: HashMap<Hex, f64> = HashMap::new();
        let mut h = |hex: Hex| *height.entry(hex).or_insert_with(|| self.relief_noise(hex));
        let mut best: HashMap<Hex, u64> = HashMap::from([(source, 0)]);
        let mut came: HashMap<Hex, Hex> = HashMap::new();
        let mut open: BinaryHeap<Reverse<(u64, i32, i32)>> = BinaryHeap::new();
        open.push(Reverse((0, source.x, source.y)));
        let mut end = source;
        while let Some(Reverse((cost, x, y))) = open.pop() {
            let at = Hex::new(x, y);
            if best.get(&at).is_some_and(|b| *b < cost) {
                continue;
            }
            let on_rim = at.all_neighbors().iter().any(|n| !self.contains(*n));
            if at != source && (on_rim || water.contains(&at)) {
                end = at;
                break;
            }
            let here = h(at);
            for next in at.all_neighbors() {
                if !self.contains(next) {
                    continue;
                }
                let rise = (h(next) - here).max(0.0);
                let step = 1 + (rise * 1000.0 * UPHILL) as u64;
                let total = cost + step;
                if best.get(&next).is_none_or(|b| total < *b) {
                    best.insert(next, total);
                    came.insert(next, at);
                    open.push(Reverse((total, next.x, next.y)));
                }
            }
        }
        let mut path = vec![end];
        let mut at = end;
        while let Some(prev) = came.get(&at) {
            path.push(*prev);
            at = *prev;
        }
        path.reverse();
        path
    }

    fn index_rivers(&mut self) {
        for river in &self.skeleton.rivers {
            // A river runs at the lowest level it has reached, so water never
            // climbs.
            let mut level = i32::MAX;
            for hex in river {
                level = level.min(self.level_of(self.relief_noise(*hex)));
                let chunk = chunk_of(*hex, self.chunk_radius);
                self.index
                    .entry((chunk.x, chunk.y))
                    .or_default()
                    .water
                    .insert(*hex, level);
            }
        }
    }

    fn is_water(&self, hex: Hex) -> bool {
        let chunk = chunk_of(hex, self.chunk_radius);
        self.index
            .get(&(chunk.x, chunk.y))
            .is_some_and(|f| f.water.contains_key(&hex))
    }

    /// A town wants flat, low ground with water close by. Each chunk offers
    /// its best site near its centre; the best sites win, spaced apart.
    fn site_towns(&self) -> Vec<Town> {
        let t = &self.rules.towns;
        let r = self.chunk_radius as i32;
        let sites = self.pick_spaced(t.count, t.spacing, |c| {
            let centre = chunk_hexes(c, self.chunk_radius).next()?;
            centre
                .range((r / 2) as u32)
                .step_by(3)
                .filter(|h| !self.is_water(*h))
                .map(|h| {
                    let level = self.level_of(self.relief_noise(h));
                    let rough: i32 = h
                        .all_neighbors()
                        .iter()
                        .map(|n| (self.level_of(self.relief_noise(*n)) - level).abs())
                        .sum();
                    let wet = h.range(4).any(|w| self.is_water(w));
                    let score =
                        100 - 10 * level as i64 - 15 * rough as i64 + if wet { 30 } else { 0 };
                    (score, h)
                })
                .max_by_key(|(s, h)| (*s, self.lot(*h)))
        });
        sites
            .into_iter()
            .enumerate()
            .map(|(i, centre)| Town {
                centre,
                radius: t.radius,
                factory: (i as u32) < t.factories,
            })
            .collect()
    }

    /// What a road pays to cross `to` from `from`, on the natural ground: a
    /// road finds the easy way, bridges when it must, and avoids a climb.
    fn road_cost(&self, from: Hex, to: Hex) -> Option<u32> {
        if !self.contains(to) {
            return None;
        }
        let palette = &self.rules.terrain;
        let (terrain, level) = self.natural(to);
        let (_, was) = self.natural(from);
        let ground = if self.is_water(to) {
            8
        } else if terrain == palette.wood {
            3
        } else if terrain == palette.hedge {
            2
        } else if terrain == palette.wet {
            4
        } else {
            1
        };
        Some(ground + 2 * (level - was).unsigned_abs())
    }

    /// The cheapest road from `from` to `to`, by A*.
    fn route(&self, from: Hex, to: Hex) -> Vec<Hex> {
        let mut open: BinaryHeap<Reverse<(u32, u32, i32, i32)>> = BinaryHeap::new();
        let mut best: HashMap<Hex, u32> = HashMap::from([(from, 0)]);
        let mut came: HashMap<Hex, Hex> = HashMap::new();
        open.push(Reverse((from.unsigned_distance_to(to), 0, from.x, from.y)));
        while let Some(Reverse((_, g, x, y))) = open.pop() {
            let at = Hex::new(x, y);
            if at == to {
                break;
            }
            if best.get(&at).is_some_and(|b| *b < g) {
                continue;
            }
            for next in at.all_neighbors() {
                let Some(step) = self.road_cost(at, next) else {
                    continue;
                };
                let cost = g + step;
                if best.get(&next).is_none_or(|b| cost < *b) {
                    best.insert(next, cost);
                    came.insert(next, at);
                    open.push(Reverse((
                        cost + next.unsigned_distance_to(to),
                        cost,
                        next.x,
                        next.y,
                    )));
                }
            }
        }
        let mut path = vec![to];
        let mut at = to;
        while let Some(prev) = came.get(&at) {
            path.push(*prev);
            at = *prev;
        }
        path.reverse();
        if path.first() == Some(&from) {
            path
        } else {
            Vec::new()
        }
    }

    /// Join every town: the shortest links that connect them all (Prim),
    /// then `extra_links` more, shortest first, for a second way round.
    fn lay_roads(&self) -> Vec<Vec<Hex>> {
        let towns = &self.skeleton.towns;
        if towns.len() < 2 {
            return Vec::new();
        }
        let dist = |a: usize, b: usize| towns[a].centre.unsigned_distance_to(towns[b].centre);
        let mut joined = vec![0usize];
        let mut links: Vec<(usize, usize)> = Vec::new();
        while joined.len() < towns.len() {
            let (a, b) = joined
                .iter()
                .flat_map(|&a| {
                    (0..towns.len())
                        .filter(|b| !joined.contains(b))
                        .map(move |b| (a, b))
                })
                .min_by_key(|&(a, b)| (dist(a, b), a, b))
                .expect("a town is left to join");
            links.push((a, b));
            joined.push(b);
        }
        let mut spare: Vec<(usize, usize)> = (0..towns.len())
            .flat_map(|a| (a + 1..towns.len()).map(move |b| (a, b)))
            .filter(|&(a, b)| !links.contains(&(a, b)) && !links.contains(&(b, a)))
            .collect();
        spare.sort_by_key(|&(a, b)| (dist(a, b), a, b));
        links.extend(
            spare
                .into_iter()
                .take(self.rules.roads.extra_links as usize),
        );
        links
            .into_iter()
            .map(|(a, b)| self.route(towns[a].centre, towns[b].centre))
            .filter(|p| !p.is_empty())
            .collect()
    }

    fn index_rest(&mut self) {
        let radius = self.chunk_radius;
        for road in &self.skeleton.roads {
            for hex in road {
                let chunk = chunk_of(*hex, radius);
                self.index
                    .entry((chunk.x, chunk.y))
                    .or_default()
                    .road
                    .insert(*hex);
            }
        }
        for (i, town) in self.skeleton.towns.iter().enumerate() {
            let mut chunks: Vec<Hex> = town
                .centre
                .range(town.radius)
                .map(|h| chunk_of(h, radius))
                .collect();
            chunks.sort_by_key(|c| (c.x, c.y));
            chunks.dedup();
            for chunk in chunks {
                self.index
                    .entry((chunk.x, chunk.y))
                    .or_default()
                    .towns
                    .push(i);
            }
        }
    }
}
