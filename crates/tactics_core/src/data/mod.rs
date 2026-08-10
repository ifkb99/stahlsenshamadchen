//! Moddable game data: JSON definitions and the registry that loads them.

mod balance;
mod cores;
mod defs;
mod manifest;
mod registry;
mod scale;

pub use balance::{Balance, ReactionRules};
pub use cores::{
    AVERAGE, CheckContext, CoreDef, CoreIndex, RoleDef, SkillDef, TraitCondition, TraitDef,
    TraitEffect,
};
pub use defs::*;
pub use manifest::ModManifest;
pub use registry::{DataError, DataRegistry, ValidationReport, parse_color};
pub use scale::Scale;
