//! Mines, as content (ASSAULT.md).
//!
//! A mine is data the way a round is: what sets it off, how many of them a
//! sown hex holds, and what it does to whoever does. Where they are sown,
//! by whom and how thickly is the scenario's ([`crate::map::Minefield`]);
//! what happens when one goes off is the engine's (`battle::mines`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::MovementClass;

/// One kind of mine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MineDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Percent chance that one under her goes off, by how she moves: an
    /// anti-tank mine needs the weight of a vehicle and a man walks over
    /// it, an anti-personnel mine is set off by a foot and a track crushes
    /// it harmlessly. A class it does not name never sets one off.
    #[serde(default)]
    pub triggers: HashMap<MovementClass, u32>,
    /// How many a fully sown hex holds. Each that goes off is one fewer,
    /// and the chance of the next falls with what is left: the vehicles
    /// that drive a lane through a field clear it, at a price.
    #[serde(default = "one")]
    pub per_hex: u32,
    /// What it spends inside her, in the substance points a penetration
    /// spends: rolled into her crew, her troops and her modules exactly as
    /// a shell's would be.
    #[serde(default)]
    pub damage: i32,
    /// Whether it takes her running gear with it: a broken track, the
    /// commonest thing a mine does to a tank, and the one that leaves her
    /// sitting in the field it was laid to hold her in.
    #[serde(default)]
    pub cripples: bool,
}

fn one() -> u32 {
    1
}

impl MineDef {
    /// Percent chance one goes off under a crew that moves like `class`.
    pub fn trigger_for(&self, class: MovementClass) -> u32 {
        self.triggers.get(&class).copied().unwrap_or(0).min(100)
    }
}
