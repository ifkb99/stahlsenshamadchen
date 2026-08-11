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
mod command;
mod fog;
mod movement;
mod orders;

pub use combat::{
    AttackPreview, CounterPreview, HitBreakdown, HitFactor, HitModifier, MAX_HIT, MIN_HIT,
    expected_damage, hit_breakdown, hit_chance, preview_attack, struck_facing, weapon_ready,
};
pub use command::{
    CommandState, Contact, CutOff, Formation, FormationId, Mission, MissionChange, WaitingOrders,
    nearest_exit,
};
pub use fog::{FogMap, SideFog, SightGrid, los_clear, unit_vision};
pub use movement::{
    destination_blocked, edge_cost as movement_edge_cost, move_points, path_to, reachable,
};
pub use orders::{Event, FireIntent, Order, OrderError, UnitIntent};

use crate::ai::AiConfig;
use crate::data::{DataError, DataRegistry, ValidationReport};
use crate::map::{HexMap, MapKind, UnitPlacement};
use crate::roster::{GirlId, Roster};
use hexx::{EdgeDirection, Hex};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Stable handle to a unit. Units are never removed from the roster, only
/// marked dead, so ids stay valid for the whole battle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct UnitId(pub u32);

impl UnitId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// One side (faction) in a battle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideState {
    pub name: String,
    /// `None` = human controlled.
    pub ai: Option<AiConfig>,
}

/// Where a battle is in the plan/resolve cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    /// Sides are writing orders. Nothing on the board moves; one flag per
    /// side records who has finished.
    Planning { committed: Vec<bool> },
    /// Orders are playing out. `tick` counts from 0 to the scale's
    /// `ticks_per_round`.
    Resolving { tick: u32 },
}

/// A crewed vehicle on the battlefield.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unit {
    pub id: UnitId,
    pub side: u8,
    /// Vehicle definition id.
    pub vehicle: String,
    /// Who is riding in it. Handles into [`BattleState::roster`], not
    /// definition ids: a crew member is a person with a history, and the
    /// battle needs to be able to mark her.
    pub crew: Vec<GirlId>,
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
    /// Damage type of the last hit this unit took, if any. Read by the
    /// campaign when working out what became of the crew.
    pub last_hit_by: Option<crate::data::DamageType>,
    /// How much this crew has had to take. Walks them up the morale ladder;
    /// shed a little at the end of every round.
    pub pressure: u32,
    /// Under the commander's personal tasking, and therefore excused from
    /// her formation's standing mission until recalled.
    ///
    /// Set when a direct radioed order reaches her, cleared by an explicit
    /// recall (`ClearIntent`) or by a NEW mission being set for her
    /// formation — a fresh formation order collects everyone. This is what
    /// makes a hand-placed vehicle stay where her commander put her instead
    /// of drifting back to the mission's axis at the next planning phase,
    /// which the first playtest rightly read as the game overriding the
    /// player. Detached is not idle: the battle drill still applies, and
    /// she still shoots on her arc.
    #[serde(default)]
    pub detached: bool,
    pub alive: bool,
    /// This vehicle drove off the map by an exit objective.
    ///
    /// Off the board — so `alive` is false and nothing can see it, shoot it
    /// or be blocked by it — but emphatically *not* destroyed: the campaign
    /// counts it among the survivors and its crew walk home. Everything that
    /// classifies a unit at the end of a battle has to ask this before it
    /// reads `alive`, or a successful withdrawal is recorded as a massacre.
    #[serde(default)]
    pub exited: bool,
}

impl Unit {
    /// Where this unit's orders will leave it, or where it stands if it has
    /// nowhere to go.
    pub fn planned_destination(&self) -> Hex {
        self.intent.path.last().copied().unwrap_or(self.pos)
    }
}

