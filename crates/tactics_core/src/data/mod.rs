//! Moddable game data: JSON definitions and the registry that loads them.

mod balance;
mod defs;
mod manifest;
mod registry;
mod scale;

pub use balance::Balance;
pub use defs::*;
pub use manifest::ModManifest;
pub use registry::{parse_color, DataError, DataRegistry, ValidationReport};
pub use scale::Scale;
