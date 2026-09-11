//! How the AI thinks, as data rather than as constants scattered through
//! `ai/`.
//!
//! This is deliberately a block of its own rather than a handful more fields on
//! [`Balance`](super::Balance), and the distinction is worth keeping: `balance`
//! says what the *rules* are — how much a point of gunnery is worth, how far
//! through a plate a marginal round gets — and applies to a human player's
//! shot exactly as it does to a machine's. Nothing here reaches a rule. Every
//! number below changes only what a commander *decides*, and a mod that
//! rewrote every one of them would leave a replay of a human-versus-human
//! battle bit-for-bit identical. Filing them under `balance` would have quietly
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
//! There are two families here and they answer different questions. The
//! goal chooser's numbers — [`impatience`](PlannerRules::impatience),
//! [`horizon_rounds`](PlannerRules::horizon_rounds),
//! [`boarding_rounds`](PlannerRules::boarding_rounds),
//! [`deviation_cost`](PlannerRules::deviation_cost),
//! [`devolved`](PlannerRules::devolved) — say what a commander is willing to
//! *drive* for and who she trusts to pick their own ground. The evaluator's
//! — [`score_worth`](PlannerRules::score_worth),
//! [`order_worth`](PlannerRules::order_worth),
//! [`order_complement`](PlannerRules::order_complement),
//! [`pull_under_fire`](PlannerRules::pull_under_fire),
//! [`distance_decay`](PlannerRules::distance_decay),
//! [`plateau`](PlannerRules::plateau),
//! [`cover_prior`](PlannerRules::cover_prior) and
//! [`elevation_prior`](PlannerRules::elevation_prior) — say what a piece of
//! ground is worth once she is looking at it, and in particular what an *order* is worth
//! against the terrain, which is the question the design memo opens with.
//! They are four terms in one sum, which is why they arrived as one chunk:
//! sweeping any of them alone says less than sweeping the shape.
//!
//! **The prize is the sweep.** Until these were data the only way to ask what
//! one of them was worth was to edit Rust and rebuild, so mostly nobody
//! asked: `horizon_rounds` was set on the impatience arithmetic alone and
//! only later measured to prune nothing at six — the whole radius-20 map.
//! Note what the first sweep bought, because it is the honest advertisement
//! for this file: `--sweep planner.horizon_rounds=1,2,3,4,6,8` came back
//! **null**, a horizon of one round and one of eight being the same game
//! inside the seed noise floor. That is a negative result delivered in four
//! seconds instead of a belief held for another month.

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
    12.0
}

/// Serde's default for [`PlannerRules::devolved`].
fn devolved() -> f32 {
    0.6
}

/// Serde's default for [`PlannerRules::exit_urgency`].
fn exit_urgency() -> f32 {
    3.0
}

/// Serde's default for [`PlannerRules::score_worth`].
///
/// **1.0 is the game before this field existed** — an objective's `value`
/// reached the evaluator unscaled — and 3.0 is what ships, for the reason on
/// the field itself. The two are the same rule at different rates; the
/// default follows the shipped value because a mod that declares no `planner`
/// block must play the game the base mod plays, which is the contract
/// `the_planner_numbers_can_be_swept_and_ship_at_the_values_they_replaced`
/// keeps and the reason `deviation_cost`'s default has moved three times.
fn score_worth() -> f32 {
    3.0
}

/// Serde's default for [`PlannerRules::order_worth`].
///
/// The value that most nearly reproduces what `mission_weight` did. That
/// field was a flat 2.0 multiplied by an arrival reward of 1.5, so being on
/// the ordered ground was worth 3.0 to every crew on the field regardless of
/// what she was. This is a share of her instead, against an arrival reward
/// rebased to 1.0 so the number means what it says, and the shipped maps put
/// eleven substance points on a typical chassis (11.33 on `battle_plains`,
/// 10.67 on `battle_forest`, 7.50 on the partly-crewed `river_crossing`): a
/// quarter of eleven is 2.75 against the 3.0 it replaces, and the two are
/// exactly equal at twelve.
///
/// What the rebase does move is the *ratio* between arriving and getting
/// nearer. Arriving used to be worth ten hexes of the shared
/// [`PlannerRules::distance_decay`] slope and is now worth about seven. That
/// is the price of the number being a share rather than a multiple of a
/// multiple, and it is the thing to sweep if an order starts landing near its
/// ground rather than on it.
fn order_worth() -> f32 {
    0.25
}