/// Why a battle stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndReason {
    /// One side (or every side) was wiped out.
    Eliminated,
    /// The sides lost each other: [`STALEMATE_ROUNDS`] rounds passed with no
    /// damage dealt and nobody holding an enemy in sight, so both disengage
    /// with whatever they have left. Without this, survivors who lose
    /// contact in the fog wander until they happen to collide — hundreds of
    /// rounds, with the campaign stuck behind them.
    ///
    /// Note this no longer implies a draw. Breaking contact is how the
    /// *shooting* stops; who won is then read off the objectives, and only a
    /// level score is honestly nobody's battle.
    Stalemate,
    /// A side reached the map's `victory_score`. The ground was worth more
    /// than the enemy's tanks, which is the whole point of writing an
    /// objective down.
    Objectives,
    /// A side lost a formation its map declared it could not afford to lose:
    /// the commanding officer named in a [`crate::map::LossCondition`] is
    /// dead, or the formation carrying the scenario is gone. Only a map that
    /// wrote the condition down can end this way.
    Decapitated,
}

/// Rounds without contact before the battle is called off. Contact means a
/// hit landed or some side can see an enemy, so a long careful approach
/// under observation is not mistaken for a stalemate.
pub const STALEMATE_ROUNDS: u32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BattleResult {
    /// `None` means a draw: either mutual destruction or a stalemate.
    pub winner: Option<u8>,
    pub reason: EndReason,
}

