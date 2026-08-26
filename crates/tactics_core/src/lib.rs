//! Pure simulation core for the hex tactics engine.
//!
//! This crate has no Bevy dependency: battle state is plain data that can be
//! cloned and stepped headlessly, which is what makes search-based AI,
//! deterministic tests, and replays feasible.
//!
//! Layout:
//! - [`data`]: moddable JSON definitions (characters, vehicles, weapons,
//!   terrain) and the mod registry that loads and validates them.
//! - [`map`]: the CDDA-inspired hex map format (palette + ASCII rows +
//!   elevation) and the runtime [`map::HexMap`].
//! - [`battle`]: the WEGO battle simulation. Both sides plan a round, then a
//!   tick loop resolves it: intents in, events out.
//! - [`roster`]: cadets as mutable per-campaign instances, as opposed to the
//!   immutable [`data`] definitions they are stamped from.
//! - [`overworld`]: the strategic layer simulation.
//! - [`save`]: serialising a game in progress, such that reloading it
//!   produces the same future as not having saved.
//! - [`ai`]: the swappable [`ai::AiPlanner`] trait and its implementations.

pub mod ai;
pub mod battle;
pub mod data;
pub mod force;
pub mod map;
pub mod overworld;
pub mod roster;
pub mod save;

pub use hexx;
pub use hexx::{EdgeDirection, Hex};

/// Engine-wide hex orientation. Pointy-top reads best for the isometric
/// presentation and all offset-coordinate parsing assumes it.
pub const ORIENTATION: hexx::HexOrientation = hexx::HexOrientation::Pointy;

/// Engine-wide offset mode for map files: odd rows are shoved right.
pub const OFFSET_MODE: hexx::OffsetHexMode = hexx::OffsetHexMode::Odd;

/// Convert a map-file column/row pair into axial coordinates.
pub fn offset_to_hex(col: i32, row: i32) -> Hex {
    Hex::from_offset_coordinates([col, row], OFFSET_MODE, ORIENTATION)
}

/// Convert axial coordinates back into map-file column/row.
pub fn hex_to_offset(hex: Hex) -> [i32; 2] {
    hex.to_offset_coordinates(OFFSET_MODE, ORIENTATION)
}
