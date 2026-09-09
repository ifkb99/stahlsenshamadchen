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

/// What a crew who has stopped obeying does instead.
///
/// The old model had exactly one answer and never said so out loud: she
/// froze. A crew on a rung that does not obey would not advance, would not
/// fall back and would not even break for cover, because all three ran
/// through the same `obeys` gate — so the losing side's story was "everyone
/// stands still until they are shot", which is the worst possible shape for
/// a morale system. Fear should buy something, even if what it buys is
/// sometimes a worse death.
///
/// Which of these a crew reaches for is her temperament, not the rung's:
/// see [`DefianceDef`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefianceResponse {
    /// She stops, and she stops doing anything. No movement, and no fire she
    /// was not explicitly ordered to give — she is a passenger in her own
    /// vehicle. This is what every broken crew did before there was a
    /// choice, which is why it is the default: a mod that declares no
    /// [`MoraleRules::defiance`] gets exactly the old game.
    #[default]
    Freeze,
    /// She breaks contact — puts distance between herself and everything she
    /// can see that can hurt her, taking cover where the ground offers it.
    /// Deliberately *away from contact* rather than toward an exit tile: a
    /// frightened crew reverses out of the fight, she does not navigate to a
    /// designated lane twenty hexes away, and the map is not going to have
    /// edges forever.
    Flight,
    /// She will not be moved and she will not be careful. No falling back,
    /// and the ambush discipline that holds an unseen crew's fire is off —
    /// she shoots at what she can see, whether or not waiting would have
    /// been wiser.
    Fight,
}

/// One way of defying an order, and what predisposes a crew to it.
///
/// Declared by the mod, because which responses exist is content — a gentler
/// game lists only `flight`, a grimmer one might add a rung's worth of
/// something else. What each response *does* is engine, hence
/// [`DefianceResponse`]; the same split [`crate::data::ModuleEffect`] makes.
///
/// The score is `base` plus the commanding cadet's `core` plus whatever her
/// traits say about this response, and the highest wins with ties going to
/// the earlier entry. Two consequences worth stating:
///
/// - **Cores are the floor and traits are the differentiator.** Every cadet
///   has cores whether or not anybody wrote her a personality, so the
///   mechanism works on the day it ships; a trait that names a response
///   moves her off that floor. That is the growth path — `reckless` and
///   `craven` are content, not code.
/// - **Ties to the earlier entry is what makes this additive.** Cores
///   default to `AVERAGE`, so a cadet nobody has written cores for scores
///   every response identically and takes whichever is listed first. List
///   `freeze` first and an unremarkable crew behaves exactly as she did
///   before this existed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefianceDef {
    pub id: String,
    /// How the log says it: "Ilse Brandt falls back". A sentence fragment
    /// with the cadet's name in front of it.
    pub name: String,
    pub response: DefianceResponse,
    /// The core that predisposes a crew to this. Absent means the response
    /// rests on `base` alone — which is what a "default" response wants.
    #[serde(default)]
    pub core: Option<String>,
    /// Added to every crew's score for this response, whoever she is.
    #[serde(default)]
    pub base: i32,
}

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
    /// Percentage points of hit chance a crew standing on this rung loses.
    ///
    /// Suppression, and deliberately *not* a second fear system: pressure is
    /// already collected in one place and already walks this ladder, so what
    /// being shot at costs a gunner is a number on the rung she is driven
    /// to rather than a parallel counter with its own decay. The additivity
    /// rule falls out of that for free — a mod with one rung has no
    /// suppression at all, and the zero default means a ladder written
    /// before this existed shoots exactly as it always did.
    ///
    /// Negative by convention; nothing forbids a mod from declaring a rung
    /// where fear sharpens somebody up.
    #[serde(default)]
    pub accuracy: i32,
}

fn yes() -> bool {
    true
}

/// What frightens a crew, how fast they recover, and what it takes to hold on
/// anyway.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// What a crew who has stopped obeying does instead, best score first
    /// on a tie. Empty means she freezes, which is what she did before this
    /// existed — the additivity rule, applied to the newest thing on the
    /// oldest ladder.
    #[serde(default)]
    pub defiance: Vec<DefianceDef>,
    /// Pressure shed at the end of each round.
    pub recovery: u32,
    /// Skill points above average that shed one extra point of pressure. A
    /// disciplined crew does not merely resist the order to run; they settle
    /// faster afterwards.
    pub recovery_per_skill: i32,
    /// Extra pressure shed by a crew still in contact with the officer
    /// commanding her formation.
    ///
    /// The mirror of [`Self::leader_lost`], and the reason that field wanted
    /// a twin: losing a commander already costs a formation its nerve, and
    /// nothing had ever paid it back for still having one. This is what
    /// makes rallying a thing leaders *do* rather than a thing that happens
    /// to a crew who got far enough away, and what makes a leader worth
    /// keeping alive for a reason other than succession bookkeeping.
    ///
    /// `#[serde(default)]`, so a mod that says nothing rallies exactly as it
    /// did. Note a mod with no `command` block has no radios and therefore
    /// nobody out of contact, so this would reach every crew — which is the
    /// honest reading of a game that does not model isolation at all.
    #[serde(default)]
    pub recovery_near_leader: u32,
    /// How many substance points one point of pressure is worth to anybody
    /// weighing a shot.
    ///
    /// The exchange rate between the two currencies this engine now prices
    /// ground in. Everything that decides where to stand, which gun to bring
    /// to bear or which round to chamber spends [`crate::battle::round_worth`],
    /// and this is the only number that lets the pressure half of it reach
    /// that decision: `worth = expected damage + expected pressure *
    /// point_worth`.
    ///
    /// **In `morale` and not in `planner`,** by CLAUDE.md's own test for
    /// which block a number belongs in: it reaches the loader's AP-or-HE
    /// choice, which is a crew's decision and therefore a human player's
    /// crew's decision too. A mod that rewrote every `planner` field leaves a
    /// human-versus-human battle bit-identical; a mod that moves this one
    /// does not, because the loader in a player's tank reaches for a
    /// different round.
    ///
    /// `#[serde(default)]` to 0.0, which is the game before this existed:
    /// suppression is still *charged* — the crew still feels it — but it is
    /// invisible to every chooser, so nobody fires a belt at a glacis for the
    /// noise it makes. That is the additivity rule applied to an exchange
    /// rate rather than to a rule.
    #[serde(default)]
    pub point_worth: f32,
}

