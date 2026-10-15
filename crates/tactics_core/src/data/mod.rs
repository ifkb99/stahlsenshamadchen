//! Moddable game data: JSON definitions and the registry that loads them.

mod ammo;
mod balance;
mod casualties;
mod command;
mod cores;
mod defs;
mod forces;
mod manifest;
mod march;
mod modules;
mod morale;
mod net;
pub mod patch;
mod planner;
mod registry;
mod scale;
mod templates;
mod worldgen;

pub use ammo::{AmmoClass, AmmoDef, SmokeDef};
pub use balance::{Balance, ReactionRules, RouteExposure};
pub use casualties::Casualties;
pub use command::{CommandRules, RankDef};
pub use cores::{
    AVERAGE, CheckContext, CoreDef, CoreIndex, RoleDef, SkillDef, TraitCondition, TraitDef,
    TraitEffect,
};
pub use defs::*;
pub use forces::{ForceDef, ForceUnit};
pub use manifest::ModManifest;
pub use march::March;
pub use modules::{ModuleDef, ModuleEffect, STANDARD_MODULES};
pub use morale::{
    DefianceDef, DefianceResponse, MoraleRules, MoraleRung, RoundPressure, ShotFelt, holds_together,
};
pub use net::{GoalNet, Layer, NetError, Probe};
pub use planner::{GoalEvents, PlannerRules};
pub use registry::{DataError, DataRegistry, ValidationReport, parse_color};
pub use scale::Scale;
pub use templates::{FixAndFlank, TemplateDef, TemplateKind};
pub use worldgen::{
    Cover, GroundPalette, Relief, Rivers, Roads, SettingError, SkeletonFeature, SummaryRule,
    TerrainShare, Towns, Villages, WorldGen, WorldOption, WorldSetting, WorldSetup,
};
