//! Greedy utility planner: score every tile one unit could hold, take the
//! best, and engage whatever that tile can reach. Cheap, tunable, and honest
//! about fog.
//!
//! A unit needs two orders — where to drive and what to shoot — so the
//! planner buffers the pair and hands them over one call at a time, keeping
//! [`AiPlanner::next_order`] the only entry point the callers need.

use super::{
    AiConfig, AiPlanner, Evaluator, difficulty_noise, next_unplanned_unit, resolve_doctrine,
};
use crate::battle::{BattleState, FireIntent, Order, UnitId, reachable};
use crate::data::DataRegistry;
use hexx::Hex;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::VecDeque;

/// The best tile found for one unit, and the shot that came with it.
struct Choice {
    dest: Hex,
    attack: Option<(UnitId, usize)>,
}

/// One scored candidate: the tile, its (possibly noisy) score, and the
/// attack the evaluator found from it.
type Candidate = (Hex, f32, Option<(UnitId, usize)>);

/// Rounds a taxi run costs over and above the driving: walking to the
/// tailgate, climbing in, and stepping off at the far end.
///
/// An AI pricing constant in the same family as the evaluator's `SUPPORT` and
/// the brain's `DEVOLVED` — it exists to stop a platoon mounting up to save
/// herself half a hex, which is the failure mode a pure time comparison has.
///
/// Priced at two rounds first, on the mechanics alone: a mount resolves the
/// tick she reaches the carrier and a dismount the tick after it is ordered.
/// That was too cheap, and the harness said so. A platoon delivered near her
/// objective would re-board for a three-hex hop, ride one hex, meet the
/// at-the-objective dismount reflex, and step off again — costing the
/// commanded side a win and three platoons over 36 battles. Four is the
/// measured price: it removes the short-hop churn entirely while leaving
/// every genuinely long journey (six a run, unchanged at six rounds and
/// beyond) still worth taking. The mechanical cost was never the whole cost;
/// a ride is not door to door, and the walk at each end is real.
const BOARDING_ROUNDS: f32 = 4.0;

/// Where this unit is ultimately trying to be, as far as anyone can say.
///
/// Her formation's standing mission first — the commander has already decided
/// which ground matters and a taxi run toward some other hex would be a unit
/// arguing with her orders — and failing that the nearest piece of ground her
/// side may hold. `None` for a unit with neither, which is a unit with no
/// reason to go anywhere and therefore no reason to ride.
///
/// Deliberately the *anchor* rather than a scored tile: this answers "how far
/// is the journey", which is a question about the destination, and the
/// evaluator's per-tile gradient cannot answer it because a greedy planner
/// only ever sees one round of ground.
fn journey_end(state: &BattleState, unit: UnitId) -> Option<Hex> {
    use crate::battle::Mission;
    let me = state.unit(unit)?;
    let mission = state
        .command
        .formation_of(unit)
        .and_then(|f| f.mission_for(unit));
    if let Some(mission) = mission {
        return match mission {
            Mission::Advance { to } | Mission::Assault { to } => Some(*to),
            Mission::Hold { at } => *at,
            Mission::Recon { toward } => Some(*toward),
            // An ordered withdrawal is a lane, and driving to it is exactly
            // what a taxi is for — but a unit under orders to leave has more
            // urgent business than a rendezvous, and `Withdraw` already pulls
            // hard through the evaluator. Support anchors on other people,
            // who move.
            Mission::Withdraw { .. } | Mission::Support { .. } => None,
        };
    }
    state
        .map
        .objectives()
        .iter()
        .filter(|o| o.kind == crate::map::ObjectiveKind::Hold)
        .filter(|o| o.open_to(me.side))
        .map(|o| o.anchor())
        .min_by_key(|anchor| (anchor.distance_to(me.pos), anchor.x, anchor.y))
}

/// How many rounds it takes this unit to cover `hexes`, by her own speed.
///
/// Movement points are a speed on this scale — one point is about one hex of
/// clear ground per round — so this is the whole of the arithmetic, and it is
/// the reason a taxi run is worth planning at all: a platoon walks at one and
/// her ride drives at six.
fn rounds_to_cover(registry: &DataRegistry, state: &BattleState, unit: UnitId, hexes: i32) -> f32 {
    let points = state
        .unit(unit)
        .and_then(|u| registry.vehicle(&u.vehicle))
        .map(|v| v.movement.points.max(1))
        .unwrap_or(1);
    hexes as f32 / points as f32
}

