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
/// Three different kinds of number live here, and which kind a new field is
/// decides how it must be written. [`Self::vision_per_observation`] and
/// [`Self::speed_per_driving`] are *percentages of the vehicle's base*, since
/// what a sharp-eyed commander buys you depends on what they are looking
/// through. [`Self::accuracy_per_gunnery`], [`Self::downhill_bonus`] and
/// [`Self::blind_penalty`] are *percentage points of hit chance*, because hit
/// chance is already a 0..=100 quantity with no base to scale against. And
/// [`Self::cover_to_hit_percent`] is a *percentage of a rating declared
/// elsewhere* — the terrain's own `cover` — which is the kind to reach for
/// when a number's job is to say how much of somebody else's number counts.
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
    /// How much worse someone is at a job that is not hers.
    ///
    /// Crews are short-handed far more often than they are complete — the
    /// school has ten cadets and its tanks have four seats each — so somebody
    /// covering an empty gunner's seat is the normal case, not an edge one.
    /// A penalty rather than nothing, because a commander can lay a gun; she
    /// is just not the gunner.
    pub substitution_penalty: i32,
    /// How big a target one cadet is when a penetration rolls what it found
    /// inside, on the same scale as a module's `size`. At the default 2
    /// with the standard module set (sizes 3+4+2+1), a four-cadet crew is a
    /// little under half of what there is to hit — which is the CM-shaped
    /// truth of it: most of the inside of a tank is people.
    pub crew_weight: i32,
    /// Ledger points of behind-armor damage per effect roll, rounding up.
    /// At 4, a machine-gun burst into a soft car rolls once and an 88
    /// through a glacis rolls three or four times.
    pub points_per_effect: i32,
    /// Chance an ammunition-rack hit sets the racks off, as a percent,
    /// scaled down by how empty they are: the full figure with full racks,
    /// none with none. Zero is the gentle game where nothing ever burns —
    /// difficulty is a mod.
    pub brewup_percent: i32,
    /// Percentage points of brew-up chance shaved per point of the
    /// vehicle's `safety` stat — wet stowage, in one number. Safety was
    /// already the stat for "what a knocked-out vehicle costs the cadets
    /// inside" at the campaign's fate rolls; this makes it matter while
    /// the shooting is still happening, so a vehicle designed around her
    /// crew is measurably harder to torch, not merely gentler afterwards.
    pub brew_safety_percent: i32,
    /// Percentage points of hit chance gained for shooting downhill.
    ///
    /// One number rather than a per-level slope because what height buys a
    /// gunner is mostly the first level of it: she can see the whole
    /// vehicle instead of whatever the intervening ground has left of it.
    /// Stacking it per elevation step would make a mountain battery
    /// unmissable, which is not what standing above somebody is worth.
    pub downhill_bonus: i32,
    /// What fraction of a terrain's `cover` rating is subtracted from hit
    /// chance, as a percent.
    ///
    /// Cover is declared once per terrain and spends itself in two places:
    /// here, and in how hard the tile is to see into. At the default 50 a
    /// town's cover of 40 is twenty points of accuracy, which is the number
    /// the game has always used — it simply used to be a `/ 2` in
    /// `combat.rs`, where no modder could reach it.
    pub cover_to_hit_percent: i32,
    /// Percentage points of hit chance lost firing at a tile rather than at
    /// a unit anybody can see.
    ///
    /// Deliberately large. Blind fire is shelling a map reference, and the
    /// only reason it is worth doing at all is that a shell landing on
    /// ground is still a shell landing on ground — which is why artillery,
    /// whose whole trade is exactly that, is the weapon this number is
    /// really about.
    pub blind_penalty: i32,
    /// Round-to-round penetration variance, as a percent.
    ///
    /// No two shells leave the same barrel identically, and armor plate is
    /// not uniform either: a fired round's penetration is scaled by a
    /// uniformly random factor in `100 ± pen_scatter` percent before it
    /// meets the plate. This is what makes a marginal shot *marginal* —
    /// sometimes through, sometimes a bounce — rather than a foregone
    /// conclusion the AI can price as certainty. Zero collapses every
    /// matchup to a hard threshold, which is both a legitimate mod choice
    /// and what the deterministic tests set. An integer percent rather than
    /// a float because the analytic chance the AI reads and the roll the
    /// resolver makes must count the same finite outcomes, exactly.
    pub pen_scatter: i32,
}

