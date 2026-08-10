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
use crate::map::ObjectiveKind;
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
            Order::ClearIntent { unit } => {
                let side = self.planning_unit_side(*unit)?;
                let _ = side;
                let u = self.unit_mut(*unit).ok_or(OrderError::NoSuchUnit)?;
                u.intent = UnitIntent::default();
                u.planned = false;
                Ok(Vec::new())
            }
            Order::SetMission { formation, mission } => {
                self.set_mission(registry, *formation, mission.clone())
            }
            Order::Commit { side } => self.commit(*side),
        }
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
        match &mission {
            Mission::Advance { to } | Mission::Recon { toward: to } => {
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
        }
        // An order is sent here; whether it has *arrived* is the signals net's
        // business. A delay of zero — which is every battle whose mod declares
        // no `command` block — puts it straight onto the formation, so the old
        // game is this same line rather than a branch around it.
        let delay = self.mission_delay(registry, formation);
        if delay == 0 {
            self.command.set_mission(formation, mission.clone());
        } else {
            self.command.set_incoming(formation, mission.clone(), delay);
        }
        Ok(vec![Event::MissionAssigned {
            formation: id,
            mission,
        }])
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
        let side = self.planning_unit_side(id)?;
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
        let unit = self.unit_mut(id).ok_or(OrderError::NoSuchUnit)?;
        unit.intent.fire = fire;
        unit.planned = true;
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

        self.resolve_movement(registry, &mut events);
        events.extend(fog::recompute(registry, self));
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
        let losses: Vec<(u8, Hex)> = events
            .iter()
            .filter_map(|e| match e {
                Event::UnitDestroyed { unit, at } => {
                    self.units.get(unit.index()).map(|u| (u.side, *at))
                }
                _ => None,
            })
            .collect();

        for id in hits {
            add(self, id, rules.hit);
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

    /// Does any side currently have an enemy in sight?
    fn in_contact(&self) -> bool {
        (0..self.sides.len() as u8).any(|side| !self.fog.side(side).spotted.is_empty())
    }

    fn finish(&mut self, winner: Option<u8>, reason: EndReason, events: &mut Vec<Event>) {
        self.over = Some(BattleResult { winner, reason });
        events.push(Event::BattleEnded { winner, reason });
    }
}
