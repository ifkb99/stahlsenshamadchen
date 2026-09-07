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
mod danger;
mod fog;
mod movement;
mod orders;

pub use combat::{
    AttackPreview, CounterPreview, HitBreakdown, HitFactor, HitModifier, ShellInFlight,
    best_weapon_from, blast_overmatches, expected_damage, flight_ticks, hit_breakdown, hit_chance,
    penetration_chance, penetration_share, preview_attack, struck_facing, weapon_ready,
};
pub use command::{
    CommandState, Contact, CutOff, Formation, FormationId, Goal, Latitude, Mission, MissionChange,
    WaitingOrders, nearest_exit,
};
pub use danger::{Bearing, fire_on, incoming};
pub use fog::{FogMap, SideFog, SightGrid, los_clear, unit_vision};
pub use movement::{
    MoveGrid, Roads, along_the_bearing, destination_blocked, edge_cost as movement_edge_cost,
    move_points, path_to, reachable, roads, step_toward,
};
pub use orders::{Event, FireIntent, Order, OrderError, UnitIntent};

use crate::ai::AiConfig;
use crate::data::{DataError, DataRegistry, ModuleEffect, ValidationReport};
use crate::map::{HexMap, MapKind, UnitPlacement};
use crate::roster::{CadetId, Roster};
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

/// How one cadet aboard a vehicle is doing, mid-battle.
///
/// Deliberately three states and not a hit-point bar: *wounded* is the
/// dramatic middle where she is still at her station and worse at it, and
/// *out* is deliberately ambiguous — dead or unconscious is a question the
/// battle cannot answer and the roster's fate machinery resolves when the
/// shooting stops, with worse odds from a vehicle that burned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrewCondition {
    #[default]
    Fine,
    /// Hurt but working her station, at the substitution penalty's worth of
    /// worse — being wounded is like doing somebody else's job.
    Wounded,
    /// No longer part of the fight. Whether she comes home is the roster's
    /// question, not the battle's.
    Out,
    /// She never got in. Wounded from a previous battle, or still walking
    /// back from one, and so not fit to deploy — her seat is on the roll and
    /// empty in the vehicle.
    ///
    /// Distinct from [`Self::Out`] in the one way that matters: an absent
    /// cadet is not a casualty of *this* battle. Nothing inside can hit her,
    /// her empty seat does not make the vehicle look half-destroyed, and she
    /// takes no fresh wound home. She is still listed in
    /// [`Unit::crew`] rather than filtered out of it, because the campaign
    /// hands the crew back at the end and a cadet dropped from the list here
    /// would be a cadet deleted from her tank forever.
    Absent,
}

