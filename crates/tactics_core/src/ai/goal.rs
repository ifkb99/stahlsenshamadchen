//! What a crew is trying to achieve, and who decides.
//!
//! # The seam
//!
//! This module exists to be replaced in half. Choosing a goal is judgement
//! and is the part a learned policy would take over; generating the
//! candidates is knowledge about the map and the rules, and staying shared is
//! the point of it. Splitting them that way is what keeps a policy's action
//! space to a handful of statements rather than to every hex on a
//! 1261-tile board, and lets it inherit pathing, boarding, dismounting,
//! opportunity fire and defiance from an executor that already works.
//!
//! In the vocabulary of the options framework: [`candidates`] is the
//! initiation set, [`GoalChooser`] is the policy over options,
//! [`crate::battle::Goal::finished`] is the termination condition, and the
//! executor in [`super::utility`] is the intra-option policy.
//!
//! # What is deliberately NOT here
//!
//! A subordinate weighing her own goal against the one she was ordered.
//! `DoctrineDef::initiative` exists for exactly that and is still unread; the
//! comparison belongs here and is the next chunk. It is left out so that this
//! one can be measured on its own — landing both at once would mean not
//! knowing which of them moved the numbers, which is the attribution problem
//! that has already cost this project a day.

use crate::battle::{BattleState, Goal, Mission, UnitId};
use crate::data::DataRegistry;
use crate::map::ObjectiveKind;
use hexx::Hex;
use rand::RngExt;
use rand_chacha::ChaCha8Rng;

/// How much a round of driving costs, in the same units a tile is scored in.
///
/// Without it every crew on the field walks to whichever single hex scores
/// highest, because nothing prices the walk — which is the queue the plateau
/// rule was invented to break up, rebuilt one level higher. With it, ground
/// three rounds away has to be worth about a point more than ground she can
/// reach now.
///
/// Set against the scale the evaluator already speaks: a typical objective is
/// worth 2–3 and `MISSION_WEIGHT` is 2.0, so a third of a point a round makes
/// a crew willing to spend three or four rounds reaching real ground and
/// unwilling to cross the map for a marginal tile. It belongs in `mod.json`
/// with the rest of the evaluator's numbers — see TODO — and is a constant
/// here for the same reason those still are.
const IMPATIENCE: f32 = 0.35;

/// The goals worth considering for this crew right now.
///
/// Deliberately a short list of *places that mean something*, not a raster of
/// the map. Scoring every reachable tile is what the executor already does
/// for its next hop and it is the hottest thing the AI has; doing it over a
/// three-round radius would be seven hundred tiles a unit a round. More to
/// the point, a raster is the wrong object: "somewhere near the ford" is not
/// a different intention from "the ford", and a policy choosing among six
/// meaningful options can be reasoned about while one choosing among seven
/// hundred hexes cannot.
///
/// Ordering is deterministic — objectives in map order, then the mission,
/// then the fallback — because a chooser that breaks ties by position in this
/// list must break them the same way on every machine.
pub fn candidates(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    mission: Option<&Mission>,
    best_reachable: Option<Hex>,
) -> Vec<Goal> {
    let Some(me) = state.unit(unit) else {
        return vec![Goal::Hold];
    };
    let mut out = Vec::new();
    let mut push = |hex: Hex| {
        let goal = Goal::Take(hex);
        if !out.contains(&goal) {
            out.push(goal);
        }
    };

    // **Orders first, and orders alone.** A mission that names ground does
    // not compete with the crew's own ideas here — it replaces them. That is
    // not timidity about the goal layer, it is the rule the whole direction
    // memo was written to restore: an order that a subordinate may outbid
    // because she likes a different hill is a weighted opinion, not an order.
    // Making the mission merely one more candidate silently undid step 1, and
    // `a_cut_off_unit_keeps_the_orders_she_had` caught it — a crew under
    // orders and a crew with none started choosing the same ground.
    //
    // This is also exactly the seam the next chunk needs. Subordinate
    // initiative is *the widening of this list*: a doctrine with high
    // `initiative` admits her own candidates alongside the ordered one and
    // lets her weigh them, and a doctrine with none never does. Nothing else
    // has to move for that to arrive.
    if let Some(hex) = mission.and_then(ordered_ground) {
        return vec![Goal::Take(hex), Goal::Hold];
    }

    // Ground the map says is worth having. Exits are not here: leaving is
    // either an order (`Mission::Withdraw`) or a crew's own nerve failing
    // (`DefianceResponse::Flight`), and neither of those is a goal she
    // reasons her way to.
    for objective in state.map.objectives() {
        if objective.kind != ObjectiveKind::Hold || !objective.open_to(me.side) {
            continue;
        }
        // One crew per piece of ground. A section takes a hex each rather
        // than four vehicles queueing for the same one, and saying it in the
        // candidate list says it once — the alternative is a tie-break buried
        // in whatever does the scoring, which is where this problem lived
        // before and where it was hard to see.
        for hex in &objective.hexes {
            if claimed_by_another(registry, state, unit, *hex) {
                continue;
            }
            push(*hex);
        }
    }

    // The executor's own answer for this round, which guarantees the goal
    // layer can never do worse than having no goal layer: if nothing on the
    // list beats the tile she would have driven to anyway, she drives to it.
    if let Some(hex) = best_reachable
        && hex != me.pos
    {
        push(hex);
    }

    out.push(Goal::Hold);
    out
}

