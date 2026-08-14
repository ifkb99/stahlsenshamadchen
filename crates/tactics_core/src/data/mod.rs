//! Moddable game data: JSON definitions and the registry that loads them.

mod ammo;
mod balance;
mod command;
mod cores;
mod defs;
mod manifest;
mod morale;
mod registry;
mod scale;

pub use ammo::{AmmoClass, AmmoDef};
pub use balance::{Balance, ReactionRules};
pub use command::CommandRules;
pub use cores::{
    AVERAGE, CheckContext, CoreDef, CoreIndex, RoleDef, SkillDef, TraitCondition, TraitDef,
    TraitEffect,
};
pub use defs::*;
pub use manifest::ModManifest;
pub use morale::{MoraleRules, MoraleRung, holds_together};
pub use registry::{DataError, DataRegistry, ValidationReport, parse_color};
pub use scale::Scale;
