//! How fast a column moves across a generated world: the `march` block of
//! `mod.json` (WORLD.md W2.3).
//!
//! The designer's ruling (2026-09-26) is that a column is not a tank. The
//! tiles are the truth, so an army marches over the same ground its vehicles
//! would fight on, at the pace of the slowest of them — and then at a share
//! of that, because a column keeps its spacing, halts, and does not drive
//! round the clock. A rule rather than a planner number: it moves a human's
//! companies exactly as it moves a machine's, so it is its own block beside
//! `balance`, not a field of `planner`.

use serde::{Deserialize, Serialize};

/// The pace of a column on the march.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct March {
    /// The share of its slowest vehicle's speed a column keeps up, in
    /// percent: spacing, traffic, the time a column takes to get going and
    /// close up again.
    pub column_percent: u32,
    /// Hours a day a column is on the road.
    pub hours_per_day: u32,
    /// Minutes in every marching hour spent halted.
    pub halt_minutes_per_hour: u32,
}

impl Default for March {
    fn default() -> Self {
        Self {
            column_percent: 25,
            hours_per_day: 6,
            halt_minutes_per_hour: 10,
        }
    }
}

impl March {
    /// The movement points a column whose slowest vehicle has `points` a
    /// round spends in a day, with `rounds_per_hour` rounds to the hour.
    ///
    /// Movement points and not tiles, because a tile of wood costs more than
    /// a tile of road: the day's march is a budget the route spends. Integer
    /// arithmetic, rounded down, like every rate in `balance`.
    pub fn day_budget(&self, points: u32, rounds_per_hour: u32) -> u32 {
        let minutes = self.hours_per_day * 60u32.saturating_sub(self.halt_minutes_per_hour);
        points * rounds_per_hour * minutes * self.column_percent / (60 * 100)
    }
}
