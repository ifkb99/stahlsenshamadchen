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
use std::collections::BTreeMap;

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
    pub villages: Villages,
    #[serde(default)]
    pub roads: Roads,
    /// Which terrain each kind of ground is written as.
    #[serde(default)]
    pub terrain: GroundPalette,
    /// How a campaign hex is named from the tiles inside it: the first rule
    /// that holds wins, so the last should hold always.
    #[serde(default = "default_summary")]
    pub summary: Vec<SummaryRule>,
    /// How many tiles past the furthest any crew could see, shoot or plan a
    /// march the world is kept loaded (WORLD.md W1.5). The rule is that no
    /// question a battle asks may reach unloaded ground; this is the slack
    /// for the questions nobody has counted.
    #[serde(default = "default_residency_margin")]
    pub residency_margin: u32,
}

fn default_residency_margin() -> u32 {
    4
}

fn default_radius() -> u32 {
    6
}

impl WorldGen {
    /// How many campaign hexes a world of these rules holds: a hexagon of
    /// [`Self::radius`].
    pub fn hexes(&self) -> u32 {
        3 * self.radius * self.radius + 3 * self.radius + 1
    }

    /// These rules as the player chose to have them: every setting's chosen
    /// option (its default where `choices` says nothing) applied in the
    /// order the settings are declared, each option's fields patched by
    /// their json path. A world made with every setting at its default is
    /// these rules exactly, so long as every default option sets nothing —
    /// which `validate-mods` warns about when it does not.
    pub fn with_settings(
        &self,
        settings: &[WorldSetting],
        choices: &BTreeMap<String, String>,
    ) -> Result<WorldGen, SettingError> {
        for id in choices.keys() {
            if !settings.iter().any(|s| &s.id == id) {
                return Err(SettingError::UnknownSetting(id.clone()));
            }
        }
        let mut json =
            serde_json::to_value(self).map_err(|e| SettingError::Rules(e.to_string()))?;
        for setting in settings {
            let chosen = choices.get(&setting.id).unwrap_or(&setting.default);
            let option = setting
                .option(chosen)
                .ok_or_else(|| SettingError::UnknownOption {
                    setting: setting.id.clone(),
                    option: chosen.clone(),
                })?;
            for (path, value) in &option.set {
                super::patch::patch_json(&mut json, path, value.clone()).map_err(|e| {
                    SettingError::Field {
                        setting: setting.id.clone(),
                        option: option.id.clone(),
                        path: path.clone(),
                        why: e,
                    }
                })?;
            }
        }
        serde_json::from_value(json).map_err(|e| SettingError::Rules(e.to_string()))
    }
}

/// One choice the player makes about the world before a campaign, the way a
/// strategy game's map setup offers "map size" or "rainfall": a name, the
/// options, and which of them a world gets when nobody chooses (WORLD.md
/// W6.1). Each option is only the `worldgen` fields it sets, so a setting is
/// content — a mod adds one, or an option to one, without a line of Rust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSetting {
    pub id: String,
    pub name: String,
    /// The option a world gets when the player says nothing. It should set
    /// nothing, so that a world with every setting at its default is the
    /// mod's own world.
    pub default: String,
    pub options: Vec<WorldOption>,
}

impl WorldSetting {
    /// The option called `id`.
    pub fn option(&self, id: &str) -> Option<&WorldOption> {
        self.options.iter().find(|o| o.id == id)
    }
}

/// One option of a [`WorldSetting`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldOption {
    pub id: String,
    pub name: String,
    /// A line for the setup screen saying what the option makes.
    #[serde(default)]
    pub text: String,
    /// The `worldgen` fields this option sets, by json path within the
    /// block (`cover.wood_percent`, `relief.shares`), and to what.
    #[serde(default)]
    pub set: BTreeMap<String, serde_json::Value>,
}

