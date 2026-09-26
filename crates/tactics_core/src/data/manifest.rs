use super::{Balance, Casualties, PlannerRules, Scale};
use serde::{Deserialize, Serialize};

/// `mod.json` at the root of every mod directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModManifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Ids of mods that must load before this one. Later mods override
    /// definitions with the same id.
    #[serde(default)]
    pub dependencies: Vec<String>,
    /// The axes of temperament in this game. Like [`Self::scale`], a whole
    /// block replaces the previous one rather than merging: a mod that
    /// redefines the cores is defining a different kind of person, and a
    /// half-merged set would be neither.
    #[serde(default)]
    pub cores: Option<Vec<super::CoreDef>>,
    /// Trainable skills, each naming the cores it draws on. Replaced
    /// wholesale for the same reason.
    #[serde(default)]
    pub skills: Option<Vec<super::SkillDef>>,
    /// Jobs aboard a vehicle, and which skills each answers for.
    #[serde(default)]
    pub roles: Option<Vec<super::RoleDef>>,
    /// Things that are true about a cadet but are not numbers.
    #[serde(default)]
    pub traits: Option<Vec<super::TraitDef>>,
    /// How long crews take to act on orders. Replaced wholesale, like the
    /// other rule blocks — a difficulty mod says only what it changes.
    #[serde(default)]
    pub reaction: Option<super::ReactionRules>,
    /// What a crew can take before it stops doing as it is told. A mod that
    /// ships a one-rung ladder has cadets who never waver.
    #[serde(default)]
    pub morale: Option<super::MoraleRules>,
    /// How far an order carries and how long it takes to arrive. Replaced
    /// wholesale like the blocks above; a mod that declares none has a side
    /// whose every unit is always in contact and whose missions land the
    /// instant they are given, which is the game before chains of command
    /// existed.
    #[serde(default)]
    pub command: Option<super::CommandRules>,
    /// The ladder of rank, lowest first. Replaced wholesale: a ladder
    /// half-merged from two mods would put somebody's lieutenant between
    /// somebody else's sergeants.
    #[serde(default)]
    pub ranks: Option<Vec<super::RankDef>>,
    /// What the engine's hexes, rounds and ticks mean in metres and seconds.
    ///
    /// Unlike a vehicle or a weapon, scale is a property of the game rather
    /// than of one definition, so there is exactly one in effect at a time:
    /// the last mod in load order that declares a block wins outright. A mod
    /// that only adds content says nothing here and inherits whatever the
    /// game it is extending decided.
    #[serde(default)]
    pub scale: Option<Scale>,
    /// What a point of crew skill is worth. Same one-in-effect rule as
    /// [`Self::scale`].
    #[serde(default)]
    pub balance: Option<Balance>,
    /// What a battle costs the cadets who fought it. Same one-in-effect rule
    /// as [`Self::scale`]: a campaign that wants attrition to bite declares
    /// this block and changes nothing else.
    #[serde(default)]
    pub casualties: Option<Casualties>,
    /// How the AI thinks: what it will drive for, how far ahead it looks,
    /// when a commander stops assigning ground. Deliberately its own block
    /// rather than fields on [`Self::balance`] — nothing in it reaches a
    /// rule, so a mod that rewrote every field would leave a human-versus-
    /// human battle bit-for-bit identical. Same one-in-effect rule as
    /// [`Self::scale`].
    #[serde(default)]
    pub planner: Option<PlannerRules>,
    /// How a world is made (WORLD.md, W1). Optional in the registry as well
    /// as here: a mod that declares none cannot generate a world, and plays
    /// every scenario and hand-drawn campaign as it did. Same one-in-effect
    /// rule as [`Self::scale`].
    #[serde(default)]
    pub worldgen: Option<super::WorldGen>,
}
