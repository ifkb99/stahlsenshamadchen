//! Intents in, events out: the planning and resolution boundary.
//!
//! During planning, [`BattleState::apply`] records what units are told to do
//! without moving anything. When every side has committed, resolution runs in
//! ticks via [`BattleState::step_tick`], and everyone's orders play out
//! together.

use super::STALEMATE_ROUNDS;
use super::{
    BattleResult, BattleState, EndReason, FormationId, Mission, Phase, UnitId, combat, fog,
    movement,
};
use crate::data::{ArmorFacing, DataRegistry};
use crate::map::{LossTrigger, ObjectiveKind};
use hexx::Hex;
use serde::{Deserialize, Serialize};

/// What a unit will do with its guns this round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FireIntent {
    /// Shoot at whatever presents itself. This covers overwatch and what used
    /// to be a special-cased counterattack: a unit holding fire answers
    /// anyone who shows up in its sights.
    #[default]
    Hold,
    /// Engage a specific enemy as soon as the shot exists. The shot is not
    /// range-checked when ordered, because the unit may be driving into
    /// position as part of the same round.
    Target { target: UnitId, weapon: usize },
    /// Shell a tile without a confirmed target, at a heavy accuracy penalty.
    Area { at: Hex, weapon: usize },
}

/// Everything a unit was told to do this round.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitIntent {
    /// Hexes still to be walked, excluding the tile the unit stands on.
    pub path: Vec<Hex>,
    pub fire: FireIntent,
}

impl UnitIntent {
    pub fn is_empty(&self) -> bool {
        self.path.is_empty() && self.fire == FireIntent::Hold
    }
}

/// Everything a side can ask the simulation to do while planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Order {
    /// Route a unit to `to` along the cheapest path it can afford this round.
    SetMove { unit: UnitId, to: Hex },
    /// Tell a unit what to shoot at.
    SetFire { unit: UnitId, fire: FireIntent },
    /// The commander's own order to one crew, carried by the wire.
    ///
    /// Identical to [`Self::SetMove`] plus [`Self::SetFire`] for a girl who
    /// can hear it, and *held at the radio* for one who cannot: it waits in
    /// her formation's queue and is delivered at the first planning phase she
    /// is back in contact for.
    ///
    /// The distinction between this and the two orders above is the whole
    /// point of it, and it is a distinction about **who is speaking** rather
    /// than about what is said. A planner issuing `SetMove` is a crew's own
    /// judgment about her own tank — she does not need to be radioed her own
    /// decision, and gating it on contact would make a cut-off girl freeze
    /// instead of soldiering on. This is the *commander* talking, and a
    /// commander who cannot be heard has not given an order yet. The engine
    /// cannot tell one caller from another, so the caller says which it is by
    /// which order it sends.
    ///
    /// Either half may be `None`: an order about the route says nothing about
    /// the target, and vice versa.
    Radio {
        unit: UnitId,
        to: Option<Hex>,
        fire: Option<FireIntent>,
    },
    /// Forget a unit's orders; it reverts to unplanned and holds fire.
    ClearIntent { unit: UnitId },
    /// Give a formation its standing mission, replacing any it already had.
    ///
    /// Unlike the orders above this one outlives the round: a unit's intent is
    /// cleared when the round opens, a formation's mission is not. It travels
    /// through the same entry point as everything else precisely so that the
    /// human, the built-in brain and any future external one command in one
    /// vocabulary that saves and replays already know how to carry.
    SetMission {
        formation: FormationId,
        mission: Mission,
    },
    /// "…and then this": add a mission to the end of a formation's plan
    /// instead of replacing it. Refused behind a terminal mission — nothing
    /// follows a stand-fast or a retreat — and travels the wire exactly as a
    /// replacement does; the radio does not care what the envelope says.
    QueueMission {
        formation: FormationId,
        mission: Mission,
    },
    /// This side is done planning. When every side with units has committed,
    /// the round starts resolving.
    Commit { side: u8 },
}

