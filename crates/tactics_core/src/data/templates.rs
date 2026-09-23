//! Tactical templates: the plays a cadet knows.
//!
//! PLANNING.md's first layer is ground (`crate::ground`); this is the second,
//! the vocabulary. A template is roles, the ground each role needs, and when
//! each moves — matched onto the terrain by a commander who knows it
//! (`crate::ai::plan`).
//!
//! **What a template *is* is Rust; what it *weighs* is data.** Matching "fix
//! and flank" onto ground is an algorithm — find a firing position onto the
//! enemy, find a covered way to a hex off his frontal arc — and a JSON schema
//! general enough to express it would be a programming language nobody asked
//! for. So [`TemplateKind`] names the algorithm and carries its numbers, and
//! a mod may add as many templates of a kind as it likes, tuned differently:
//! a cautious fix-and-flank that will only move unseen, a hasty one that
//! accepts a glimpse.
//!
//! **Who knows a template** is the designer's ruling of 2026-09-23: templates
//! belong to cadets, and an academy's doctrine decides which ones its
//! graduates are taught (`DoctrineDef::teaches`). A cadet may know more than
//! her academy taught (`CharacterDef::templates`).

use serde::{Deserialize, Serialize};

/// One template: a named play and the algorithm that matches it onto ground.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TemplateDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    pub kind: TemplateKind,
}

/// Which algorithm a template is, with its numbers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TemplateKind {
    /// One formation takes a firing position onto the enemy and holds him
    /// there; another goes round by a covered route to ground off his
    /// frontal arc, and moves in when the first is in place.
    FixAndFlank(FixAndFlank),
}

/// The numbers a fix-and-flank is weighed by.
///
/// **The flank is priced in the currency.** What going round is worth is what
/// the manoeuvre element's guns would put on the enemy per round from the
/// flank, less what they would put on him standing in the line beside the
/// fix — the resolver's own arithmetic ([`crate::battle::best_weapon_from`]),
/// face, plate, range and all. A flank round a hull the line can already beat
/// buys little; round one whose front nothing aboard can touch, everything.
/// The first draft paid a flat worth per face of the enemy and lost battles
/// with it: medium tanks against medium tanks split two-and-two to buy a flank
/// the 75 did not need.
///
/// **Splitting a force is priced too.** While the flankers are on their way
/// the fixing element fights alone, and the enemy is not obliged to stay
/// fixed: every round of the transit costs the fire the fixing element will
/// take at its position ([`crate::battle::incoming`]). So a commander fixes
/// from ground where the enemy cannot hurt her much — hull-down, in cover,
/// seen by few — or does not split at all. The second draft, with the flank
/// priced and the split not, still lost: two tanks held a position against
/// four while the other two spent five rounds going round.
///
/// The other terms are in the same unit: a hex of the flanking route the
/// enemy can see, and a round spent getting into place, cost what a round of
/// fire is worth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FixAndFlank {
    /// Movement points a hex of the flanking route costs extra for being in
    /// sight of a known enemy: how much the manoeuvre wants dead ground.
    pub exposure_price: u32,
    /// Nearest and furthest the flanking position may be from the enemy
    /// being flanked, in hexes.
    pub standoff: [u32; 2],
    /// Rounds of fire the better position is expected to be worth: what the
    /// per-round gain in worth is multiplied by.
    pub rounds_of_fire: f32,
    /// Worth each hex of the flanking route the enemy can see costs.
    pub per_hex_exposed: f32,
    /// Worth each round the slower element needs to get into place costs.
    pub per_round: f32,
    /// Share of the fire the fixing element takes at its position, per round
    /// the flankers are away, that is charged to the plan. One is all of it.
    pub alone_share: f32,
    /// What a candidate must score before it is worth doing at all rather
    /// than the ordinary allocation of ground.
    pub threshold: f32,
}

impl Default for FixAndFlank {
    fn default() -> Self {
        Self {
            exposure_price: 3,
            standoff: [2, 5],
            rounds_of_fire: 4.0,
            per_hex_exposed: 1.5,
            per_round: 2.0,
            alone_share: 1.0,
            threshold: 0.0,
        }
    }
}
