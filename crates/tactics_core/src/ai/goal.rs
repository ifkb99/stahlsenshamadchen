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

use crate::battle::{BattleState, Goal, Mission, Roads, Unit, UnitId, roads};
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

/// How many rounds of driving the chooser bothers to price a road for.
///
/// At `IMPATIENCE` a sixth round of driving costs two points, which is most
/// of a good objective, so ground further off than this is ground she is not
/// going to pick however cheap the road turns out to be. Stopping the walk
/// there is most of what makes pricing every road affordable: the horizon is
/// a few hundred tiles of a 1261-tile map rather than all of it. Anything
/// beyond it falls back to the crow flight, which is what the chooser used
/// for everything before it could see a road at all.
const HORIZON: u32 = 6;

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

/// The road to one goal, as facts rather than as a price.
///
/// Deliberately un-weighted: what a commander *makes* of these is a question
/// about her, and [`UtilityChooser`] answers it with her doctrine and her
/// foresight. Keeping the two apart is what lets a poor commander read the
/// same map and get less out of it.
#[derive(Debug, Clone, Copy)]
struct Drive {
    /// Rounds of driving if the map were flat and empty — the crow flight,
    /// which is the whole of what the chooser knew before this existed.
    crow: f32,
    /// Rounds of driving over the real ground. Equal to `crow` where there is
    /// no road at all inside the horizon.
    road: f32,
    /// What share of that road is walked where something that can shoot her
    /// can see her, 0..=1.
    exposed: f32,
    /// Rounds the nearest enemy who can already be seen would need to reach
    /// the same ground. Infinite when nobody can be seen.
    theirs: f32,
}

/// What the drive to `to` looks like from this crew's hex, given roads
/// already priced.
///
/// `roads` ignores who is standing where (see [`crate::battle::roads`]): a
/// march takes several rounds and the field will not hold still for it, so
/// the opposition is priced as *danger along the way* rather than as a wall.
fn drive(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    roads: &Roads,
    guns: &[(Hex, i32)],
    to: Hex,
    speed: f32,
) -> Drive {
    let Some(me) = state.unit(unit) else {
        return Drive {
            crow: 0.0,
            road: 0.0,
            exposed: 0.0,
            theirs: f32::INFINITY,
        };
    };
    let crow = me.pos.distance_to(to) as f32 / speed;
    // Terrain cost is in movement points and `speed` is points a round, so
    // this is rounds. Ground with no road inside the horizon is priced at the
    // horizon or the crow flight, whichever is further — "further than I have
    // looked", which is the honest reading and the pessimistic one.
    //
    // The first draft fell back to the crow flight alone and had it exactly
    // backwards: the ground with no road inside the horizon is the ground
    // whose road is *longest*, so pricing it as the crow flies made the far
    // bank of an unfordable river the nearest thing on the map. It is still a
    // discount rather than a refusal — a crew who can see no way round may
    // very well be wrong about that, and something may open up before she
    // gets there.
    let road = match roads.cost(to) {
        Some(cost) => cost as f32 / speed,
        None => crow.max(HORIZON as f32),
    };

    let exposed = match (guns.is_empty(), roads.path(to)) {
        (false, Some(path)) => {
            let seen = path
                .iter()
                .filter(|hex| {
                    guns.iter().any(|(from, reach)| {
                        from.distance_to(**hex) <= *reach && state.sight.clear(*from, **hex)
                    })
                })
                .count();
            seen as f32 / path.len().max(1) as f32
        }
        _ => 0.0,
    };

    // Crow flight for the enemy on purpose: what her terrain costs are is not
    // something this crew knows, and guessing precisely would be a worse lie
    // than guessing roughly.
    let theirs = super::visible_enemies(state, me.side)
        .into_iter()
        .map(|enemy| {
            let speed = registry
                .vehicle(&enemy.vehicle)
                .map(|v| v.movement.points.max(1))
                .unwrap_or(1) as f32;
            enemy.pos.distance_to(to) as f32 / speed
        })
        .fold(f32::INFINITY, f32::min);

    Drive {
        crow,
        road,
        exposed,
        theirs,
    }
}

