//! How a world is made: the `worldgen` block of `mod.json`.
//!
//! WORLD.md W1.2–W1.4. The designer's ruling (2026-09-26) is that everything
//! is generated — relief, cover, rivers, towns, roads, and from those the
//! campaign map — and that the tiles are the truth: a campaign hex is a
//! summary of the ground inside it. Every number the generator uses is here,
//! because content is data; the algorithm is `crate::worldgen`.
//!
//! Optional as a block, like `command`: a mod that declares none cannot
//! generate a world, and every scenario and every hand-drawn campaign plays
//! exactly as it did. Inside the block every field defaults, so a mod can
//! say only what it wants different.

use serde::{Deserialize, Serialize};

/// Everything the generator is told.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldGen {
    /// How far the world reaches, in campaign hexes from its centre. A
    /// radius of 6 is 127 campaign hexes — about `frontier`'s 126 — and
    /// 160,000 tiles.
    #[serde(default = "default_radius")]
    pub radius: u32,
    #[serde(default)]
    pub relief: Relief,
    #[serde(default)]
    pub cover: Cover,
    #[serde(default)]
    pub rivers: Rivers,
    #[serde(default)]
    pub towns: Towns,
    #[serde(default)]
    pub roads: Roads,
    /// Which terrain each kind of ground is written as.
    #[serde(default)]
    pub terrain: GroundPalette,
    /// How a campaign hex is named from the tiles inside it: the first rule
    /// that holds wins, so the last should hold always.
    #[serde(default = "default_summary")]
    pub summary: Vec<SummaryRule>,
}

fn default_radius() -> u32 {
    6
}

impl Default for WorldGen {
    fn default() -> Self {
        Self {
            radius: default_radius(),
            relief: Relief::default(),
            cover: Cover::default(),
            rivers: Rivers::default(),
            towns: Towns::default(),
            roads: Roads::default(),
            terrain: GroundPalette::default(),
            summary: default_summary(),
        }
    }
}

/// The lie of the land.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Relief {
    /// How wide a rise is, roughly, in tiles: the spacing of the coarsest
    /// noise lattice.
    pub scale: u32,
    /// How many octaves of finer noise ride on it.
    pub octaves: u32,
    /// The share of the land, in percent, standing at each elevation level
    /// from 0 upward. Its length is how many levels there are (at most ten)
    /// and it should add up to 100. Calibrated against the world's own
    /// noise, so these are the shares a world comes out with.
    pub shares: Vec<u32>,
}

impl Default for Relief {
    fn default() -> Self {
        Self {
            scale: 48,
            octaves: 3,
            shares: vec![38, 24, 15, 10, 7, 4, 2],
        }
    }
}

/// What grows on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cover {
    /// How wide a wood is, roughly, in tiles.
    pub scale: u32,
    /// Share of the land under wood, percent.
    pub wood_percent: u32,
    /// Share of the land in hedged fields, percent: the band of ground just
    /// short of being woodland, so hedges fringe the woods.
    pub hedge_percent: u32,
    /// Share of the lowest ground that is wet, percent.
    pub wet_percent: u32,
}

impl Default for Cover {
    fn default() -> Self {
        Self {
            scale: 18,
            wood_percent: 28,
            hedge_percent: 9,
            wet_percent: 20,
        }
    }
}

/// Where the water runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rivers {
    /// How many rivers the world has.
    pub count: u32,
    /// How far apart their sources must be, in campaign hexes.
    pub spacing: u32,
}

impl Default for Rivers {
    fn default() -> Self {
        Self {
            count: 2,
            spacing: 3,
        }
    }
}

/// Where people live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Towns {
    /// How many towns.
    pub count: u32,
    /// How far apart, in campaign hexes, at the least.
    pub spacing: u32,
    /// How far a town reaches from its centre, in tiles.
    pub radius: u32,
    /// How many of them have a factory — the ground a campaign is fought
    /// over.
    pub factories: u32,
}

impl Default for Towns {
    fn default() -> Self {
        Self {
            count: 8,
            spacing: 2,
            radius: 3,
            factories: 2,
        }
    }
}

/// How the towns are joined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Roads {
    /// Roads beyond the fewest that join every town, shortest first: the
    /// loops that give a commander a second way round.
    pub extra_links: u32,
}

impl Default for Roads {
    fn default() -> Self {
        Self { extra_links: 2 }
    }
}

/// Which terrain id each kind of generated ground is written as.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GroundPalette {
    pub open: String,
    pub wood: String,
    pub hedge: String,
    pub wet: String,
    pub water: String,
    pub road: String,
    pub town: String,
}

impl Default for GroundPalette {
    fn default() -> Self {
        Self {
            open: "grass".into(),
            wood: "forest".into(),
            hedge: "hedgerow".into(),
            wet: "mud".into(),
            water: "water".into(),
            road: "road".into(),
            town: "town".into(),
        }
    }
}

/// A feature of the skeleton a campaign hex can be named for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkeletonFeature {
    Factory,
    Town,
    Road,
    River,
}

/// A terrain's share of a campaign hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerrainShare {
    pub terrain: String,
    pub percent: u32,
}

/// One rule for naming a campaign hex. Every condition it states must hold;
/// a rule that states none holds always.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryRule {
    /// The campaign terrain the hex is called if this rule holds.
    pub terrain: String,
    /// The hex holds this feature of the skeleton.
    #[serde(default)]
    pub feature: Option<SkeletonFeature>,
    /// The mean elevation of its tiles is at least this many tenths of a
    /// level.
    #[serde(default)]
    pub mean_elevation_tenths: Option<u32>,
    /// At least this share of its tiles is this terrain.
    #[serde(default)]
    pub share: Option<TerrainShare>,
}

fn default_summary() -> Vec<SummaryRule> {
    let rule = |terrain: &str| SummaryRule {
        terrain: terrain.into(),
        feature: None,
        mean_elevation_tenths: None,
        share: None,
    };
    vec![
        SummaryRule {
            feature: Some(SkeletonFeature::Factory),
            ..rule("factory")
        },
        SummaryRule {
            feature: Some(SkeletonFeature::Town),
            ..rule("city")
        },
        SummaryRule {
            mean_elevation_tenths: Some(35),
            ..rule("mountains")
        },
        SummaryRule {
            share: Some(TerrainShare {
                terrain: "forest".into(),
                percent: 40,
            }),
            ..rule("deep_forest")
        },
        SummaryRule {
            feature: Some(SkeletonFeature::Road),
            ..rule("highway")
        },
        rule("plains"),
    ]
}
