//! Greedy utility planner: score every tile one unit could hold, take the
//! best, and engage whatever that tile can reach. Cheap, tunable, and honest
//! about fog.
//!
//! A unit needs two orders — where to drive and what to shoot — so the
//! planner buffers the pair and hands them over one call at a time, keeping
//! [`AiPlanner::next_order`] the only entry point the callers need.

use super::goal::{self, GoalChooser};
use super::{
    AiConfig, AiPlanner, Evaluator, difficulty_noise, next_unplanned_unit, resolve_doctrine,
};
use crate::battle::{
    BattleState, FireIntent, Order, UnitId, along_the_bearing, reachable, step_toward,
};
use crate::data::DataRegistry;
use hexx::Hex;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::cmp::Reverse;
use std::collections::VecDeque;

/// The best tile found for one unit, and the shot that came with it.
struct Choice {
    dest: Hex,
    attack: Option<(UnitId, usize)>,
}

/// One scored candidate: the tile, its (possibly noisy) score, and the
/// attack the evaluator found from it.
type Candidate = (Hex, f32, Option<(UnitId, usize)>);

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
    /// How much of the goal chooser's deeper reasoning this commander does.
    /// Difficulty's other half — see [`super::difficulty_foresight`].
    pub foresight: f32,
    rng: ChaCha8Rng,
    /// Orders decided for a unit but not yet handed out.
    pending: VecDeque<Order>,
}

impl UtilityPlanner {
    /// A planner for one doctrine at one difficulty.
    ///
    /// Difficulty rather than a noise amplitude, which is what this used to
    /// take: there are two numbers derived from a difficulty level now and a
    /// caller who passed one of them and forgot the other would get a
    /// commander who misjudges the map but reads all of it, which is nobody.
    pub fn new(evaluator: Evaluator, difficulty: u8, seed: u64) -> Self {
        Self {
            evaluator,
            noise: difficulty_noise(difficulty),
            foresight: super::difficulty_foresight(difficulty),
            rng: ChaCha8Rng::seed_from_u64(seed),
            pending: VecDeque::new(),
        }
    }

    pub fn from_config(config: &AiConfig, seed: u64, data: &DataRegistry) -> Self {
        Self::new(
            Evaluator::new(resolve_doctrine(config, data)),
            config.difficulty,
            seed,
        )
    }

