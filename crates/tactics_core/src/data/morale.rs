//! How much a crew can take, and what happens when they cannot take more.
//!
//! # The ladder is data, and that is the point
//!
//! Morale is a list of rungs declared by the mod, each with the pressure at
//! which a crew arrives on it. The shipped game has three — steady, wavering,
//! breaking — and a mod that ships **one** has girls who never waver,
//! never hesitate and never refuse an order. Difficulty is content in this
//! project, and this is the system it most obviously applies to: somebody who
//! wants Fire Emblem should get it by loading a mod, not by the engine
//! carrying a branch through every decision a unit makes.
//!
//! So there is no `enabled` flag and no `if rules.morale_on`. A one-rung
//! ladder is the gentle game, and it costs one comparison that was going to
//! happen anyway.
//!
//! # Why a ladder rather than a number
//!
//! Borrowed from Battle Brothers, and chosen for legibility rather than
//! fidelity. Girls have licence to fail an order in this game, and a player
//! can only accept that as fair if they could *see it coming*. "Wavering" on a
//! panel before the order is given is a warning; a hidden morale value that
//! silently crosses a threshold is the game cheating.

use serde::{Deserialize, Serialize};

/// One step of the ladder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoraleRung {
    pub id: String,
    pub name: String,
    /// Pressure at which a crew arrives here. The first rung should be zero:
    /// it is where everyone starts.
    #[serde(default)]
    pub at_pressure: u32,
    /// Whether a crew on this rung will still do as it is told.
    ///
    /// Data rather than derived from position, so a mod can have a rung that
    /// looks alarming and behaves fine, or the reverse.
    #[serde(default = "yes")]
    pub obeys: bool,
}

fn yes() -> bool {
    true
}

/// What frightens a crew, how fast they recover, and what it takes to hold on
/// anyway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoraleRules {
    /// The skill that resists deviation. Discipline in the base game: Will to
    /// stand it, Intellect to have understood what was asked.
    pub skill: String,
    /// The ladder, in order. A single rung is a game where nobody ever
    /// wavers.
    pub rungs: Vec<MoraleRung>,
    /// Pressure from taking a hit.
    pub hit: u32,
    /// Pressure from watching a friend die within sight.
    pub ally_destroyed: u32,
    /// Pressure shed at the end of each round.
    pub recovery: u32,
    /// Skill points above average that shed one extra point of pressure. A
    /// disciplined crew does not merely resist the order to run; they settle
    /// faster afterwards.
    pub recovery_per_skill: i32,
}

impl Default for MoraleRules {
    fn default() -> Self {
        Self {
            skill: "discipline".into(),
            rungs: vec![
                MoraleRung {
                    id: "steady".into(),
                    name: "Steady".into(),
                    at_pressure: 0,
                    obeys: true,
                },
                MoraleRung {
                    id: "wavering".into(),
                    name: "Wavering".into(),
                    at_pressure: 6,
                    obeys: true,
                },
                MoraleRung {
                    id: "breaking".into(),
                    name: "Breaking".into(),
                    at_pressure: 12,
                    obeys: false,
                },
            ],
            hit: 3,
            ally_destroyed: 4,
            recovery: 2,
            recovery_per_skill: 4,
        }
    }
}

impl MoraleRules {
    /// Which rung a crew under this much pressure is standing on.
    ///
    /// The highest rung whose threshold has been reached, so a mod can add
    /// rungs between the shipped ones without renumbering anything.
    pub fn rung(&self, pressure: u32) -> &MoraleRung {
        self.rungs
            .iter()
            .rev()
            .find(|r| pressure >= r.at_pressure)
            .or_else(|| self.rungs.first())
            .unwrap_or_else(|| {
                // A mod with no rungs at all is a mod where nobody ever
                // wavers, which is a legitimate thing to want.
                static STEADY: std::sync::OnceLock<MoraleRung> = std::sync::OnceLock::new();
                STEADY.get_or_init(|| MoraleRung {
                    id: "steady".into(),
                    name: "Steady".into(),
                    at_pressure: 0,
                    obeys: true,
                })
            })
    }

    /// Pressure shed at the end of a round by a crew with this much of the
    /// resisting skill.
    pub fn recovered(&self, skill_level: i32) -> u32 {
        let margin = skill_level - crate::data::AVERAGE;
        let extra = if self.recovery_per_skill == 0 {
            0
        } else {
            margin / self.recovery_per_skill
        };
        (self.recovery as i32 + extra).max(0) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_walks_up_the_ladder() {
        let rules = MoraleRules::default();
        assert_eq!(rules.rung(0).id, "steady");
        assert_eq!(rules.rung(5).id, "steady");
        assert_eq!(rules.rung(6).id, "wavering");
        assert_eq!(rules.rung(11).id, "wavering");
        assert_eq!(rules.rung(12).id, "breaking");
        assert_eq!(rules.rung(999).id, "breaking");
    }

    #[test]
    fn a_one_rung_ladder_is_a_game_where_nobody_ever_wavers() {
        // The property difficulty-as-mods rests on: no branch in Rust is
        // needed to get girls who always do as they are told.
        let rules = MoraleRules {
            rungs: vec![MoraleRung {
                id: "steady".into(),
                name: "Steady".into(),
                at_pressure: 0,
                obeys: true,
            }],
            ..MoraleRules::default()
        };
        for pressure in [0, 5, 50, 5000] {
            let rung = rules.rung(pressure);
            assert_eq!(rung.id, "steady", "pressure {pressure}");
            assert!(rung.obeys);
        }
    }

    #[test]
    fn an_empty_ladder_does_not_panic() {
        let rules = MoraleRules {
            rungs: Vec::new(),
            ..MoraleRules::default()
        };
        assert!(rules.rung(100).obeys, "no rungs means nothing to fall from");
    }

    #[test]
    fn a_disciplined_crew_settles_faster() {
        let rules = MoraleRules::default();
        let ordinary = rules.recovered(crate::data::AVERAGE);
        assert!(rules.recovered(18) > ordinary);
        assert!(rules.recovered(2) < ordinary);
        assert_eq!(rules.recovered(-100), 0, "never negative");
    }
}