/// The full battle simulation state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BattleState {
    pub map: Arc<HexMap>,
    /// Sight heights for every tile, resolved once from the map and the
    /// registry. Shared rather than recomputed because line of sight is the
    /// hottest thing the simulation does and terrain never changes during a
    /// battle. `Arc` keeps state cloning — which search planners do
    /// constantly — cheap.
    pub sight: Arc<SightGrid>,
    pub sides: Vec<SideState>,
    /// The girls crewing the vehicles in this battle.
    ///
    /// Shared behind an `Arc` for the same reason the map is: search planners
    /// clone the whole state constantly and nothing in a battle rewrites the
    /// roster in place. Campaign battles are handed the campaign's roster, so
    /// the girls who fight are the same objects that carry their scars out
    /// again; scenario battles get one stamped from mod data on the spot.
    pub roster: Arc<Roster>,
    pub units: Vec<Unit>,
    pub round: u32,
    pub phase: Phase,
    pub fog: FogMap,
    pub rng: ChaCha8Rng,
    pub over: Option<BattleResult>,
    /// Round in which the sides were last in contact, for the stalemate
    /// check.
    pub last_contact_round: u32,
    /// Who currently holds each of `map.objectives()`, by the same index.
    ///
    /// Parallel to the map's list rather than keyed by id because the two are
    /// built together in one place and neither ever changes length, and
    /// because an index cannot disagree about ordering the way a hash map
    /// could — objectives are walked to produce events, so their order is
    /// part of determinism.
    ///
    /// Control persists: a side that takes the bridge and drives on still
    /// holds it until an enemy stands on it. Ground you have taken should
    /// have to be taken back.
    ///
    /// Defaulted on load so a save written before objectives existed opens as
    /// what it was — a battle with no ground worth taking. `save::rehydrate`
    /// then sizes it against the map, because scoring indexes through it.
    #[serde(default)]
    pub objective_held: Vec<Option<u8>>,
    /// Objective points each side has collected, indexed by side.
    #[serde(default)]
    pub score: Vec<u32>,
    /// Who answers to whom: the map's formations resolved against the units
    /// that actually spawned.
    ///
    /// Inert as of this chunk — populated, saved, and read by nothing that
    /// makes a decision. `#[serde(default)]` so a save written before the
    /// chain of command existed opens as what it was: one flat pool per side.
    #[serde(default)]
    pub command: CommandState,
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
        let objective_count = map.objectives().len();
        let sight = Arc::new(SightGrid::build(registry, &map));
        // Resolved before the map is moved into its `Arc`, and from the same
        // two things the units are spawned from, so membership cannot drift
        // from the roster it describes.
        let command = CommandState::from_placements(map.formations(), &file.units);
        // A scenario battle has no campaign behind it, so its girls are
        // stamped fresh from mod data and forgotten afterwards.
        let (roster, crews) = Roster::stamp_for(registry, &file.units);
        let mut state = Self {
            map: Arc::new(map),
            sight,
            sides,
            roster: Arc::new(roster),
            units: Vec::new(),
            round: 1,
            phase: Phase::Planning {
                committed: vec![false; side_count],
            },
            fog: FogMap::default(),
            rng: ChaCha8Rng::seed_from_u64(seed),
            over: None,
            last_contact_round: 1,
            objective_held: vec![None; objective_count],
            score: vec![0; side_count],
            command,
        };
        for (placement, crew) in file.units.iter().zip(&crews) {
            state.spawn_unit(registry, placement, crew.clone());
        }
        state.face_units_at_enemies(&file.units);
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
        crews: &[Vec<GirlId>],
        roster: Arc<Roster>,
        seed: u64,
    ) -> Self {
        let side_count = sides.len();
        let objective_count = map.objectives().len();
        let sight = Arc::new(SightGrid::build(registry, &map));
        // The formations travel on the map for exactly this reason: a field
        // battle the overworld assembles picks them up without this signature
        // growing, the same trip objectives already make. A declaration whose
        // members are not among these placements is dropped rather than
        // carried empty — see `CommandState::from_placements`.
        let command = CommandState::from_placements(map.formations(), placements);
        let mut state = Self {
            map: Arc::new(map),
            sight,
            sides,
            roster,
            units: Vec::new(),
            round: 1,
            phase: Phase::Planning {
                committed: vec![false; side_count],
            },
            fog: FogMap::default(),
            rng: ChaCha8Rng::seed_from_u64(seed),
            over: None,
            last_contact_round: 1,
            objective_held: vec![None; objective_count],
            score: vec![0; side_count],
            command,
        };
        for (i, placement) in placements.iter().enumerate() {
            state.spawn_unit(
                registry,
                placement,
                crews.get(i).cloned().unwrap_or_default(),
            );
        }
        state.face_units_at_enemies(placements);
        state.fog = FogMap::new(state.sides.len());
        fog::recompute(registry, &mut state);
        state
    }

    /// Turn every unit that was not given an explicit facing towards the enemy.
    ///
    /// Units used to spawn facing due east unconditionally, which on a map
    /// where the sides deploy east and west meant the eastern side presented
    /// its *rear* armour until it happened to move or fire. That is a real
    /// asymmetry — a Panther is armour 5 from the front and 2 from behind, and
    /// the damage formula divides by it — and it fell on one side only.
    ///
    /// This runs after every unit exists, because spawning one placement at a
    /// time cannot know where the other side ended up. Enemy centroids are
    /// averaged in floating point and rounded through [`Hex::round`], the same
    /// way [`crate::map::HexMap::center`] does, so the result respects the cube
    /// constraint and does not depend on iteration order.
    fn face_units_at_enemies(&mut self, placements: &[UnitPlacement]) {
        // Resolved for every side up front, so the mutation pass below does not
        // borrow the unit list while writing to it.
        let centroids: Vec<Option<Hex>> = (0..self.sides.len() as u8)
            .map(|side| {
                let (mut sx, mut sy, mut n) = (0i64, 0i64, 0i64);
                for unit in self.units.iter().filter(|u| u.side != side) {
                    sx += unit.pos.x as i64;
                    sy += unit.pos.y as i64;
                    n += 1;
                }
                (n > 0).then(|| Hex::round([sx as f32 / n as f32, sy as f32 / n as f32]))
            })
            .collect();

        // `placements` lines up with `units` by index: both setup paths spawn
        // in placement order into an empty unit list.
        for i in 0..self.units.len() {
            if placements.get(i).is_some_and(|p| p.facing.is_some()) {
                continue;
            }
            let unit = &self.units[i];
            let Some(target) = centroids.get(unit.side as usize).copied().flatten() else {
                continue;
            };
            // A unit standing exactly on the enemy centroid has no direction to
            // face; leave it as it is rather than picking one arbitrarily.
            if target == unit.pos {
                continue;
            }
            let facing = unit.pos.main_direction_to(target);
            self.units[i].facing = facing;
        }
    }

    pub fn spawn_unit(
        &mut self,
        registry: &DataRegistry,
        placement: &UnitPlacement,
        crew: Vec<GirlId>,
    ) -> UnitId {
        let id = UnitId(self.units.len() as u32);
        let vehicle = registry
            .vehicle(&placement.vehicle)
            .expect("placement validated against registry");
        let name = placement
            .name
            .clone()
            .or_else(|| {
                crew.first()
                    .and_then(|id| self.roster.get(*id))
                    .map(|g| g.name.clone())
            })
            .unwrap_or_else(|| vehicle.name.clone());
        self.units.push(Unit {
            id,
            side: placement.side,
            vehicle: placement.vehicle.clone(),
            crew,
            name,
            pos: crate::offset_to_hex(placement.at[0], placement.at[1]),
            // An explicit facing wins; otherwise this is a placeholder that
            // `face_units_at_enemies` replaces once every unit exists, since
            // spawning one at a time cannot know where the other side is.
            facing: placement
                .facing
                .map(EdgeDirection::from)
                .unwrap_or(EdgeDirection::POINTY_EAST),
            hp: vehicle.max_hp,
            intent: UnitIntent::default(),
            planned: false,
            move_credit: 0,
            cooldowns: vec![0; vehicle.weapons.len()],
            last_hit_by: None,
            pressure: 0,
            detached: false,
            alive: true,
            exited: false,
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

    /// Which rung of the morale ladder a unit is standing on.
    ///
    /// Derived from pressure rather than stored, so it cannot go stale and so
    /// a mod that changes the ladder changes every unit at once.
    pub fn morale<'r>(
        &self,
        registry: &'r DataRegistry,
        unit: &Unit,
    ) -> &'r crate::data::MoraleRung {
        registry.morale.rung(unit.pressure)
    }

    /// Whether this crew will still do as it is told.
    pub fn obeys(&self, registry: &DataRegistry, unit: &Unit) -> bool {
        self.morale(registry, unit).obeys
    }

    /// The terrain a unit is standing on, for checks that care where they
    /// happen — a lead foot is quick on a road and bogs in a field.
    pub fn terrain_at(&self, hex: Hex) -> Option<&str> {
        self.map.get(hex).map(|t| t.terrain.as_str())
    }

    pub fn alive_units(&self) -> impl Iterator<Item = &Unit> {
        self.units.iter().filter(|u| u.alive)
    }

    pub fn side_units(&self, side: u8) -> impl Iterator<Item = &Unit> {
        self.alive_units().filter(move |u| u.side == side)
    }

    /// Everyone who came through the battle: still on the board, or driven
    /// off it by an exit.
    ///
    /// Distinct from [`Self::alive_units`] on purpose. `alive` answers "is
    /// this on the battlefield", which is what targeting, movement and fog
    /// want; this answers "did she come home", which is what the campaign
    /// wants. Reading `alive` for the second question records a successful
    /// withdrawal as a burned-out vehicle and a dead crew.
    pub fn surviving_units(&self) -> impl Iterator<Item = &Unit> {
        self.units.iter().filter(|u| u.alive || u.exited)
    }

    /// Vehicles actually destroyed — the complement of
    /// [`Self::surviving_units`], and never merely `!alive`.
    pub fn lost_units(&self) -> impl Iterator<Item = &Unit> {
        self.units.iter().filter(|u| !u.alive && !u.exited)
    }

    pub fn is_over(&self) -> bool {
        self.over.is_some()
    }

    /// Every objective on this map paired with the side currently holding it.
    pub fn objectives(&self) -> impl Iterator<Item = (&crate::map::Objective, Option<u8>)> {
        self.map
            .objectives()
            .iter()
            .enumerate()
            .map(|(i, o)| (o, self.objective_held.get(i).copied().flatten()))
    }

    /// Every formation in this battle, in the order its map declared them.
    ///
    /// Empty on a map that declares none, which is one flat pool per side and
    /// exactly the game this engine played before the chain of command.
    pub fn formations(&self) -> &[Formation] {
        self.command.formations()
    }

    /// The formation a unit answers to, if it is in one.
    pub fn formation_of(&self, unit: UnitId) -> Option<&Formation> {
        self.command.formation_of(unit)
    }

    /// Objective points a side has collected so far.
    pub fn score(&self, side: u8) -> u32 {
        self.score.get(side as usize).copied().unwrap_or(0)
    }

    /// The side ahead on objectives, if exactly one is. `None` covers both
    /// "level" and "this map has no objectives", which are the same answer to
    /// the only question callers ask: can the points decide this?
    pub fn leader(&self) -> Option<u8> {
        let best = self.score.iter().copied().max()?;
        if best == 0 {
            return None;
        }
        let mut leaders = self.score.iter().enumerate().filter(|(_, s)| **s == best);
        let (side, _) = leaders.next()?;
        leaders.next().is_none().then_some(side as u8)
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
            Phase::Planning { committed } => committed.get(side as usize).copied().unwrap_or(false),
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

    /// How well this crew lays its gun.
    ///
    /// Skill ids are data, so the strings here are the engine's contract with
    /// the base mod rather than magic numbers: a mod that renames `gunnery` is
    /// defining a different game. `terrain` is where the check is happening,
    /// which traits may care about.
    pub fn gunnery(
        registry: &DataRegistry,
        roster: &Roster,
        unit: &Unit,
        terrain: Option<&str>,
    ) -> i32 {
        roster.crew_skill(
            registry,
            registry.vehicle(&unit.vehicle),
            &unit.crew,
            "gunnery",
            terrain,
        )
    }

    /// Vision range in hexes: vehicle base, scaled by how well the crew
    /// observes. Spotting is a trained skill rather than a fact about
    /// eyesight, which is why Elsa's "sees everything" is a high Perception
    /// carrying an untrained `observation`.
    pub fn vision_range(
        registry: &DataRegistry,
        roster: &Roster,
        unit: &Unit,
        terrain: Option<&str>,
    ) -> u32 {
        let base = registry
            .vehicle(&unit.vehicle)
            .map(|v| v.vision_range)
            .unwrap_or(3);
        registry.balance.vision(
            base,
            roster.crew_skill(
                registry,
                registry.vehicle(&unit.vehicle),
                &unit.crew,
                "observation",
                terrain,
            ),
        )
    }

    /// Ticks this crew waits before acting on its orders.
    ///
    /// The round is twelve ticks and the orders were given before it started,
    /// so this is where "she was told" and "she did it" come apart. A quick
    /// crew is moving almost at once; a slow one is still getting going while
    /// the round happens around them.
    ///
    /// Zero for everyone when a mod says so, which is how difficulty is
    /// turned down without a branch in here.
    pub fn reaction_delay(
        registry: &DataRegistry,
        roster: &Roster,
        unit: &Unit,
        terrain: Option<&str>,
    ) -> u32 {
        let rules = &registry.reaction;
        let level = roster.crew_skill(
            registry,
            registry.vehicle(&unit.vehicle),
            &unit.crew,
            &rules.skill,
            terrain,
        );
        rules.delay(level)
    }

    /// Driving skill used by [`super::move_points`].
    pub fn driving(
        registry: &DataRegistry,
        roster: &Roster,
        unit: &Unit,
        terrain: Option<&str>,
    ) -> i32 {
        roster.crew_skill(
            registry,
            registry.vehicle(&unit.vehicle),
            &unit.crew,
            "driving",
            terrain,
        )
    }
}