impl Default for Balance {
    fn default() -> Self {
        Self {
            vision_per_observation: 5,
            speed_per_driving: 5,
            accuracy_per_gunnery: 3,
            substitution_penalty: 2,
            crew_weight: 2,
            points_per_effect: 4,
            brewup_percent: 60,
            brew_safety_percent: 12,
            downhill_bonus: 10,
            cover_to_hit_percent: 50,
            blind_penalty: 40,
            pen_scatter: 15,
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

    /// How many percentage points of hit chance a terrain's `cover` rating
    /// takes off a shot into it.
    ///
    /// Integer arithmetic, truncating, because this feeds a hit chance that
    /// has to be reproducible bit for bit — and because the divisor it
    /// replaced truncated too, which is what let this become data without
    /// moving a single number.
    pub fn cover_against_accuracy(&self, cover: i32) -> i32 {
        cover * self.cover_to_hit_percent / 100
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
    fn cover_costs_a_shot_exactly_what_the_old_divisor_charged() {
        // The three hit-chance constants moved out of `combat.rs` at their
        // shipped values, and this is the arithmetic half of the proof that
        // nothing moved with them: the divisor truncated toward zero and so
        // does the percentage, for every cover rating a terrain can declare.
        let balance = Balance::default();
        for cover in 0..=100 {
            assert_eq!(
                balance.cover_against_accuracy(cover),
                cover / 2,
                "cover {cover}"
            );
        }
        assert_eq!(balance.downhill_bonus, 10);
        assert_eq!(balance.blind_penalty, 40);
    }

    #[test]
    fn a_mod_that_names_no_cover_rule_still_has_one() {
        // Every field here is `#[serde(default)]` through the struct-level
        // attribute, so a mod written before these numbers existed inherits
        // the game it was tuned against rather than a zeroed one — in which
        // cover would buy nothing and blind fire would be free.
        let balance: Balance = serde_json::from_str("{}").expect("empty block");
        assert_eq!(balance, Balance::default());
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

/// How long a crew takes to act.
///
/// A round is twelve ticks, orders are fixed at the start, and the world
/// changes while it resolves. A machine begins executing at tick zero. A
/// person takes a moment — to hear the order, to understand it, to get moving
/// — and that moment is the difference between a unit and a crew.
///
/// This is a data block for a reason beyond tuning: **difficulty is a mod in
/// this project**, and somebody who wants orders that simply happen should get
/// that by loading content, not by the engine carrying a branch. Setting
/// `base_ticks` and `max_ticks` to zero makes every crew instantaneous and
/// this whole system disappears without an `if` anywhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionRules {
    /// Which skill is consulted. Data, so a mod can decide that reacting is a
    /// matter of drill, or of nerve, or of nothing at all.
    pub skill: String,
    /// Ticks an ordinary crew takes before acting.
    pub base_ticks: u32,
    /// How many points above average shave one tick off. Below average adds
    /// them back at the same rate.
    pub levels_per_tick: i32,
    /// The slowest anyone can be, however bad they are.
    pub max_ticks: u32,
}

impl Default for ReactionRules {
    fn default() -> Self {
        Self {
            skill: "reactions".into(),
            base_ticks: 2,
            levels_per_tick: 4,
            max_ticks: 5,
        }
    }
}

impl ReactionRules {
    /// Ticks this crew waits before acting on its orders.
    ///
    /// Saturating rather than wrapping, and clamped both ends: a brilliant
    /// crew reaches zero and stops, and a dreadful one stops at `max_ticks`
    /// rather than spending the whole round frozen.
    pub fn delay(&self, level: i32) -> u32 {
        if self.max_ticks == 0 {
            return 0;
        }
        let margin = level - crate::data::AVERAGE;
        let shaved = if self.levels_per_tick == 0 {
            0
        } else {
            margin / self.levels_per_tick
        };
        (self.base_ticks as i32 - shaved).clamp(0, self.max_ticks as i32) as u32
    }
}

#[cfg(test)]
mod reaction_tests {
    use super::*;

    #[test]
    fn a_quicker_crew_acts_sooner() {
        let rules = ReactionRules::default();
        assert_eq!(rules.delay(crate::data::AVERAGE), 2, "ordinary");
        assert!(rules.delay(18) < rules.delay(crate::data::AVERAGE));
        assert!(rules.delay(4) > rules.delay(crate::data::AVERAGE));
    }

    #[test]
    fn nobody_waits_forever_and_nobody_acts_before_they_are_told() {
        let rules = ReactionRules::default();
        assert_eq!(rules.delay(100), 0, "a genius still cannot act early");
        assert_eq!(
            rules.delay(-100),
            rules.max_ticks,
            "and a fool is not frozen"
        );
    }

    #[test]
    fn a_gentle_mod_turns_the_whole_system_off_with_data() {
        // The property that matters for difficulty-as-mods: no branch in Rust
        // is needed to get orders that simply happen.
        let rules = ReactionRules {
            base_ticks: 0,
            max_ticks: 0,
            ..ReactionRules::default()
        };
        for level in [-20, 0, crate::data::AVERAGE, 30] {
            assert_eq!(rules.delay(level), 0, "level {level} should be instant");
        }
    }
}
