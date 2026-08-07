//! The simultaneous (WEGO) battle simulation.
//!
//! A round has two halves. During [`Phase::Planning`] every side sets
//! intents for its units — where to drive, what to engage — and nothing on
//! the board moves. Once all sides [`Order::Commit`], the round resolves in
//! [`crate::data::Scale::ticks_per_round`] ticks during which everyone moves
//! and shoots at once. How long a round and a tick *are* is mod data, not a
//! constant, which is why almost everything here takes a registry.
//!
//! There are two mutation entry points. [`BattleState::apply`] takes orders
//! (intents and commits) and returns [`Event`]s; [`BattleState::step_tick`]
//! advances resolution by one tick and returns the events it produced.
//! Stepping one tick at a time is what lets the presentation layer animate a
//! round without the simulation racing ahead of the sprites; headless callers
//! can use [`BattleState::resolve_round`] to run the whole round at once.
//!
//! Cloning a [`BattleState`] yields an independent simulation, which is what
//! search-based planners branch on.

mod combat;
mod fog;
mod movement;
mod orders;

pub use combat::{
    expected_damage, hit_breakdown, hit_chance, preview_attack, weapon_ready, AttackPreview,
    CounterPreview, HitBreakdown, HitFactor, HitModifier, MAX_HIT, MIN_HIT,
};
pub use fog::{los_clear, unit_vision, FogMap, SideFog};
pub use movement::{
    destination_blocked, edge_cost as movement_edge_cost, move_points, path_to, reachable,
};
pub use orders::{Event, FireIntent, Order, OrderError, UnitIntent};

use crate::ai::AiConfig;
use crate::data::{DataRegistry, DataError, ValidationReport};
use crate::map::{HexMap, MapKind, UnitPlacement};
use hexx::{EdgeDirection, Hex};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use std::sync::Arc;

/// Stable handle to a unit. Units are never removed from the roster, only
/// marked dead, so ids stay valid for the whole battle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct UnitId(pub u32);

impl UnitId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// One side (faction) in a battle.
#[derive(Debug, Clone)]
pub struct SideState {
    pub name: String,
    /// `None` = human controlled.
    pub ai: Option<AiConfig>,
}

/// Where a battle is in the plan/resolve cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Sides are writing orders. Nothing on the board moves; one flag per
    /// side records who has finished.
    Planning { committed: Vec<bool> },
    /// Orders are playing out. `tick` counts from 0 to the scale's
    /// `ticks_per_round`.
    Resolving { tick: u32 },
}

/// A crewed vehicle on the battlefield.
#[derive(Debug, Clone)]
pub struct Unit {
    pub id: UnitId,
    pub side: u8,
    /// Vehicle definition id.
    pub vehicle: String,
    /// Character definition ids.
    pub crew: Vec<String>,
    /// Display name (commander name unless overridden by the scenario).
    pub name: String,
    pub pos: Hex,
    pub facing: EdgeDirection,
    pub hp: i32,
    /// What this unit was told to do this round.
    pub intent: UnitIntent,
    /// Whether anyone has given this unit orders this round. Distinct from
    /// an empty intent, which is the deliberate choice to sit still and
    /// watch.
    pub planned: bool,
    /// Movement accrued but not yet spent, in `cost * ticks_per_round`
    /// units. Integer so resolution stays bit-for-bit reproducible.
    pub move_credit: u32,
    /// Ticks until each weapon can fire again, indexed like the vehicle's
    /// weapon list. Carries across rounds, so a slow gun caught mid-reload
    /// stays mid-reload.
    pub cooldowns: Vec<u32>,
    pub alive: bool,
}

impl Unit {
    /// Where this unit's orders will leave it, or where it stands if it has
    /// nowhere to go.
    pub fn planned_destination(&self) -> Hex {
        self.intent.path.last().copied().unwrap_or(self.pos)
    }
}

/// Why a battle stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// One side (or every side) was wiped out.
    Eliminated,
    /// The sides lost each other: [`STALEMATE_ROUNDS`] rounds passed with no
    /// damage dealt and nobody holding an enemy in sight, so both disengage
    /// with whatever they have left. Without this, survivors who lose
    /// contact in the fog wander until they happen to collide — hundreds of
    /// rounds, with the campaign stuck behind them.
    Stalemate,
}