/// Everything that can happen as a result of orders. The presentation layer
/// animates these; AI and campaign scripts observe them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    /// A new round opened and sides may plan again.
    RoundStarted {
        round: u32,
    },
    /// One slice of simultaneous resolution is about to play out. Events
    /// between two of these happened at the same moment.
    TickStarted {
        tick: u32,
    },
    /// Hexes this unit crossed during the current tick.
    UnitMoved {
        unit: UnitId,
        path: Vec<Hex>,
    },
    /// The unit ran into an unspotted enemy and stopped short; the rest of
    /// its route is abandoned.
    UnitTrapped {
        unit: UnitId,
        at: Hex,
    },
    /// An idle crew under a threat she has had time to take in broke for the
    /// best cover she can reach — the mid-round half of the battle drill.
    /// Announced because a vehicle moving with no order behind it reads as a
    /// bug or a betrayal; this line is the difference between "she bolted"
    /// and "she is doing exactly what a trained crew does".
    TookCover {
        unit: UnitId,
        at: Hex,
    },
    ShotFired {
        attacker: UnitId,
        from: Hex,
        at: Hex,
        weapon: String,
        blind: bool,
        /// Fired on the unit's own initiative rather than at an ordered
        /// target: overwatch, or an answer to being shot at.
        opportunity: bool,
    },
    ShotHit {
        attacker: UnitId,
        target: UnitId,
        damage: i32,
        facing: ArmorFacing,
        remaining_hp: i32,
    },
    /// The round struck and the armor held. Nothing structural happened —
    /// there is no damage floor any more — but the event is said out loud
    /// because a shot that silently does nothing is indistinguishable from
    /// a bug, and because the crew inside heard it: `rattled` is true for
    /// anything heavier than small arms, and the morale ladder charges for
    /// it. Bullets pattering on plate frighten nobody buttoned up behind
    /// it, which is deliberate — pressure from plinking would rebuild the
    /// machine-gun-grinds-a-heavy-tank defect one layer up.
    ShotBounced {
        attacker: UnitId,
        target: UnitId,
        facing: ArmorFacing,
        rattled: bool,
    },
    /// This gun has just spent the last round it can fire. Announced once,
    /// at the moment of the spend, so the silence that follows reads as a
    /// fact the player was told rather than an order the game ate. A weapon
    /// whose mod declares no ammunition never fires this: uncounted rounds
    /// are infinite ones.
    WeaponDry {
        unit: UnitId,
        weapon: String,
    },
    ShotMissed {
        attacker: UnitId,
        at: Hex,
    },
    UnitDestroyed {
        unit: UnitId,
        at: Hex,
    },
    UnitSpotted {
        unit: UnitId,
        by_side: u8,
        at: Hex,
    },
    /// A crew moved up or down the morale ladder. Emitted so the player can
    /// see a unit wavering *before* it costs them something, which is the
    /// whole bargain that makes disobedience fair.
    MoraleChanged {
        unit: UnitId,
        rung: String,
        /// Whether they will still do as they are told on this rung.
        obeys: bool,
    },
    /// A crew did not do what it was told, and why. Never silent: an order
    /// that quietly fails is indistinguishable from a bug.
    OrderRefused {
        unit: UnitId,
        rung: String,
    },
    /// A formation was told what to do. Carries the formation's *string* id
    /// rather than its handle, because this exists to be read: a mission the
    /// player or the AI set is news, and a log that cannot say "the
    /// Reconnaissance Section was sent to the upper ford" leaves the player
    /// guessing why four vehicles suddenly drove north.
    ///
    /// This is the moment the order was *sent*. With no command rules in
    /// force it is also the moment it landed; with them, the formation is
    /// still ignorant of it until [`Self::MissionReceived`] says otherwise.
    MissionAssigned {
        formation: String,
        mission: Mission,
    },
    /// A mission finished travelling and is now the formation's standing
    /// order. Only ever emitted when a mod prices latency — with no `command`
    /// block a mission arrives the instant it is given, and saying so twice
    /// would be noise.
    ///
    /// The gap between this and [`Self::MissionAssigned`] is the whole point
    /// of the system and has to be *watchable*: a player who sees her platoon
    /// keep driving north for two ticks after she redirected it should be
    /// reading the reason in the log, not inventing one.
    MissionReceived {
        formation: String,
        mission: Mission,
    },
    /// A formation finished the leg it was on and takes up the next one in
    /// its plan. Announced because a formation changing direction with no
    /// visible order behind it reads as disobedience — the promotion IS the
    /// order, given when the plan was, and the log owes the player that
    /// connection.
    MissionCompleted {
        formation: String,
        mission: Mission,
    },
    /// This unit can no longer hear her chain of command: she is outside her
    /// leader's radius (and outside any relay). She soldiers on the standing
    /// orders she was carrying when the wire went dead — mission *changes*
    /// are what cannot reach her until contact is restored.
    ///
    /// Said out loud on the tick it happens, because a unit that quietly
    /// ignores what it was told is indistinguishable from a bug. That is the
    /// same bargain the morale ladder makes: latitude to deviate is only fair
    /// when the player can see it coming and name the cause.
    OutOfContact {
        unit: UnitId,
    },
    /// A radioed order could not reach this girl and is waiting at the radio
    /// until it can. She is still driving on her last orders in the meantime.
    ///
    /// Said out loud for exactly the reason the refusal it replaces was: an
    /// order silently parked is as illegible as one silently dropped. The
    /// player has to be able to tell "she has not been told yet" from "the
    /// game ate my click", and the only difference between them is this line.
    OrdersWaiting {
        unit: UnitId,
    },
    /// A radioed order finally reached her, at the top of a round, and is now
    /// her intent for it — re-pathed from where she actually is, which may be
    /// nowhere near where she was when it was given.
    OrdersDelivered {
        unit: UnitId,
    },
    /// The wires are back: this unit is inside the command net again and will
    /// hear what she is told. Emitted only on the change, so a platoon
    /// motoring along beside its leader says nothing for the whole battle.
    ContactRestored {
        unit: UnitId,
    },
    /// Somebody in contact laid eyes on an enemy and the report reached the
    /// commander: `unit` is the enemy, `by` is the girl who filed it, `at` is
    /// where she says it was. This is the upward half of command friction —
    /// what the *side's* fog sees and what its commander has been *told* are
    /// different pictures, and this event is the only bridge between them. A
    /// spot by a cut-off unit produces no report at all, which is recon
    /// wasted, which is what the wires are for.
    ContactReported {
        unit: UnitId,
        by: UnitId,
        at: Hex,
    },
    /// A formation's leader is off the field and the next girl in its order of
    /// battle has taken over: `from` is who was lost, `to` is who now
    /// commands.
    ///
    /// Named, on both ends, because the whole system is built on events
    /// carrying their own story — a recap screen or a bark should be able to
    /// say "Kesselring is gone; Weber has the platoon" from this line alone,
    /// without reconstructing it afterwards from a corpse and a leader field.
    /// It is also the *cause* of the pressure the formation is about to feel,
    /// which is what makes that pressure fair: the ladder only ever moves for
    /// something the player watched happen.
    CommandPassed {
        formation: String,
        from: UnitId,
        to: UnitId,
    },
    /// A vehicle drove off the map by an exit objective. It is out of the
    /// battle and its crew are going home; this is not [`Self::UnitDestroyed`]
    /// and must never be presented as one.
    UnitExited {
        unit: UnitId,
        objective: String,
        at: Hex,
    },
    /// Ground changed hands. Only emitted on a change, so a side sitting on
    /// the bridge for twenty rounds says this once.
    ObjectiveTaken {
        objective: String,
        /// The side that now holds it, or `None` if it was contested back to
        /// nobody's.
        side: Option<u8>,
        at: Hex,
    },
    BattleEnded {
        winner: Option<u8>,
        reason: EndReason,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OrderError {
    #[error("the battle is over")]
    BattleOver,
    #[error("unit does not exist or is destroyed")]
    NoSuchUnit,
    #[error("orders are closed while the round resolves")]
    NotPlanningPhase,
    #[error("this side has already committed its orders")]
    AlreadyCommitted,
    #[error("no such side in this battle")]
    NoSuchSide,
    #[error("no valid path to the destination")]
    NoPath,
    #[error("no such weapon on this vehicle")]
    NoSuchWeapon,
    #[error("target is not spotted")]
    TargetNotSpotted,
    #[error("cannot target a friendly unit")]
    FriendlyTarget,
    #[error("tile is not on the map")]
    NotOnMap,
    #[error("no such formation in this battle")]
    NoSuchFormation,
    #[error("no exit by that name this side may use")]
    NoSuchExit,
    #[error("nothing follows a stand-fast or a retreat")]
    MissionIsTerminal,
    #[error("a formation cannot stand base of fire for that")]
    CannotSupportThat,
    #[error("the racks are only open during deployment")]
    LoadoutClosed,
    #[error("no such ammunition")]
    NoSuchAmmo,
    #[error("no weapon on this vehicle chambers that ammunition")]
    UnchamberedAmmo,
    #[error("the vehicle has no room for that many rounds")]
    StowageFull,
}

impl BattleState {
    /// Record one planning order. Nothing on the board moves here: orders
    /// only take effect once the round resolves.
    pub fn apply(
        &mut self,
        registry: &DataRegistry,
        order: &Order,
    ) -> Result<Vec<Event>, OrderError> {
        if self.is_over() {
            return Err(OrderError::BattleOver);
        }
        if !self.is_planning() {
            return Err(OrderError::NotPlanningPhase);
        }
        match order {
            Order::SetMove { unit, to } => {
                self.set_move(registry, *unit, *to)?;
                Ok(Vec::new())
            }
            Order::SetFire { unit, fire } => {
                self.set_fire(registry, *unit, *fire)?;
                Ok(Vec::new())
            }
            Order::Radio { unit, to, fire } => self.radio(registry, *unit, *to, *fire),
            Order::ClearIntent { unit } => {
                self.planning_unit_side(*unit)?;
                // Not sending is free, so taking back what has not gone out
                // needs no contact at all: the message never leaves the radio.
                self.command.drop_orders(*unit);
                // What it cannot do is reach *her*. A girl off the net is
                // following her last orders and there is nobody to tell her to
                // stop — clearing her intent here would be the commander
                // countermanding an order down a wire she has just been told
                // is dead.
                if !self.hears_orders(*unit) {
                    return Ok(Vec::new());
                }
                let u = self.unit_mut(*unit).ok_or(OrderError::NoSuchUnit)?;
                u.intent = UnitIntent::default();
                u.planned = false;
                // The recall: she rejoins her formation's tasking.
                u.detached = false;
                u.tasking = None;
                Ok(Vec::new())
            }
            Order::SetMission { formation, mission } => {
                self.set_mission(registry, *formation, mission.clone())
            }
            Order::QueueMission { formation, mission } => {
                self.push_mission(registry, *formation, mission.clone())
            }
            Order::Commit { side } => self.commit(*side),
        }
    }

    /// Whether a girl can currently hear her chain of command.
    ///
    /// True for a unit in no formation, for a unit who is not there at all,
    /// and — because `out_of_contact` is only ever filled where a mod declares
    /// command rules — for absolutely everybody in a game that never asked for
    /// a signals net. That is what makes the queue below additive: with no
    /// rules nothing is ever held, so no order behaves differently and the
    /// determinism baseline cannot move.
    pub fn hears_orders(&self, unit: UnitId) -> bool {
        self.formation_of(unit).is_none_or(|f| f.in_contact(unit))
    }

    /// The commander's direct order to one crew: applied now if she can hear
    /// it, held at the radio if she cannot.
    ///
    /// Validation runs **before** anything is queued, so the queue can never
    /// hold an order that was never legal — a shot at a friend or at somebody
    /// nobody has spotted is refused to the commander's face whether or not
    /// the wire is up. The one thing deliberately *not* checked for a held
    /// order is whether the destination can be pathed to: she will re-path
    /// from wherever she is when it arrives, and refusing today on the traffic
    /// of today would be answering a question nobody asked. That is exactly
    /// how a [`Mission`] behaves, and for the same reason.
    /// One round's march toward a destination that may be many rounds away:
    /// the closest reachable hex to it this round, or nothing if she can get
    /// no closer. Deterministic tiebreak, because two equally good hexes must
    /// pick the same one in every replay.
    fn march_toward(&mut self, registry: &DataRegistry, id: UnitId, destination: Hex) {
        let Some(pos) = self.unit(id).map(|u| u.pos) else {
            return;
        };
        let step = movement::reachable(registry, self, id)
            .into_keys()
            .min_by_key(|h| (destination.distance_to(*h), h.x, h.y));
        if let Some(step) = step
            && step != pos
        {
            let _ = self.set_move(registry, id, step);
        }
    }

    fn radio(
        &mut self,
        registry: &DataRegistry,
        id: UnitId,
        to: Option<Hex>,
        fire: Option<FireIntent>,
    ) -> Result<Vec<Event>, OrderError> {
        self.planning_unit_side(id)?;
        if let Some(fire) = fire {
            self.check_fire(registry, id, fire)?;
        }
        if self.hears_orders(id) {
            // The movement half is a standing personal destination, not a
            // one-round route: a commander's order does not expire for
            // naming ground beyond this round's driving. She marches toward
            // it now and keeps marching each round until she arrives —
            // which also means a far destination is no longer a refusal.
            if let Some(to) = to {
                if !self.map.contains(to) {
                    return Err(OrderError::NotOnMap);
                }
                if let Some(unit) = self.unit_mut(id) {
                    unit.tasking = Some(to);
                }
                self.march_toward(registry, id, to);
            }
            if let Some(fire) = fire {
                self.set_fire(registry, id, fire)?;
            }
            // A direct order detaches her from her formation's standing
            // mission: the commander has taken personal charge of this
            // vehicle, and the mission must not quietly reassert itself at
            // the next planning phase and march her off the ground she was
            // put on.
            if let Some(unit) = self.unit_mut(id) {
                unit.detached = true;
            }
            return Ok(Vec::new());
        }
        self.command.hold_orders(id, to, fire);
        Ok(vec![Event::OrdersWaiting { unit: id }])
    }

    /// Hand out every radioed order whose girl is back on the net.
    ///
    /// Run at the top of a round, once intents have been cleared, because
    /// **delivery is a planning-phase event**: WEGO's bargain is that
    /// resolution plays out what was planned, and an order landing mid-tick
    /// would be new information acted on inside a round that had already been
    /// committed. Walked in unit-id order, so what it emits cannot depend on a
    /// hash.
    ///
    /// The destination is re-pathed here rather than replayed: she may be
    /// half a map from where she stood when it was given. A route that no
    /// longer exists drops the movement half silently in the code but not in
    /// the log — the delivery is still announced, because what the player
    /// needs to know is that the order got there.
    fn deliver_waiting_orders(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        if self.command.waiting().is_empty() {
            return;
        }
        let mut undelivered = Vec::new();
        for (unit, orders) in self.command.take_waiting() {
            // Dead or driven off the map: dropped without a word. An order to
            // a girl who is not coming back is not news, it is an epitaph.
            if self.unit(unit).is_none() {
                continue;
            }
            if !self.hears_orders(unit) {
                undelivered.push((unit, orders));
                continue;
            }
            if let Some(to) = orders.destination {
                // The promise of the queue, kept: however far she has come
                // in the meantime, the delivered order is a destination she
                // now marches for — this round as far as the round allows,
                // and every round after until she arrives.
                if let Some(u) = self.unit_mut(unit) {
                    u.tasking = Some(to);
                }
                self.march_toward(registry, unit, to);
            }
            // The target may have burned while the order was in the drawer.
            if let Some(fire) = orders.fire
                && self.check_fire(registry, unit, fire).is_ok()
            {
                let _ = self.set_fire(registry, unit, fire);
            }
            // Delivery is when the personal tasking takes hold — until the
            // order reached her she was soldiering the standing mission,
            // which is exactly what the queue promised.
            if let Some(u) = self.unit_mut(unit) {
                u.detached = true;
            }
            events.push(Event::OrdersDelivered { unit });
        }
        self.command.keep_waiting(undelivered);
    }

    /// Record a formation's standing mission, refusing anything a formation
    /// could not actually be asked to do.
    ///
    /// Validated in the order a person would: does the formation exist, may
    /// its side still speak this round, and is what it was told to do a thing
    /// on this map. Only what cannot change as the battle plays out is
    /// checked — the same bargain [`Self::set_fire`] makes. Whether the ground
    /// is *reachable*, whether the enemy is on it, whether the withdrawal is
    /// wise: all of that is the executor's problem, and refusing on it here
    /// would make a mission a route rather than an intention.
    fn set_mission(
        &mut self,
        registry: &DataRegistry,
        formation: FormationId,
        mission: Mission,
    ) -> Result<Vec<Event>, OrderError> {
        let f = self
            .command
            .get(formation)
            .ok_or(OrderError::NoSuchFormation)?;
        let (side, id) = (f.side, f.id.clone());
        // Mirrors `planning_unit_side`: a side that has handed in its orders
        // does not get to keep issuing them, and a mission is an order.
        if self.has_committed(side) {
            return Err(OrderError::AlreadyCommitted);
        }
        self.check_mission_target(side, &id, &mission)?;
        // An order is sent here; whether it has *arrived* is the signals net's
        // business. A delay of zero — which is every battle whose mod declares
        // no `command` block — puts it straight onto the formation, so the old
        // game is this same line rather than a branch around it.
        let delay = self.mission_delay(registry, formation);
        if delay == 0 {
            self.command.set_mission(formation, mission.clone());
        } else {
            self.command.set_incoming(
                formation,
                crate::battle::MissionChange::Replace(mission.clone()),
                delay,
            );
        }
        // A fresh formation order collects everyone: whatever personal
        // tasking a member was under, the commander has now spoken to the
        // whole formation, and "detached" was never meant to survive being
        // given new orders — only to stop the OLD mission from quietly
        // reasserting itself.
        let members = self
            .command
            .get(formation)
            .map(|f| f.members.clone())
            .unwrap_or_default();
        for member in members {
            if let Some(unit) = self.unit_mut(member) {
                unit.detached = false;
                unit.tasking = None;
            }
        }
        Ok(vec![Event::MissionAssigned {
            formation: id,
            mission,
        }])
    }

    /// Add a mission to the end of a formation's plan — the queueing half of
    /// [`Self::set_mission`], sharing its validation and its wire.
    fn push_mission(
        &mut self,
        registry: &DataRegistry,
        formation: FormationId,
        mission: Mission,
    ) -> Result<Vec<Event>, OrderError> {
        let f = self
            .command
            .get(formation)
            .ok_or(OrderError::NoSuchFormation)?;
        let (side, id) = (f.side, f.id.clone());
        if self.has_committed(side) {
            return Err(OrderError::AlreadyCommitted);
        }
        // "What would this follow" is a question about the pipeline's tail:
        // the order in the air, else the back of the plan, else the standing
        // mission. Queueing behind a leg that can never end is an order that
        // can never begin, and it is refused here rather than left to sit.
        if f.latest_mission().is_some_and(|m| m.terminal()) {
            return Err(OrderError::MissionIsTerminal);
        }
        self.check_mission_target(side, &id, &mission)?;
        let delay = self.mission_delay(registry, formation);
        if delay == 0 {
            self.command.queue_mission(formation, mission.clone());
        } else {
            self.command.set_incoming(
                formation,
                crate::battle::MissionChange::Append(mission.clone()),
                delay,
            );
        }
        Ok(vec![Event::MissionAssigned {
            formation: id,
            mission,
        }])
    }

    /// Whether a mission's target is somewhere this side could be sent —
    /// or, for a base of fire, somebody it could be sent to shoot for.
    ///
    /// `ordering` is the id of the formation being given the mission, needed
    /// only by [`Mission::Support`]: a formation standing base of fire for
    /// itself is a sentence with no meaning, and the only place that can be
    /// noticed is here, where both ends of the order are in hand.
    fn check_mission_target(
        &self,
        side: u8,
        ordering: &str,
        mission: &Mission,
    ) -> Result<(), OrderError> {
        match mission {
            Mission::Advance { to } | Mission::Assault { to } | Mission::Recon { toward: to } => {
                if !self.map.contains(*to) {
                    return Err(OrderError::NotOnMap);
                }
            }
            // `None` is "stand where you are", which needs no tile to exist.
            Mission::Hold { at } => {
                if let Some(at) = at
                    && !self.map.contains(*at)
                {
                    return Err(OrderError::NotOnMap);
                }
            }
            Mission::Withdraw { via } => {
                // A withdrawal has to name a lane that is really there and
                // really this side's, or the formation would drive to the
                // map's edge and sit in the open waiting for a way out that
                // was never written down.
                let usable = self
                    .map
                    .objectives()
                    .iter()
                    .any(|o| o.id == *via && o.kind == ObjectiveKind::Exit && o.open_to(side));
                if !usable {
                    return Err(OrderError::NoSuchExit);
                }
            }
            // A base of fire is aimed at *people*, so the checks are about
            // people: they have to be in this battle, they have to be ours,
            // and they have to be somebody else. Refusing loudly matters
            // more here than for ground — a mission naming a formation that
            // is not there would score zero forever and look exactly like a
            // formation that had decided to do nothing.
            Mission::Support {
                formation: supported,
            } => {
                let target = self
                    .command
                    .formations()
                    .iter()
                    .find(|f| f.id == *supported)
                    .ok_or(OrderError::NoSuchFormation)?;
                if target.side != side || target.id == ordering {
                    return Err(OrderError::CannotSupportThat);
                }
            }
        }
        Ok(())
    }

    /// The side owning `unit`, rejecting orders from a side that already
    /// closed its planning.
    fn planning_unit_side(&self, unit: UnitId) -> Result<u8, OrderError> {
        let side = self.unit(unit).ok_or(OrderError::NoSuchUnit)?.side;
        if self.has_committed(side) {
            return Err(OrderError::AlreadyCommitted);
        }
        Ok(side)
    }

    fn set_move(&mut self, registry: &DataRegistry, id: UnitId, to: Hex) -> Result<(), OrderError> {
        self.planning_unit_side(id)?;
        let (path, _cost) = movement::path_to(registry, self, id, to).ok_or(OrderError::NoPath)?;
        let unit = self.unit_mut(id).ok_or(OrderError::NoSuchUnit)?;
        // path includes the starting tile; the intent is what remains to walk.
        unit.intent.path = path.into_iter().skip(1).collect();
        unit.planned = true;
        Ok(())
    }

    fn set_fire(
        &mut self,
        registry: &DataRegistry,
        id: UnitId,
        fire: FireIntent,
    ) -> Result<(), OrderError> {
        self.planning_unit_side(id)?;
        self.check_fire(registry, id, fire)?;
        let unit = self.unit_mut(id).ok_or(OrderError::NoSuchUnit)?;
        unit.intent.fire = fire;
        unit.planned = true;
        Ok(())
    }

    /// Everything about a fire order that can be judged when it is given.
    ///
    /// Split out of [`Self::set_fire`] so a radioed order can be checked
    /// before it is queued: an order held for a girl who cannot hear it has to
    /// have been legal when it was given, or the queue becomes a way to smuggle
    /// a shot at a friendly past the rules by being out of contact at the time.
    fn check_fire(
        &self,
        registry: &DataRegistry,
        id: UnitId,
        fire: FireIntent,
    ) -> Result<(), OrderError> {
        let side = self.unit(id).ok_or(OrderError::NoSuchUnit)?.side;
        // Validate only what cannot change as the round plays out. Range and
        // line of sight are deliberately not checked here: a unit may be
        // ordered to drive into a firing position and engage in the same
        // round, and the crew shoots on the tick the shot appears.
        match fire {
            FireIntent::Hold => {}
            FireIntent::Target { target, weapon } => {
                let tgt = self.unit(target).ok_or(OrderError::NoSuchUnit)?;
                if tgt.side == side {
                    return Err(OrderError::FriendlyTarget);
                }
                if !self.fog.side(side).spotted.contains(&target) {
                    return Err(OrderError::TargetNotSpotted);
                }
                self.weapon_def(registry, id, weapon)?;
            }
            FireIntent::Area { at, weapon } => {
                if !self.map.contains(at) {
                    return Err(OrderError::NotOnMap);
                }
                self.weapon_def(registry, id, weapon)?;
            }
        }
        Ok(())
    }

    fn weapon_def<'r>(
        &self,
        registry: &'r DataRegistry,
        unit: UnitId,
        index: usize,
    ) -> Result<&'r crate::data::WeaponDef, OrderError> {
        let u = self.unit(unit).ok_or(OrderError::NoSuchUnit)?;
        registry
            .vehicle(&u.vehicle)
            .and_then(|v| v.weapons.get(index))
            .and_then(|w| registry.weapon(w))
            .ok_or(OrderError::NoSuchWeapon)
    }

    fn commit(&mut self, side: u8) -> Result<Vec<Event>, OrderError> {
        if side as usize >= self.sides.len() {
            return Err(OrderError::NoSuchSide);
        }
        let living = self.living_sides();
        match &mut self.phase {
            Phase::Resolving { .. } => return Err(OrderError::NotPlanningPhase),
            Phase::Planning { committed } => {
                if committed[side as usize] {
                    return Err(OrderError::AlreadyCommitted);
                }
                committed[side as usize] = true;
                // Sides with nothing left on the board have nothing to say.
                let all_in = living
                    .iter()
                    .all(|s| committed.get(*s as usize).copied().unwrap_or(true));
                if all_in {
                    self.phase = Phase::Resolving { tick: 0 };
                }
            }
        }
        Ok(Vec::new())
    }

    /// Advance resolution by one tick, returning what happened in it.
    /// Returns nothing while planning or once the battle is over.
    pub fn step_tick(&mut self, registry: &DataRegistry) -> Vec<Event> {
        let Phase::Resolving { tick } = self.phase else {
            return Vec::new();
        };
        if self.is_over() {
            return Vec::new();
        }

        let mut events = vec![Event::TickStarted { tick }];
        for unit in self.units.iter_mut().filter(|u| u.alive) {
            for cd in &mut unit.cooldowns {
                *cd = cd.saturating_sub(1);
            }
        }

        // Orders in transit land at the top of the tick, before anybody
        // drives: a mission that took one tick to arrive is one the formation
        // acts on as this tick's movement resolves, not a round later.
        self.deliver_missions(&mut events);

        // The drill runs after delivery on purpose: an order that lands this
        // tick is the commander speaking *now*, and a fresh explicit order
        // outranks the reflex. It runs before movement so the tick she
        // reacts on is the tick she starts driving.
        self.run_crew_drill(registry, &mut events);

        self.resolve_movement(registry, &mut events);
        events.extend(fog::recompute(registry, self));
        // Succession runs first and runs always: who commands a formation is
        // a fact about the formation, not about anybody's radio, so this is
        // the one thing here that is not gated on a mod declaring command
        // rules. It has to precede the contact recompute — a successor who
        // takes over in the same step anchors the net immediately, which is
        // the difference between a leader's death costing her platoon a tick
        // and costing it the rest of the battle.
        self.pass_command(&mut events);
        // Contact is read after everyone has moved and before anyone shoots,
        // so a vehicle that drove out of its leader's radius is out of contact
        // in the same tick it left rather than the next one.
        self.recompute_contact(registry, &mut events);
        self.resolve_fire(registry, &mut events);
        events.extend(fog::recompute(registry, self));

        self.apply_pressure(registry, &mut events);

        // Exits are settled before control, so a vehicle that drives off the
        // map is gone before anyone asks who is standing where.
        if self.resolve_exits(&mut events) {
            events.extend(fog::recompute(registry, self));
        }
        // The picture is rebuilt last, once the tick's seeing is settled:
        // after the post-fire fog recompute (a gun that revealed itself is
        // reportable the same tick) and after exits (a reporter who has
        // driven off the map files nothing from the road home).
        self.recompute_picture(registry, &mut events);
        // Control is re-read every tick so driving onto the bridge takes it
        // there and then, but points are only paid at the end of a round —
        // an objective is worth holding for a *round*, and paying per tick
        // would make the scale of the score an accident of `ticks_per_round`.
        self.update_objective_control(&mut events);
        let next = tick + 1;
        let round_over = next >= registry.scale.ticks_per_round;
        if round_over {
            self.award_objective_points();
        }

        if self.in_contact() || events.iter().any(|e| matches!(e, Event::ShotHit { .. })) {
            self.last_contact_round = self.round;
        }
        self.check_victory(&mut events);
        if self.is_over() {
            return events;
        }

        if round_over {
            self.begin_round(registry, &mut events);
        } else {
            self.phase = Phase::Resolving { tick: next };
        }
        events
    }

    /// Run the rest of the round in one go. Convenient for headless callers;
    /// the presentation layer steps tick by tick instead so it can animate.
    pub fn resolve_round(&mut self, registry: &DataRegistry) -> Vec<Event> {
        let mut events = Vec::new();
        while !self.is_over() && self.resolving_tick().is_some() {
            events.extend(self.step_tick(registry));
        }
        events
    }

    /// The mid-round half of the battle drill: nobody under fire waits for
    /// the next planning phase to survive.
    ///
    /// The planning-table drill (the delegation layer's `executor_only`)
    /// covers a crew who is already threatened when the round is planned.
    /// This covers the one who is ambushed at tick four: an idle crew — no
    /// route left to drive, no target she was told to watch — who has had
    /// time to take in a threat breaks for the best cover she can reach.
    /// Return fire needs no twin here because opportunity fire already is
    /// one; this is its movement half, and it prices time with the same
    /// clock. `spotted_since` says when her side first laid eyes on each
    /// enemy, her `reactions` delay says how long she needs to catch up, so
    /// a tank she has watched crawl toward her for three rounds is answered
    /// the instant it comes into range while a gun that appears out of a
    /// treeline costs her the same stunned ticks it costs her gunner.
    ///
    /// What it will never touch: a crew with a route in hand keeps driving
    /// it (delaying plans already given is the sin the reaction-latency
    /// post-mortem forbids), a crew with a fire order is on deliberate
    /// overwatch and trusts her gun, and a crew whose morale rung no longer
    /// obeys is exactly as frozen as the rung says she is. A crew already on
    /// the best cover she can reach stands her ground — the comparison is
    /// strict, so equal cover never causes a pointless shuffle and each dash
    /// is to strictly better ground, which is what makes the drill settle
    /// instead of oscillate.
    fn run_crew_drill(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        let now = self.round as u64 * registry.scale.ticks_per_round as u64
            + self.resolving_tick().unwrap_or(0) as u64;
        let ids: Vec<UnitId> = self
            .units
            .iter()
            .filter(|u| u.alive && u.intent.is_empty())
            .map(|u| u.id)
            .collect();
        for id in ids {
            let Some(unit) = self.unit(id) else { continue };
            let pos = unit.pos;
            if !self.obeys(registry, unit) {
                continue;
            }
            let delay =
                super::stats::reaction_delay(registry, &self.roster, unit, self.terrain_at(pos))
                    as u64;
            let fog = self.fog.side(unit.side);
            // A threat she has caught up with, on the same per-enemy clock
            // opportunity fire pays.
            let noticed = crate::ai::threats(registry, self, id)
                .into_iter()
                .any(|enemy| {
                    fog.spotted_since
                        .get(&enemy)
                        .is_none_or(|since| now >= since + delay)
                });
            if !noticed {
                continue;
            }
            let cover_at = |hex: Hex| {
                self.terrain_at(hex)
                    .and_then(|t| registry.terrain(t))
                    .map_or(0, |t| t.cover)
            };
            let here = cover_at(pos);
            // Best cover wins; among equals the cheapest drive, then
            // coordinates, so replays agree on where she bolted to.
            let dest = movement::reachable(registry, self, id)
                .into_iter()
                .filter(|&(hex, _)| hex != pos && cover_at(hex) > here)
                .min_by_key(|&(hex, cost)| (std::cmp::Reverse(cover_at(hex)), cost, hex.x, hex.y))
                .map(|(hex, _)| hex);
            let Some(dest) = dest else { continue };
            let Some((path, _)) = movement::path_to(registry, self, id, dest) else {
                continue;
            };
            if let Some(unit) = self.unit_mut(id) {
                unit.intent.path = path.into_iter().skip(1).collect();
            }
            events.push(Event::TookCover { unit: id, at: dest });
        }
    }

    /// Everyone advances along their ordered route as far as this tick's
    /// movement credit allows.
    fn resolve_movement(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        // A crew that has had enough will not drive into more of it. They are
        // not out of the fight — firing is deliberately untouched below — they
        // simply will not advance, which is what frightened people do.
        let refusing: Vec<UnitId> = self
            .units
            .iter()
            .filter(|u| u.alive && !u.intent.path.is_empty() && !self.obeys(registry, u))
            .map(|u| u.id)
            .collect();
        for id in refusing {
            // She gets to try to hold. Discipline is the skill that resists,
            // rolled three-dice against her level, so training buys reliability
            // rather than a coin flip — and disobedience becomes something a
            // player can train away instead of a fact about the girl.
            let level = self.unit(id).map(|u| {
                self.roster.crew_skill(
                    registry,
                    registry.vehicle(&u.vehicle),
                    &u.crew,
                    &registry.morale.skill,
                    self.terrain_at(u.pos),
                )
            });
            if let Some(level) = level
                && crate::data::holds_together(&mut self.rng, level)
            {
                continue;
            }
            let rung = self
                .unit(id)
                .map(|u| registry.morale.rung(u.pressure).name.clone())
                .unwrap_or_default();
            // Said out loud, and the order is cleared so the unit does not sit
            // silently failing the same instruction for twelve ticks.
            events.push(Event::OrderRefused { unit: id, rung });
            if let Some(unit) = self.unit_mut(id) {
                unit.intent.path.clear();
            }
        }

        let ids: Vec<UnitId> = self
            .units
            .iter()
            .filter(|u| u.alive)
            .map(|u| u.id)
            .collect();
        // Held across the loop because the body takes `&mut self`; cloning the
        // `Arc` is a refcount bump, not a copy of the roster.
        let roster = self.roster.clone();
        for id in ids {
            let Some(unit) = self.unit(id) else { continue };
            if unit.intent.path.is_empty() {
                continue;
            }
            let (side, start) = (unit.side, unit.pos);
            let gained = movement::move_points(
                registry,
                &roster,
                unit,
                self.map.get(unit.pos).map(|t| t.terrain.as_str()),
            );
            {
                let unit = self.unit_mut(id).expect("alive above");
                unit.move_credit += gained;
            }

            // The leg starts where the unit stands, so the presentation layer
            // can animate straight from the sprite's current tile.
            let mut walked = vec![start];
            let mut trapped_at = None;
            let mut blocked = false;
            while let Some(unit) = self.unit(id) {
                let Some(&next) = unit.intent.path.first() else {
                    break;
                };
                let Some(cost) = movement::edge_cost_for(registry, self, unit, unit.pos, next)
                else {
                    // The route stopped being walkable (terrain lookup gone).
                    break;
                };
                let price = cost * registry.scale.ticks_per_round;
                if unit.move_credit < price {
                    break;
                }
                // One unit per hex. A friend in the way is traffic: it will
                // probably have driven on by the next tick, so hold and try
                // again. An enemy is the end of the advance either way, but
                // only one nobody had spotted counts as an ambush.
                if let Some(other) = self.unit_at(next) {
                    if other.side != side {
                        if self.fog.side(side).spotted.contains(&other.id) {
                            blocked = true;
                        } else {
                            trapped_at = Some(unit.pos);
                        }
                    }
                    break;
                }
                let unit = self.unit_mut(id).expect("alive above");
                unit.move_credit -= price;
                let facing = unit.pos.neighbor_direction(next);
                unit.pos = next;
                if let Some(dir) = facing {
                    unit.facing = dir;
                }
                unit.intent.path.remove(0);
                walked.push(next);
            }

            if walked.len() > 1 {
                fog::clear_reveal(self, id);
                events.push(Event::UnitMoved {
                    unit: id,
                    path: walked,
                });
            }
            if blocked || trapped_at.is_some() {
                // The route ran into somebody; the rest of it is off.
                if let Some(unit) = self.unit_mut(id) {
                    unit.intent.path.clear();
                }
            }
            if let Some(at) = trapped_at {
                events.push(Event::UnitTrapped { unit: id, at });
            }
        }
    }

    /// Everyone who has a shot takes it. Wrecks are cleared only once every
    /// gun has spoken, so a tick's shots are genuinely simultaneous.
    fn resolve_fire(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        let ids: Vec<UnitId> = self
            .units
            .iter()
            .filter(|u| u.alive)
            .map(|u| u.id)
            .collect();
        for id in ids {
            combat::fire_if_able(registry, self, id, events);
        }
        combat::reap(self, events);
    }

    /// Turn this tick's events into pressure on the crews that felt them.
    ///
    /// Reads the events rather than being called from inside combat, so that
    /// every source of fear is in one place and adding another — suppression,
    /// a commander lost, being outflanked — is a line here rather than a hook
    /// threaded through the shooting code.
    fn apply_pressure(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        let rules = &registry.morale;
        let mut before: Vec<(UnitId, String)> = self
            .units
            .iter()
            .filter(|u| u.alive)
            .map(|u| (u.id, rules.rung(u.pressure).id.clone()))
            .collect();

        let add = |state: &mut BattleState, id: UnitId, amount: u32| {
            if let Some(unit) = state.unit_mut(id) {
                unit.pressure = unit.pressure.saturating_add(amount);
            }
        };

        // Collected first: the borrow of `events` has to end before units are
        // touched, and iterating in event order keeps this deterministic.
        let hits: Vec<UnitId> = events
            .iter()
            .filter_map(|e| match e {
                Event::ShotHit { target, .. } => Some(*target),
                _ => None,
            })
            .collect();
        // A shell that strikes and fails to get through still rings the
        // hull like a bell; small-arms fire does not (`rattled` is false),
        // or suppression would quietly rebuild the damage floor's defect in
        // morale instead of hit points.
        let clangs: Vec<UnitId> = events
            .iter()
            .filter_map(|e| match e {
                Event::ShotBounced {
                    target,
                    rattled: true,
                    ..
                } => Some(*target),
                _ => None,
            })
            .collect();
        let losses: Vec<(u8, Hex)> = events
            .iter()
            .filter_map(|e| match e {
                Event::UnitDestroyed { unit, at } => {
                    self.units.get(unit.index()).map(|u| (u.side, *at))
                }
                _ => None,
            })
            .collect();
        // Read off the event for the same reason everything else here is: the
        // succession pass has no business knowing what fear costs, and one
        // place that turns a tick's news into pressure is worth more than a
        // hook in each system that produces news.
        let bereaved: Vec<Vec<UnitId>> = events
            .iter()
            .filter_map(|e| match e {
                Event::CommandPassed { formation, .. } => self
                    .command
                    .formations()
                    .iter()
                    .find(|f| f.id == *formation)
                    .map(|f| f.members.clone()),
                _ => None,
            })
            .collect();

        for id in hits {
            add(self, id, rules.hit);
        }
        for id in clangs {
            add(self, id, rules.bounced);
        }
        for members in bereaved {
            // The whole formation, wherever it is standing: unlike watching a
            // friend burn, losing your commander is news that travels the
            // chain of command rather than the line of sight. Members are in
            // unit-id order, and `add` quietly skips anyone no longer on the
            // field.
            for id in members {
                add(self, id, rules.leader_lost);
            }
        }
        for (side, at) in losses {
            // Watching a friend go up is worse than hearing about it, so this
            // only reaches the ones who could see it happen.
            let watchers: Vec<UnitId> = self
                .units
                .iter()
                .filter(|u| u.alive && u.side == side && self.fog.side(side).visible.contains(&at))
                .map(|u| u.id)
                .collect();
            for id in watchers {
                add(self, id, rules.ally_destroyed);
            }
        }

        // Say what changed, once, and only for crews that actually moved.
        before.retain(|(id, _)| self.unit(*id).is_some());
        for (id, was) in before {
            let Some(unit) = self.unit(id) else { continue };
            let now = rules.rung(unit.pressure);
            if now.id != was {
                events.push(Event::MoraleChanged {
                    unit: id,
                    rung: now.name.clone(),
                    obeys: now.obeys,
                });
            }
        }
    }

    /// Open a new round: clear last round's orders and let sides plan again.
    fn begin_round(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        self.round += 1;
        // Crews settle between rounds. A disciplined one settles faster, which
        // is what makes discipline worth training rather than merely a saving
        // throw at the worst moment.
        let ids: Vec<UnitId> = self.units.iter().map(|u| u.id).collect();
        for id in ids {
            let level = self.units.get(id.index()).map(|u| {
                self.roster.crew_skill(
                    registry,
                    registry.vehicle(&u.vehicle),
                    &u.crew,
                    &registry.morale.skill,
                    self.terrain_at(u.pos),
                )
            });
            let shed = level.map(|l| registry.morale.recovered(l)).unwrap_or(0);
            if let Some(unit) = self.units.get_mut(id.index()) {
                unit.pressure = unit.pressure.saturating_sub(shed);
            }
        }
        for unit in self.units.iter_mut() {
            unit.intent = UnitIntent::default();
            unit.planned = false;
            unit.move_credit = 0;
        }
        self.phase = Phase::Planning {
            committed: vec![false; self.sides.len()],
        };
        events.push(Event::RoundStarted { round: self.round });
        // A personal destination reached is a personal destination done:
        // she holds the ground she was sent to, still detached, and the
        // panel stops saying she is on her way.
        for unit in self.units.iter_mut().filter(|u| u.alive) {
            if unit.tasking == Some(unit.pos) {
                unit.tasking = None;
            }
        }
        // Plans advance at the top of the round, so a promoted leg steers the
        // planning that is about to happen rather than arriving a round late.
        self.promote_missions(registry, events);
        // Last, and after the round is open: an order held at the radio is
        // delivered *into* the planning phase it arrives for, which means the
        // intent it becomes survives the clearing above and the player sees it
        // on the board she is about to plan on.
        self.deliver_waiting_orders(registry, events);
    }

    /// Drive off the map anyone standing on an exit they are entitled to use.
    ///
    /// Returns whether anybody left, because a unit leaving changes what its
    /// side can see and the fog is only free to recompute when nothing moved.
    ///
    /// Exits are all-or-nothing and immediate: a vehicle that reaches the
    /// lane is out. There is deliberately no "you may only leave if damaged"
    /// rule here — whether leaving is *allowed* is a matter for orders and
    /// the chain of command, not for the hex it happens on.
    ///
    /// Objectives are walked in map-file order and units in id order, so what
    /// this emits cannot depend on hash iteration order.
    fn resolve_exits(&mut self, events: &mut Vec<Event>) -> bool {
        let map = std::sync::Arc::clone(&self.map);
        let mut any = false;
        for objective in map
            .objectives()
            .iter()
            .filter(|o| o.kind == ObjectiveKind::Exit)
        {
            for index in 0..self.units.len() {
                let unit = &self.units[index];
                if !unit.alive || !objective.open_to(unit.side) || !objective.contains(unit.pos) {
                    continue;
                }
                let (id, side, at) = (unit.id, unit.side, unit.pos);
                let unit = &mut self.units[index];
                // Off the board but not destroyed. `exited` is what keeps the
                // campaign from mourning her.
                unit.alive = false;
                unit.exited = true;
                unit.intent = UnitIntent::default();
                if let Some(score) = self.score.get_mut(side as usize) {
                    *score += objective.value;
                }
                events.push(Event::UnitExited {
                    unit: id,
                    objective: objective.id.clone(),
                    at,
                });
                any = true;
            }
        }
        any
    }

    /// Work out who is standing on each objective, and hand it over when that
    /// answer changes.
    ///
    /// A side takes an objective by having a living unit on one of its hexes
    /// while no enemy does; a hex held by nobody stays with whoever took it
    /// last, so ground has to be taken back rather than merely vacated. When
    /// two sides are both on it, it is contested and belongs to neither —
    /// which is what stops a defender collecting points while being overrun.
    ///
    /// Objectives are walked in map-file order and units in id order, so the
    /// events this emits cannot depend on hash iteration order.
    fn update_objective_control(&mut self, events: &mut Vec<Event>) {
        // The map is shared behind an `Arc`, so taking a handle to it costs a
        // refcount and frees `self` to be written to inside the loop.
        let map = std::sync::Arc::clone(&self.map);
        for (index, objective) in map.objectives().iter().enumerate() {
            // An exit is not ground anyone holds — you pass through it, and
            // `resolve_exits` has already taken anyone who did. Its slot in
            // `objective_held` stays `None` for the whole battle.
            if objective.kind == ObjectiveKind::Exit {
                continue;
            }
            let mut occupiers: Vec<u8> = self
                .units
                .iter()
                .filter(|u| u.alive && objective.contains(u.pos))
                .map(|u| u.side)
                .collect();
            occupiers.sort_unstable();
            occupiers.dedup();
            let claimant = match occupiers.as_slice() {
                [side] => Some(*side),
                // Nobody there: control stands. Several sides there: nobody's.
                [] => self.objective_held[index],
                _ => None,
            };
            if self.objective_held[index] != claimant {
                self.objective_held[index] = claimant;
                events.push(Event::ObjectiveTaken {
                    objective: objective.id.clone(),
                    side: claimant,
                    at: objective.anchor(),
                });
            }
        }
    }

    /// Pay each objective's value to whoever holds it. Silent: a side quietly
    /// collecting two points a round is a running total, not news, and the
    /// log exists to carry the things that are.
    fn award_objective_points(&mut self) {
        let map = std::sync::Arc::clone(&self.map);
        for (index, objective) in map.objectives().iter().enumerate() {
            let Some(side) = self.objective_held[index] else {
                continue;
            };
            if let Some(score) = self.score.get_mut(side as usize) {
                *score += objective.value;
            }
        }
    }

    fn check_victory(&mut self, events: &mut Vec<Event>) {
        if self.over.is_some() {
            return;
        }
        // Decapitation is read before *everything*, including the score. The
        // score-before-board rule below says the mission outranks the force
        // spent; this says the same thing one level up — a scenario that
        // declared a formation its side cannot survive losing has stated what
        // the battle was for, and a side that has lost it has lost whatever
        // ground it happens to be standing on and however many points it has
        // collected on the way. Reading the points first would let a raid pay
        // for its own headquarters with objective hexes.
        //
        // A map that declares no `loss_conditions` walks an empty list, which
        // is how this stays an additive rule rather than a new phase of the
        // battle.
        if let Some(loser) = self.decapitated() {
            // A decapitation names who lost; who *won* it is a separate
            // question, and in the two-sided battle every map is, the answer
            // is the only other army on the field. With more sides than that
            // nobody has beheaded anybody in particular, so the points break
            // the tie exactly as they do for a stalemate — and only if they
            // point at somebody who is not the side that just came apart.
            let others: Vec<u8> = (0..self.sides.len() as u8)
                .filter(|s| *s != loser)
                .collect();
            let winner = match others.as_slice() {
                [only] => Some(*only),
                _ => self.leader().filter(|s| *s != loser),
            };
            self.finish(winner, EndReason::Decapitated, events);
            return;
        }
        // The score is read *before* the board, because the mission being
        // accomplished outranks the force being spent. That ordering is not
        // pedantry: a side whose mission is to withdraw reaches its target on
        // the same tick its last vehicle drives off the map, and checking the
        // board first would hand that battle to the enemy for holding a field
        // nobody wanted.
        //
        // A map that sets no `victory_score` cannot end this way at all,
        // which is what keeps objectives an additive rule: say nothing and
        // the battle is fought to the death exactly as it always was.
        if let Some(target) = self.map.victory_score()
            && let Some(side) = self
                .score
                .iter()
                .position(|s| *s >= target)
                .map(|s| s as u8)
        {
            self.finish(Some(side), EndReason::Objectives, events);
            return;
        }
        let living = self.living_sides();
        if living.len() <= 1 {
            // A side still on the board when the other has nothing left has
            // won, whatever the score says. `leader` only breaks the tie when
            // *nobody* is left: mutual destruction over ground somebody held
            // is not the same battle as mutual destruction in an empty field.
            let winner = living.first().copied().or_else(|| self.leader());
            self.finish(winner, EndReason::Eliminated, events);
            return;
        }
        if self.round.saturating_sub(self.last_contact_round) >= STALEMATE_ROUNDS {
            // Breaking contact ends the shooting; the points say who won it.
            let winner = self.leader();
            self.finish(winner, EndReason::Stalemate, events);
        }
    }

    /// The first side whose map-declared loss condition has come true, if any.
    ///
    /// Conditions are walked in map-file order, so which one fires when two
    /// come true on the same tick cannot depend on a hash. A condition naming
    /// a formation this battle does not have — possible on the overworld path,
    /// where a terrain map's declarations meet an army's units and an empty
    /// formation is dropped — can never fire, which is the right answer: the
    /// stake was placed on somebody who is not here.
    fn decapitated(&self) -> Option<u8> {
        for condition in self.map.loss_conditions() {
            let Some(formation) = self
                .command
                .formations()
                .iter()
                .find(|f| f.id == condition.formation)
            else {
                continue;
            };
            // `lost` throughout, never `!alive`: a vehicle that drove off by
            // an exit is off the board but home, and reading `alive` here
            // would turn every ordered withdrawal into a decapitation.
            let lost = |id: &UnitId| {
                self.units
                    .get(id.index())
                    .is_some_and(|u| !u.alive && !u.exited)
            };
            let fallen = match condition.when {
                LossTrigger::LeaderLost => formation.founding_leader.iter().any(lost),
                LossTrigger::Wiped => {
                    formation.members.iter().any(lost)
                        && formation
                            .members
                            .iter()
                            .all(|id| self.units.get(id.index()).is_some_and(|u| !u.alive))
                }
            };
            if fallen {
                return Some(condition.side);
            }
        }
        None
    }

    /// Does any side currently have an enemy in sight?
    fn in_contact(&self) -> bool {
        (0..self.sides.len() as u8).any(|side| !self.fog.side(side).spotted.is_empty())
    }

    fn finish(&mut self, winner: Option<u8>, reason: EndReason, events: &mut Vec<Event>) {
        self.over = Some(BattleResult { winner, reason });
        events.push(Event::BattleEnded { winner, reason });
    }
}
