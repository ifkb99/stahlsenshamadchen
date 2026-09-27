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
    Landform = 5,
    Ridge = 6,
}

/// Which generator made a world: saved beside its seed and rules, and a
/// save made by another is refused rather than regenerated. A world is
/// saved as how to make it again (W1.6), so a change to *how* — a new noise,
/// a new layer, a river drawn differently — would otherwise hand a loaded
/// campaign different ground under the same armies with no error at all.
/// Bump it with any change that moves a tile.
///
/// 2 is W6.4: simplex noise in place of lattice value noise, and the
/// landform and ridge layers.
pub const GENERATOR_VERSION: u32 = 2;

/// SplitMix64: the one mixing function the world is made with.
fn mix(state: u64, value: u64) -> u64 {
    let mut z = (state ^ value).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The gradient at one corner of the simplex grid: one of eight directions,
/// the compass points and the diagonals, chosen by hash.
fn gradient(seed: u64, channel: Channel, octave: u32, i: i64, j: i64) -> (f64, f64) {
    const D: f64 = std::f64::consts::FRAC_1_SQRT_2;
    const G: [(f64, f64); 8] = [
        (1.0, 0.0),
        (-1.0, 0.0),
        (0.0, 1.0),
        (0.0, -1.0),
        (D, D),
        (-D, D),
        (D, -D),
        (-D, -D),
    ];
    let h = [channel as u64, octave as u64, i as u64, j as u64]
        .into_iter()
        .fold(mix(0, seed), mix);
    G[(h >> 61) as usize]
}

/// Simplex noise at a point of the plane, roughly in `[-1, 1]`.
///
/// It replaced lattice value noise (W6.4), whose bilinear blend across a
/// square grid drew wood edges and relief bands along the grid's axes —
/// straight lines a hex map shows plainly. Simplex has no preferred axis.
/// Everything is `+`, `*` and `floor` on correctly-rounded IEEE values and
/// the skew factors are constants, so a tile is the same tile on every
/// machine, which is the rule this module was written under.
fn simplex(seed: u64, channel: Channel, octave: u32, x: f64, y: f64) -> f64 {
    const F2: f64 = 0.366_025_403_784_438_6; // (sqrt 3 - 1) / 2
    const G2: f64 = 0.211_324_865_405_187_1; // (3 - sqrt 3) / 6
    let s = (x + y) * F2;
    let (i, j) = ((x + s).floor(), (y + s).floor());
    let t = (i + j) * G2;
    let (x0, y0) = (x - (i - t), y - (j - t));
    let (i1, j1) = if x0 > y0 { (1.0, 0.0) } else { (0.0, 1.0) };
    let corners = [
        (x0, y0, 0.0, 0.0),
        (x0 - i1 + G2, y0 - j1 + G2, i1, j1),
        (x0 - 1.0 + 2.0 * G2, y0 - 1.0 + 2.0 * G2, 1.0, 1.0),
    ];
    let mut total = 0.0;
    for (dx, dy, di, dj) in corners {
        let falloff = 0.5 - dx * dx - dy * dy;
        if falloff > 0.0 {
            let (gx, gy) = gradient(seed, channel, octave, (i + di) as i64, (j + dj) as i64);
            let f2 = falloff * falloff;
            total += f2 * f2 * (gx * dx + gy * dy);
        }
    }
    // Scaled so the extremes of eight unit gradients reach about one.
    (total * 99.0).clamp(-1.0, 1.0)
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

/// What one octave of noise makes of a raw simplex value.
#[derive(Clone, Copy)]
enum Shape {
    /// `[0, 1)`, even: rises, hollows, woods.
    Plain,
    /// Sharp crests where the noise crosses zero, broad troughs between:
    /// ranges with valleys, rather than blobs.
    Ridged,
}

/// Fractal noise at `hex`: `octaves` layers, each twice as fine and half as
/// strong as the last, normalised to `[0, 1]`. Each octave is turned by the
/// 3-4-5 angle and shifted, so their grids never line up — a rotation by a
/// Pythagorean triple is exact in `+` and `*`, with no sine to round
/// differently on another machine.
fn fractal(seed: u64, channel: Channel, hex: Hex, scale: u32, octaves: u32, shape: Shape) -> f64 {
    let (mut px, mut py) = plane(hex);
    let mut total = 0.0;
    let mut weight = 0.0;
    let mut amplitude = 1.0;
    let mut cell = scale.max(1) as f64;
    for octave in 0..octaves.max(1) {
        let n = simplex(seed, channel, octave, px / cell, py / cell);
        let v = match shape {
            Shape::Plain => 0.5 + 0.5 * n,
            Shape::Ridged => {
                let r = 1.0 - n.abs();
                r * r
            }
        };
        total += v * amplitude;
        weight += amplitude;
        amplitude *= 0.5;
        cell = (cell * 0.5).max(1.0);
        (px, py) = (0.6 * px - 0.8 * py + 131.0, 0.8 * px + 0.6 * py + 71.0);
    }
    total / weight
}

/// Plain fractal noise, the shape everything but the ridges is made of.
fn noise(seed: u64, channel: Channel, hex: Hex, scale: u32, octaves: u32) -> f64 {
    fractal(seed, channel, hex, scale, octaves, Shape::Plain)
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
    /// [`GENERATOR_VERSION`] when it was saved. Absent in a save from
    /// before the field, which was generator 1.
    #[serde(default = "first_generator")]
    generator: u32,
}

fn first_generator() -> u32 {
    1
}

impl Serialize for GeneratedWorld {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        SavedGeneration {
            seed: self.seed,
            rules: self.rules.clone(),
            chunk_radius: self.chunk_radius,
            generator: GENERATOR_VERSION,
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for GeneratedWorld {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let saved = SavedGeneration::deserialize(d)?;
        if saved.generator != GENERATOR_VERSION {
            return Err(serde::de::Error::custom(format!(
                "this world was made by generator {}, and this build makes worlds with \
                 generator {GENERATOR_VERSION}: regenerated, it would be different ground",
                saved.generator
            )));
        }
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

    /// The height of the land before it is cut into levels: three layers
    /// weighted by the mod (W6.4). The hills are the local rise and fall.
    /// The landform is the country's broad shape — uplands and lowlands
    /// several campaign hexes across, the scale at which a campaign map
    /// should read as regions rather than salt and pepper. The ridges are
    /// ranges with valleys between them, raised where the landform is high,
    /// because mountains stand on uplands. A mod that weights neither is the
    /// hills alone.
    fn relief_noise(&self, hex: Hex) -> f64 {
        let r = &self.rules.relief;
        let hills = noise(self.seed, Channel::Relief, hex, r.scale, r.octaves);
        let land_share = r.landform_percent.min(100) as f64 / 100.0;
        let ridge_share = r.ridge_percent.min(100) as f64 / 100.0;
        if land_share == 0.0 && ridge_share == 0.0 {
            return hills;
        }
        let land = noise(self.seed, Channel::Landform, hex, r.landform_scale, 2);
        let ridge = if ridge_share > 0.0 {
            let crest = fractal(
                self.seed,
                Channel::Ridge,
                hex,
                r.ridge_scale,
                3,
                Shape::Ridged,
            );
            // Ranges rise out of the uplands and die away in the lowlands.
            let upland = ((land - 0.35) / 0.4).clamp(0.0, 1.0);
            crest * (0.25 + 0.75 * upland)
        } else {
            0.0
        };
        let hill_share = (1.0 - land_share - ridge_share).max(0.0);
        hills * hill_share + land * land_share + ridge * ridge_share
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

    /// The campaign map this world adds up to: every campaign hex, at its
    /// chunk's coordinates, named and raised by [`Self::summary`]. The
    /// campaign's coordinates *are* the chunks', so a campaign hex and the
    /// ground under it are one address.
    pub fn campaign_map(&self) -> crate::map::HexMap {
        let mut map = crate::map::HexMap::default();
        for chunk in self.chunks() {
            if let Some(summary) = self.summary(chunk) {
                map.insert(chunk, &summary.terrain, summary.elevation);
            }
        }
        map
    }

    /// Where a column stands when it stands on campaign hex `chunk`: the
    /// square of a town centred in it, else the tile nearest its centre that
    /// a tracked vehicle can stand on — never the middle of a river.
    pub fn stand_tile(&self, registry: &DataRegistry, chunk: Hex) -> Hex {
        if let Some(town) = self
            .skeleton
            .towns
            .iter()
            .find(|t| chunk_of(t.centre, self.chunk_radius) == chunk)
        {
            return town.centre;
        }
        chunk_hexes(chunk, self.chunk_radius)
            .find(|h| {
                self.tile(*h).is_some_and(|(t, _)| {
                    registry
                        .terrain(t)
                        .is_some_and(|d| d.cost_for(crate::data::MovementClass::Tracked).is_some())
                })
            })
            .unwrap_or_else(|| crate::world::chunk_centre(chunk, self.chunk_radius))
    }

    /// The campaign hex each army begins on, resolving each [`Place`] against
    /// this world: the `rank`-th town (or factory town) furthest `toward`,
    /// and if another army already stands there, the nearest free campaign
    /// hex to it.
    ///
    /// [`Place`]: crate::map::Place
    pub fn place_armies(&self, places: &[crate::map::Place]) -> Vec<Hex> {
        let mut taken: Vec<Hex> = Vec::new();
        for place in places {
            let (dx, dy) = place.toward.vector();
            let mut towns: Vec<&Town> = self
                .skeleton
                .towns
                .iter()
                .filter(|t| place.feature == crate::map::PlaceFeature::Town || t.factory)
                .collect();
            towns.sort_by(|a, b| {
                let along = |t: &Town| {
                    let (x, y) = plane(t.centre);
                    x * dx + y * dy
                };
                along(b)
                    .total_cmp(&along(a))
                    .then(self.lot(a.centre).cmp(&self.lot(b.centre)))
            });
            let home = towns
                .get(place.rank as usize)
                .or(towns.last())
                .map_or(Hex::ZERO, |t| chunk_of(t.centre, self.chunk_radius));
            let spot = home
                .spiral_range(0..=self.rules.radius * 2)
                .find(|c| {
                    c.unsigned_distance_to(Hex::ZERO) <= self.rules.radius && !taken.contains(c)
                })
                .unwrap_or(home);
            taken.push(spot);
        }
        taken
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
        let count = self.rules.rivers.count(self.rules.hexes());
        let sources = self.pick_spaced(count, self.rules.rivers.spacing, |c| {
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
        let sites = self.pick_spaced(t.count(self.rules.hexes()), t.spacing, |c| {
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

/// A world drawn a pixel a tile, as rows of RGBA: what the setup screen
/// shows the player and what `examples/worldgen --picture` writes, one
/// drawing for both so the instrument and the screen cannot disagree.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Picture {
    /// The pixel `hex` falls on, if it is inside the picture. A row per `r`,
    /// and within it `q` shifted half a pixel a row, which puts every tile
    /// on a pixel of its own; rows are 0.866 of a column apart on the
    /// ground, which whoever shows the picture should stretch back.
    fn at(&self, origin: (i32, i32), hex: Hex) -> Option<usize> {
        let x = (2 * hex.x + hex.y).div_euclid(2) - origin.0;
        let y = hex.y - origin.1;
        (x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height)
            .then(|| ((y as u32 * self.width + x as u32) * 4) as usize)
    }
}

/// Draw `world`: each tile in its terrain's map colour, lighter the higher
/// it stands and lit from the north-west so slopes read as slopes; the
/// skeleton over it a touch heavier than its tiles, because at a pixel a
/// tile a river one tile wide vanishes and these are what a commander plans
/// around; factories ringed; and `marks` — a colour at a tile, the armies —
/// last.
pub fn picture(
    registry: &DataRegistry,
    world: &GeneratedWorld,
    marks: &[(Hex, [u8; 3])],
) -> Picture {
    let tiles: Vec<(Hex, &str, i32)> = world.chunks().flat_map(|c| world.chunk_tiles(c)).collect();
    let col = |h: Hex| (2 * h.x + h.y).div_euclid(2);
    let (min_x, max_x) = tiles
        .iter()
        .map(|(h, _, _)| col(*h))
        .fold((i32::MAX, i32::MIN), |(a, b), x| (a.min(x), b.max(x)));
    let (min_y, max_y) = tiles
        .iter()
        .map(|(h, _, _)| h.y)
        .fold((i32::MAX, i32::MIN), |(a, b), y| (a.min(y), b.max(y)));
    let origin = (min_x, min_y);
    let mut pic = Picture {
        width: (max_x - min_x + 1).max(1) as u32,
        height: (max_y - min_y + 1).max(1) as u32,
        rgba: Vec::new(),
    };
    pic.rgba = vec![0; (pic.width * pic.height * 4) as usize];
    let put = |pic: &mut Picture, hex: Hex, [r, g, b]: [u8; 3]| {
        if let Some(i) = pic.at(origin, hex) {
            pic.rgba[i..i + 4].copy_from_slice(&[r, g, b, 255]);
        }
    };
    let palette = &world.rules.terrain;
    let levels: HashMap<Hex, i32> = tiles.iter().map(|(h, _, l)| (*h, *l)).collect();
    let mut colours: HashMap<&str, [u8; 3]> = HashMap::new();
    const WATER: [u8; 3] = [60, 120, 215];
    for (hex, terrain, level) in &tiles {
        let base = *colours.entry(terrain).or_insert_with(|| {
            registry
                .terrain(terrain)
                .and_then(|t| crate::data::parse_color(&t.color))
                .unwrap_or([128, 128, 128])
        });
        let colour = if *terrain == palette.water {
            WATER
        } else {
            let upslope = levels
                .get(&(*hex + Hex::new(0, -1)))
                .copied()
                .unwrap_or(*level);
            let lit = (*level - upslope) as f32 * 0.16;
            let height = 0.70 + 0.07 * (*level).clamp(0, 9) as f32;
            base.map(|c| (c as f32 * (height + lit)).clamp(0.0, 255.0) as u8)
        };
        put(&mut pic, *hex, colour);
    }
    for river in &world.skeleton.rivers {
        for hex in river {
            put(&mut pic, *hex, WATER);
            put(&mut pic, *hex + Hex::new(1, 0), WATER);
        }
    }
    for road in &world.skeleton.roads {
        for hex in road {
            put(&mut pic, *hex, [214, 196, 150]);
        }
    }
    for town in &world.skeleton.towns {
        for hex in town.centre.range(town.radius) {
            put(&mut pic, hex, [236, 232, 220]);
        }
        if town.factory {
            for ring in [town.radius + 1, town.radius + 2] {
                for hex in town.centre.ring(ring) {
                    put(&mut pic, hex, [250, 200, 30]);
                }
            }
        }
    }
    for (at, colour) in marks {
        for hex in at.range(6) {
            put(&mut pic, hex, *colour);
        }
    }
    pic
}
