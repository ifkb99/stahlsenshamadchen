//! How much a crew's quality is worth, as data rather than divisors.
//!
//! Crew stats run 0..=5 and modify the hardware they are sitting in. The
//! original coefficients were flat divisors — `awareness / 4` bonus hexes of
//! vision, `driving / 5` bonus movement points — chosen when a vehicle saw 3
//! hexes and moved 2. The scale decision multiplied those bases by four to
//! six times and left the divisors alone, so the best scout in the school was
//! worth one hex out of twenty and the best driver one point out of seven.
//! Crew quality is the emotional core of the genre; it cannot be a rounding
//! error.
//!
//! The fix is to make the bonus a percentage of the vehicle's own base, which
//! is scale-independent by construction: retuning vision ranges or movement
//! allowances no longer silently retunes what a crew is worth. The
//! percentages themselves live here, in the `balance` block of `mod.json`,
//! because a number a modder would want to change does not belong in Rust.
//!
//! All fields are integers so the arithmetic stays exact and the simulation
//! stays bit-for-bit reproducible.

use serde::{Deserialize, Serialize};

/// Per-point value of each crew stat.
///
/// Two different kinds of number live here, which is worth reading carefully
/// before adding a third. [`Self::vision_per_awareness`] and
/// [`Self::speed_per_driving`] are *percentages of the vehicle's base*, since
/// what a sharp-eyed commander buys you depends on what they are looking
/// through. [`Self::accuracy_per_gunnery`] is *percentage points of hit
/// chance*, because hit chance is already a 0..=100 quantity with no base to
/// scale against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Balance {
    /// Percent of the vehicle's base vision range added per point of the
    /// crew's best awareness. At the default 5, a gifted scout (awareness 5)
    /// sees a quarter further than the same car with a novice aboard.
    pub vision_per_observation: i32,
    /// Percent of the vehicle's base movement allowance added per point of
    /// the crew's best driving. Because movement points are a speed, this
    /// reads directly: +25% on a 30 km/h medium tank is 36 km/h.
    pub speed_per_driving: i32,
    /// Percentage points of hit chance added per point of the crew's best
    /// gunnery. This one was never devalued by the scale change — hit chance
    /// has no base to be a fraction of — but it lived as a bare `* 3` in
    /// `combat.rs`, which is the same design smell.
    pub accuracy_per_gunnery: i32,
}

impl Default for Balance {
    fn default() -> Self {
        Self {
            vision_per_observation: 5,
            speed_per_driving: 5,
            accuracy_per_gunnery: 3,
        }
    }
}

impl Balance {
    /// Apply a percent-of-base crew bonus to a vehicle stat.
    ///
    /// Rounds to nearest so a small vehicle still feels the bonus eventually
    /// rather than truncating it away every time, and floors at 1 because a
    /// stat of zero means "cannot see" or "cannot move", which no crew should
    /// be able to inflict on their own vehicle. `points` is signed so that a
    /// future wound model can pass a penalty through the same path.
    pub fn scaled(base: u32, percent_per_point: i32, points: i32) -> u32 {
        let percent = 100 + percent_per_point as i64 * points as i64;
        let scaled = base as i64 * percent;
        // Round half away from zero; `i64` division truncates toward zero, so
        // negatives need the nudge in the other direction.
        let rounded = if scaled >= 0 {
            (scaled + 50) / 100
        } else {
            (scaled - 50) / 100
        };
        rounded.max(1) as u32
    }

    /// How far a skill level sits from ordinary.
    ///
    /// Every crew effect is measured from [`crate::data::AVERAGE`] rather than
    /// from zero, and this is the change of meaning that matters most in the
    /// stat rework: an ordinary crew now changes nothing, and a *poor* one is
    /// a penalty. Under the old 0-5 stats every crew member could only help,
    /// so nobody was ever a liability and it never mattered who rode in which
    /// tank.
    fn margin(level: i32) -> i32 {
        level - crate::data::AVERAGE
    }

    /// Vision range for a vehicle with `base` sight and a crew observing at
    /// `observation`.
    pub fn vision(&self, base: u32, observation: i32) -> u32 {
        Self::scaled(base, self.vision_per_observation, Self::margin(observation))
    }

    /// Movement allowance for a vehicle with `base` points and a crew driving
    /// at `driving`.
    pub fn speed(&self, base: u32, driving: i32) -> u32 {
        Self::scaled(base, self.speed_per_driving, Self::margin(driving))
    }

    /// Hit chance change, in percentage points, for a crew shooting at
    /// `gunnery`. Negative for a crew worse than ordinary.
    pub fn accuracy(&self, gunnery: i32) -> i32 {
        Self::margin(gunnery) * self.accuracy_per_gunnery
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gifted_crew_is_worth_a_quarter_of_the_vehicle() {
        let balance = Balance::default();
        // Skill 15 is five above ordinary. The recon car sees 20 and the tank
        // destroyer 10; the same scout is worth five hexes in one and three in
        // the other, which is the point of scaling against the base rather
        // than adding a flat bonus.
        assert_eq!(balance.vision(20, 15), 25);
        assert_eq!(balance.vision(10, 15), 13); // 12.5, rounded to nearest
        assert_eq!(balance.speed(5, 15), 6); // 6.25 -> 6, i.e. 30 -> 36 km/h
        assert_eq!(balance.speed(3, 15), 4); // the heavy tank finally notices
    }

    #[test]
    fn an_ordinary_crew_changes_nothing() {
        // Effects are measured from AVERAGE, so a competent-but-unremarkable
        // crew leaves the vehicle exactly as designed.
        let balance = Balance::default();
        assert_eq!(balance.vision(12, crate::data::AVERAGE), 12);
        assert_eq!(balance.speed(7, crate::data::AVERAGE), 7);
        assert_eq!(balance.accuracy(crate::data::AVERAGE), 0);
    }

    #[test]
    fn a_poor_crew_is_a_liability() {
        // The property the old 0-5 stats could not express: every crew member
        // could only ever help, so it never mattered who rode in which tank.
        let balance = Balance::default();
        assert!(balance.vision(20, 6) < 20, "a bad observer sees less");
        assert!(balance.speed(6, 6) < 6, "a bad driver is slower");
        assert!(balance.accuracy(6) < 0, "a bad gunner shoots worse");
    }

    #[test]
    fn a_stat_never_scales_to_zero() {
        // A wound model will eventually pass negative points through here.
        assert_eq!(Balance::scaled(4, -50, 5), 1);
        assert_eq!(Balance::scaled(1, -100, 5), 1);
    }
}
