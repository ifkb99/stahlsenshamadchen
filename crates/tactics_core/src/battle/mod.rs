//! The turn-based battle simulation.
//!
//! The single mutation entry point is [`BattleState::apply`]: orders go in,
//! a list of [`Event`]s comes out. The presentation layer animates events;
//! AI planners and campaign scripts consume the same stream. Cloning a
//! [`BattleState`] yields an independent simulation, which is what
//! search-based planners branch on.

mod combat;
mod fog;
mod movement;
mod orders;

pub use combat::{
    expected_damage, hit_breakdown, hit_chance, preview_attack, AttackPreview, CounterPreview,
    HitBreakdown, HitFactor, HitModifier, MAX_HIT, MIN_HIT,
};
pub use fog::{los_clear, unit_vision, FogMap, SideFog};
pub use movement::{edge_cost as movement_edge_cost, move_points, path_to, reachable};
pub use orders::{Event, Order, OrderError};

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
    pub moved: bool,
    pub acted: bool,
    pub alive: bool,
}

/// Why a battle stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// One side (or every side) was wiped out.
    Eliminated,
    /// The sides lost each other: [`STALEMATE_TURNS`] rounds passed with no
    /// damage dealt and nobody holding an enemy in sight, so both disengage
    /// with whatever they have left. Without this, survivors who lose
    /// contact in the fog wander until they happen to collide — hundreds of
    /// rounds, with the campaign stuck behind them.
    Stalemate,
}

/// Rounds without contact before the battle is called off. Contact means a
/// hit landed or some side can see an enemy, so a long careful approach
/// under observation is not mistaken for a stalemate.
pub const STALEMATE_TURNS: u32 = 8;

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
    pub turn: u32,
    pub active_side: u8,
    pub fog: FogMap,
    pub rng: ChaCha8Rng,
    pub over: Option<BattleResult>,
    /// Round in which the sides were last in contact, for the stalemate
    /// check.
    pub last_contact_turn: u32,
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
        let sides = file
            .sides
            .iter()
            .map(|s| SideState {
                name: s.name.clone(),
                ai: s.ai.clone(),
            })
            .collect();
        let mut state = Self {
            map: Arc::new(map),
            sides,
            units: Vec::new(),
            turn: 1,
            active_side: 0,
            fog: FogMap::default(),
            rng: ChaCha8Rng::seed_from_u64(seed),
            over: None,
            last_contact_turn: 1,
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
        let mut state = Self {
            map: Arc::new(map),
            sides,
            units: Vec::new(),
            turn: 1,
            active_side: 0,
            fog: FogMap::default(),
            rng: ChaCha8Rng::seed_from_u64(seed),
            over: None,
            last_contact_turn: 1,
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
            moved: false,
            acted: false,
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
}

/// Derived stats: crew quality modifies vehicle hardware.
pub mod stats {
    use super::*;

    /// Best gunnery among the crew.
    pub fn gunnery(registry: &DataRegistry, unit: &Unit) -> i32 {
        crew_best(registry, unit, |s| s.gunnery)
    }

    /// Vision range in hexes: vehicle base + awareness bonus.
    pub fn vision_range(registry: &DataRegistry, unit: &Unit) -> u32 {
        let base = registry
            .vehicle(&unit.vehicle)
            .map(|v| v.vision_range)
            .unwrap_or(3);
        let bonus = crew_best(registry, unit, |s| s.awareness) / 4;
        (base as i32 + bonus).max(1) as u32
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
