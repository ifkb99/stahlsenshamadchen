//! The strategic layer: armies move between objectives on a hex map,
//! capture income-producing tiles, and trigger battles when they clash.
//!
//! Information is softer than in battles: armies are visible to everyone
//! unless they sit in `concealing` terrain with no enemy adjacent.

use crate::ai::{AiConfig, AiPlanner};
use crate::data::{DataRegistry, MovementClass};
use crate::map::{HexMap, MapFile, MapKind, UnitPlacement};
use hexx::Hex;
use rand::seq::IndexedRandom;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArmyId(pub u32);

impl ArmyId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone)]
pub struct OverworldSide {
    pub name: String,
    pub funds: i32,
    /// `None` = human controlled.
    pub ai: Option<AiConfig>,
}

/// A stack of units moving as one piece on the strategic map.
#[derive(Debug, Clone)]
pub struct Army {
    pub id: ArmyId,
    pub side: u8,
    pub name: String,
    pub pos: Hex,
    pub movement: u32,
    pub moved: bool,
    /// Units that spawn into battles this army fights. Battle casualties
    /// are written back here.
    pub units: Vec<UnitPlacement>,
    pub alive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverworldOrder {
    /// Move toward `to`; moving onto a visible enemy army attacks it.
    MoveArmy {
        army: ArmyId,
        to: Hex,
    },
    EndTurn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverworldEvent {
    TurnStarted {
        side: u8,
        turn: u32,
    },
    Income {
        side: u8,
        amount: i32,
    },
    ArmyMoved {
        army: ArmyId,
        path: Vec<Hex>,
    },
    ObjectiveCaptured {
        at: Hex,
        side: u8,
    },
    /// Two armies met; the game layer should run a battle and report the
    /// outcome back via [`OverworldState::apply_battle_result`].
    BattleTriggered {
        attacker: ArmyId,
        defender: ArmyId,
        at: Hex,
    },
    ArmyDestroyed {
        army: ArmyId,
    },
    GameEnded {
        winner: Option<u8>,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OverworldError {
    #[error("the campaign is over")]
    GameOver,
    #[error("army does not exist or was destroyed")]
    NoSuchArmy,
    #[error("it is not that side's turn")]
    NotYourTurn,
    #[error("army has already moved")]
    AlreadyMoved,
    #[error("no valid path to the destination")]
    NoPath,
}

#[derive(Debug, Clone)]
pub struct OverworldState {
    pub map: Arc<HexMap>,
    pub sides: Vec<OverworldSide>,
    pub armies: Vec<Army>,
    /// Owner side of each captured objective tile.
    pub owners: HashMap<Hex, u8>,
    pub turn: u32,
    pub active_side: u8,
    pub over: Option<Option<u8>>,
}

#[derive(Debug, thiserror::Error)]
pub enum OverworldSetupError {
    #[error("map `{0}` not found in registry")]
    MissingMap(String),
    #[error("map `{0}` is not an overworld map")]
    NotAnOverworldMap(String),
    #[error(transparent)]
    Map(#[from] crate::map::MapError),
}

/// Armies traverse the overworld as tracked columns with generous climb.
const ARMY_CLASS: MovementClass = MovementClass::Tracked;
const ARMY_CLIMB: i32 = 9;

impl OverworldState {
    pub fn from_map(registry: &DataRegistry, map_id: &str) -> Result<Self, OverworldSetupError> {
        let file: &MapFile = registry
            .map(map_id)
            .ok_or_else(|| OverworldSetupError::MissingMap(map_id.to_string()))?;
        if file.kind != MapKind::Overworld {
            return Err(OverworldSetupError::NotAnOverworldMap(map_id.to_string()));
        }
        let map = HexMap::from_map_file(file)?;
        let sides = file
            .sides
            .iter()
            .map(|s| OverworldSide {
                name: s.name.clone(),
                funds: s.funds,
                ai: s.ai.clone(),
            })
            .collect();
        let armies = file
            .armies
            .iter()
            .enumerate()
            .map(|(i, a)| Army {
                id: ArmyId(i as u32),
                side: a.side,
                name: a.name.clone(),
                pos: crate::offset_to_hex(a.at[0], a.at[1]),
                movement: a.movement,
                moved: false,
                units: a.units.clone(),
                alive: true,
            })
            .collect();
        Ok(Self {
            map: Arc::new(map),
            sides,
            armies,
            owners: HashMap::new(),
            turn: 1,
            active_side: 0,
            over: None,
        })
    }

    pub fn army(&self, id: ArmyId) -> Option<&Army> {
        self.armies.get(id.index()).filter(|a| a.alive)
    }

    pub fn army_mut(&mut self, id: ArmyId) -> Option<&mut Army> {
        self.armies.get_mut(id.index()).filter(|a| a.alive)
    }

    pub fn army_at(&self, hex: Hex) -> Option<&Army> {
        self.armies.iter().find(|a| a.alive && a.pos == hex)
    }

    pub fn side_armies(&self, side: u8) -> impl Iterator<Item = &Army> {
        self.armies
            .iter()
            .filter(move |a| a.alive && a.side == side)
    }

    /// Soft fog: an army is hidden from `observer` only while it sits in
    /// concealing terrain with no enemy army adjacent.
    pub fn army_visible_to(&self, registry: &DataRegistry, army: &Army, observer: u8) -> bool {
        if army.side == observer {
            return true;
        }
        let concealed = self
            .map
            .get(army.pos)
            .and_then(|t| registry.terrain(&t.terrain))
            .is_some_and(|t| t.concealing);
        if !concealed {
            return true;
        }
        self.side_armies(observer)
            .any(|a| a.pos.distance_to(army.pos) <= 1)
    }

    pub fn visible_armies<'s>(&'s self, registry: &DataRegistry, observer: u8) -> Vec<&'s Army> {
        self.armies
            .iter()
            .filter(|a| a.alive && self.army_visible_to(registry, a, observer))
            .collect()
    }

    fn edge_cost(&self, registry: &DataRegistry, from: Hex, to: Hex) -> Option<u32> {
        crate::battle::movement_edge_cost(registry, &self.map, ARMY_CLASS, ARMY_CLIMB, from, to)
    }

    /// Every tile this army could end its move on, with the cheapest cost to
    /// get there. Other armies block both movement and parking, matching
    /// what [`Self::apply_move`] will actually allow. Ignores whether the
    /// army has already moved, so callers can preview a spent army's reach.
    pub fn reachable(&self, registry: &DataRegistry, id: ArmyId) -> HashMap<Hex, u32> {
        let Some(army) = self.army(id) else {
            return HashMap::new();
        };
        let mut best: HashMap<Hex, u32> = HashMap::new();
        let mut heap = BinaryHeap::new();
        best.insert(army.pos, 0);
        heap.push((Reverse(0u32), army.pos.x, army.pos.y));

        while let Some((Reverse(cost), x, y)) = heap.pop() {
            let hex = Hex::new(x, y);
            if best.get(&hex).is_some_and(|&c| c < cost) {
                continue;
            }
            for next in hex.all_neighbors() {
                if self.army_at(next).is_some() {
                    continue;
                }
                let Some(step) = self.edge_cost(registry, hex, next) else {
                    continue;
                };
                let total = cost + step;
                if total > army.movement {
                    continue;
                }
                if best.get(&next).is_none_or(|&c| total < c) {
                    best.insert(next, total);
                    heap.push((Reverse(total), next.x, next.y));
                }
            }
        }
        best
    }

    /// Visible enemy armies this army could engage this turn: those sitting
    /// next to a tile it can reach (or next to where it already stands).
    pub fn attack_targets(&self, registry: &DataRegistry, id: ArmyId) -> Vec<ArmyId> {
        let Some(army) = self.army(id) else {
            return Vec::new();
        };
        let reach = self.reachable(registry, id);
        self.armies
            .iter()
            .filter(|e| e.alive && e.side != army.side)
            .filter(|e| self.army_visible_to(registry, e, army.side))
            .filter(|e| {
                e.pos.distance_to(army.pos) == 1
                    || reach.keys().any(|hex| hex.distance_to(e.pos) == 1)
            })
            .map(|e| e.id)
            .collect()
    }

    /// Armies that may pile into a battle at `at` alongside `principal`.
    ///
    /// Attackers must still have their move in hand, since joining an
    /// assault is what they spend their turn on. Defenders answer whatever
    /// they have been up to: holding ground where you already stand is not
    /// a separate action.
    pub fn reinforcement_candidates(
        &self,
        at: Hex,
        side: u8,
        principal: ArmyId,
        attacking: bool,
    ) -> Vec<ArmyId> {
        self.armies
            .iter()
            .filter(|a| a.alive && a.side == side && a.id != principal)
            .filter(|a| a.pos.distance_to(at) <= 1)
            .filter(|a| !attacking || !a.moved)
            .map(|a| a.id)
            .collect()
    }

    /// Spend the turn of every army committed to a battle.
    pub fn commit_to_battle(&mut self, armies: &[ArmyId]) {
        for id in armies {
            if let Some(army) = self.army_mut(*id) {
                army.moved = true;
            }
        }
    }

    pub fn apply(
        &mut self,
        registry: &DataRegistry,
        order: &OverworldOrder,
    ) -> Result<Vec<OverworldEvent>, OverworldError> {
        if self.over.is_some() {
            return Err(OverworldError::GameOver);
        }
        let mut events = match order {
            OverworldOrder::MoveArmy { army, to } => self.apply_move(registry, *army, *to)?,
            OverworldOrder::EndTurn => self.apply_end_turn(registry),
        };
        self.check_victory(&mut events);
        Ok(events)
    }

    fn apply_move(
        &mut self,
        registry: &DataRegistry,
        id: ArmyId,
        to: Hex,
    ) -> Result<Vec<OverworldEvent>, OverworldError> {
        let (side, pos, movement, moved) = {
            let army = self.army(id).ok_or(OverworldError::NoSuchArmy)?;
            (army.side, army.pos, army.movement, army.moved)
        };
        if side != self.active_side {
            return Err(OverworldError::NotYourTurn);
        }
        if moved {
            return Err(OverworldError::AlreadyMoved);
        }

        let path = hexx::algorithms::a_star(pos, to, |from, next| {
            if from == next {
                return Some(0);
            }
            // Armies may not path through any other army; ending on an
            // enemy is the attack case, handled below.
            if next != to && self.army_at(next).is_some() {
                return None;
            }
            self.edge_cost(registry, from, next)
        })
        .ok_or(OverworldError::NoPath)?;

        // Trim the path to this turn's movement budget.
        let mut budget = movement;
        let mut walked = vec![pos];
        for pair in path.windows(2) {
            let Some(step) = self.edge_cost(registry, pair[0], pair[1]) else {
                break;
            };
            if step > budget {
                break;
            }
            // Stop short of any occupied tile; battle triggers if hostile.
            if self.army_at(pair[1]).is_some() {
                break;
            }
            budget -= step;
            walked.push(pair[1]);
        }
        let destination = *walked.last().expect("path starts at pos");

        let mut events = Vec::new();
        {
            let army = self.army_mut(id).expect("checked above");
            army.pos = destination;
            army.moved = true;
        }
        events.push(OverworldEvent::ArmyMoved {
            army: id,
            path: walked,
        });

        // Capture objectives by standing on them.
        if let Some(tile) = self.map.get(destination)
            && registry
                .terrain(&tile.terrain)
                .is_some_and(|t| t.capturable)
            && self.owners.get(&destination) != Some(&side)
        {
            self.owners.insert(destination, side);
            events.push(OverworldEvent::ObjectiveCaptured {
                at: destination,
                side,
            });
        }

        // If we stopped adjacent to the ordered destination because an
        // enemy holds it, that's an attack.
        if let Some(defender) = self.army_at(to)
            && defender.side != side
            && destination.distance_to(to) == 1
        {
            events.push(OverworldEvent::BattleTriggered {
                attacker: id,
                defender: defender.id,
                at: to,
            });
        }
        Ok(events)
    }

    fn apply_end_turn(&mut self, registry: &DataRegistry) -> Vec<OverworldEvent> {
        let side_count = self.sides.len() as u8;
        let mut next = self.active_side;
        for _ in 0..side_count {
            next = (next + 1) % side_count;
            if self.side_armies(next).next().is_some() {
                break;
            }
        }
        if next <= self.active_side {
            self.turn += 1;
        }
        self.active_side = next;
        for army in self.armies.iter_mut().filter(|a| a.alive && a.side == next) {
            army.moved = false;
        }

        let mut events = vec![OverworldEvent::TurnStarted {
            side: next,
            turn: self.turn,
        }];
        let amount: i32 = self
            .owners
            .iter()
            .filter(|(_, owner)| **owner == next)
            .filter_map(|(hex, _)| self.map.get(*hex))
            .filter_map(|tile| registry.terrain(&tile.terrain))
            .map(|t| t.income)
            .sum();
        if amount > 0 {
            self.sides[next as usize].funds += amount;
            events.push(OverworldEvent::Income { side: next, amount });
        }
        events
    }

    /// Feed a battle outcome back into the strategic layer. Every
    /// participating army gets its surviving roster back; armies that lost
    /// everything are destroyed, and a victorious attacker advances onto the
    /// contested tile.
    pub fn apply_battle_result(
        &mut self,
        attacker: ArmyId,
        defender: ArmyId,
        survivors: &[(ArmyId, Vec<UnitPlacement>)],
    ) -> Vec<OverworldEvent> {
        let mut events = Vec::new();
        let defender_pos = self.army(defender).map(|a| a.pos);

        for (id, units) in survivors {
            let id = *id;
            if let Some(army) = self.army_mut(id) {
                army.units = units.clone();
                if army.units.is_empty() {
                    army.alive = false;
                    events.push(OverworldEvent::ArmyDestroyed { army: id });
                }
            }
        }

        if let (Some(pos), Some(att)) = (defender_pos, self.army(attacker))
            && self.army(defender).is_none()
        {
            let att_id = att.id;
            if let Some(a) = self.army_mut(att_id) {
                a.pos = pos;
            }
        }
        self.check_victory(&mut events);
        events
    }

    fn check_victory(&mut self, events: &mut Vec<OverworldEvent>) {
        if self.over.is_some() {
            return;
        }
        let mut living: Vec<u8> = self
            .armies
            .iter()
            .filter(|a| a.alive)
            .map(|a| a.side)
            .collect();
        living.sort_unstable();
        living.dedup();
        if living.len() <= 1 {
            let winner = living.first().copied();
            self.over = Some(winner);
            events.push(OverworldEvent::GameEnded { winner });
        }
    }
}

/// Which nearby armies an AI side throws into a battle. Concentration of
/// force is almost always right here -- a battle is fought to the death, so
/// arriving outnumbered is the main way to lose one -- but this is a
/// separate function so smarter (or more cowardly) doctrines can replace it.
pub fn ai_reinforcements(
    state: &OverworldState,
    at: Hex,
    side: u8,
    principal: ArmyId,
    attacking: bool,
) -> Vec<ArmyId> {
    state.reinforcement_candidates(at, side, principal, attacking)
}

/// Baseline overworld AI: push each army toward the most valuable visible
/// target (weak enemy armies and uncaptured objectives).
pub struct SimpleOverworldPlanner {
    rng: ChaCha8Rng,
    /// Chance to pick a suboptimal target, derived from difficulty.
    blunder: f64,
}

impl SimpleOverworldPlanner {
    pub fn with_difficulty(difficulty: u8, seed: u64) -> Self {
        Self {
            rng: ChaCha8Rng::seed_from_u64(seed),
            blunder: match difficulty {
                1 => 0.5,
                2 => 0.3,
                3 => 0.15,
                4 => 0.05,
                _ => 0.0,
            },
        }
    }
}

pub fn make_overworld_planner(
    config: &AiConfig,
    seed: u64,
) -> Box<dyn AiPlanner<OverworldState, OverworldOrder>> {
    Box::new(SimpleOverworldPlanner::with_difficulty(
        config.difficulty.clamp(1, 5),
        seed,
    ))
}

impl AiPlanner<OverworldState, OverworldOrder> for SimpleOverworldPlanner {
    fn next_order(
        &mut self,
        registry: &DataRegistry,
        state: &OverworldState,
        side: u8,
    ) -> OverworldOrder {
        let Some(army) = state.side_armies(side).find(|a| !a.moved) else {
            return OverworldOrder::EndTurn;
        };

        let mut targets: Vec<(Hex, f32)> = Vec::new();
        // Enemy armies we can see: value inversely proportional to size.
        for enemy in state.visible_armies(registry, side) {
            if enemy.side != side {
                let strength = enemy.units.len().max(1) as f32;
                let ours = army.units.len().max(1) as f32;
                targets.push((enemy.pos, 6.0 * (ours / strength)));
            }
        }
        // Objectives we don't own.
        for (hex, tile) in state.map.iter() {
            let Some(t) = registry.terrain(&tile.terrain) else {
                continue;
            };
            if t.capturable && state.owners.get(&hex) != Some(&side) {
                targets.push((hex, 4.0 + t.income as f32 * 0.5));
            }
        }
        if targets.is_empty() {
            return OverworldOrder::EndTurn;
        }

        targets.sort_by(|a, b| {
            let da = army.pos.distance_to(a.0) as f32 - a.1;
            let db = army.pos.distance_to(b.0) as f32 - b.1;
            da.total_cmp(&db)
        });
        let pick = if targets.len() > 1 && self.rng.random_bool(self.blunder) {
            targets[1..].choose(&mut self.rng).copied()
        } else {
            None
        };
        let (dest, _) = pick.unwrap_or(targets[0]);
        OverworldOrder::MoveArmy {
            army: army.id,
            to: dest,
        }
    }
}