/// What a shot that arrived did to the plate, as the pressure ladder prices
/// it.
///
/// Two cases and not three: whether the crew were *rattled* by a bounce is a
/// property of the round rather than of the outcome, and it travels in
/// [`RoundPressure`] beside the suppression the round carries. Keeping it out
/// of here is what stops the price list growing a case every time a round
/// gains a property.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ShotFelt {
    /// The round came through, and spent this share of the budget its
    /// datasheet lists — 1.0 for a clean penetration by a full-strength
    /// crew, less for a round that only scraped through the partial band or
    /// a platoon firing with a third of her riflemen. The ladder's outcome
    /// price scales with it (the designer's ruling, 2026-09-09): what
    /// frightens a crew is what the shell did, not the fact of its arrival,
    /// so a round that spends nothing frightens nobody beyond what the round
    /// itself declares as suppression.
    Penetrated {
        /// [`crate::battle::Event::ShotHit`]'s `damage` over its `budget`,
        /// clamped to one. The charge reads the event; the expectation reads
        /// the profile's penetration share by the same arithmetic.
        spent: f32,
    },
    /// The round struck and the armour held.
    Bounced,
}

/// The two things about a round that the pressure ladder charges for.
///
/// A tiny struct rather than two arguments because it travels together
/// everywhere: from the racks to the resolver, from the resolver to the event
/// stream, and from the event stream back to [`MoraleRules::pressure_for`].
/// It lives in `data` rather than beside `battle::combat::Round` because
/// `data` may not depend on `battle`, and because this is the shape of the
/// question the ladder asks rather than the shape of a round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RoundPressure {
    /// Bullets rather than shells. The morale block's `bounced` price is
    /// deliberately not charged for these.
    pub small_arms: bool,
    /// [`crate::data::AmmoDef::suppression`] for the round that arrived.
    pub suppression: u32,
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
                    accuracy: 0,
                },
                MoraleRung {
                    id: "wavering".into(),
                    name: "Wavering".into(),
                    at_pressure: 6,
                    obeys: true,
                    accuracy: -10,
                },
                MoraleRung {
                    id: "breaking".into(),
                    name: "Breaking".into(),
                    at_pressure: 12,
                    obeys: false,
                    accuracy: -20,
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
            // The engine's stand-in ladder keeps the old silence: a mod that
            // declares no morale block at all gets crews who freeze, because
            // that is what they did before there was anything else to do.
            defiance: Vec::new(),
            recovery: 2,
            recovery_per_skill: 4,
            recovery_near_leader: 0,
            // A mod that declares no morale block at all gets a game in which
            // fear is felt and never weighed, which is what every planner in
            // this engine did before suppression joined the currency.
            point_worth: 0.0,
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
                    accuracy: 0,
                })
            })
    }

    /// What one shot that arrived costs the crew it arrived at, in pressure.
    ///
    /// **The one price list.** Two things ask it and they must not drift:
    /// [`crate::battle::BattleState`] charging a tick's events after the
    /// shooting (`apply_pressure`), and
    /// [`crate::battle::expected_pressure`] telling a planner what a shot is
    /// expected to be worth before it is fired. Two copies of these three
    /// lines would be a gunner aiming at a number no resolver honours, which
    /// is the exact defect `round_worth` was written to remove on the damage
    /// side.
    ///
    /// The shape: an outcome price from the ladder, plus whatever the round
    /// itself brings. A penetration costs [`Self::hit`] *and*
    /// [`Self::penetrated`], because the shell that came through is both a
    /// hit and the news that the armour did not hold — **scaled by the share
    /// of its budget the round actually spent**, so a shell that broke up on
    /// the plate and got a fifth of itself through costs a fifth, and a
    /// remnant platoon's bullet costs what the one point it still puts
    /// inside is worth. Before the scale every landing round cost the full
    /// price, and once fear was priced at all "a shot that accomplishes
    /// nothing" stopped existing: a platoon with no riflemen left still
    /// expected eight points of pressure a hit and opened up. A bounce costs
    /// [`Self::bounced`] only when the round was heavy enough to ring the
    /// hull. Suppression is charged on top in every case, which is what makes
    /// a belt of machine-gun fire against a glacis worth firing.
    ///
    /// A real number rather than ladder points, because the expectation is
    /// one; the charge rounds it at the ledger, once, in `apply_pressure`.
    pub fn pressure_for(&self, outcome: ShotFelt, round: RoundPressure) -> f32 {
        let outcome_price = match outcome {
            ShotFelt::Penetrated { spent } => {
                (self.hit + self.penetrated) as f32 * spent.clamp(0.0, 1.0)
            }
            // Bullets pattering on plate frighten nobody buttoned up behind
            // it — through *this* price. A belt that declares suppression
            // frightens them through that one, which is the designer saying
            // so per round rather than the engine deciding it per class.
            ShotFelt::Bounced if round.small_arms => 0.0,
            ShotFelt::Bounced => self.bounced as f32,
        };
        outcome_price + round.suppression as f32
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
                accuracy: 0,
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
