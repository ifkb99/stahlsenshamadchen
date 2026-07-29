//! Moddable game data: JSON definitions and the registry that loads them.

mod defs;
mod manifest;
mod registry;

pub use defs::*;
pub use manifest::ModManifest;
pub use registry::{parse_color, DataError, DataRegistry, ValidationReport};
