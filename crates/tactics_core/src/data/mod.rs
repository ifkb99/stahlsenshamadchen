//! Moddable game data: JSON definitions and the registry that loads them.

mod balance;
mod defs;
mod manifest;
mod registry;
mod scale;

pub use balance::Balance;
pub use defs::*;
pub use manifest::ModManifest;
pub use registry::{DataError, DataRegistry, ValidationReport, parse_color};
pub use scale::Scale;