pub struct UtilityPlanner {
    /// What this side values. Doctrine, not difficulty.
    pub evaluator: Evaluator,
    /// Uniform noise amplitude added to every candidate score. Difficulty,
    /// not doctrine.
    pub noise: f32,
    rng: ChaCha8Rng,
    /// Orders decided for a unit but not yet handed out.
    pending: VecDeque<Order>,
}

impl UtilityPlanner {
    pub fn new(evaluator: Evaluator, noise: f32, seed: u64) -> Self {
        Self {
            evaluator,
            noise,
            rng: ChaCha8Rng::seed_from_u64(seed),
            pending: VecDeque::new(),
        }
    }

    pub fn from_config(config: &AiConfig, seed: u64, data: &DataRegistry) -> Self {
        Self::new(
            Evaluator::new(resolve_doctrine(config, data)),
            difficulty_noise(config.difficulty),
            seed,
        )
    }

    /// A planner with the balanced default doctrine, for callers that only
    /// care about skill: MCTS uses this for its policy opponent.
    pub fn with_difficulty(difficulty: u8, seed: u64) -> Self {
        Self::new(
            Evaluator::new(Default::default()),
            difficulty_noise(difficulty),
            seed,
        )
    }

    /// Whoever is walking toward `carrier` to get aboard her, if anybody.
    ///
    /// Read off `boarding`, which is the engine's own standing-order state,
    /// so this never asks whose idea the mount was. A player's `M` sets the
    /// same field, and a carrier her side's executor is planning for her —
    /// one in a formation under a mission, which is the delegated case — will
    /// go and collect the platoon the player just ordered aboard. A carrier
    /// the player is steering herself is hers to steer, as it should be.
    ///
    /// Lowest id when two are walking toward the same ride. This planner
    /// never creates that (it only offers rides nobody has claimed), but a
    /// keyboard can.
    fn fare_waiting_on(&self, state: &BattleState, carrier: UnitId) -> Option<UnitId> {
        state
            .units
            .iter()
            .filter(|u| u.alive && u.aboard.is_none() && u.boarding == Some(carrier))
            .map(|u| u.id)
            .min_by_key(|id| id.index())
    }

    /// The carrier worth mounting, if riding beats walking.
    ///
    /// Every gate here is a reason a real platoon would refuse the lift:
    ///
    /// - **She has somewhere to be.** No journey, no taxi.
    /// - **Nobody is shooting at either of them.** Mounting under fire is how
    ///   a platoon dies in the back of a hull, and a ride already under threat
    ///   would eject her on the tick she boarded — the dismount reflex and
    ///   this one would spend the battle arguing.
    /// - **The ride is free.** Full carriers and ones somebody else is
    ///   already walking to are not offers, which is what keeps two platoons
    ///   from both marching at a taxi with one seat.
    /// - **It saves time.** The whole decision, in one comparison: rounds
    ///   spent walking the journey, against rounds spent walking to the
    ///   tailgate plus rounds spent being driven, plus what boarding costs.
    ///   Nothing in it knows what an APC is.
    fn ride_worth_taking(
        &self,
        registry: &DataRegistry,
        state: &BattleState,
        unit: UnitId,
    ) -> Option<UnitId> {
        let me = state.unit(unit)?;
        if !registry
            .vehicle(&me.vehicle)
            .is_some_and(|v| v.movement.class == crate::data::MovementClass::Foot)
        {
            return None;
        }
        if super::threatened(registry, state, unit) {
            return None;
        }
        let goal = journey_end(state, unit)?;
        let on_foot = rounds_to_cover(registry, state, unit, me.pos.distance_to(goal));

        let mut best: Option<(f32, UnitId)> = None;
        // `side_units` walks alive units; taking ids in order and comparing
        // strictly keeps the choice independent of iteration order.
        let mut carriers: Vec<&crate::battle::Unit> = state
            .side_units(me.side)
            .filter(|c| c.id != unit && c.aboard.is_none())
            .collect();
        carriers.sort_unstable_by_key(|c| c.id.index());
        for carrier in carriers {
            let capacity = registry
                .vehicle(&carrier.vehicle)
                .map(|v| v.capacity)
                .unwrap_or(0);
            if capacity == 0 || state.passengers(carrier.id).len() as u32 >= capacity {
                continue;
            }
            if self.fare_waiting_on(state, carrier.id).is_some() {
                continue;
            }
            if super::threatened(registry, state, carrier.id) {
                continue;
            }
            // The rendezvous closes from both ends, so the walk to the
            // tailgate is shared: she covers her share and the carrier covers
            // the rest, which is why this divides the gap by the two speeds
            // added together rather than by hers alone.
            let gap = me.pos.distance_to(carrier.pos);
            let closing = rounds_to_cover(registry, state, unit, gap)
                .min(rounds_to_cover(registry, state, carrier.id, gap));
            let driven =
                rounds_to_cover(registry, state, carrier.id, carrier.pos.distance_to(goal));
            let by_taxi = closing + driven + BOARDING_ROUNDS;
            if by_taxi < on_foot && best.is_none_or(|(t, _)| by_taxi < t) {
                best = Some((by_taxi, carrier.id));
            }
        }
        best.map(|(_, id)| id)
    }

