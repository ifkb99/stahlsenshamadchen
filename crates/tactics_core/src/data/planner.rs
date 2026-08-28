//! How the AI thinks, as data rather than as constants scattered through
//! `ai/`.
//!
//! This is deliberately a block of its own rather than five more fields on
//! [`Balance`](super::Balance), and the distinction is worth keeping: `balance`
//! says what the *rules* are — how much a point of gunnery is worth, how far
//! through a plate a marginal round gets — and applies to a human player's
//! shot exactly as it does to a machine's. Nothing here reaches a rule. Every
//! number below changes only what a commander *decides*, and a mod that
//! rewrote all five would leave a replay of a human-versus-human battle
//! bit-for-bit identical. Filing them under `balance` would have quietly
//! merged "what is true on this battlefield" with "how well is this side
//! played", which is the same conflation difficulty spent a whole arc
//! separating when it stopped being a lottery and became a lens.
//!
//! Unlike `balance`, the fields here are floats. `Balance` is all-integer on
//! purpose, so its arithmetic stays exact; these numbers are weights in the
//! evaluator's own currency, where every neighbouring term — every
//! [`DoctrineDef`](super::DoctrineDef) weight — is already `f32`. An integer
//! `impatience` would have to be a percentage of something, and there is
//! nothing to be a percentage of.
//!
//! Every field defaults to exactly the constant it replaced, so a mod that
//! declares no `planner` block — or one that declares the block and omits a
//! field — gets the AI the game has always had. That is the same additivity
//! contract the rest of the mod data keeps, and it is what makes the
//! determinism snapshot the check on this file.
//!
//! **The prize is the sweep.** `--sweep planner.horizon_rounds=2,4,6` asks
//! what looking further ahead is *worth to a commander*, which has never been
//! measured: `HORIZON` was set on the impatience arithmetic alone and only
//! later measured to prune nothing at six — the whole radius-20 map — and
//! until these were data the only way to ask was to edit Rust and rebuild.

use serde::{Deserialize, Serialize};

/// Serde's default for [`PlannerRules::impatience`].
///
/// A function rather than `#[serde(default)]`, for the reason every default in
/// this file is one: zero is not the neutral value for any of them. At
/// `impatience: 0` nothing prices the walk and every crew on the field marches
/// to whichever single hex scores highest, which is the queue the plateau rule
/// was invented to break up rebuilt one layer higher.
fn impatience() -> f32 {
    0.35
}

/// Serde's default for [`PlannerRules::horizon_rounds`].
fn horizon_rounds() -> u32 {
    4
}

/// Serde's default for [`PlannerRules::boarding_rounds`].
fn boarding_rounds() -> f32 {
    4.0
}

/// Serde's default for [`PlannerRules::deviation_cost`].
fn deviation_cost() -> f32 {
    2.0
}

/// Serde's default for [`PlannerRules::devolved`].
fn devolved() -> f32 {
    0.6
}

/// Serde's default for [`PlannerRules::exit_urgency`].
fn exit_urgency() -> f32 {
    3.0
}

