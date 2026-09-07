//! Moddable game data: JSON definitions and the registry that loads them.

mod ammo;
mod balance;
mod casualties;
mod command;
mod cores;
mod defs;
mod manifest;
mod modules;
mod morale;
mod planner;
mod registry;
mod scale;

pub use ammo::{AmmoClass, AmmoDef};
pub use balance::{Balance, ReactionRules};
pub use casualties::Casualties;
pub use command::CommandRules;
pub use cores::{
    AVERAGE, CheckContext, CoreDef, CoreIndex, RoleDef, SkillDef, TraitCondition, TraitDef,
    TraitEffect,
};
pub use defs::*;
pub use manifest::ModManifest;
pub use modules::{ModuleDef, ModuleEffect, STANDARD_MODULES};
pub use morale::{
    DefianceDef, DefianceResponse, MoraleRules, MoraleRung, RoundPressure, ShotFelt, holds_together,
};
pub use planner::PlannerRules;
pub use registry::{DataError, DataRegistry, ValidationReport, parse_color};
pub use scale::Scale;