    fn noisy_score(&mut self, score: f32) -> f32 {
        if self.noise > 0.0 {
            score + self.rng.random_range(-self.noise..self.noise)
        } else {
            score
        }
    }

    /// Decide everything one unit will do this round.
    ///
    /// Crate-visible because [`super::SideCommand`] routes each unit to a
    /// per-formation instance of this planner: the executor half of chain of
    /// command *is* this planner, aimed by whatever mission the evaluator
    /// finds on the unit's formation.
    pub(crate) fn plan_unit(
        &mut self,
        registry: &DataRegistry,
        state: &BattleState,
        unit: UnitId,
    ) -> Vec<Order> {
        let Some(pos) = state.unit(unit).map(|u| u.pos) else {
            return Vec::new();
        };
        // A passenger plans exactly one thing: whether this is where she gets
        // off — dismount when the ride is under a threat that can actually
        // hurt it (a taxi in an RPG's sights is a coffin), or when the ride
        // has brought her to ground worth holding.
        //
        // A drop-off *short* of the objective was tried here and measurably
        // did not earn its keep: dismounting a round before the carrier came
        // under fire cost two more platoons over 36 battles, because it
        // traded dying in the back of a hull for walking the last stretch in
        // the open. The narrated battles say why no look-ahead could have
        // worked — the taxi is usually killed by something the side has not
        // spotted, so there was nothing to see coming. What is left is a
        // question about where the carrier was sent, not about when the
        // passenger jumped; see TODO under Chain of Command.
        if let Some(carrier) = state.unit(unit).and_then(|u| u.aboard) {
            let ride_threatened = super::threatened(registry, state, carrier);
            let at_the_objective = state.unit(carrier).is_some_and(|c| {
                state
                    .map
                    .objectives()
                    .iter()
                    .filter(|o| o.kind == crate::map::ObjectiveKind::Hold)
                    .filter(|o| o.open_to(c.side))
                    .any(|o| o.hexes.iter().any(|h| c.pos.distance_to(*h) <= 2))
            });
            return if ride_threatened || at_the_objective {
                vec![Order::Dismount { unit }]
            } else {
                vec![Order::SetFire {
                    unit,
                    fire: FireIntent::Hold,
                }]
            };
        }
        // The driver's half of a taxi run. A platoon walks at one hex a round
        // and her ride drives at six, so a passenger marching after a carrier
        // that is doing its own planning never catches it — the pickup has to
        // be somebody's job, and it is the carrier's. While anybody is
        // boarding her she drives to meet them and then stands still until
        // they are up: a rendezvous, closed from both ends, which halves the
        // wait and cannot be got by making the infantry walk faster.
        //
        // Self-preservation still outranks the schedule. A carrier under fire
        // she can feel falls straight through to the ordinary scoring and
        // saves herself, which also keeps the two reflexes from arguing —
        // without it a threatened taxi would sit for her fare while the
        // passenger aboard was being told to dismount.
        if let Some(fare) = self.fare_waiting_on(state, unit)
            && !super::threatened(registry, state, unit)
        {
            let Some(theirs) = state.unit(fare).map(|u| u.pos) else {
                return Vec::new();
            };
            // Adjacent is aboard next tick, so there is nothing left to do
            // but hold the door. Said out loud rather than returning nothing,
            // so she counts as planned and the round can close.
            if pos.distance_to(theirs) <= 1 {
                return vec![Order::SetFire {
                    unit,
                    fire: FireIntent::Hold,
                }];
            }
            let mut approach: Vec<Hex> = reachable(registry, state, unit).into_keys().collect();
            approach.sort_unstable_by_key(|h| (h.x, h.y));
            if let Some(dest) = approach
                .into_iter()
                .min_by_key(|h| (h.distance_to(theirs), h.x, h.y))
                && dest != pos
            {
                return vec![Order::SetMove { unit, to: dest }];
            }
            return vec![Order::SetFire {
                unit,
                fire: FireIntent::Hold,
            }];
        }

        // The fare's half. Ride when riding is faster than walking, by the
        // arithmetic and nothing else: how far the journey is, how fast she
        // walks it, how far the ride is and how fast it drives. Infantry take
        // taxis across a map and walk the last few hexes for exactly the
        // reason people do.
        if let Some(into) = self.ride_worth_taking(registry, state, unit) {
            return vec![Order::Mount { unit, into }];
        }

        let mut options: Vec<Hex> = reachable(registry, state, unit).into_keys().collect();
        options.sort_unstable_by_key(|h| (h.x, h.y));

        let mut scored: Vec<Candidate> = Vec::with_capacity(options.len());
        for tile in options {
            let tile_score = self.evaluator.score_tile(registry, state, unit, tile);
            scored.push((tile, self.noisy_score(tile_score.score), tile_score.attack));
        }
        let top = scored
            .iter()
            .map(|(_, s, _)| *s)
            .fold(f32::NEG_INFINITY, f32::max);

        // Among tiles the evaluator cannot meaningfully tell apart, take the
        // one closest to where she already stands. This is the fix for the
        // skillgap instrument's inversion finding, and it is deterministic
        // rather than another kind of noise. Open ground scores in broad
        // plateaus, and the old rule — strictly-better-or-keep-the-first,
        // over a fixed (x, y) sweep — sent every unit on a side to the SAME
        // corner of every plateau: identical crews made identical choices
        // and arrived as a queue, which is why a noiseless side clumped,
        // burned its movement crossing its own plateau, and lost to any
        // opponent scattered by randomness. Preferring the nearest
        // equivalent tile keeps a dispersed side dispersed (units standing
        // apart stay apart when the ground between is all the same),
        // conserves movement for ground that is actually better, and is
        // what a crew would do: nobody drives across a field to park on
        // identical grass.
        const PLATEAU: f32 = 0.3;
        let best = scored
            .into_iter()
            .filter(|(_, s, _)| *s >= top - PLATEAU)
            .min_by_key(|(tile, _, _)| (pos.distance_to(*tile), tile.x, tile.y))
            .map(|(tile, _, attack)| Choice { dest: tile, attack });

        let (dest, attack) = match best {
            Some(choice) => (choice.dest, choice.attack),
            None => (pos, None),
        };
        let mut orders = Vec::new();
        if dest != pos {
            orders.push(Order::SetMove { unit, to: dest });
        }
        match attack {
            Some((target, weapon)) => orders.push(Order::SetFire {
                unit,
                fire: FireIntent::Target { target, weapon },
            }),
            // Nothing worth engaging: watch the ground instead. Said out
            // loud so the unit counts as planned rather than forgotten.
            None if orders.is_empty() => orders.push(Order::SetFire {
                unit,
                fire: FireIntent::Hold,
            }),
            None => {}
        }
        orders
    }
}

impl AiPlanner<BattleState, Order> for UtilityPlanner {
    fn next_order(&mut self, registry: &DataRegistry, state: &BattleState, side: u8) -> Order {
        while let Some(order) = self.pending.pop_front() {
            // A unit destroyed since the order was queued has nothing to say.
            let unit = match &order {
                Order::SetMove { unit, .. } | Order::SetFire { unit, .. } => Some(*unit),
                _ => None,
            };
            if unit.is_none_or(|u| state.unit(u).is_some()) {
                return order;
            }
        }
        let Some(unit) = next_unplanned_unit(state, side) else {
            return Order::Commit { side };
        };
        self.pending = self.plan_unit(registry, state, unit).into();
        match self.pending.pop_front() {
            Some(order) => order,
            // Nothing to say about a unit that cannot be planned; hold fire
            // so it is marked planned and the round can proceed.
            None => Order::SetFire {
                unit,
                fire: FireIntent::Hold,
            },
        }
    }
}
