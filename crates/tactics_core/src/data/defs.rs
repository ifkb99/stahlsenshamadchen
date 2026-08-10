//! Serde definitions for everything a mod can declare.
//!
//! Every definition carries a string `id` that other definitions reference.
//! Mods loaded later (in dependency order) may override earlier definitions
//! by re-declaring the same id.

use super::Scale;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// How a unit traverses terrain. Terrain definitions price movement per
/// class; a missing entry means that class cannot enter the tile at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovementClass {
    Foot,
    Wheeled,
    Tracked,
    Boat,
    Air,
}

/// A person. Rides in vehicles, has a face and a name.
///
/// Characters are people; vehicles are hardware, and a unit on the battlefield
/// is always a crew inside one. What she can *do* is not written here — see
/// [`crate::data::SkillDef`] — only who she is and what she has been taught.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacterDef {
    pub id: String,
    pub name: String,
    /// Temperament, by core id. Anything unnamed sits at
    /// [`crate::data::AVERAGE`], so a definition states only what makes her
    /// unusual.
    #[serde(default)]
    pub cores: std::collections::HashMap<String, i32>,
    /// What she has been taught, by skill id. A skill she is not listed for is
    /// untrained and falls back to her cores at a penalty — not to zero.
    #[serde(default)]
    pub skills: std::collections::HashMap<String, i32>,
    /// Trait ids she starts with. The rest she earns.
    #[serde(default)]
    pub traits: Vec<String>,
    /// Asset-relative path to a portrait image, e.g. `mods/base/portraits/anka.png`.
    #[serde(default)]
    pub portrait: Option<String>,
    #[serde(default)]
    pub bio: String,
}

/// Directional armor. Which facing an incoming shot strikes is derived from
/// the attack direction relative to the target's facing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArmorSpec {
    pub front: i32,
    pub side: i32,
    pub rear: i32,
}

/// The facing arc a shot lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmorFacing {
    Front,
    Side,
    Rear,
}

impl ArmorSpec {
    pub fn value(&self, facing: ArmorFacing) -> i32 {
        match facing {
            ArmorFacing::Front => self.front,
            ArmorFacing::Side => self.side,
            ArmorFacing::Rear => self.rear,
        }
    }
}

fn default_max_climb() -> i32 {
    1
}

/// Movement capability of a vehicle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovementSpec {
    pub class: MovementClass,
    /// Movement points per turn.
    pub points: u32,
    /// Maximum elevation difference (up or down) the vehicle can cross in a
    /// single step.
    #[serde(default = "default_max_climb")]
    pub max_climb: i32,
}

/// A vehicle chassis: the hardware half of a battlefield unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VehicleDef {
    pub id: String,
    pub name: String,
    /// Asset-relative sprite path; placeholder art is generated when absent.
    #[serde(default)]
    pub sprite: Option<String>,
    /// Freeform classification tag ("light_tank", "artillery", ...).
    #[serde(default)]
    pub class: String,
    /// Hit points, Advance Wars style (typically 10).
    pub max_hp: i32,
    pub movement: MovementSpec,
    pub armor: ArmorSpec,
    /// Base vision range in hexes, before crew bonuses.
    pub vision_range: u32,
    /// Weapon definition ids, in display order. Index 0 is the main gun.
    pub weapons: Vec<String>,
    /// Crew roles this vehicle needs ("commander", "driver", "gunner", ...).
    #[serde(default)]
    pub crew_slots: Vec<String>,
    /// How survivable this vehicle is for the girls inside it, 0-5.
    ///
    /// Separate from armour on purpose: armour decides whether the vehicle
    /// dies, safety decides what that costs its crew. A thinly armoured car
    /// everyone can jump out of and a heavy tank whose ammunition sits under
    /// the turret floor are different problems, and the roster is where the
    /// difference shows up. See [`crate::roster::resolve_crew_fate`].
    #[serde(default = "default_safety")]
    pub safety: i32,
    /// Requisition cost on the overworld.
    #[serde(default)]
    pub cost: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DamageType {
    /// Armor-piercing shells: full armor interaction.
    Kinetic,
    /// High explosive: armor facing counts half.
    Explosive,
    /// Small arms / MG: poor against any armor.
    SmallArms,
}

/// Middling protection, for content written before `safety` existed.
fn default_safety() -> i32 {
    3
}

/// A weapon mounted on a vehicle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeaponDef {
    pub id: String,
    pub name: String,
    /// Base damage on the AW-style 10-HP scale.
    pub damage: i32,
    /// Penetration vs armor; damage scales by `pen / (pen + armor)`.
    pub penetration: i32,
    pub damage_type: DamageType,
    /// `[min, max]` range in hexes. A min of 2+ means no point-blank shots
    /// (typical for artillery).
    pub range: [u32; 2],
    /// Base hit chance percentage at closest range.
    pub accuracy: i32,
    /// Hit chance lost per hex of distance beyond the first.
    #[serde(default)]
    pub accuracy_falloff: i32,
    /// Indirect-fire weapons may fire without line of sight if a friendly
    /// unit spots the target.
    #[serde(default)]
    pub indirect: bool,
    /// Ticks before this weapon can fire again, as a practical aimed rate of
    /// fire rather than a mechanical reload — at the default scale a tick is
    /// 5 s, so 4 is a shot every 20 s.
    ///
    /// `None` means a full round, so a weapon that says nothing keeps the
    /// one-shot-per-round cadence; anything faster fires several times while
    /// a round plays out. This is an `Option` rather than a serde default
    /// because "a full round" is now [`Scale::ticks_per_round`], which lives
    /// in the mod being loaded and is not reachable from a `fn() -> u32`.
    /// Read it through [`Self::reload`].
    #[serde(default)]
    pub reload_ticks: Option<u32>,
}