impl CrewCondition {
    /// Whether she is still part of the fight — still somebody a shell can
    /// find, still somebody working a station. Wounded counts: she is at her
    /// post and worse at it, which is the whole point of the middle state.
    ///
    /// Named as a question rather than left as a match on the variant
    /// because [`Self::Out`] and [`Self::Absent`] both answer no for
    /// completely different reasons, and every caller that wanted "not Out"
    /// wanted this instead. [`BattleState::substance`] is deliberately the
    /// exception: it distinguishes all four, because a seat nobody is
    /// sitting in and a seat whose cadet has been hit are opposite kinds of
    /// nothing.
    pub fn fighting(self) -> bool {
        matches!(self, Self::Fine | Self::Wounded)
    }
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
    pub crew: Vec<CadetId>,
    /// Display name (commander name unless overridden by the scenario).
    pub name: String,
    pub pos: Hex,
    pub facing: EdgeDirection,
    /// What this unit was told to do this round.
    pub intent: UnitIntent,
    /// Whether anyone has given this unit orders this round. Distinct from
    /// an empty intent, which is the deliberate choice to sit still and
    /// watch.
    pub planned: bool,
    /// Movement accrued but not yet spent, in `cost * ticks_per_round`
    /// units. Integer so resolution stays bit-for-bit reproducible.
    pub move_credit: u32,
    /// Hexes crossed so far this round, zeroed when the next one is planned.
    ///
    /// The resolver knows [`Self::move_credit`], which is what she has left
    /// to spend, and that is not the same question: a vehicle parked all
    /// round and one that has just finished a four-hex dash can both be out
    /// of credit. This is what actually happened, and both ends of a shot
    /// read it — a crossing target is harder to hit, and a gun laid from a
    /// moving vehicle is harder to lay.
    ///
    /// On the unit rather than in the planner for the reason `goal` is:
    /// `tests/save.rs` forks a battle through a save file and requires the
    /// same future, which private planner memory would not survive. Zeroed
    /// in `begin_round`, so during the planning phase it reads zero for
    /// everybody — which is the truth, since nobody has driven yet.
    #[serde(default)]
    pub moved: u32,
    /// Ticks until each weapon can fire again, indexed like the vehicle's
    /// weapon list. Carries across rounds, so a slow gun caught mid-reload
    /// stays mid-reload.
    pub cooldowns: Vec<u32>,
    /// What is in her racks: [`crate::data::AmmoDef`] id to rounds remaining.
    ///
    /// Stamped from the vehicle's `stowage` at spawn, adjustable during
    /// deployment through [`BattleState::set_loadout`], and spent by firing —
    /// one round per shot, including blind shells into empty ground. The gun
    /// that empties its last listed round announces it and goes silent.
    ///
    /// A `BTreeMap` for the same reason the vehicle's `stowage` is one: the
    /// moment a shot deducts a round, these keys are walked to produce
    /// events, and hash order in an event stream is a determinism bug this
    /// project has already shipped once.
    ///
    /// `#[serde(default)]` so a save written before ammunition existed opens
    /// as what it was: crews who never counted their shells.
    #[serde(default)]
    pub ammo: std::collections::BTreeMap<String, u32>,
    /// What is still working inside her: [`crate::data::ModuleDef`] id to
    /// hits remaining before that module is destroyed.
    ///
    /// Stamped at spawn from what the vehicle carries — each module's
    /// `toughness`, so every entry starts at full — and counted down by the
    /// outcome engine, where a value of zero means destroyed rather than
    /// absent. Absent means the vehicle never had one, which is a different
    /// sentence entirely: a crew who lost their wireless set and a crew who
    /// never had one behave the same today and will not behave the same
    /// under a repair or resupply rule.
    ///
    /// The outcome engine counts these down; the gates that make them
    /// matter are [`Unit::module_ok`] (the gun and the radio) and
    /// [`Unit::mobility_halves`] (the tracks).
    ///
    /// A `BTreeMap` for the same reason [`Self::ammo`] is one: the moment a
    /// hit breaks something these keys are walked to produce events, and
    /// hash order in an event stream is a determinism bug this project has
    /// already shipped once and did not notice for months.
    ///
    /// `#[serde(default)]` so a save written before modules existed opens as
    /// what it was — a vehicle with nothing inside to lose but her crew.
    #[serde(default)]
    pub modules: std::collections::BTreeMap<String, u32>,
    /// How each cadet aboard is doing, index-aligned with [`Self::crew`] —
    /// a parallel `Vec` rather than a map because save files are JSON and
    /// the alignment is the invariant anyway: seat *k* of the crew list is
    /// entry *k* here, always.
    ///
    /// Empty means everyone is fine, which is both the spawn state and what
    /// a save written before wounds existed opens as. The outcome engine
    /// grows it to crew length the first time anyone is hurt.
    #[serde(default)]
    pub crew_state: Vec<CrewCondition>,
    /// The crew left her: a bail-out under fire, rolled through the same
    /// discipline check that governs refusing orders. The vehicle is a
    /// wreck as far as the battle is concerned — reaped like a kill,
    /// scored like a loss — but the cadets are walking home, which the
    /// campaign's fate machinery treats very differently from burning.
    #[serde(default)]
    pub abandoned: bool,
    /// The ammunition went up. Instantly destroyed, and the fate rolls for
    /// everyone aboard carry the fire.
    #[serde(default)]
    pub brewed: bool,
    /// The carrier this unit is riding in, when she is riding at all.
    ///
    /// Aboard, she is off the map in every sense that matters to the enemy:
    /// unspottable, untargetable, occupying no hex of her own (her `pos`
    /// mirrors the carrier's so nothing downstream reads a stale tile), and
    /// silent — she neither sees for her side nor shoots for it. What she
    /// keeps is her radio, her nerves, and her share of whatever comes
    /// through the carrier's armor.
    #[serde(default)]
    pub aboard: Option<UnitId>,
    /// A standing order to go and board this carrier: she marches toward it
    /// round after round — re-pathed from wherever she stands, like a
    /// personal tasking — and steps aboard the tick she arrives alongside.
    #[serde(default)]
    pub boarding: Option<UnitId>,
    /// She has been told to get off: executed at the next transport pass,
    /// onto the first free tile beside the carrier.
    #[serde(default)]
    pub dismounting: bool,
    /// Destroyed as a vehicle by catastrophic damage that was not a fire —
    /// blast overmatch flattening a soft skin, for now. Kept separate from
    /// [`Self::brewed`] because the campaign's fate rolls care about the
    /// difference between a crushed hull and a burning one.
    #[serde(default)]
    pub wrecked: bool,
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
    /// Where her commander's personal order is taking her, until she gets
    /// there. A destination, never a path: she re-paths from wherever she
    /// stands each round, marching across as many rounds as the ground
    /// demands — a movement order does not expire for being far away, it is
    /// executed until arrival. Cleared when she reaches it (she then holds
    /// there, still detached), when she is recalled, or when her formation
    /// is given fresh orders.
    #[serde(default)]
    pub tasking: Option<Hex>,
    /// What she has decided to do about it: her own goal, as opposed to her
    /// formation's mission or her commander's [`Self::tasking`].
    ///
    /// Kept across rounds, which is the whole of its value — a planner that
    /// re-decides where it is going every round is a planner that never gets
    /// anywhere, and measuring that is what put this here. Cleared when the
    /// goal finishes ([`Goal::finished`]) and wherever `tasking` clears,
    /// because fresh orders end her own errand too.
    #[serde(default)]
    pub goal: Option<crate::battle::Goal>,
    /// How hard her commander meant [`Self::tasking`]: whether the battle
    /// drill may set the march aside to keep her alive.
    ///
    /// Travels with the destination and is cleared with it, because latitude
    /// is a property of an order rather than of a crew — the same cadet is
    /// pressed on one ridge and given her head on the next.
    #[serde(default)]
    pub latitude: crate::battle::Latitude,
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
    /// Whether the module carrying this effect still works. A vehicle with
    /// no module of the effect at all answers `true`: content that never
    /// declared a gun module cannot have it shot out, which is the
    /// additivity rule — a mod without modules is today's game.
    pub fn module_ok(&self, registry: &DataRegistry, effect: ModuleEffect) -> bool {
        let mut declared = false;
        for (id, hits) in &self.modules {
            if registry.module(id).is_some_and(|m| m.effect == effect) {
                declared = true;
                if *hits > 0 {
                    return true;
                }
            }
        }
        !declared
    }