/// Serde's default for [`PlannerRules::order_complement`].
///
/// 0.0 is the game before the field existed — an order quoted against what
/// she has left and nothing else — and a half ships, for the reason on the
/// field: the designer's "scaled down", and the instrument could not choose
/// better. The default follows the shipped value, as every field in this
/// block's does, because a mod that declares no `planner` block must play
/// the game the base mod plays.
fn order_complement() -> f32 {
    0.5
}

/// Serde's default for [`PlannerRules::pull_under_fire`].
fn pull_under_fire() -> f32 {
    0.25
}

/// Serde's default for [`PlannerRules::distance_decay`].
fn distance_decay() -> f32 {
    0.15
}

/// Serde's default for [`PlannerRules::plateau`].
fn plateau() -> f32 {
    0.3
}

/// Serde's default for [`PlannerRules::cover_prior`].
fn cover_prior() -> f32 {
    0.03
}

/// Serde's default for [`PlannerRules::elevation_prior`].
fn elevation_prior() -> f32 {
    0.4
}

/// Serde's default for [`PlannerRules::kill_bonus`].
fn kill_bonus() -> f32 {
    4.0
}

/// Serde's default for [`PlannerRules::attack_worth`].
fn attack_worth() -> f32 {
    2.0
}

/// Serde's default for [`PlannerRules::attack_floor`].
fn attack_floor() -> f32 {
    0.5
}

/// Serde's default for [`PlannerRules::withdrawn_attack`].
fn withdrawn_attack() -> f32 {
    0.0625
}

/// Serde's default for [`PlannerRules::exposure_cap`].
fn exposure_cap() -> f32 {
    4.0
}

/// Serde's default for [`PlannerRules::crowding_adjacent`].
fn crowding_adjacent() -> f32 {
    0.45
}

/// Serde's default for [`PlannerRules::crowding_near`].
fn crowding_near() -> f32 {
    0.15
}

/// Serde's default for [`PlannerRules::support_range`].
fn support_range() -> f32 {
    4.0
}

/// Serde's default for [`PlannerRules::out_of_support`].
fn out_of_support() -> f32 {
    0.12
}

/// Serde's default for [`PlannerRules::advance_slope`].
fn advance_slope() -> f32 {
    0.3
}