impl WeaponDef {
    /// Ticks between shots, resolving the "a full round" default against the
    /// scale in force.
    pub fn reload(&self, scale: &Scale) -> u32 {
        self.reload_ticks.unwrap_or(scale.ticks_per_round).max(1)
    }
}

/// How a side fights, as opposed to how well it thinks.
///
/// Doctrine is the identity knob: two academies running the same planner at
/// the same difficulty should still be recognisable opponents, one massing
/// armour and trading fire while the other cedes ground and shoots from
/// cover. Planners read these weights instead of hard-coding coefficients,
/// which is what lets a mod add a new fighting style without Rust.
///
/// Every field is defaulted, so a doctrine file may set only what it cares
/// about. Weights are multipliers around 1.0 unless noted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DoctrineDef {
    pub id: String,
    pub name: String,
    pub description: String,
    /// 0..=1: how much expected damage outweighs self-preservation.
    pub aggression: f32,
    /// Worth of terrain cover when choosing where to sit.
    pub cover_value: f32,
    /// Worth of high ground.
    pub elevation_value: f32,
    /// Preference for staying near friends and concentrating fire, rather
    /// than spreading across the map.
    pub concentration: f32,
    /// Appetite for advancing into ground nobody has scouted.
    pub scouting: f32,
    /// How much the ground a map declares worth taking is worth to this
    /// doctrine, as a multiplier on each objective's own value.
    ///
    /// Defaulted rather than required because a doctrine written before
    /// objectives existed must not silently become one that ignores them —
    /// a planner that does not care about the objectives loses on points
    /// without ever noticing there was anything to lose.
    #[serde(default = "default_objective_value")]
    pub objective_value: f32,
    /// Willingness to spend indirect fire rather than hold it.
    pub indirect_appetite: f32,
    /// Fraction of starting strength lost before the side looks for a way
    /// out. Reserved until morale and withdrawal exist.
    pub withdraw_threshold: f32,
    /// Reserved for chain of command: acting without orders when out of
    /// contact with a commander.
    pub initiative: f32,
    /// Reserved for chain of command: how much a commander devolves
    /// decisions to subordinates.
    pub delegation: f32,
}

fn default_objective_value() -> f32 {
    1.0
}

impl Default for DoctrineDef {
    /// The balanced doctrine, used when a side names none. These are the
    /// coefficients the utility planner used before doctrine existed, so
    /// behaviour without a doctrine file is unchanged.
    fn default() -> Self {
        Self {
            id: "balanced".into(),
            name: "Balanced".into(),
            description: "No pronounced style.".into(),
            aggression: 0.6,
            cover_value: 1.0,
            elevation_value: 1.0,
            concentration: 1.0,
            scouting: 1.0,
            objective_value: default_objective_value(),
            indirect_appetite: 1.0,
            withdraw_threshold: 0.7,
            initiative: 0.5,
            delegation: 0.5,
        }
    }
}

/// A terrain type occupying one hex tile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainDef {
    pub id: String,
    pub name: String,
    /// Default palette glyph, used by map generators. Map files may remap
    /// glyphs freely in their own palettes.
    #[serde(default)]
    pub glyph: Option<char>,
    /// Asset-relative sprite path; placeholder art is generated when absent.
    #[serde(default)]
    pub sprite: Option<String>,
    /// Placeholder-art color as `#rrggbb`.
    #[serde(default = "default_terrain_color")]
    pub color: String,
    /// Movement point cost to enter, per movement class. Missing class =
    /// impassable for that class.
    #[serde(default)]
    pub move_cost: HashMap<MovementClass, u32>,
    /// Percentage damage reduction granted to occupants.
    #[serde(default)]
    pub cover: i32,
    /// Extra height (in elevation steps) this terrain adds when blocking
    /// line of sight, e.g. forests and buildings.
    #[serde(default)]
    pub vision_block: i32,
    /// On the overworld, armies inside concealing terrain are hidden from
    /// enemies unless adjacent.
    #[serde(default)]
    pub concealing: bool,
    /// Overworld: funds generated per turn for the owning side.
    #[serde(default)]
    pub income: i32,
    /// Overworld: whether an army can capture this tile as an objective.
    #[serde(default)]
    pub capturable: bool,
}

fn default_terrain_color() -> String {
    "#808080".to_string()
}

impl TerrainDef {
    /// Movement cost for the class, `None` if impassable.
    pub fn cost_for(&self, class: MovementClass) -> Option<u32> {
        self.move_cost.get(&class).copied()
    }
}