    /// What fraction of this unit's fighting bodies are still on their
    /// feet, as (have, total) over every [`ModuleEffect::Troops`] module
    /// aboard. `None` when the unit has no troops modules at all — a tank
    /// crew is not a fraction of anything, and every weapon it fires is
    /// fired at full weight. The three meanings the design doc promises
    /// hang off this one reading: interior weight is the module's own
    /// `size` (free), firepower scales by this fraction, and a remnant is
    /// simply this reaching zero while the cadets live.
    pub fn troops(&self, registry: &DataRegistry) -> Option<(u32, u32)> {
        let mut have = 0u32;
        let mut total = 0u32;
        let mut any = false;
        for (id, hits) in &self.modules {
            let Some(module) = registry.module(id) else {
                continue;
            };
            if module.effect != ModuleEffect::Troops {
                continue;
            }
            any = true;
            have += (*hits).min(module.toughness);
            total += module.toughness;
        }
        (any && total > 0).then_some((have, total))
    }

    /// How much of this vehicle's movement her running gear still delivers,
    /// in halves so the arithmetic stays integer: 2 intact, 1 damaged, 0
    /// destroyed. Reads the worst mobility module aboard, because one
    /// thrown track stops the tank however healthy the other is.
    pub fn mobility_halves(&self, registry: &DataRegistry) -> u32 {
        let mut halves = 2u32;
        for (id, hits) in &self.modules {
            let Some(module) = registry.module(id) else {
                continue;
            };
            if module.effect != ModuleEffect::Mobility {
                continue;
            }
            let own = if *hits == 0 {
                0
            } else if *hits < module.toughness {
                1
            } else {
                2
            };
            halves = halves.min(own);
        }
        halves
    }

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
    /// Movement costs for every tile, resolved once from the map and the
    /// registry. Shared for exactly the reasons [`Self::sight`] is: terrain
    /// never changes during a battle, the searches ask for it per edge of
    /// every tile they touch, and a planner clones the whole state to branch.
    pub moves: Arc<MoveGrid>,
    pub sides: Vec<SideState>,
    /// The cadets crewing the vehicles in this battle.
    ///
    /// Shared behind an `Arc` for the same reason the map is: search planners
    /// clone the whole state constantly and nothing in a battle rewrites the
    /// roster in place. Campaign battles are handed the campaign's roster, so
    /// the cadets who fight are the same objects that carry their scars out
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
    /// Rounds that have been fired and have not arrived yet, in the order
    /// they were fired.
    ///
    /// Only indirect fire ever appears here — see [`ShellInFlight`] for why —
    /// and it is a `Vec` rather than anything keyed because the firing order
    /// *is* the resolution order, and these rolls reach the event stream.
    ///
    /// `#[serde(default)]` so a save written before shells flew opens as what
    /// it was: a battle whose artillery arrived the instant it was fired.
    #[serde(default)]
    pub shells: Vec<ShellInFlight>,
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
        let moves = Arc::new(MoveGrid::build(registry, &map));
        // Resolved before the map is moved into its `Arc`, and from the same
        // two things the units are spawned from, so membership cannot drift
        // from the roster it describes.
        let command = CommandState::from_placements(map.formations(), &file.units);
        // A scenario battle has no campaign behind it, so its cadets are
        // stamped fresh from mod data and forgotten afterwards.
        let (roster, crews) = Roster::stamp_for(registry, &file.units);
        let mut state = Self {
            map: Arc::new(map),
            sight,
            moves,
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
            shells: Vec::new(),
            command,
        };
        for (placement, crew) in file.units.iter().zip(&crews) {
            state.spawn_unit(registry, placement, crew.clone());
        }
        state.face_units_at_enemies(&file.units);
        state.board_mounted_starts(registry, &file.units);
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
        crews: &[Vec<CadetId>],
        roster: Arc<Roster>,
        seed: u64,
    ) -> Self {
        let side_count = sides.len();
        let objective_count = map.objectives().len();
        let sight = Arc::new(SightGrid::build(registry, &map));
        let moves = Arc::new(MoveGrid::build(registry, &map));
        // The formations travel on the map for exactly this reason: a field
        // battle the overworld assembles picks them up without this signature
        // growing, the same trip objectives already make. A declaration whose
        // members are not among these placements is dropped rather than
        // carried empty — see `CommandState::from_placements`.
        let command = CommandState::from_placements(map.formations(), placements);
        let mut state = Self {
            map: Arc::new(map),
            sight,
            moves,
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
            shells: Vec::new(),
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
        state.board_mounted_starts(registry, placements);
        state.fog = FogMap::new(state.sides.len());
        fog::recompute(registry, &mut state);
        state
    }

    /// Board everyone a scenario says starts aboard, after every unit exists.
    ///
    /// A placement naming `aboard_at` rides the transport standing on those
    /// coordinates. A dangling reference degrades to spawning on foot — the
    /// map should fight even when its author moved the halftrack and forgot
    /// the riders — and validation is where the author hears about it.
    ///
    /// This is a method rather than a passage inside one constructor because
    /// there are two ways into a battle and both of them are real: the
    /// scenario path ([`Self::from_map`]) is what every shipped battlefield
    /// uses, and the campaign's field battles come through
    /// [`Self::from_placements`]. Living in only one of them meant a mounted
    /// column written into a map file quietly walked instead.
    fn board_mounted_starts(&mut self, registry: &DataRegistry, placements: &[UnitPlacement]) {
        for (i, placement) in placements.iter().enumerate() {
            let Some(coords) = placement.aboard_at else {
                continue;
            };
            let carrier_pos = crate::offset_to_hex(coords[0], coords[1]);
            let rider = UnitId(i as u32);
            let Some(carrier) = self
                .units
                .iter()
                .find(|u| u.pos == carrier_pos && u.id != rider && u.aboard.is_none())
                .map(|u| u.id)
            else {
                continue;
            };
            let fits = self
                .unit(carrier)
                .and_then(|c| registry.vehicle(&c.vehicle))
                .is_some_and(|v| v.capacity as usize > self.passengers(carrier).len());
            let on_foot = self
                .unit(rider)
                .and_then(|u| registry.vehicle(&u.vehicle))
                .is_some_and(|v| v.movement.class == crate::data::MovementClass::Foot);
            if fits && on_foot {
                let pos = self.unit(carrier).map(|c| c.pos).unwrap_or(carrier_pos);
                if let Some(u) = self.units.get_mut(rider.index()) {
                    u.aboard = Some(carrier);
                    u.pos = pos;
                }
            }
        }
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
        crew: Vec<CadetId>,
    ) -> UnitId {
        let id = UnitId(self.units.len() as u32);
        let vehicle = registry
            .vehicle(&placement.vehicle)
            .expect("placement validated against registry");
        // A placement that names no cadets gets an anonymous, average crew —
        // one per seat the chassis declares. Without hit points, dying is
        // something that happens to the people aboard, and whether a
        // vehicle is mortal must never depend on whether a scenario author
        // wrote a roster. Anonymous cadets have no stated cores or skills,
        // which the character model already reads as "average at
        // everything": the same fighting strength crewless units always
        // had, plus the ability to be lost.
        let crew = if crew.is_empty() && !vehicle.crew_slots.is_empty() {
            let anonymous: Vec<CadetId> = vehicle
                .crew_slots
                .iter()
                .map(|role| {
                    // Trained to average at exactly what her seat demands,
                    // not merely average-at-heart: an untrained skill reads
                    // through cores at a penalty, and a penalty here would
                    // make writing a roster change how fast every tank in a
                    // mod drives. The anonymous crew must be the crew the
                    // crewless vehicle always effectively had.
                    let skills = registry
                        .role(role)
                        .map(|r| {
                            r.skills
                                .iter()
                                .map(|s| (s.clone(), crate::data::AVERAGE))
                                .collect()
                        })
                        .unwrap_or_default();
                    let def = crate::data::CharacterDef {
                        id: format!("anonymous_{role}"),
                        name: registry
                            .role(role)
                            .map(|r| r.name.clone())
                            .unwrap_or_else(|| role.clone()),
                        skills,
                        ..Default::default()
                    };
                    Arc::make_mut(&mut self.roster).enlist(placement.side, &def, registry)
                })
                .collect();
            anonymous
        } else {
            crew
        };
        let name = placement
            .name
            .clone()
            .or_else(|| {
                crew.first()
                    .and_then(|id| self.roster.get(*id))
                    .map(|g| g.name.clone())
            })
            .unwrap_or_else(|| vehicle.name.clone());
        let crew_state = self.who_deploys(&crew);
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
            intent: UnitIntent::default(),
            planned: false,
            move_credit: 0,
            moved: 0,
            cooldowns: vec![0; vehicle.weapons.len()],
            // She drives out with what her chassis is written to carry. A
            // vehicle that declares no stowage spawns with an empty map,
            // which is every vehicle in every mod written before this
            // existed and is why nothing had to change to keep working.
            ammo: vehicle.stowage.clone(),
            // Everything aboard starts intact, which is why this is
            // `toughness` rather than zero: the map counts hits *remaining*,
            // so a module runs down to nothing rather than up to a limit and
            // "is it destroyed" is a comparison against zero wherever it is
            // asked. `modules_for` is what decides which modules those are,
            // including the standard set a chassis that declares none
            // inherits.
            modules: registry
                .modules_for(vehicle)
                .into_iter()
                .map(|m| (m.id.clone(), m.toughness))
                .collect(),
            crew_state,
            abandoned: false,
            brewed: false,
            wrecked: false,
            aboard: None,
            boarding: None,
            dismounting: false,
            last_hit_by: None,
            pressure: 0,
            detached: false,
            tasking: None,
            goal: None,
            latitude: Latitude::default(),
            alive: true,
            exited: false,
        });
        id
    }

    /// Which of this crew actually climbs in, as the seat-aligned condition
    /// list the unit spawns with.
    ///
    /// This is where a wound earns its keep. Until it existed a cadet carried
    /// a [`crate::roster::CadetStatus::Wounded`] from one battle to the next
    /// and it cost her side nothing visible: she deployed anyway, and only
    /// `crew_skill`'s quiet "she is not ready" filter took her bonuses away.
    /// A consequence the player cannot see is not a consequence. Now she
    /// stays behind, her seat is empty, and whoever is left covers for her at
    /// the substitution penalty — which is the same arithmetic a crew short
    /// of a gunner has always used.
    ///
    /// Two deliberate refusals:
    ///
    /// - **An all-empty vehicle is never produced.** If nobody named is fit,
    ///   the walking wounded go out anyway, because the campaign has no pool
    ///   of replacements to draw on and a vehicle with no crew at all is one
    ///   nothing inside can kill — see the anonymous-crew comment in
    ///   [`Self::spawn_unit`] for why that must never happen.
    /// - **Nobody is removed from [`Unit::crew`].** The campaign takes the
    ///   crew list back at the end of the battle, so a cadet filtered out here
    ///   would be a cadet deleted from her tank for good.
    ///
    /// Returns an empty vec when everyone is fit, which keeps the common case
    /// — every scenario battle, every save written before this — byte for
    /// byte what it was.
    fn who_deploys(&self, crew: &[CadetId]) -> Vec<CrewCondition> {
        let fit = |id: &CadetId| {
            self.roster
                .get(*id)
                .is_none_or(|cadet| cadet.status.is_ready())
        };
        if crew.iter().all(fit) || !crew.iter().any(fit) {
            return Vec::new();
        }
        crew.iter()
            .map(|id| {
                if fit(id) {
                    CrewCondition::Fine
                } else {
                    CrewCondition::Absent
                }
            })
            .collect()
    }

    /// Change what one vehicle is carrying, before the battle starts.
    ///
    /// Loading out is a decision made in the assembly area, not under fire:
    /// legal only during the planning phase of round one, which is this
    /// engine's deployment. After that the racks are whatever the crew drove
    /// out with, and the only thing that changes them is firing.
    ///
    /// Three refusals, and each is a different mistake. A round no mod
    /// declares is [`OrderError::NoSuchAmmo`]. A round that exists but which
    /// nothing aboard this vehicle can chamber is
    /// [`OrderError::UnchamberedAmmo`] — a Panther cannot take 88 mm shells
    /// however much room she has. And a loadout heavier than the chassis is
    /// [`OrderError::StowageFull`].
    ///
    /// Capacity is the sum of the counts the vehicle's `stowage` declares,
    /// which is deliberately crude: it prices a machine-gun belt and an 88 mm
    /// shell as one unit of space each, so the medium tank's 2870 "rounds" of
    /// capacity would in principle let her fill the hull with armour-piercing.
    /// A real prep phase wants volume per round, and this is the surface it
    /// will refine rather than the model it will keep. It exists now so that
    /// UI has something to call, and it is called by nothing.
    ///
    /// A `count` of zero takes that round off the vehicle entirely rather
    /// than leaving an empty rack behind, so the map states what is aboard
    /// and never what used to be.
    pub fn set_loadout(
        &mut self,
        registry: &DataRegistry,
        unit: UnitId,
        ammo: &str,
        count: u32,
    ) -> Result<(), OrderError> {
        if self.is_over() {
            return Err(OrderError::BattleOver);
        }
        // Round one specifically, not merely "we are planning": every later
        // planning phase happens with the enemy in the next field.
        if self.round != 1 || !self.is_planning() {
            return Err(OrderError::LoadoutClosed);
        }
        let u = self.unit(unit).ok_or(OrderError::NoSuchUnit)?;
        let vehicle = registry
            .vehicle(&u.vehicle)
            .ok_or(OrderError::NoSuchUnit)?
            .clone();
        if registry.ammo(ammo).is_none() {
            return Err(OrderError::NoSuchAmmo);
        }
        let chambers = vehicle
            .weapons
            .iter()
            .filter_map(|w| registry.weapon(w))
            .any(|w| w.ammo.iter().any(|a| a == ammo));
        if !chambers {
            return Err(OrderError::UnchamberedAmmo);
        }

        let capacity: u32 = vehicle.stowage.values().sum();
        let others: u32 = u
            .ammo
            .iter()
            .filter(|(id, _)| id.as_str() != ammo)
            .map(|(_, n)| *n)
            .sum();
        if others.saturating_add(count) > capacity {
            return Err(OrderError::StowageFull);
        }

        let u = self.unit_mut(unit).ok_or(OrderError::NoSuchUnit)?;
        if count == 0 {
            u.ammo.remove(ammo);
        } else {
            u.ammo.insert(ammo.to_string(), count);
        }
        Ok(())
    }

    pub fn unit(&self, id: UnitId) -> Option<&Unit> {
        self.units.get(id.index()).filter(|u| u.alive)
    }

    pub fn unit_mut(&mut self, id: UnitId) -> Option<&mut Unit> {
        self.units.get_mut(id.index()).filter(|u| u.alive)
    }

    pub fn unit_at(&self, hex: Hex) -> Option<&Unit> {
        self.occupants(hex).next()
    }

    /// Everyone standing on `hex`, in id order.
    ///
    /// The one place the passenger filter lives: a passenger's `pos` mirrors
    /// her carrier's, so she must never answer for the hex, and doing it once
    /// here keeps every occupancy, targeting and collision read in the game
    /// passenger-blind at the same time.
    ///
    /// **A hex can hold more than one crew**, so this is the honest question
    /// and [`Self::unit_at`] is a convenience that answers "whichever comes
    /// first in id order". Reach for the singular only where one answer is
    /// genuinely what is wanted — a mouse click, a HUD line — and never to
    /// decide whether there is room, which is [`Self::room_for`], or who a
    /// shot or a burst finds, which is everybody.
    pub fn occupants(&self, hex: Hex) -> impl Iterator<Item = &Unit> {
        self.units
            .iter()
            .filter(move |u| u.alive && u.aboard.is_none() && u.pos == hex)
    }

    /// How much of `hex`'s capacity is already taken.
    pub fn crowding(&self, registry: &DataRegistry, hex: Hex) -> u32 {
        self.occupants(hex)
            .filter_map(|u| registry.vehicle(&u.vehicle))
            .map(|v| v.footprint())
            .sum()
    }

    /// Whether `unit` would fit on `hex` alongside whoever is already there —
    /// **as far as her own side can tell**.
    ///
    /// A terrain that declares no `capacity` keeps the rule this game had
    /// before stacking: one crew to a hex, whatever size she is. That is why
    /// this asks the terrain first and only counts footprints if the terrain
    /// opted in — see [`crate::data::TerrainDef::capacity`].
    ///
    /// **An enemy her side has not spotted takes up no room.** That is not an
    /// approximation, it is the fog rule: an order refused because a hex is
    /// "full" announces that somebody is standing there, and this engine
    /// deliberately lets that move resolve as an ambush instead. Two crews can
    /// therefore end a tick over capacity, which is correct — they have just
    /// driven into each other. `unspotted_enemies_still_ambush` and
    /// `hidden_enemies_do_not_show_up_as_holes_in_the_move_range` both failed
    /// the moment this counted everybody, which is how the rule got written
    /// down here rather than rediscovered later.
    pub fn room_for(&self, registry: &DataRegistry, unit: &Unit, hex: Hex) -> bool {
        let others: Vec<&Unit> = self
            .occupants(hex)
            .filter(|other| other.id != unit.id)
            .filter(|other| {
                other.side == unit.side || self.fog.side(unit.side).spotted.contains(&other.id)
            })
            .collect();
        let capacity = self
            .terrain_at(hex)
            .and_then(|id| registry.terrain(id))
            .and_then(|t| t.capacity);
        let Some(capacity) = capacity else {
            return others.is_empty();
        };
        let mine = registry
            .vehicle(&unit.vehicle)
            .map(|v| v.footprint())
            .unwrap_or(1);
        let taken: u32 = others
            .iter()
            .filter_map(|o| registry.vehicle(&o.vehicle))
            .map(|v| v.footprint())
            .sum();
        taken + mine <= capacity.max(mine)
    }

    /// Everyone riding in `carrier`, in id order.
    pub fn passengers(&self, carrier: UnitId) -> Vec<UnitId> {
        self.units
            .iter()
            .filter(|u| u.alive && u.aboard == Some(carrier))
            .map(|u| u.id)
            .collect()
    }

    /// Every enemy on `hex` that `side` may act on: the ones its fog spots.
    /// Callers that would otherwise reach for [`Self::occupants`] should
    /// prefer this, so an order refusal never betrays a unit the side cannot
    /// see.
    pub fn spotted_enemies_at(&self, hex: Hex, side: u8) -> impl Iterator<Item = &Unit> {
        self.occupants(hex)
            .filter(move |u| u.side != side && self.fog.side(side).spotted.contains(&u.id))
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
    /// Cadets aboard `unit` still part of the fight. An empty `crew_state`
    /// means nobody has been hurt and nobody stayed behind, so the whole
    /// crew counts — which is every battle written before either existed.
    pub fn fighting_crew(&self, unit: &Unit) -> usize {
        unit.crew
            .iter()
            .enumerate()
            .filter(|(seat, _)| {
                unit.crew_state
                    .get(*seat)
                    .copied()
                    .unwrap_or(CrewCondition::Fine)
                    .fighting()
            })
            .count()
    }

    /// The fraction of this vehicle's fighting substance still aboard: her
    /// cadets (two points each — fine is two, wounded one, out zero) and her
    /// modules (hits remaining over toughness), as one 0..=1 number.
    ///
    /// This is the condition score that replaces the hit-point fraction
    /// everywhere the AI used to read one: withdraw thresholds, the
    /// formation "beaten" metric, search scoring. It is deliberately a
    /// *substance* measure rather than a "can she fight" measure — a
    /// mission-killed vehicle with a live crew scores low and wants out,
    /// which is exactly the behavior those thresholds exist to produce.
    pub fn condition(&self, registry: &DataRegistry, unit: &Unit) -> f32 {
        let (have, total) = self.substance(registry, unit);
        if total == 0 {
            return 1.0;
        }
        have as f32 / total as f32
    }

    /// The raw pair behind [`Self::condition`]: substance points remaining
    /// and the vehicle's full complement. Exposed because "could one round
    /// plausibly finish her" is a question about the numerator, not the
    /// fraction — the AI's kill flag and the preview's `lethal` both
    /// compare a round's effect budget against what is actually left.
    pub fn substance(&self, registry: &DataRegistry, unit: &Unit) -> (u32, u32) {
        let mut have = 0u32;
        let mut total = 0u32;
        for (seat, _) in unit.crew.iter().enumerate() {
            // A seat nobody is sitting in counts for neither half. Charging
            // an absent cadet's two points to the denominator would make a
            // vehicle that deployed short-handed read as one that had
            // already been shot up — braver crews would flee it and the AI
            // would price it as a kill nearly made.
            match unit
                .crew_state
                .get(seat)
                .copied()
                .unwrap_or(CrewCondition::Fine)
            {
                CrewCondition::Fine => {
                    total += 2;
                    have += 2;
                }
                CrewCondition::Wounded => {
                    total += 2;
                    have += 1;
                }
                CrewCondition::Out => total += 2,
                CrewCondition::Absent => {}
            }
        }
        for (id, hits) in &unit.modules {
            let toughness = registry.module(id).map(|m| m.toughness).unwrap_or(1);
            total += toughness;
            have += (*hits).min(toughness);
        }
        (have, total)
    }

    /// What a vehicle on this field is *like*, in substance points: the mean
    /// full complement across everything fielded, dead and alive alike.
    ///
    /// A reference scale, and the reason the AI can talk about danger as a
    /// fraction without a constant in Rust saying how big a tank is. A mod
    /// whose vehicles carry ten cadets apiece, or a scenario of nothing but
    /// scout sections, moves this with the content instead of measuring its
    /// units against a number written for the base game.
    ///
    /// Constant for the length of a battle: a unit's *full* complement is
    /// structural — how many seats and modules the chassis declares — and
    /// nothing during a fight adds or removes either. Casualties move the
    /// remaining half only. The dead are counted for the same reason: the
    /// reference is what this scenario put on the field, not who is left on
    /// it, and a shrinking denominator would make everyone quietly braver
    /// as the battle wore on.
    pub fn typical_substance(&self, registry: &DataRegistry) -> f32 {
        if self.units.is_empty() {
            return 1.0;
        }
        let total: u32 = self
            .units
            .iter()
            .map(|u| self.substance(registry, u).1)
            .sum();
        (total as f32 / self.units.len() as f32).max(1.0)
    }

    pub fn obeys(&self, registry: &DataRegistry, unit: &Unit) -> bool {
        self.morale(registry, unit).obeys
    }

    /// What this crew does instead of what she was told.
    ///
    /// Only meaningful for a crew who is not obeying; every caller checks
    /// [`Self::obeys`] first, and asking it of a steady crew answers with
    /// whatever her temperament would be if she broke, which is a fair
    /// question to ask a panel.
    ///
    /// **The senior cadet still fighting decides.** Not the best score
    /// aboard and not an average: somebody says "back her out" or "keep
    /// firing" and the rest of the crew does it, and in this engine that is
    /// whoever is left in the most forward seat — the same leaders-last
    /// ordering the interior model already uses, so a commander going out
    /// hands her temperament to the next woman down along with everything
    /// else. That is a rule about people, so it is deliberately not
    /// `crew_skill`'s "best aboard".
    pub fn defiance(&self, registry: &DataRegistry, unit: &Unit) -> crate::data::DefianceResponse {
        let rules = &registry.morale;
        if rules.defiance.is_empty() {
            return crate::data::DefianceResponse::default();
        }
        let speaker = unit.crew.iter().enumerate().find(|(seat, id)| {
            unit.crew_state
                .get(*seat)
                .copied()
                .unwrap_or(CrewCondition::Fine)
                .fighting()
                && self.roster.get(**id).is_some()
        });
        let vehicle = registry.vehicle(&unit.vehicle);
        let ctx = crate::data::CheckContext {
            terrain: self.terrain_at(unit.pos),
            vehicle_class: vehicle.map(|v| v.class.as_str()),
            crew_size: unit
                .crew
                .iter()
                .filter(|id| self.roster.get(**id).is_some())
                .count(),
        };
        // Nobody aboard the campaign knows about — an anonymously crewed
        // vehicle, which is every scenario battle written before cadets
        // existed — has no temperament to read, so she does what a crew has
        // always done.
        let Some(cadet) = speaker.and_then(|(_, id)| self.roster.get(*id)) else {
            return rules
                .defiance
                .first()
                .map(|d| d.response)
                .unwrap_or_default();
        };
        rules
            .defiance
            .iter()
            .map(|d| {
                let core = d
                    .core
                    .as_deref()
                    .and_then(|c| registry.core_index.get(c))
                    .and_then(|i| cadet.cores.get(i).copied())
                    .unwrap_or(crate::data::AVERAGE);
                let from_traits: i32 = cadet
                    .traits
                    .iter()
                    .filter_map(|id| registry.trait_def(id))
                    .map(|t| t.defiance_modifier(&d.id, &ctx))
                    .sum();
                (d.base + core + from_traits, d.response)
            })
            // `max_by_key` on an iterator returns the LAST maximum, and the
            // additivity argument in `DefianceDef` rests on ties going to the
            // first. Fold explicitly rather than relying on a subtlety of the
            // standard library that a reader would have to look up.
            .fold(
                None::<(i32, crate::data::DefianceResponse)>,
                |best, next| match best {
                    Some((score, _)) if score >= next.0 => best,
                    _ => Some(next),
                },
            )
            .map(|(_, response)| response)
            .unwrap_or_default()
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

    /// The battle's clock as one number: `round * ticks_per_round + tick`.
    ///
    /// The convention [`SideFog::spotted_since`] already runs on, and now
    /// what [`ShellInFlight::lands`] is written in. Named here because three
    /// separate places were spelling the same multiplication out, and a
    /// clock that two systems compute slightly differently is a clock that
    /// will eventually disagree with itself.
    pub fn absolute_tick(&self, registry: &DataRegistry) -> u64 {
        self.round as u64 * registry.scale.ticks_per_round as u64
            + self.resolving_tick().unwrap_or(0) as u64
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
