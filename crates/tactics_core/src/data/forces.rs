//! Forces: what a side brings to a fight, by doctrine (REIMAGINED.md,
//! *Infantry*; the designer's rulings of 2026-10-14).
//!
//! A doctrine is not a setting the learned chooser should learn once. It
//! comes from what a side has — its training, its equipment, its manpower —
//! so a force names its doctrine beside its units, and training draws
//! matchups from every force a mod ships so no network learns one method.
//! The base mod ships company teams of the early Cold War: a Western team of
//! a tank platoon and a mechanised platoon, a Soviet motor rifle company
//! heavy in infantry and a Soviet tank company heavy in armour, among others.
//!
//! A unit that `carries` starts with that passenger aboard; infantry ride to
//! the fight the way they did, a squad to a carrier.

use serde::{Deserialize, Serialize};

/// One force.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForceDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The doctrine its commander fights by (a `DoctrineDef` id).
    pub doctrine: String,
    pub units: Vec<ForceUnit>,
}

/// One line of a force: `count` of a vehicle, each carrying a passenger if
/// it says so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForceUnit {
    pub vehicle: String,
    #[serde(default = "one")]
    pub count: u32,
    /// A vehicle id each of these starts with aboard.
    #[serde(default)]
    pub carries: Option<String>,
}

fn one() -> u32 {
    1
}