/// The farthest this crew's guns reach, in hexes.
///
/// A gate on the exposure count rather than a shot price: what is being asked
/// is "could she be shot at from there", and a weapon that cannot reach the
/// hex answers no whatever it would do if it could. Pricing the shot properly
/// is [`super::best_weapon_against`]'s job and it is far too expensive to run
/// for every hex of every candidate road.
fn longest_shot(registry: &DataRegistry, unit: &Unit) -> i32 {
    registry
        .vehicle(&unit.vehicle)
        .map(|v| {
            v.weapons
                .iter()
                .filter_map(|w| registry.weapon(w))
                .map(|w| w.range[1] as i32)
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// The hand-written chooser: doctrine-weighted value of being there, less
/// what the drive costs — the road, the fire along it, and whether it is
/// hers to take when she arrives.
///
/// Note what it still does *not* do. It has no model of what the enemy does
/// next beyond where she stands now, no notion of what her own side is
/// doing elsewhere, and no memory of what happened last time. Those are what
/// a search or a learned policy would add; leaving them out is what keeps
/// this cheap enough to run for every crew every time a goal finishes.
pub struct UtilityChooser<'a> {
    pub evaluator: &'a super::Evaluator,
    /// How much of the deeper reasoning this commander actually does, 0..=1.
    ///
    /// Difficulty's second half, and the half that makes deepening this
    /// chooser worth anything. The blur below models a commander who reads
    /// the map and misjudges it; foresight models one who does not read all
    /// of it — she sees the objective and the distance to it as the crow
    /// flies, and not the river in between, nor the gun covering the open
    /// ground, nor that the enemy is nearer to the bridge than she is.
    ///
    /// It has to be a separate axis from the blur, because more terms make
    /// the blur matter *less*: a value function that separates a good goal
    /// from a bad one more sharply is one a blurred commander still ranks
    /// correctly. Deepening the chooser and leaving difficulty as noise alone
    /// would have made difficulty mean less rather than more, which is the
    /// opposite of what this is for.
    ///
    /// Zero is exactly the chooser before any of it: crow flight, no road, no
    /// fire on the way, nobody racing her for the ground. One is all of it.
    pub foresight: f32,
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
        // Priced once for the whole list rather than once per candidate.
        // Every goal is a place to drive from the same hex, so one Dijkstra
        // answers all of them — and the A* per candidate this replaced cost
        // five to ten times as much per planned order.
        let roads = roads(registry, state, unit, HORIZON);
        // Who could shoot at the march, and from how far. Fog-honest: a crew
        // cannot route around a gun nobody has seen, and letting her would
        // leak the enemy's whole order of battle into her pathfinding.
        let guns: Vec<(Hex, i32)> = super::visible_enemies(state, me.side)
            .into_iter()
            .map(|enemy| (enemy.pos, longest_shot(registry, enemy)))
            .collect();
        let mut best: Option<(f32, Goal)> = None;
        for goal in candidates {
            let hex = match goal {
                Goal::Hold => me.pos,
                Goal::Take(hex) => *hex,
            };
            let worth = self.evaluator.score_tile(registry, state, unit, hex).score;
            let drive = drive(registry, state, unit, &roads, &guns, hex, speed);
            let doctrine = &self.evaluator.doctrine;
            let sight = self.foresight.clamp(0.0, 1.0);
            let blur = if self.noise > 0.0 {
                self.rng.random_range(-self.noise..self.noise)
            } else {
                0.0
            };
            // How long the drive looks to *her*. A commander with no
            // foresight prices it as the crow flies — "it is only over
            // there" — and finds the river when she gets to it; one with all
            // of it reads the ground. Blended rather than switched, so a
            // difficulty level in between is a commander who half-reads the
            // map rather than one who flips a coin about it.
            let rounds = drive.crow * (1.0 - sight) + drive.road * sight;
            // The two things she may not notice at all. Both are in rounds
            // and both are priced per round, so they read against each other
            // and against `IMPATIENCE` with no conversion to remember: the
            // part of the march spent under a gun, and the wait for ground
            // somebody else gets to first. Doctrine says how much she minds;
            // foresight says whether she sees it; either at zero is the
            // chooser before it could see a road at all.
            let exposed = rounds * drive.exposed * sight;
            let late = (rounds - drive.theirs).max(0.0) * sight;
            let value = worth
                - IMPATIENCE * rounds
                - doctrine.route_caution * exposed
                - doctrine.contest_aversion * late
                + blur;
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