    /// A planner with the balanced default doctrine, for callers that only
    /// care about skill: MCTS uses this for its policy opponent.
    pub fn with_difficulty(difficulty: u8, seed: u64) -> Self {
        Self::new(Evaluator::new(Default::default()), difficulty, seed)
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
            let by_taxi = closing + driven + registry.planner.boarding_rounds;
            if by_taxi < on_foot && best.is_none_or(|(t, _)| by_taxi < t) {
                best = Some((by_taxi, carrier.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// The blur this crew is seeing the field through this round: a lean in
    /// one direction, not a haze over every tile separately.
    ///
    /// Difficulty noise used to be drawn per candidate tile and the planner
    /// then took an argmax over every tile she could reach — which is a
    /// selection bias, not a handicap. The maximum of ninety independent
    /// draws from ±0.5 is about +0.49 every single time, while the objective
    /// gradient this was competing with is 0.54 a hex, so the tile that won
    /// was reliably the one that drew luckiest rather than the one nearer the
    /// bridge. Worse, it scaled the wrong way: a faster vehicle reaches more
    /// tiles, takes the maximum over more draws, and wanders harder. Measured
    /// on `battle_plains`, the 7 MP recon car finished further from the
    /// objective than she deployed while the 3 MP howitzer merely twitched.
    ///
    /// A lean is drawn once per unit per round and applied as a smooth
    /// function of where a tile lies relative to her, so no tile can win by
    /// drawing well — there is nothing to draw. What it buys instead is a
    /// coherent misjudgement: today she favours the left, and she favours it
    /// consistently, which is what "the same candidates through a blurrier
    /// lens" was supposed to mean in the first place. A flat per-unit offset
    /// would have been the obvious reading and is a no-op: adding the same
    /// number to every candidate changes no argmax.
    ///
    /// Zero at difficulty 5, exactly as before, so a noiseless side is
    /// untouched by this.
    fn lean(&mut self) -> (f32, f32) {
        if self.noise <= 0.0 {
            return (0.0, 0.0);
        }
        (
            self.rng.random_range(-self.noise..self.noise),
            self.rng.random_range(-self.noise..self.noise),
        )
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

        // Drawn once, before the sweep, so every tile is judged through the
        // same lens. `span` normalises it against how far she can actually
        // get, which keeps a difficulty level worth the same to a howitzer as
        // to a recon car — the old draw was worth much more to whoever could
        // reach more ground.
        let lean = self.lean();
        let span = options
            .iter()
            .map(|h| pos.distance_to(*h))
            .max()
            .unwrap_or(1)
            .max(1) as f32;
        let mut scored: Vec<Candidate> = Vec::with_capacity(options.len());
        for tile in options {
            let tile_score = self.evaluator.score_tile(registry, state, unit, tile);
            let blur =
                (lean.0 * (tile.x - pos.x) as f32 + lean.1 * (tile.y - pos.y) as f32) / span / 2.0;
            scored.push((tile, tile_score.score + blur, tile_score.attack));
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
        //
        // The two keys after distance replaced `(tile.x, tile.y)`, which was
        // a **compass**: smallest x is west, so where the first key tied every
        // crew in the game edged west, which is forwards for a side attacking
        // west and backwards for one attacking east. On the mirrored arena
        // that is worth points to whichever end is on the east — it is the
        // same bug as the one this comment describes, one layer up, and it
        // survived the fix because the fix only changed which tile *wins*, not
        // how a tie between winners breaks. The rule now, here and in
        // `step_toward`: **a tiebreak may only read quantities a reflection
        // preserves.** Distances do. A score does. A dot product of two
        // differences does, because a reflection negates both. A coordinate
        // does not, and no total order on coordinates can.
        //
        // The width of the band is `planner.plateau`, and it is one of the
        // few numbers in that block whose zero is not the gentle setting:
        // at zero there is no plateau, the argmax is bare again, and the
        // pathology above comes back.
        let plateau = registry.planner.plateau;
        let facing = Hex::from(state.unit(unit).map(|u| u.facing).unwrap_or_default());
        let best = scored
            .into_iter()
            .filter(|(_, s, _)| *s >= top - plateau)
            .min_by_key(|(tile, score, _)| {
                (
                    pos.distance_to(*tile),
                    // Among tiles equally near, the better one. The plateau
                    // rule is about not *driving* for a tiny gain; it was
                    // never about declining one that costs nothing.
                    Reverse((score * 1000.0) as i32),
                    // And among those, the one furthest the way she is already
                    // looking — which is at the enemy, because that is where
                    // `face_units_at_enemies` pointed her and where every
                    // shot since has kept her.
                    -along_the_bearing(*tile - pos, facing),
                )
            })
            .map(|(tile, _, attack)| Choice { dest: tile, attack });

        let (best_dest, attack) = match best {
            Some(choice) => (choice.dest, choice.attack),
            None => (pos, None),
        };

        // The goal layer. She keeps the goal she has until it finishes, and
        // chooses a new one from a short list of places that mean something
        // when it does — objectives, where her orders point, and (always) the
        // tile this round's sweep just picked, which is the guarantee that a
        // goal can never leave her worse off than having none.
        //
        // The sweep above still runs, and has to: its answer is a candidate,
        // and its `attack` is what she shoots at while driving. What changed
        // is that its answer is no longer automatically her destination.
        let live = state
            .unit(unit)
            .and_then(|u| u.goal)
            .filter(|g| !g.finished(registry, state, unit));
        let goal = match live {
            Some(goal) => goal,
            None => {
                let options = goal::candidates(
                    registry,
                    state,
                    unit,
                    state
                        .command
                        .formation_of(unit)
                        .and_then(|f| f.mission_for(unit)),
                    Some(best_dest),
                    self.evaluator.doctrine.initiative,
                );
                let mut chooser = goal::UtilityChooser {
                    evaluator: &self.evaluator,
                    noise: self.noise,
                    foresight: self.foresight,
                    rng: &mut self.rng,
                };
                chooser.choose(registry, state, unit, &options)
            }
        };

        // One leg toward it. A `SetMove` naming ground beyond this round's
        // movement is refused by the engine — which is the right rule for an
        // order and the reason this planner only ever *scored* ground it
        // could reach — so a goal several rounds off is walked a leg at a
        // time through the same `step_toward` a commander's personal tasking
        // uses. Two implementations of "closest reachable" would be two
        // answers to where she is going.
        let dest = match goal {
            crate::battle::Goal::Hold => None,
            crate::battle::Goal::Take(hex) => step_toward(registry, state, unit, hex),
        };
        let mut orders = vec![Order::SetGoal { unit, goal }];
        if let Some(dest) = dest {
            orders.push(Order::SetMove { unit, to: dest });
        }
        match attack {
            Some((target, weapon)) => orders.push(Order::SetFire {
                unit,
                fire: FireIntent::Target { target, weapon },
            }),
            // Nothing worth engaging: watch the ground instead. Said out
            // loud so the unit counts as planned rather than forgotten —
            // and `SetGoal` does not count, because stating an intention is
            // not doing anything about it.
            None if dest.is_none() => orders.push(Order::SetFire {
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