/// The numbers that govern how the AI thinks: what it is willing to drive
/// for, how far ahead it looks, and when a commander stops assigning ground.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlannerRules {
    /// How much a round of driving costs, in the same units a tile is scored
    /// in.
    ///
    /// The goal chooser's price of a march. Without it every crew walks to
    /// whichever single hex scores highest, because nothing prices the walk;
    /// with it, ground three rounds away has to be worth about a point more
    /// than ground she can reach now.
    ///
    /// Set against the scale the evaluator already speaks: a typical
    /// objective is worth 2–3 and a mission's ground 2.0, so a third of a
    /// point a round makes a crew willing to spend three or four rounds
    /// reaching real ground and unwilling to cross the map for a marginal
    /// tile.
    #[serde(default = "impatience")]
    pub impatience: f32,
    /// How many rounds of driving the goal chooser bothers to price a road
    /// for.
    ///
    /// At the default `impatience` a fourth round of driving already costs a
    /// point and a half, which is most of a good objective, so ground further
    /// off than this is ground she is not going to pick however cheap the
    /// road turns out to be. Ground beyond it is priced at the horizon rather
    /// than at the crow flight — "further than I have looked", which is the
    /// honest reading and the pessimistic one.
    ///
    /// It is also the only thing bounding the Dijkstra, so it is the field
    /// here with a performance cost attached: measured on `river_crossing`,
    /// `roads` costs 61 / 106 / 155 / 219 microseconds at a horizon of two,
    /// three, four and six rounds. Six was the original value and pruned
    /// nothing — six rounds is thirty to forty-two movement points against a
    /// radius-20 hexagon, so the horizon was the whole map.
    ///
    /// A number in *rounds* rather than in hexes on purpose: a recon car
    /// looks further ahead than a heavy tank, which is right.
    #[serde(default = "horizon_rounds")]
    pub horizon_rounds: u32,
    /// Rounds a taxi run costs over and above the driving: walking to the
    /// tailgate, climbing in, and stepping off at the far end.
    ///
    /// It exists to stop a platoon mounting up to save herself half a hex,
    /// which is the failure mode a pure time comparison has. Priced at two
    /// rounds first, on the mechanics alone — a mount resolves the tick she
    /// reaches the carrier and a dismount the tick after it is ordered — and
    /// that was too cheap: a platoon delivered near her objective would
    /// re-board for a three-hex hop, ride one hex, meet the at-the-objective
    /// dismount reflex and step off again, costing the commanded side a win
    /// and three platoons over 36 battles. Four is the measured price. The
    /// mechanical cost was never the whole cost; a ride is not door to door,
    /// and the walk at each end is real.
    #[serde(default = "boarding_rounds")]
    pub boarding_rounds: f32,
    /// What it costs a subordinate under orders to act on her own idea
    /// instead, before her doctrine's `initiative` erodes it.
    ///
    /// The price of deviation, not a veto on it. A crew under orders has her
    /// own candidates on the list in proportion to
    /// [`DoctrineDef::initiative`](super::DoctrineDef::initiative), and each
    /// of them is charged `deviation_cost * (1 - initiative)` for not being
    /// what she was told to do — so massed armour at 0.3 needs her own idea
    /// to be worth 1.4 more than her orders and recon pull at 0.9 needs 0.2.
    /// At `initiative: 1` there is no charge and she weighs her orders as one
    /// option among several; at `initiative: 0` there are no other options to
    /// weigh and the charge is never levied, which is the game before
    /// subordinate initiative existed.
    ///
    /// Set against the scale the evaluator already speaks, like
    /// [`Self::impatience`]: a mission's ground is worth 2.0 and a typical
    /// objective 2–3, so 2.0 makes a low-initiative crew hold her course
    /// against anything short of a clearly better piece of ground and lets a
    /// high-initiative one take the good firing position she is driving past.
    ///
    /// `Hold` is deliberately exempt: standing still was already on an
    /// ordered crew's list before initiative existed, and charging her for it
    /// would change the game a doctrine with no initiative plays.
    #[serde(default = "deviation_cost")]
    pub deviation_cost: f32,
    /// The `delegation` level at or beyond which a commander stops assigning
    /// ground and trusts her formations' own judgment.
    ///
    /// Directive command in the Auftragstaktik tradition, as opposed to the
    /// detailed orders a centralized doctrine writes. Withdrawal is exempt in
    /// the code that reads this: whether to keep fighting is never devolved.
    ///
    /// Measured before it was believed — pinning elastic defence to anchor
    /// hexes cost it 16 of 24 wins against a flat opponent, because choosing
    /// its own ground is its game. Note this reads against
    /// [`DoctrineDef::delegation`](super::DoctrineDef::delegation), so a mod
    /// moving it moves which of its *shipped* doctrines devolve: at 0.6 the
    /// base mod's elastic defence (0.7) and recon pull (0.9) do and massed
    /// armour (0.3) does not.
    #[serde(default = "devolved")]
    pub devolved: f32,
    /// How much a crew that is finished wants the exit, in the same units as
    /// an objective's `value`.
    ///
    /// What an exit *pays* and how badly a broken crew wants it are different
    /// quantities, and multiplying by `value` the way ground does conflates
    /// them: a retreat lane must be worth almost no points — winning by
    /// running away is not winning — while still pulling hard enough to cross
    /// a map. So this is set to the worth of a good piece of ground, and a
    /// tank down to its last hit point pulls toward the lane about as hard as
    /// a fresh one pulls toward the bridge.
    #[serde(default = "exit_urgency")]
    pub exit_urgency: f32,
}

impl Default for PlannerRules {
    fn default() -> Self {
        Self {
            impatience: impatience(),
            horizon_rounds: horizon_rounds(),
            boarding_rounds: boarding_rounds(),
            deviation_cost: deviation_cost(),
            devolved: devolved(),
            exit_urgency: exit_urgency(),
        }
    }
}
