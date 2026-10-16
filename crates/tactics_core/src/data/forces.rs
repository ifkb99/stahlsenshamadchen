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
//!
//! **A force is made of platoons, and a platoon is a formation**: each line
//! is one platoon unless it says it is several (`platoons`), and the first
//! vehicle of each leads it. That is the chain of command a commander has
//! to give orders down (REIMAGINED.md L3), and without it every crew in a
//! training battle chose for herself among the whole battle's objectives.
//!
//! **Platoons can also be bought** ([`PlatoonDef`], `platoons/` in a mod):
//! a catalogue of what a side may field, each priced by its chassis'
//! `cost`, from which `force::buy` draws a force to a budget. Hand-written
//! forces are a handful of matchups a network could learn; bought ones are
//! as many as there are seeds, and equal budgets are as fair as the prices.

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
    /// What it brings, **in the order it deploys**: the first line takes the
    /// deployment hexes nearest the front. The order is the author's to give
    /// and it matters — the Soviet motor rifle company listed its nine
    /// carriers before its tanks and won 42.8% against the field where the
    /// same force with its tanks first won 52.1% (2026-10-15).
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
    /// How many platoons the line is: its vehicles split as evenly as they
    /// go, the larger platoons first. One, the default, is the whole line
    /// as one platoon.
    #[serde(default = "one")]
    pub platoons: u32,
}

impl ForceUnit {
    /// How many vehicles each of this line's platoons has, in deployment
    /// order: nine in three is three each, four in three is two, one, one.
    pub fn platoon_sizes(&self) -> Vec<u32> {
        let n = self.platoons.clamp(1, self.count.max(1));
        (0..n)
            .map(|i| self.count / n + u32::from(i < self.count % n))
            .collect()
    }
}

/// One platoon a force may buy: a line of a force, with a name. Its price
/// is its vehicles' and passengers' `cost`, read off the chassis, so the
/// catalogue has no prices of its own to drift from the roster's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlatoonDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    pub unit: ForceUnit,
}

fn one() -> u32 {
    1
}