/// What the player chose for a world: its seed, if she named one, and an
/// option for any settings she changed. What she did not change is the
/// setting's default, so an empty choice is the mod's own world.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSetup {
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default)]
    pub choices: BTreeMap<String, String>,
}

impl WorldSetup {
    /// `size=large,woodland=heavy` — how the instruments and `STAHL_WORLD`
    /// write a choice. `default` and the empty string are no choice at all;
    /// `seed=<n>` names the seed.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut setup = Self::default();
        for pair in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            if pair == "default" {
                continue;
            }
            let (key, value) = pair
                .split_once('=')
                .ok_or_else(|| format!("`{pair}` is not `setting=option`"))?;
            if key == "seed" {
                setup.seed = Some(
                    value
                        .parse()
                        .map_err(|_| format!("`{value}` is not a seed"))?,
                );
            } else {
                setup.choices.insert(key.to_string(), value.to_string());
            }
        }
        Ok(setup)
    }
}

/// Why a player's choices could not be made into rules.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingError {
    #[error("no world setting is called `{0}`")]
    UnknownSetting(String),
    #[error("world setting `{setting}` has no option `{option}`")]
    UnknownOption { setting: String, option: String },
    #[error("world setting `{setting}`, option `{option}`: `{path}`: {why}")]
    Field {
        setting: String,
        option: String,
        path: String,
        why: String,
    },
    #[error("the rules the settings make do not read back: {0}")]
    Rules(String),
}