/// Rounds without contact before the battle is called off. Contact means a
/// hit landed or some side can see an enemy, so a long careful approach
/// under observation is not mistaken for a stalemate.
pub const STALEMATE_ROUNDS: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BattleResult {
    /// `None` means a draw: either mutual destruction or a stalemate.
    pub winner: Option<u8>,
    pub reason: EndReason,
}

/// The full battle simulation state.
#[derive(Debug, Clone)]
pub struct BattleState {
    pub map: Arc<HexMap>,
    pub sides: Vec<SideState>,
    pub units: Vec<Unit>,
    pub round: u32,
    pub phase: Phase,
    pub fog: FogMap,
    pub rng: ChaCha8Rng,
    pub over: Option<BattleResult>,
    /// Round in which the sides were last in contact, for the stalemate
    /// check.
    pub last_contact_round: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum BattleSetupError {
    #[error("map `{0}` not found in registry")]
    MissingMap(String),
    #[error("map `{0}` is not a battle map")]
    NotABattleMap(String),
    #[error(transparent)]
    Map(#[from] crate::map::MapError),
    #[error("battle setup data invalid: {0:?}")]
    Invalid(Vec<String>),
    #[error(transparent)]
    Data(#[from] DataError),
}

impl BattleState {
    /// Build a battle from a scenario map in the registry.
    pub fn from_map(
        registry: &DataRegistry,
        map_id: &str,
        seed: u64,
    ) -> Result<Self, BattleSetupError> {
        let file = registry
            .map(map_id)
            .ok_or_else(|| BattleSetupError::MissingMap(map_id.to_string()))?;
        if file.kind != MapKind::Battle {
            return Err(BattleSetupError::NotABattleMap(map_id.to_string()));
        }
        let mut report = ValidationReport::default();
        file.validate_into(registry, &mut report);
        if !report.is_ok() {
            return Err(BattleSetupError::Invalid(report.errors));
        }
        let map = HexMap::from_map_file(file)?;
        let sides: Vec<SideState> = file
            .sides
            .iter()
            .map(|s| SideState {
                name: s.name.clone(),
                ai: s.ai.clone(),
            })
            .collect();
        let side_count = sides.len();
        let mut state = Self {
            map: Arc::new(map),
            sides,
            units: Vec::new(),
            round: 1,
            phase: Phase::Planning {
                committed: vec![false; side_count],
            },
            fog: FogMap::default(),
            rng: ChaCha8Rng::seed_from_u64(seed),
            over: None,
            last_contact_round: 1,
        };
        for placement in &file.units {
            state.spawn_unit(registry, placement);
        }
        state.fog = FogMap::new(state.sides.len());
        fog::recompute(registry, &mut state);
        Ok(state)
    }

    /// Build a battle directly from placements (used by the overworld when
    /// two armies clash on a generated or terrain-picked map).
    pub fn from_placements(
        registry: &DataRegistry,
        map: HexMap,
        sides: Vec<SideState>,
        placements: &[UnitPlacement],
        seed: u64,
    ) -> Self {
        let side_count = sides.len();
        let mut state = Self {
            map: Arc::new(map),
            sides,
            units: Vec::new(),
            round: 1,
            phase: Phase::Planning {
                committed: vec![false; side_count],
            },
            fog: FogMap::default(),
            rng: ChaCha8Rng::seed_from_u64(seed),
            over: None,
            last_contact_round: 1,
        };
        for placement in placements {
            state.spawn_unit(registry, placement);
        }
        state.fog = FogMap::new(state.sides.len());
        fog::recompute(registry, &mut state);
        state
    }

    pub fn spawn_unit(&mut self, registry: &DataRegistry, placement: &UnitPlacement) -> UnitId {
        let id = UnitId(self.units.len() as u32);
        let vehicle = registry
            .vehicle(&placement.vehicle)
            .expect("placement validated against registry");
        let name = placement
            .name
            .clone()
            .or_else(|| {
                placement
                    .crew
                    .first()
                    .and_then(|c| registry.character(c))
                    .map(|c| c.name.clone())
            })
            .unwrap_or_else(|| vehicle.name.clone());
        self.units.push(Unit {
            id,
            side: placement.side,
            vehicle: placement.vehicle.clone(),
            crew: placement.crew.clone(),
            name,
            pos: crate::offset_to_hex(placement.at[0], placement.at[1]),
            facing: EdgeDirection::POINTY_EAST,
            hp: vehicle.max_hp,
            intent: UnitIntent::default(),
            planned: false,
            move_credit: 0,
            cooldowns: vec![0; vehicle.weapons.len()],
            alive: true,
        });
        id
    }

    pub fn unit(&self, id: UnitId) -> Option<&Unit> {
        self.units.get(id.index()).filter(|u| u.alive)
    }

    pub fn unit_mut(&mut self, id: UnitId) -> Option<&mut Unit> {
        self.units.get_mut(id.index()).filter(|u| u.alive)
    }

    pub fn unit_at(&self, hex: Hex) -> Option<&Unit> {
        self.units.iter().find(|u| u.alive && u.pos == hex)
    }

    /// The unit at `hex` if `side` may act on it as a target: an enemy its
    /// fog currently spots. Callers that would otherwise reach for
    /// [`Self::unit_at`] should prefer this, so an order refusal never
    /// betrays a unit the side cannot see.
    pub fn spotted_enemy_at(&self, hex: Hex, side: u8) -> Option<&Unit> {
        self.unit_at(hex)
            .filter(|u| u.side != side && self.fog.side(side).spotted.contains(&u.id))
    }

    pub fn alive_units(&self) -> impl Iterator<Item = &Unit> {
        self.units.iter().filter(|u| u.alive)
    }

    pub fn side_units(&self, side: u8) -> impl Iterator<Item = &Unit> {
        self.alive_units().filter(move |u| u.side == side)
    }

    pub fn is_over(&self) -> bool {
        self.over.is_some()
    }

    /// Sides that still have living units.
    pub fn living_sides(&self) -> Vec<u8> {
        let mut sides: Vec<u8> = self.alive_units().map(|u| u.side).collect();
        sides.sort_unstable();
        sides.dedup();
        sides
    }

    /// Whether sides are still writing orders.
    pub fn is_planning(&self) -> bool {
        matches!(self.phase, Phase::Planning { .. })
    }

    /// The tick being resolved, or `None` while planning.
    pub fn resolving_tick(&self) -> Option<u32> {
        match self.phase {
            Phase::Resolving { tick } => Some(tick),
            Phase::Planning { .. } => None,
        }
    }

    /// Whether `side` has finished writing orders this round.
    pub fn has_committed(&self, side: u8) -> bool {
        match &self.phase {
            Phase::Planning { committed } => {
                committed.get(side as usize).copied().unwrap_or(false)
            }
            // Resolution means everybody committed.
            Phase::Resolving { .. } => true,
        }
    }

    /// Units of `side` still waiting for orders this round.
    pub fn unplanned_units(&self, side: u8) -> impl Iterator<Item = &Unit> {
        self.side_units(side).filter(|u| !u.planned)
    }
}

/// Derived stats: crew quality modifies vehicle hardware.
///
/// Every bonus here is a percentage of the vehicle's own base, read from the
/// mod's [`crate::data::Balance`] block. The flat divisors these replaced
/// were tuned when a tank saw three hexes, and the scale decision quietly
/// devalued them to nothing; scaling against the base means retuning vision
/// or speed never silently retunes what a crew is worth again.
pub mod stats {
    use super::*;

    /// Best gunnery among the crew.
    pub fn gunnery(registry: &DataRegistry, unit: &Unit) -> i32 {
        crew_best(registry, unit, |s| s.gunnery)
    }

    /// Vision range in hexes: vehicle base scaled by the crew's awareness.
    pub fn vision_range(registry: &DataRegistry, unit: &Unit) -> u32 {
        let base = registry
            .vehicle(&unit.vehicle)
            .map(|v| v.vision_range)
            .unwrap_or(3);
        registry
            .balance
            .vision(base, crew_best(registry, unit, |s| s.awareness))
    }

    fn crew_best(registry: &DataRegistry, unit: &Unit, f: impl Fn(&crate::data::CrewStats) -> i32) -> i32 {
        unit.crew
            .iter()
            .filter_map(|c| registry.character(c))
            .map(|c| f(&c.stats))
            .max()
            .unwrap_or(0)
    }

    /// Driving bonus used by [`super::move_points`].
    pub fn driving(registry: &DataRegistry, unit: &Unit) -> i32 {
        crew_best(registry, unit, |s| s.driving)
    }
}