/// Serde's default for [`PlannerRules::search_slope`].
fn search_slope() -> f32 {
    0.15
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
    /// to be worth 2.1 more than her orders and recon pull at 0.9 needs 0.3.
    /// At `initiative: 1` there is no charge and she weighs her orders as one
    /// option among several; at `initiative: 0` there are no other options to
    /// weigh and the charge is never levied, which is the game before
    /// subordinate initiative existed.
    ///
    /// Set against the scale the evaluator already speaks, like
    /// [`Self::impatience`]: it makes a low-initiative crew hold her course
    /// against anything short of a clearly better piece of ground and lets a
    /// high-initiative one take the good firing position she is driving past.
    ///
    /// **It has now moved twice, 2.0 → 3.0 → 12.0, and both moves are the same
    /// event.** The number's job is to sit between the shipped doctrines'
    /// `initiative` values — `the_shipped_doctrines_straddle_the_price_of_deviating`
    /// is that property as a test — and it is quoted in the evaluator's
    /// currency, so it has to move whenever the currency does.
    ///
    /// Phase 2 was the first: the threat term used to be zero over most of the
    /// map (a six-hex gate with a distance falloff) and became the resolver's
    /// own expected damage everywhere a found gun can reach, three to eight
    /// points of it. At 2.0 massed armour — the doctrine whose whole character
    /// is driving at the hex it was given — stopped to fight from ground of
    /// its own, and 3.0 was the smallest value that restored the straddle
    /// (3.0 through 6.0 all did; at 8.0 elastic defence stopped deviating
    /// too).
    ///
    /// Wave 1 was the second, and larger: both the attack and the threat term
    /// are now a *round* of fire rather than one shot, and every gun in the
    /// base mod fires between two and twelve times in a round; on top of that
    /// the fire a crew cannot be hurt by is priced at all now. So the fighting
    /// half of the score grew by roughly its cadence while the price of
    /// disobedience did not, and at 3.0 every doctrine deviated again.
    ///
    /// Measured on `pressed_stage`, where the crew and the gun she can see are
    /// both mediums, and measured **twice** because the mechanism and the
    /// content it enables move the currency by different amounts: with
    /// suppression declared nowhere the straddle holds from 8.5 to 19, and
    /// with the base mod's shipped suppression and `point_worth` it holds from
    /// 12 to 29. One value has to serve both, or the field would mean
    /// something different in a mod that declines the new rule; 12.0 is the
    /// bottom of the intersection and the band is still eight points wide, so
    /// this is not a knife edge either.
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
    /// What one point of a map objective's `value` is worth, in substance
    /// points a round — the exchange rate between the scoreboard and the
    /// fighting.
    ///
    /// Every other term in [`Evaluator::score_tile`](crate::ai::Evaluator)
    /// is denominated in what the resolver says a round of fire is worth: the
    /// attack term, the threat term, the danger overlay a player reads. The
    /// objective term was not. It was `value * decay` on a scale of its own,
    /// chosen when a shot was priced per trigger pull and threat was zero
    /// over most of the map, and after cadence and pressure joined the
    /// currency it was worth two to twelve times less than the terms it has
    /// to argue with. Measured on `ridge_arena`, where a bare level-2 crest
    /// worth 3 sits under every found gun, the difficulty-5 commander lost to
    /// the difficulty-1 one: she alone could see what the crest cost and
    /// nothing told her what holding it was for.
    ///
    /// So this is the sentence the evaluator was missing — *a point of score
    /// is worth this much of a tank, per round* — and it is a mod's to say,
    /// because how much a battle is about its ground rather than about its
    /// tanks is a design decision and not a rule of the battlefield.
    ///
    /// It scales an exit's pull through [`Self::exit_urgency`] as well, which
    /// is quoted in objective-value units and would otherwise silently stop
    /// meaning that.
    ///
    /// **3.0 ships and 1.0 is the game before, and the honest report of the
    /// sweep is that the win column could not tell them apart.** On
    /// `--arena ridge_arena` at eight seeds and 576 battles a row, `5 over 1`
    /// reads 52.3 / 53.3 / 53.5 / 52.1 / 53.0 / 54.0 per cent at 1, 2, 3, 4,
    /// 6 and 12, which is one spread of noise from end to end. That is not a
    /// null of the `pull_under_fire` kind — the term is evaluated constantly —
    /// it is a **symmetric** number: both commanders get the same rate, so it
    /// changes what a battle is about rather than which side is better at it,
    /// and a table whose whole content is one side against another cannot see
    /// it. The one value the table *can* see is zero, and it is catastrophic:
    /// 9.2 per cent and 225 draws of 288, because nothing then leaves cover.
    ///
    /// What it moves is the battle. Over the determinism baseline's four
    /// seeds of `river_crossing`, 1.0 to 3.0 takes `ObjectiveTaken` from five
    /// to eight and drops the rounds fought from 22 to 17, with two more
    /// vehicles destroyed: ground changes hands and the fights over it are
    /// settled sooner. 3.0 is also the rate at which the ridge's crest, worth
    /// three points, prices at 13.5 substance points against the 8 to 29 a
    /// round that a found gun puts on a crew standing there — inside a factor
    /// of two of the fire it has to argue with, which is the comparison this
    /// field exists to make possible. Below that it is arithmetic nobody
    /// consults; far above it a crew drives onto scoring ground through
    /// anything.
    #[serde(default = "score_worth")]
    pub score_worth: f32,
    /// The share of what a crew has left, per round, that being on the ground
    /// her commander named is worth to her.
    ///
    /// The size of an order's pull, and therefore the answer to the question
    /// the whole design memo opens with: an order competes with the terrain,
    /// so how much is it worth against a good hedge? Quoted **as a share of
    /// her** rather than as a number, which is the whole of what Wave 2 did
    /// to it. The number it replaces, `mission_weight`, was a flat 2.0 on the
    /// objective scale, so a fresh heavy tank and a shot-up scout car valued
    /// the same order identically while the danger at the ordered ground was
    /// priced in substance points that meant very different things to the two
    /// of them. An order is now worth a quarter of a heavy and a quarter of a
    /// scout car, which is what "go and take that hill" actually asks of
    /// each.
    ///
    /// **A quarter is the designer's number and the sweep is what keeps it
    /// from being magic.** `--sweep planner.order_worth=…` on the delegation
    /// table is the instrument, because that table is where a subordinate who
    /// will not go where she was sent shows up. Measured at 288 battles a
    /// cell with `--set planner.devolved=1.1`, so that both commanded
    /// doctrines actually assign ground — in the shipped game elastic defence
    /// devolves and issues none, which is why the same sweep without it moves
    /// three wins in 36 and says nothing:
    ///
    /// | `order_worth` | massed armour's tax | bounding overwatch's tax |
    /// | --- | --- | --- |
    /// | 0.0 | +2 | +53 |
    /// | 0.1 | −6 | +43 |
    /// | **0.25** | **−10** | **+35** |
    /// | 0.5 | −15 | +29 |
    /// | 1.0 | −11 | +30 |
    /// | 2.0 | −18 | +30 |
    ///
    /// A doctrine's tax is what it loses by fighting through missions instead
    /// of for itself, against the same flat opponent, and zero is the target.
    /// Both flat controls are **bit-identical at every value**, which is the
    /// additivity claim measured rather than asserted: this number reaches
    /// commanded sides and nothing else. Massed armour crosses zero somewhere
    /// under a tenth and overshoots; bounding overwatch takes most of the
    /// improvement available to it by a quarter and flattens. A quarter is
    /// where the first is still near zero and the second has stopped
    /// improving, which is a defensible reading of a table with a noise floor
    /// of about a standard deviation of 8 wins in 288 — not a knife edge, and
    /// not a number the instrument chose on its own.
    ///
    /// Two doctrine terms still scale it before it is added: `1.5 -
    /// delegation` (how literally this commander expects to be obeyed,
    /// floored at 1.0 under [`Latitude::Binding`](crate::battle::Latitude))
    /// and `objective_value` (how much this doctrine cares about ground at
    /// all). Note it does *not* govern how strongly the letter of the order
    /// is enforced against her own judgment — that is
    /// [`Self::deviation_cost`], one layer up in the goal chooser, and the
    /// two are worth keeping apart: this is what the ground is worth, that
    /// is what disobedience costs.
    #[serde(default = "order_worth")]
    pub order_worth: f32,
    /// How much of an order is quoted against her full complement rather
    /// than against what she has left.
    ///
    /// [`Self::order_worth`] is a share of what she still has aboard, and
    /// the threat term is what the resolver says a round of fire would take
    /// off her, so as a crew wears down the two move in opposite directions:
    /// the danger at the ordered ground is priced as a fraction of *less*
    /// and the order as a share of *less*, and a half-destroyed crew weighs
    /// her orders against the guns covering them about four times more
    /// cautiously than a fresh one does. Two defensible rules, and their
    /// product was a consequence nobody had chosen. The designer's ruling
    /// (2026-09-09) is that an order is quoted against her *and*, scaled
    /// down, against what she was raised as: at `c`, the order is worth
    /// `order_worth × ((1 − c) × left + c × full)` a round, so a fresh crew
    /// reads her orders exactly as before at every value, and a crew with
    /// nothing left still holds them at `c` of what a fresh one would.
    ///
    /// 0 is the game before, and 1 quotes the order against her full
    /// complement only, which makes a nearly finished crew as obedient as a
    /// fresh one while the danger term prices the same ground as far more —
    /// the other end of the same product. `validate-mods` warns outside
    /// `0..=1`: below zero a worn crew values an order *less* than a share
    /// of what she has left, and above one more than a fresh one would.
    ///
    /// **A half ships, and the honest report of the sweep is that it is
    /// below the instrument's floor.** On the delegation table at eight
    /// seeds × 36 with `--set planner.devolved=1.1` (288 battles a cell, the
    /// sweep that chose `order_worth`), 0, 0.25, 0.5, 0.75 and 1 are
    /// bit-identical in 140 of 160 non-baseline rows, and the twenty that
    /// differ move by one win or a handful of shots, nearly all at 1.0. On
    /// `ridge_arena` the skill table is bit-identical at every value,
    /// because no side there fights under a mission and the field is never
    /// reached. The number is live — something moves at 1 — and a crew both
    /// worn and under orders is rare enough in AI-vs-AI play that the table
    /// cannot see her; the case it is for is a player's, a crew half gone
    /// and still told to hold. So the value is the designer's, not the
    /// instrument's, which is recorded rather than dressed up.
    #[serde(default = "order_complement")]
    pub order_complement: f32,
    /// `mission_weight`, retired in Wave 2 and kept here only so that a mod
    /// still declaring it can be told.
    ///
    /// A removed field is the one kind of mod error this project's data
    /// machinery cannot otherwise report: serde ignores what it does not
    /// recognise, so a modder who had tuned `mission_weight` would load
    /// cleanly, play a different game from the one they wrote, and have
    /// nothing to grep for. Deserialised under its old name, never
    /// serialised (so `--set planner.mission_weight=…` fails with the list of
    /// fields that do exist, which names its replacement), and **warned about
    /// rather than refused** by `validate-mods`: a stale key from an older
    /// engine is a thing a mod may legitimately carry, and refusing to load
    /// over one would be a harsher rule than this file applies to any value.
    #[serde(default, rename = "mission_weight", skip_serializing)]
    pub retired_mission_weight: Option<f32>,
    /// The share of a movement to contact's pull that survives being shot
    /// at.
    ///
    /// `Advance` and `Recon` are orders that halt and fight what they meet:
    /// while somebody is shooting at her where she stands, the pull toward
    /// the commander's hex is cut to this fraction and her own appetites —
    /// the shot in front of her, cover, the threat she is under — decide the
    /// round. Nothing latches; when the enemy is dead or lost the damping
    /// evaporates and the march resumes.
    ///
    /// **1.0 is not the neutral value, it is a different game.** An advance
    /// that presses on through fire is precisely
    /// [`Mission::Assault`](crate::battle::Mission::Assault), which is
    /// exempt from this term already — so a mod that sets this to 1.0 has
    /// not turned a rule off, it has made the two verbs synonyms and thrown
    /// away the distinction a player uses `G` and `X` to express. The
    /// playtest that forced the term is in the code comment beside it: a
    /// delegated advance walked into effective fire at the bridge and was
    /// gone by round three, because "go there" outbid every local reason not
    /// to.
    #[serde(default = "pull_under_fire")]
    pub pull_under_fire: f32,
    /// How much a hex of distance takes off what a piece of ground is
    /// worth: the slope of the gradient that leads a crew to it.
    ///
    /// A greedy one-round planner can only see the tiles it can reach this
    /// round, so ground whose value is flat until you arrive is ground it
    /// cannot navigate to. Every objective and every mission is therefore
    /// scored as a reward for *being* there minus this slope for being away
    /// from it, which is what lets a unit twenty hexes off still know which
    /// way to drive.
    ///
    /// **One number for both objectives and missions, on purpose.** It used
    /// to be one number because [`Self::order_worth`]'s predecessor was
    /// quoted in objective-value units — "an order pulls about as hard as the
    /// ford" — and that sentence was only true while the two gradients had
    /// the same shape. The two are quoted in different things now (a point of
    /// score against a share of herself), so the reason is a better one: the
    /// slope is not a statement about what ground is worth at all, it is a
    /// statement about **how far off a crew can still tell which way to
    /// drive**, and that is a fact about her and the map rather than about
    /// the prize. Two slopes would be two answers to it.
    ///
    /// Shallower reaches further and flattens the choice between distant
    /// ground; steeper makes a crew take the nearest worthwhile thing and
    /// ignore the map. It was swept over 24 battles when objectives were
    /// introduced and both halves of the curve were bad in different ways —
    /// see the note on `Evaluator::objective_value`.
    #[serde(default = "distance_decay")]
    pub distance_decay: f32,
    /// How much better a tile has to score before a crew will drive across
    /// open ground to reach it.
    ///
    /// Tiles within this much of the best are treated as ground the
    /// evaluator cannot meaningfully tell apart, and among them she takes
    /// the one nearest where she already stands. It is a dispersion rule
    /// before it is a movement-economy one: open ground scores in broad
    /// plateaus, and without this every identical crew on a side made the
    /// identical choice and arrived as a queue — which is what made a
    /// noiseless side play *worse* than a randomly scattered one and
    /// inverted the whole difficulty ladder.
    ///
    /// At 0 the plateau rule is off and that pathology is back, so unlike
    /// most numbers here zero is not the gentle setting. Too wide and a crew
    /// declines real improvements to stay where she is; the band wants to be
    /// about the width of the noise between two pieces of indistinguishable
    /// grass, which is what 0.3 was measured to be.
    #[serde(default = "plateau")]
    pub plateau: f32,
    /// What a point of a terrain's `cover` is worth to a crew choosing
    /// ground, before the doctrine's own `cover_value` scales it.
    ///
    /// A *prior*, and the word is doing work. What cover actually does to a
    /// shot is `balance.cover_against_accuracy`, and since Phase 2 the
    /// evaluator's threat term reads it through the resolver itself — per
    /// gun, per bearing, with range and sight and obliquity alongside it. So
    /// this number is not "what cover is worth"; it is what cover is worth
    /// *against the enemies nobody has found yet*. Threat is fog-gated and
    /// therefore exactly zero until the first contact, and a crew with an
    /// empty list would otherwise have no reason to prefer a wood to a
    /// field.
    ///
    /// It is also where a doctrine's taste lives —
    /// [`DoctrineDef::cover_value`](super::DoctrineDef::cover_value) is read
    /// here and nowhere else — so setting it to zero does not merely remove a
    /// bonus, it silences that field and makes two doctrines with opposite
    /// opinions about ground identical before contact. Measured over the
    /// mirrored arena and the three shipped maps when the threat term started
    /// reading the candidate tile; the numbers are in ARCH-TODO.md.
    #[serde(default = "cover_prior")]
    pub cover_prior: f32,
    /// The same, for a level of elevation:
    /// [`Self::cover_prior`]'s twin, scaled by
    /// [`DoctrineDef::elevation_value`](super::DoctrineDef::elevation_value).
    ///
    /// Larger per unit because a level is ten metres and terrain cover runs
    /// to thirty-odd points, so the two arrive on the evaluator's scale at
    /// roughly the same size. What high ground does to a shot is
    /// `balance.downhill_bonus` and, through the sight grid, what it lets her
    /// see at all; this is the appetite for it that survives those being
    /// priced properly.
    #[serde(default = "elevation_prior")]
    pub elevation_prior: f32,
    /// What a shot that would finish a crew is worth on top of what it does.
    ///
    /// The one judgment in the attack term: `best_weapon_against` already
    /// prices the damage and the fear a round of fire does, and this says
    /// that a round which *ends* the argument is worth more than the
    /// substance it happens to take with it. Quoted in the same worth per
    /// round the rest of the sum is, so at 4.0 a finishing shot carries
    /// about a third of a medium tank over and above what it destroys.
    ///
    /// Zero is the arithmetic with no judgment in it. She still takes the
    /// killing shot whenever it is the best round of fire on offer — it
    /// nearly always is — but she will not cross a field for it.
    #[serde(default = "kill_bonus")]
    pub kill_bonus: f32,
    /// What a round of fire from a tile is worth per point of the doctrine's
    /// [`aggression`](super::DoctrineDef::aggression).
    ///
    /// The attack half of `score_tile` is multiplied by `attack_worth *
    /// (attack_floor + aggression)`, which is the one place a doctrine's
    /// appetite reaches the ground she picks rather than the orders she is
    /// given — everything else `aggression` touches is posture and caution.
    ///
    /// It stays a product of two fields rather than being folded into the
    /// slope-and-intercept it is algebraically equal to (`1.0 + 2.0 *
    /// aggression`), because f32 multiplication is not associative enough
    /// for that rewrite to be free: the two forms differ in the last bit for
    /// some values of `aggression`, and the determinism baseline would move
    /// for a tidier line. The arithmetic the numbers were measured in is the
    /// arithmetic that ships.
    #[serde(default = "attack_worth")]
    pub attack_worth: f32,
    /// The appetite a doctrine with no `aggression` at all still brings to a
    /// round of fire, in the units `aggression` is quoted in.
    ///
    /// [`Self::attack_worth`]'s partner. At the shipped pair a commander at
    /// `aggression: 0` prices a round of fire at 1.0 and one at
    /// `aggression: 1` at 3.0, so the whole doctrine range spans a factor of
    /// three. At zero the floor goes with it and a doctrine with no appetite
    /// stops valuing fire at all, which is not a cautious commander but a
    /// blind one: she would decline a free shot to stand on marginally
    /// better grass.
    #[serde(default = "attack_floor")]
    pub attack_floor: f32,
    /// What a round of fire is worth to a crew who has been ordered out, as a
    /// share of what it is worth to everybody else.
    ///
    /// A scale on the attack term rather than a term of its own, because a
    /// withdrawing crew still answers what is in front of her and simply
    /// does not *seek*. The shipped number is a quarter of one **shot** —
    /// 0.25 divided by the four times a medium tank fires in a round — and
    /// the sentence is what is preserved rather than the digit: when cadence
    /// arrived and the attack term became a round of fire, the flat quarter
    /// put 4.6 points back in front of a withdrawing crew and
    /// `an_ordered_withdrawal_needs_no_wounds` caught Anka Weiss planning
    /// away from her lane, ten hexes off it to fifteen.
    ///
    /// Measured on that stage anything at or below 0.15 keeps the withdrawal
    /// a withdrawal and 0.20 does not, so it is not a knife edge. **1.0 is
    /// the rule's absence**: an ordered retreat weighed exactly like any
    /// other march, which is the game before the term existed.
    #[serde(default = "withdrawn_attack")]
    pub withdrawn_attack: f32,
    /// Most a crew may multiply the danger she reads by for being small,
    /// worn down, or carrying somebody.
    ///
    /// Exposure is `typical_substance / left` times what her passengers add,
    /// and both halves run away at the bottom: a remnant platoon with one
    /// point left divides by one and a carrier full of infantry doubles it
    /// again. Four is "refuses what a fresh crew accepts", which is as far as
    /// the distinction still says anything; past it she reads every tile on
    /// the map as certain death and the threat term stops discriminating.
    ///
    /// There is no neutral value — at 1.0 nobody is ever more careful than a
    /// fresh crew, which is the same flattening from the other end.
    #[serde(default = "exposure_cap")]
    pub exposure_cap: f32,
    /// What standing one hex from the nearest friend costs.
    ///
    /// The close half of the spacing band, and deliberately **not** scaled
    /// by [`concentration`](super::DoctrineDef::concentration): one shell
    /// killing two vehicles is survival rather than taste, and a doctrine
    /// that wants to mass is not entitled to buy it off. Zero is the game
    /// before the band, when mass was a monotonic pull toward the nearest
    /// friend all the way to adjacency and massed armour clumped into
    /// artillery bait.
    #[serde(default = "crowding_adjacent")]
    pub crowding_adjacent: f32,
    /// The same, two hexes out: the far edge of the crowding penalty.
    ///
    /// [`Self::crowding_adjacent`]'s taper, and the band is two hexes wide
    /// because a shell's splash is. Beyond it there is nothing to pay until
    /// [`Self::support_range`] is exceeded, which is the interval drill
    /// actually teaches: close enough for mutual support, far enough that
    /// one round cannot take two vehicles.
    #[serde(default = "crowding_near")]
    pub crowding_near: f32,
    /// Farthest a friend can stand and still be supporting, in hexes.
    ///
    /// Support also demands a *sight line* from the friend's planned
    /// position — near but masked is not mutual support — so this is a
    /// radius on a check rather than a radius on its own. Friends count from
    /// where they are heading rather than where they stand, which is what
    /// makes a formation converge instead of chase.
    ///
    /// Widening it is a cheaper way to disperse a doctrine than lowering
    /// [`Self::out_of_support`], because it moves where the penalty starts
    /// rather than how fast it grows.
    #[serde(default = "support_range")]
    pub support_range: f32,
    /// What each hex beyond [`Self::support_range`] costs, before the
    /// doctrine's `concentration` scales it.
    ///
    /// The old monotonic pull, kept as the outer half of the band. It is the
    /// **only** thing on a battlefield that reads
    /// [`concentration`](super::DoctrineDef::concentration) — `force::muster`
    /// reads it when an army is bought, and nothing else does — so setting
    /// this to zero does not merely remove a penalty, it silences that field
    /// and makes two doctrines with opposite opinions about spacing
    /// indistinguishable once the shooting starts. The same trap
    /// [`Self::cover_prior`] carries for `cover_value`.
    #[serde(default = "out_of_support")]
    pub out_of_support: f32,
    /// How hard a hex nearer the closest enemy pulls, per point of the
    /// doctrine's `aggression`.
    ///
    /// With something to shoot, close on it. This is not the attack term —
    /// that one prices a shot she can actually take from the tile, through
    /// the resolver — it is the appetite for contact that survives having no
    /// shot yet, and it is what walks a crew the last few hexes into range
    /// of a gun she has found but cannot reach.
    ///
    /// Zero is a defensible game rather than a broken one: with objectives
    /// on the map the score term already says which way to walk, and a
    /// doctrine that never closes for its own sake is what elastic defence
    /// is trying to be.
    #[serde(default = "advance_slope")]
    pub advance_slope: f32,
    /// How hard the middle of the map pulls a crew who has found nobody, per
    /// point of the doctrine's `scouting`.
    ///
    /// The fallback for a map that names no ground at all: "wander towards
    /// the middle and find somebody", which is the whole of what this
    /// evaluator could do before objectives existed. It is deliberately not
    /// [`Self::distance_decay`] despite sharing its shipped magnitude — that
    /// number is the slope of a gradient leading to a *named* piece of
    /// ground, and folding the two together would mean a designer asking how
    /// far an order reaches also changed how a lost crew searches an empty
    /// map.
    ///
    /// Zero leaves a crew with no contact, no objectives and no orders
    /// standing still, which is a legitimate answer to a map that has given
    /// her nothing to do and a very dull one.
    #[serde(default = "search_slope")]
    pub search_slope: f32,
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
            score_worth: score_worth(),
            order_worth: order_worth(),
            order_complement: order_complement(),
            retired_mission_weight: None,
            pull_under_fire: pull_under_fire(),
            distance_decay: distance_decay(),
            plateau: plateau(),
            cover_prior: cover_prior(),
            elevation_prior: elevation_prior(),
            kill_bonus: kill_bonus(),
            attack_worth: attack_worth(),
            attack_floor: attack_floor(),
            withdrawn_attack: withdrawn_attack(),
            exposure_cap: exposure_cap(),
            crowding_adjacent: crowding_adjacent(),
            crowding_near: crowding_near(),
            support_range: support_range(),
            out_of_support: out_of_support(),
            advance_slope: advance_slope(),
            search_slope: search_slope(),
        }
    }
}
