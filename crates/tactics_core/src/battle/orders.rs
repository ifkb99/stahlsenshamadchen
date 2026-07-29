//! Orders in, events out: the sim's single mutation boundary.

use super::{combat, fog, movement, BattleResult, BattleState, EndReason, UnitId, STALEMATE_TURNS};
use crate::data::{ArmorFacing, DataRegistry, WeaponDef};
use hexx::Hex;

/// Everything a side can ask the simulation to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Order {
    /// Move a unit along the cheapest path to `to`.
    Move { unit: UnitId, to: Hex },
    /// Fire at a spotted enemy unit.
    Attack {
        unit: UnitId,
        target: UnitId,
        /// Index into the vehicle's weapon list.
        weapon: usize,
    },
    /// Fire at a tile without a confirmed target, at a heavy accuracy
    /// penalty. Hits whatever happens to be there.
    BlindFire { unit: UnitId, at: Hex, weapon: usize },
    /// End the unit's turn without acting.
    Wait { unit: UnitId },
    /// Pass play to the next side.
    EndTurn,
}

/// Everything that can happen as a result of an order. The presentation
/// layer animates these; AI and campaign scripts observe them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    TurnStarted {
        side: u8,
        turn: u32,
    },
    UnitMoved {
        unit: UnitId,
        path: Vec<Hex>,
    },
    /// The unit ran into an unspotted enemy mid-path and stopped short.
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
        counter: bool,
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
    #[error("it is not that side's turn")]
    NotYourTurn,
    #[error("unit has already moved")]
    AlreadyMoved,
    #[error("unit has already acted")]
    AlreadyActed,
    #[error("no valid path to the destination")]
    NoPath,
    #[error("no such weapon on this vehicle")]
    NoSuchWeapon,
    #[error("target is not spotted")]
    TargetNotSpotted,
    #[error("target is out of range")]
    OutOfRange,
    #[error("no line of sight to the target")]
    NoLineOfSight,
    #[error("cannot target a friendly unit")]
    FriendlyTarget,
    #[error("tile is not on the map")]
    NotOnMap,
}

impl BattleState {
    /// Apply one order, mutating the state and returning what happened.
    pub fn apply(
        &mut self,
        registry: &DataRegistry,
        order: &Order,
    ) -> Result<Vec<Event>, OrderError> {
        if self.is_over() {
            return Err(OrderError::BattleOver);
        }
        let mut events = match order {
            Order::Move { unit, to } => self.apply_move(registry, *unit, *to)?,
            Order::Attack {
                unit,
                target,
                weapon,
            } => self.apply_attack(registry, *unit, *target, *weapon)?,
            Order::BlindFire { unit, at, weapon } => {
                self.apply_blind_fire(registry, *unit, *at, *weapon)?
            }
            Order::Wait { unit } => {
                let u = self.active_unit_mut(*unit)?;
                u.moved = true;
                u.acted = true;
                Vec::new()
            }
            Order::EndTurn => self.apply_end_turn(registry),
        };
        let hit = events.iter().any(|e| matches!(e, Event::ShotHit { .. }));
        if hit || self.in_contact() {
            self.last_contact_turn = self.turn;
        }
        self.check_victory(&mut events);
        Ok(events)
    }

    fn active_unit_mut(&mut self, id: UnitId) -> Result<&mut super::Unit, OrderError> {
        let active = self.active_side;
        let unit = self.unit_mut(id).ok_or(OrderError::NoSuchUnit)?;
        if unit.side != active {
            return Err(OrderError::NotYourTurn);
        }
        Ok(unit)
    }

