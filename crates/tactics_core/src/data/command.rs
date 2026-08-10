//! How far an order carries, and how long it takes to get there.
//!
//! Today a side is an all-seeing hand: it names any of its units and that unit
//! obeys, instantly, wherever it is standing. The `command` block is what turns
//! that hand back into a chain of people. It prices two things and nothing
//! else:
//!
//! - **Contact.** A girl answers to her formation's leader if she is close
//!   enough to hear her — a radius in hexes, widened or narrowed by how well
//!   the leader's crew works the radio, and optionally *relayed* through
//!   anyone already in contact, which is what makes a signals vehicle worth
//!   fielding rather than a stat on a sheet.
//! - **Latency.** A mission does not take effect the tick it is issued. It
//!   travels, and it arrives later, priced in the same tick currency the
//!   [`ReactionRules`] already speak — which is the whole reason this block
//!   reuses that type rather than inventing a second one. The post-mortem in
//!   `girls.md` is emphatic about *what* may be delayed: new information, an
//!   order the girl was not expecting. Never the execution of a plan she was
//!   already given, which is what killed the first attempt at reaction
//!   latency.
//!
//! Like [`crate::data::Scale`] and [`crate::data::Balance`] this is a property
//! of the game rather than of a piece of content, so a mod that declares a
//! block replaces the previous one wholesale and a mod that says nothing
//! inherits what it extends.
//!
//! **Absence is the game as it was.** `registry.command` is an `Option`, and
//! `None` means infinite radius, zero latency, everybody always in contact —
//! not because Rust branches on it in the places that matter, but because the
//! contact set is never computed and therefore empty, and the delay is zero so
//! a mission lands the tick it is given. The same holds for a block whose
//! coefficients are all zero, which is the property
//! `a_command_block_with_zero_coefficients_is_the_game_without_one` pins to the
//! byte.

use super::ReactionRules;
use serde::{Deserialize, Serialize};

/// The `command` block: what a chain of command costs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CommandRules {
    /// Command radius in hexes for an ordinary leader, measured from her
    /// vehicle. Hexes rather than metres because everything a player judges by
    /// eye is in hexes; the scale block says what one is worth.
    pub radius: u32,
    /// Hexes added per point of the leader's `signals` skill above
    /// [`crate::data::AVERAGE`], and subtracted per point below it. Signed, so
    /// a mod can make radios matter enormously or not at all, and so a poor
    /// signaller is a liability rather than merely not a help — the same rule
    /// the crew balance block settled on.
    pub radius_per_signals: i32,
    /// Whether contact chains through the people already in it. With relay on,
    /// a platoon strung out along a road stays under command as long as each
    /// link holds; with it off, everyone must hear the leader herself.
    pub relay: bool,
    /// How long a mission spends in transit, as a function of the *leader's*
    /// skill at [`ReactionRules::skill`] — `command` in the base game, because
    /// getting an order out clearly and quickly is her job and not her
    /// subordinates'.
    ///
    /// Reusing [`ReactionRules`] is deliberate and is the design doc's
    /// instruction: an order taking time to arrive and a crew taking time to
    /// act on it are the same currency spent twice, so they had better be the
    /// same type with the same knobs. A mod that sets `max_ticks: 0` here has
    /// missions that simply happen.
    pub latency: ReactionRules,
}

impl Default for CommandRules {
    fn default() -> Self {
        Self {
            radius: 6,
            radius_per_signals: 2,
            relay: true,
            latency: ReactionRules {
                skill: "command".into(),
                base_ticks: 2,
                levels_per_tick: 4,
                max_ticks: 5,
            },
        }
    }
}

impl CommandRules {
    /// The radius a leader (or a relaying crew) actually reaches, given how
    /// well they work the radio.
    ///
    /// Floors at 1 rather than 0: a vehicle can always be shouted at by the
    /// one alongside it, and a radius of zero would mean a leader out of
    /// contact with herself, which is not a state anything downstream is
    /// prepared to reason about.
    pub fn radius_for(&self, signals: i32) -> u32 {
        let margin = signals - crate::data::AVERAGE;
        (self.radius as i32 + self.radius_per_signals * margin).max(1) as u32
    }

    /// Ticks a mission spends in transit to a leader with this much of the
    /// latency skill. Zero is the ordinary answer for a gentle mod, and it is
    /// what makes the instant path in [`crate::battle::BattleState`] the same
    /// code rather than a branch.
    pub fn delay(&self, level: i32) -> u32 {
        self.latency.delay(level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::AVERAGE;

    #[test]
    fn a_good_signaller_reaches_further_and_a_bad_one_less() {
        let rules = CommandRules::default();
        assert_eq!(
            rules.radius_for(AVERAGE),
            6,
            "ordinary is the stated radius"
        );
        assert_eq!(rules.radius_for(AVERAGE + 3), 12);
        assert_eq!(rules.radius_for(AVERAGE - 2), 2);
    }

    #[test]
    fn a_radius_never_falls_to_nothing() {
        // A leader out of contact with her own vehicle is not a state the
        // contact graph could act on: she is the root of it.
        let rules = CommandRules::default();
        assert_eq!(rules.radius_for(-100), 1);
    }

    #[test]
    fn a_gentle_mod_turns_the_whole_system_off_with_data() {
        // The additivity rule, at the block's own level: enormous radius, no
        // delay at any skill, and nothing in the engine has to know.
        let rules = CommandRules {
            radius: 999,
            radius_per_signals: 0,
            relay: true,
            latency: ReactionRules {
                skill: "command".into(),
                base_ticks: 0,
                levels_per_tick: 0,
                max_ticks: 0,
            },
        };
        for level in [-20, 0, AVERAGE, 30] {
            assert_eq!(rules.delay(level), 0, "level {level} should be instant");
            assert_eq!(rules.radius_for(level), 999);
        }
    }
}