/// The ground a mission names, if it names any. `Hold` with no hex, a
/// withdrawal and a support task all describe a posture rather than a
/// destination, and leave the crew to work out where to stand.
fn ordered_ground(mission: &Mission) -> Option<Hex> {
    match mission {
        Mission::Advance { to } | Mission::Assault { to } | Mission::Recon { toward: to } => {
            Some(*to)
        }
        Mission::Hold { at } => *at,
        Mission::Withdraw { .. } | Mission::Support { .. } => None,
    }
}

/// Whether this hex is already somebody else's, counting both the crews
/// standing on it and the ones driving for it.
///
/// This used to be "is any friend on it or making for it", which was the
/// dispersion rule the plateau tie-break was quietly doing, said once and
/// visibly. Capacity made that statement too strong: a hex is 100 m across,
/// a wood holds five footprints, and a platoon that cannot pick the timber
/// its own carrier is sitting in has no way to *use* the ground. So the rule
/// is now the same one the engine enforces — is there room — and on terrain
/// that declares no capacity it collapses back to one crew per hex, exactly
/// as before.
///
/// A passenger is skipped: her `pos` mirrors her carrier's, so counting her
/// would charge the hex twice for one vehicle.
fn claimed_by_another(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    hex: Hex,
) -> bool {
    let Some(me) = state.unit(unit) else {
        return false;
    };
    let taken: u32 = state
        .units
        .iter()
        .filter(|u| u.alive && u.id != unit && u.side == me.side)
        .filter(|u| (u.aboard.is_none() && u.pos == hex) || u.goal == Some(Goal::Take(hex)))
        .filter_map(|u| registry.vehicle(&u.vehicle))
        .map(|v| v.footprint())
        .sum();
    if taken == 0 {
        return false;
    }
    let capacity = state
        .terrain_at(hex)
        .and_then(|id| registry.terrain(id))
        .and_then(|t| t.capacity);
    let Some(capacity) = capacity else {
        return true;
    };
    let mine = registry
        .vehicle(&me.vehicle)
        .map(|v| v.footprint())
        .unwrap_or(1);
    taken + mine > capacity.max(mine)
}

/// Picks one goal from the candidates. **The replaceable half.**
///
/// A learned policy implements this and nothing else: it sees the state and a
/// short list of options and returns one of them. Everything about how the
/// goal is then carried out — routes, taxis, when to shoot, what happens when
/// her nerve goes — stays where it is.
pub trait GoalChooser {
    fn choose(
        &mut self,
        registry: &DataRegistry,
        state: &BattleState,
        unit: UnitId,
        candidates: &[Goal],
    ) -> Goal;
}

/// The hand-written chooser: doctrine-weighted value of being there, less
/// what the drive costs.
///
/// Note what it does *not* do. It does not look at the route, or at what
/// might happen on the way, or at what the enemy will do about it. Those are
/// the things a search or a learned policy would add, and leaving them out is
/// what keeps this cheap enough to run for every unit every time a goal
/// finishes.
pub struct UtilityChooser<'a> {
    pub evaluator: &'a super::Evaluator,
    /// Difficulty, as it applies to judgement rather than to fidgeting.
    ///
    /// This is where the blur belongs now, and moving it here is most of what
    /// makes difficulty mean something. It used to jitter the choice between
    /// interchangeable hexes, which produced a unit that twitched; applied to
    /// a handful of goals it produces a commander who **goes to the wrong
    /// place** — an error a player can see, name, and punish.
    ///
    /// A per-candidate draw is safe here and was not safe over tiles. The
    /// pathology was an argmax over ninety interchangeable options, where the
    /// maximum of ninety draws is nearly the top of the range every time; over
    /// five or six meaningful options the same draw is an opinion about which
    /// objective matters, which is exactly what a worse commander has.
    pub noise: f32,
    pub rng: &'a mut ChaCha8Rng,
}

impl GoalChooser for UtilityChooser<'_> {
    fn choose(
        &mut self,
        registry: &DataRegistry,
        state: &BattleState,
        unit: UnitId,
        candidates: &[Goal],
    ) -> Goal {
        let Some(me) = state.unit(unit) else {
            return Goal::Hold;
        };
        let speed = registry
            .vehicle(&me.vehicle)
            .map(|v| v.movement.points.max(1))
            .unwrap_or(1) as f32;
        let mut best: Option<(f32, Goal)> = None;
        for goal in candidates {
            let hex = match goal {
                Goal::Hold => me.pos,
                Goal::Take(hex) => *hex,
            };
            let worth = self.evaluator.score_tile(registry, state, unit, hex).score;
            let rounds = me.pos.distance_to(hex) as f32 / speed;
            let blur = if self.noise > 0.0 {
                self.rng.random_range(-self.noise..self.noise)
            } else {
                0.0
            };
            let value = worth - IMPATIENCE * rounds + blur;
            // Strictly greater, so ties go to the earlier candidate and the
            // list's order is the tie-break. That is why `candidates` is
            // ordered rather than gathered.
            if best.is_none_or(|(v, _)| value > v) {
                best = Some((value, *goal));
            }
        }
        best.map(|(_, goal)| goal).unwrap_or(Goal::Hold)
    }
}
