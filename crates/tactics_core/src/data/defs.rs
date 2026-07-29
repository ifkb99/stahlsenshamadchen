//! Serde definitions for everything a mod can declare.
//!
//! Every definition carries a string `id` that other definitions reference.
//! Mods loaded later (in dependency order) may override earlier definitions
//! by re-declaring the same id.

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

/// Crew member stats. Characters are people; vehicles are hardware. A unit on
/// the battlefield is always a crew inside a vehicle, and both sides of that
/// pairing contribute to its performance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct CrewStats {
    /// Improves hit chance when firing.
    pub gunnery: i32,
    /// Improves movement (bonus movement points at high values).
    pub driving: i32,
    /// Improves vision range.
    pub awareness: i32,
    /// Resistance to panic (reserved for future morale systems).
    pub morale: i32,
    /// Buffs nearby friendly units (reserved for future aura systems).
    pub leadership: i32,
}

/// A person. Rides in vehicles, has a face and a name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CharacterDef {
    pub id: String,
    pub name: String,
    /// Asset-relative path to a portrait image, e.g. `mods/base/portraits/anka.png`.
    #[serde(default)]
    pub portrait: Option<String>,
    #[serde(default)]
    pub bio: String,
    #[serde(default)]
    pub stats: CrewStats,
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