    fn weapon_of<'r>(
        &self,
        registry: &'r DataRegistry,
        unit: UnitId,
        index: usize,
    ) -> Result<&'r WeaponDef, OrderError> {
        let u = self.unit(unit).ok_or(OrderError::NoSuchUnit)?;
        registry
            .vehicle(&u.vehicle)
            .and_then(|v| v.weapons.get(index))
            .and_then(|w| registry.weapon(w))
            .ok_or(OrderError::NoSuchWeapon)
    }

    fn apply_move(
        &mut self,
        registry: &DataRegistry,
        id: UnitId,
        to: Hex,
    ) -> Result<Vec<Event>, OrderError> {
        {
            let unit = self.active_unit_mut(id)?;
            if unit.moved {
                return Err(OrderError::AlreadyMoved);
            }
            if unit.acted {
                return Err(OrderError::AlreadyActed);
            }
        }
        let (path, _cost) = movement::path_to(registry, self, id, to).ok_or(OrderError::NoPath)?;
        if path.len() > 1 && self.unit_at(to).is_some() {
            return Err(OrderError::NoPath);
        }

        // Walk the path; unspotted enemies ambush us (stop on the tile
        // before them).
        let mut stopped_at = path[0];
        let mut walked = vec![path[0]];
        let mut trapped = false;
        for &step in &path[1..] {
            if self.unit_at(step).is_some() {
                trapped = true;
                break;
            }
            stopped_at = step;
            walked.push(step);
        }

        let facing = walked
            .windows(2)
            .last()
            .and_then(|w| w[0].neighbor_direction(w[1]));
        {
            let unit = self.unit_mut(id).expect("checked above");
            unit.pos = stopped_at;
            if let Some(dir) = facing {
                unit.facing = dir;
            }
            unit.moved = true;
            if trapped {
                unit.acted = true;
            }
        }
        fog::clear_reveal(self, id);

        let mut events = vec![Event::UnitMoved {
            unit: id,
            path: walked,
        }];
        if trapped {
            events.push(Event::UnitTrapped {
                unit: id,
                at: stopped_at,
            });
        }
        events.extend(fog::recompute(registry, self));
        Ok(events)
    }

    fn apply_attack(
        &mut self,
        registry: &DataRegistry,
        id: UnitId,
        target: UnitId,
        weapon_index: usize,
    ) -> Result<Vec<Event>, OrderError> {
        {
            let unit = self.active_unit_mut(id)?;
            if unit.acted {
                return Err(OrderError::AlreadyActed);
            }
        }
        let weapon = self.weapon_of(registry, id, weapon_index)?.clone();
        let (att_pos, att_side) = {
            let u = self.unit(id).expect("checked above");
            (u.pos, u.side)
        };
        let tgt = self.unit(target).ok_or(OrderError::NoSuchUnit)?;
        if tgt.side == att_side {
            return Err(OrderError::FriendlyTarget);
        }
        let tgt_pos = tgt.pos;
        if !self.fog.side(att_side).spotted.contains(&target) {
            return Err(OrderError::TargetNotSpotted);
        }
        let dist = att_pos.distance_to(tgt_pos);
        if !(weapon.range[0] as i32..=weapon.range[1] as i32).contains(&dist) {
            return Err(OrderError::OutOfRange);
        }
        // Direct-fire weapons need line of sight; indirect ones only need
        // the target spotted (checked above), i.e. a friendly spotter.
        if !weapon.indirect && !fog::los_clear(registry, &self.map, att_pos, tgt_pos) {
            return Err(OrderError::NoLineOfSight);
        }

        {
            let unit = self.unit_mut(id).expect("checked above");
            if unit.pos != tgt_pos {
                unit.facing = unit.pos.main_direction_to(tgt_pos);
            }
            unit.moved = true;
            unit.acted = true;
        }
        Ok(combat::resolve_attack(registry, self, id, &weapon, target, false))
    }

    fn apply_blind_fire(
        &mut self,
        registry: &DataRegistry,
        id: UnitId,
        at: Hex,
        weapon_index: usize,
    ) -> Result<Vec<Event>, OrderError> {
        {
            let unit = self.active_unit_mut(id)?;
            if unit.acted {
                return Err(OrderError::AlreadyActed);
            }
        }
        if !self.map.contains(at) {
            return Err(OrderError::NotOnMap);
        }
        let weapon = self.weapon_of(registry, id, weapon_index)?.clone();
        let (att_pos, att_side) = {
            let u = self.unit(id).expect("checked above");
            (u.pos, u.side)
        };
        let dist = att_pos.distance_to(at);
        if !(weapon.range[0] as i32..=weapon.range[1] as i32).contains(&dist) {
            return Err(OrderError::OutOfRange);
        }
        if !weapon.indirect && !fog::los_clear(registry, &self.map, att_pos, at) {
            return Err(OrderError::NoLineOfSight);
        }

        {
            let unit = self.unit_mut(id).expect("checked above");
            if unit.pos != at {
                unit.facing = unit.pos.main_direction_to(at);
            }
            unit.moved = true;
            unit.acted = true;
        }

        let target = self
            .unit_at(at)
            .filter(|t| t.side != att_side)
            .map(|t| t.id);
        let events = match target {
            Some(target) => combat::resolve_attack(registry, self, id, &weapon, target, true),
            None => {
                let mut ev = vec![
                    Event::ShotFired {
                        attacker: id,
                        from: att_pos,
                        at,
                        weapon: weapon.id.clone(),
                        blind: true,
                        counter: false,
                    },
                    Event::ShotMissed { attacker: id, at },
                ];
                fog::reveal_to_all(self, id);
                ev.extend(fog::recompute(registry, self));
                ev
            }
        };
        Ok(events)
    }

    fn apply_end_turn(&mut self, registry: &DataRegistry) -> Vec<Event> {
        let side_count = self.sides.len() as u8;
        let living = self.living_sides();
        // Advance to the next side that still has units.
        let mut next = self.active_side;
        for _ in 0..side_count {
            next = (next + 1) % side_count;
            if living.contains(&next) {
                break;
            }
        }
        if next <= self.active_side {
            self.turn += 1;
        }
        self.active_side = next;
        for unit in self.units.iter_mut().filter(|u| u.alive && u.side == next) {
            unit.moved = false;
            unit.acted = false;
        }
        let mut events = vec![Event::TurnStarted {
            side: next,
            turn: self.turn,
        }];
        events.extend(fog::recompute(registry, self));
        events
    }

    fn check_victory(&mut self, events: &mut Vec<Event>) {
        if self.over.is_some() {
            return;
        }
        let living = self.living_sides();
        if living.len() <= 1 {
            self.finish(living.first().copied(), EndReason::Eliminated, events);
        } else if self.turn.saturating_sub(self.last_contact_turn) >= STALEMATE_TURNS {
            self.finish(None, EndReason::Stalemate, events);
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
