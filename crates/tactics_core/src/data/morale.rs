//! How much a crew can take, and what happens when they cannot take more.
//!
//! # The ladder is data, and that is the point
//!
//! Morale is a list of rungs declared by the mod, each with the pressure at
//! which a crew arrives on it. The shipped game has three — steady, wavering,
//! breaking — and a mod that ships **one** has cadets who never waver,
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
//! fidelity. Cadets have licence to fail an order in this game, and a player
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
    /// Pressure from a shell that struck and did not get through.
    ///
    /// The armor held, and the crew still heard it arrive — being rung like
    /// a bell is real even when nothing breaks. Less than [`Self::hit`] in
    /// the base data, and **never charged for small-arms fire**: bullets
    /// pattering on plate frighten nobody buttoned up behind it, and letting
    /// them would quietly rebuild the machine-gun-grinds-a-heavy-tank defect
    /// one layer up, in morale instead of hit points. `#[serde(default)]`
    /// for the same additivity reason as [`Self::leader_lost`]: a mod that
    /// says nothing feels nothing.
    #[serde(default)]
    pub bounced: u32,
    /// Pressure from a round coming *through* the armor, on top of
    /// [`Self::hit`]. This is the spike the bail-out check rides on: every
    /// penetration also rolls the crew's discipline (`holds_together`),
    /// and a crew that fails abandons the vehicle. Together they are what
    /// keeps time-to-kill honest without hit points — tanks are mostly
    /// lost because the crew leaves or dies, not because every box inside
    /// is ticked — and what makes discipline training visibly be the
    /// thing that keeps a damaged tank in the fight. `#[serde(default)]`:
    /// a mod that says nothing spikes nothing, and its crews stay however
    /// long the dice inside let them.
    #[serde(default)]
    pub penetrated: u32,
    /// Pressure from watching a friend die within sight.
    pub ally_destroyed: u32,
    /// Pressure on every surviving member of a formation whose leader is
    /// gone — destroyed, or driven off the map — at the moment command
    /// passes to somebody else.
    ///
    /// `#[serde(default)]` rather than required, so a mod that says nothing
    /// feels nothing: losing a commander then costs a formation exactly what
    /// it cost before this existed, which is the additivity rule applied to
    /// the newest rung on the oldest ladder. Unlike a hit or a friend
    /// burning, this one reaches the whole formation wherever it is standing
    /// — the news travels the chain of command, not the line of sight.
    #[serde(default)]
    pub leader_lost: u32,
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
            bounced: 1,
            penetrated: 5,
            ally_destroyed: 4,
            // Mirrors the shipped `mod.json`: watching your commander go is
            // at least as bad as watching anybody else go. Note this is the
            // *engine's* stand-in for a mod that declares no morale block at
            // all; a mod that declares one and omits the field gets zero,
            // which is the field's serde default and the gentler reading.
            leader_lost: 4,
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

/// A GURPS-shaped success roll: three dice against a skill, succeeding on a
/// roll at or under it.
///
/// The *shape* matters more than the numbers. Three dice cluster hard around
/// ten, so a disciplined crew holds nearly always and a poor one nearly never,
/// with genuine upsets at the edges. A flat roll would make good crews feel
/// arbitrary — which is fatal in a game where cadets are allowed to disobey,
/// because the player has to be able to trust that training bought them
/// something.
pub fn holds_together(rng: &mut impl rand::RngExt, skill_level: i32) -> bool {
    let roll: i32 = (0..3).map(|_| rng.random_range(1..=6)).sum();
    roll <= skill_level
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
        // needed to get cadets who always do as they are told.
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
    fn discipline_decides_who_holds_and_the_dice_cluster() {
        use rand::SeedableRng;
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(7);
        let rate = |rng: &mut rand_chacha::ChaCha8Rng, level| {
            (0..2000).filter(|_| holds_together(rng, level)).count() as f32 / 2000.0
        };
        let poor = rate(&mut rng, 6);
        let ordinary = rate(&mut rng, 10);
        let excellent = rate(&mut rng, 15);
        assert!(
            poor < ordinary && ordinary < excellent,
            "{poor} {ordinary} {excellent}"
        );
        // Three dice cluster: training should be reliable, not a coin flip.
        assert!(
            excellent > 0.9,
            "a disciplined crew nearly always holds: {excellent}"
        );
        assert!(poor < 0.15, "a poor one nearly never does: {poor}");
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