impl Default for WorldGen {
    fn default() -> Self {
        Self {
            radius: default_radius(),
            relief: Relief::default(),
            cover: Cover::default(),
            rivers: Rivers::default(),
            towns: Towns::default(),
            villages: Villages::default(),
            roads: Roads::default(),
            terrain: GroundPalette::default(),
            summary: default_summary(),
            residency_margin: default_residency_margin(),
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
    /// How much of the height is the country's broad shape — uplands and
    /// lowlands — in percent (W6.4). Zero is none: the hills alone.
    pub landform_percent: u32,
    /// How wide an upland or a lowland is, roughly, in tiles. A campaign
    /// hex is about forty across, so a few hundred is a region of them.
    pub landform_scale: u32,
    /// How much of the height is ranges with valleys between them, in
    /// percent. Zero is none.
    pub ridge_percent: u32,
    /// How far apart the ranges run, roughly, in tiles.
    pub ridge_scale: u32,
}

impl Default for Relief {
    fn default() -> Self {
        Self {
            scale: 48,
            octaves: 3,
            shares: vec![38, 24, 15, 10, 7, 4, 2],
            landform_percent: 0,
            landform_scale: 240,
            ridge_percent: 0,
            ridge_scale: 120,
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
    /// Share of the land that is hedge, percent: the boundaries of the
    /// fields in hedgerow country (W6.6), which comes in districts.
    pub hedge_percent: u32,
    /// Share of the lowest ground that is wet, percent.
    pub wet_percent: u32,
    /// How much the woods lean toward high ground, in percent of the wood
    /// score (W6.6). Zero is none: woods wherever the cover noise says.
    pub wood_on_high: u32,
    /// How much the woods lean toward steep ground, likewise.
    pub wood_on_slope: u32,
    /// How wide a field is, roughly, in tiles: the spacing of the hedges in
    /// hedgerow country.
    pub field_size: u32,
    /// How wide a district of hedgerow country is, roughly, in tiles.
    pub bocage_scale: u32,
}

impl Default for Cover {
    fn default() -> Self {
        Self {
            scale: 18,
            wood_percent: 28,
            hedge_percent: 9,
            wet_percent: 20,
            wood_on_high: 0,
            wood_on_slope: 0,
            field_size: 5,
            bocage_scale: 90,
        }
    }
}

/// Where the water runs (WORLD.md W6.5): a drainage network, not a count
/// of rivers. Every tile drains somewhere, down to the rim of the world;
/// what a watercourse is depends on how much land drains through it, in
/// campaign hexes of catchment, the way a real one does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rivers {
    /// Catchment, in campaign hexes, at which water gathers into a stream:
    /// a tile wide, fordable and slow (`terrain.stream`). Zero is no running
    /// water at all.
    pub stream: u32,
    /// Catchment at which a stream is a river: a tile of `terrain.water`,
    /// crossed at a bridge or not at all.
    pub river: u32,
    /// Catchment at which a river is broad: three tiles wide.
    pub broad: u32,
    /// How far, in tiles, a river's floodplain reaches from its bank: the
    /// valley floor beside it, where the meadows are wet.
    pub floodplain: u32,
    /// The share of a floodplain, in percent, that is wet meadow.
    pub meadow_percent: u32,
}

impl Default for Rivers {
    fn default() -> Self {
        Self {
            stream: 3,
            river: 12,
            broad: 60,
            floodplain: 2,
            meadow_percent: 50,
        }
    }
}

/// `hexes / every`, rounded to the nearest, and never fewer than one while
/// `every` asks for any at all.
fn per(hexes: u32, every: u32) -> u32 {
    if every == 0 {
        return 0;
    }
    ((hexes + every / 2) / every).max(1)
}

/// Where people live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Towns {
    /// One town for every this many campaign hexes of world: a density, like
    /// [`Rivers::every`], and for the same reason.
    pub every: u32,
    /// How far apart, in campaign hexes, at the least.
    pub spacing: u32,
    /// How far a town reaches from its centre, in tiles.
    pub radius: u32,
    /// How many of them have a factory — the ground a campaign is fought
    /// over. A count, not a density: a campaign's ending names every
    /// factory, so how many there are is the length of the war, and a
    /// larger map should not quietly make it longer.
    pub factories: u32,
    /// What a town is worth to a fight near it, as an objective's value
    /// (WORLD.md W3.4), and what a factory town is.
    pub worth: u32,
    pub factory_worth: u32,
    /// How near a fight a town has to be, in tiles, to be fought over.
    pub contested_within: u32,
    /// How many of the towns are cities (W6.7): the best sites, and the
    /// factories go to them first. Zero is every town a town.
    pub cities: u32,
    /// How far a city reaches from its centre, in tiles. Zero is a town's
    /// `radius`.
    pub city_radius: u32,
}

impl Default for Towns {
    fn default() -> Self {
        // Eight in the default world of 127 hexes, as `count: 8` was.
        Self {
            every: 16,
            spacing: 2,
            radius: 3,
            factories: 2,
            worth: 2,
            factory_worth: 4,
            contested_within: 30,
            cities: 0,
            city_radius: 0,
        }
    }
}

impl Towns {
    /// How many towns a world of `hexes` campaign hexes has.
    pub fn count(&self, hexes: u32) -> u32 {
        per(hexes, self.every)
    }
}

/// The villages between the towns (W6.7): a cluster of houses a tile or two
/// across, on low ground by water, every few kilometres, the way farmland
/// is settled. Not objectives and not campaign hexes' names — a campaign
/// that fought over every village would be about nothing else — but cover
/// on the ground, and a tactical fact wherever a fight comes near one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Villages {
    /// How many villages for every ten campaign hexes of world. Zero is
    /// none, the game before.
    pub per_ten_hexes: u32,
    /// How far a village reaches from its centre, in tiles.
    pub radius: u32,
    /// How far apart villages stand, and how far from a town, in tiles.
    pub spacing: u32,
}

impl Default for Villages {
    fn default() -> Self {
        Self {
            per_ten_hexes: 0,
            radius: 1,
            spacing: 12,
        }
    }
}

impl Villages {
    /// How many villages a world of `hexes` campaign hexes has.
    pub fn count(&self, hexes: u32) -> u32 {
        (hexes * self.per_ten_hexes + 5) / 10
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
    /// Running water too small to need a bridge (WORLD.md W6.5).
    pub stream: String,
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
            stream: "stream".into(),
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
