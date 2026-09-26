//! The seam between the campaign and a battle: a clash on the campaign map
//! staged as a field battle, and the battle's result handed back.
//!
//! Everything here used to live in the game crate's battle screen, beside the
//! sprites. None of it is presentation — who deploys where, which formation an
//! army fills, what orders it arrives under, which battlefield the ground
//! picks, and when an army counts as having *withdrawn* are campaign rules —
//! and while it lived behind Bevy no instrument could play a campaign: the
//! harness could fight battles and could not fight the war they were part of.
//! Now the game crate and [`crate::harness::campaign`] walk the same path:
//!
//! 1. [`battlefield_for`] picks the map from the ground the clash is on.
//! 2. [`Clash::muster`] gathers the armies committed to it.
//! 3. [`Clash::problem`] asks whether it can be staged at all, *before* the
//!    campaign commits anybody to it.
//! 4. [`Clash::stage`] builds the battle, and [`Clash::inherit_missions`]
//!    hands it whatever the armies were already trying to do.
//! 5. [`FieldBattle::report`] reads the finished battle into the
//!    [`BattleReport`] that [`OverworldState::apply_battle_result`] folds back.

use crate::battle::{
    BattleSetupError, BattleState, Fate, FormationId, Latitude, Mission, Order, SideState,
    formation_exit,
};
use crate::data::{DataRegistry, MovementClass};
use crate::map::{Battlefield, MapError, MapKind, ObjectiveKind, UnitPlacement};
use crate::overworld::{ArmyMission, ArmyUnit, BattleReport, CrewLoss, ElementId, OverworldState};
use crate::roster::{CadetId, Roster};
use crate::{Hex, hex_to_offset};
use std::collections::HashMap;
use std::sync::Arc;

/// One army committed to a field battle.
#[derive(Debug, Clone, PartialEq)]
pub struct BattleForce {
    pub army: ElementId,
    pub side: u8,
    pub units: Vec<ArmyUnit>,
    /// The standing orders this army was carrying when it was committed, so
    /// what was decided on the map can colour the fight it caused. See
    /// [`Clash::inherit_missions`].
    pub mission: Option<ArmyMission>,
}

/// A fight the campaign has caused, before it is fought: the battlefield, the
/// sides, and every army committed to it.
#[derive(Debug, Clone)]
pub struct Clash {
    pub map_id: String,
    /// The campaign's cadets, so the crews that fight are the same people
    /// who walk away from it.
    pub roster: Arc<Roster>,
    /// The army that started it, and the one that was attacked. These two
    /// decide who advances onto the contested tile afterwards, and they are
    /// the only two whose standing orders reach the battle.
    pub attacker: ElementId,
    pub defender: ElementId,
    pub sides: Vec<SideState>,
    pub attacker_side: u8,
    pub forces: Vec<BattleForce>,
    /// Where the dice of this fight start: `world::engagement_seed` of the
    /// campaign's seed and this fight's own key — the day, the contested
    /// tile, the two principals. Stage with it, and the battle is the same
    /// one whether it is the first fight of the campaign or the fortieth,
    /// and whether it was reached by playing or by loading a save.
    pub seed: u64,
}

/// Why a battle could not be staged at all.
///
/// Every arm of this used to be an `expect`, which meant a mod that dropped a
/// vehicle between one save and the next took the whole game down rather than
/// declining one fight. The campaign asks the same question through
/// [`Clash::problem`] *before* it commits its armies, so in practice nobody
/// should ever see one of these; it exists so that the day somebody does,
/// they see a line in the log.
#[derive(Debug, thiserror::Error)]
pub enum StagingError {
    #[error("no map `{0}` in the loaded mods")]
    MissingMap(String),
    #[error("map `{0}` will not parse: {1}")]
    Map(String, MapError),
    #[error("{0}")]
    Setup(#[from] BattleSetupError),
}

/// Bookkeeping for a battle that resolves a clash: who started it, and the
/// army each unit was drawn from.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldBattle {
    pub attacker: ElementId,
    pub defender: ElementId,
    /// The army each unit was drawn from, by the unit's id, in id order.
    ///
    /// Pairs rather than a list indexed by id, because an id is a name and
    /// not a position (WORLD.md, W0.5): a vehicle that keeps her world id
    /// into a battle is not unit number *n*.
    pub origins: Vec<(crate::battle::UnitId, ElementId)>,
}

/// The battle map a clash on `terrain` is fought over.
///
/// The terrain says so itself (`TerrainDef::battlefield`), which is the
/// answer for everything the shipped campaign stands on; a terrain that names
/// nothing falls back to the `battle_<terrain>` convention, and then to the
/// first battle map by id. `None` when no mod ships a battle map at all — a
/// campaign with nowhere to fight declines the fight rather than crashing.
///
/// The last arm used to take whichever battle map a `HashMap` iterated first,
/// which is a different map from one run to the next. A shrug is allowed; a
/// shrug that depends on hash order is not, in an engine that promises the
/// same campaign from the same seed.
pub fn battlefield_for(registry: &DataRegistry, terrain: &str) -> Option<String> {
    let is_battle = |id: &String| {
        registry
            .maps
            .get(id)
            .is_some_and(|m| m.kind == MapKind::Battle)
    };
    if let Some(named) = registry
        .terrain(terrain)
        .and_then(|t| t.battlefield.clone())
        .filter(&is_battle)
    {
        return Some(named);
    }
    let preferred = format!("battle_{terrain}");
    if is_battle(&preferred) {
        return Some(preferred);
    }
    registry
        .maps
        .values()
        .filter(|m| m.kind == MapKind::Battle)
        .map(|m| m.id.clone())
        .min()
}

impl Clash {
    /// The clash `attacker` caused by engaging `defender`, with `joiners`
    /// piling in and the cadets in `called_up` riding hurt, on `map_id`.
    ///
    /// Principals first, then joiners, each army once: an army could be
    /// listed twice if it were both principal and joiner, and unit origins
    /// have to stay unique. The roster is a snapshot of the campaign's, with
    /// the roll made on it — the battle reads the copy, and casualties come
    /// back in the report and are applied to the campaign's own.
    pub fn muster(
        state: &OverworldState,
        attacker: ElementId,
        defender: ElementId,
        joiners: &[ElementId],
        called_up: &[CadetId],
        map_id: String,
    ) -> Self {
        let sides: Vec<SideState> = state
            .sides
            .iter()
            .map(|s| SideState {
                name: s.name.clone(),
                ai: s.ai.clone(),
            })
            .collect();
        let attacker_side = state.army(attacker).map(|a| a.side).unwrap_or(0);
        let mut forces: Vec<BattleForce> = Vec::new();
        for id in [attacker, defender].iter().chain(joiners) {
            if forces.iter().any(|f| f.army == *id) {
                continue;
            }
            if let Some(army) = state.army(*id) {
                forces.push(BattleForce {
                    army: *id,
                    side: army.side,
                    units: army.units.clone(),
                    mission: army.mission.clone(),
                });
            }
        }
        let at = state
            .army(defender)
            .or_else(|| state.army(attacker))
            .map_or(Hex::ZERO, |a| a.pos);
        let seed = crate::world::engagement_seed(
            state.seed,
            crate::world::EngagementKey {
                when: state.turn as u64,
                at,
                attacker: attacker.0,
                defender: defender.0,
            },
        );
        Self {
            map_id,
            seed,
            // `mustered` is the only thing that ever writes
            // `Cadet::called_up`, and it writes it here, so the answer for
            // this fight cannot still be standing at the next one.
            roster: Arc::new(state.roster.mustered(called_up)),
            attacker,
            defender,
            sides,
            attacker_side,
            forces,
        }
    }

    /// Build the battle, on the map the terrain picked, with the campaign's
    /// own cadets. Returns the state and the bookkeeping that sends each
    /// unit's casualties home to the right army.
    pub fn stage(
        &self,
        registry: &DataRegistry,
        seed: u64,
    ) -> Result<(BattleState, FieldBattle), StagingError> {
        let file = registry
            .map(&self.map_id)
            .ok_or_else(|| StagingError::MissingMap(self.map_id.clone()))?;
        let field = Battlefield::from_map_file(file)
            .map_err(|e| StagingError::Map(self.map_id.clone(), e))?;
        let (placements, crews, origins) = deploy(
            registry,
            &self.roster,
            &field,
            &self.forces,
            self.attacker_side,
        );
        let state = BattleState::from_placements(
            registry,
            field,
            self.sides.clone(),
            &placements,
            &crews,
            self.roster.clone(),
            seed,
        )?;
        // Units are spawned in placement order, so the n-th unit came from
        // the n-th placement's army, whatever she is called.
        let origins = state.units.iter().map(|u| u.id).zip(origins).collect();
        Ok((
            state,
            FieldBattle {
                attacker: self.attacker,
                defender: self.defender,
                origins,
            },
        ))
    }

    /// Whether the campaign can stage this fight, as a sentence for the log.
    ///
    /// `None` means it can. The campaign calls this before
    /// `commit_to_battle`, because refusing a battle is survivable and losing
    /// the whole run to a panic is not. It stages the battle and throws it
    /// away, which costs about a millisecond of grid building once per clash;
    /// anything cleverer would be a second copy of the rules about what a
    /// valid order of battle is, and a second copy is how two answers drift.
    pub fn problem(&self, registry: &DataRegistry) -> Option<String> {
        self.stage(registry, 0).err().map(|e| e.to_string())
    }

    /// Hand the battle whatever its armies were already trying to do, and
    /// say so: one line per formation told, for the log.
    ///
    /// Only [`ArmyMission::Withdraw`] maps onto a battle mission today, and
    /// only for the two *principal* armies — the one that attacked and the
    /// one that was attacked — because those are the two whose intent caused
    /// this fight; a neighbour who piled in came to help with somebody
    /// else's decision. A withdrawing army's formations are ordered out by
    /// the nearest lane their side may use, which is the same rule their own
    /// commander would have applied once they were beaten: the campaign's
    /// order is that they should not wait to be beaten first.
    ///
    /// `Advance` and `Hold` deliberately do **not** map. The battle brain
    /// already advances on the ground the map declares worth holding, so
    /// translating them would either say what it is already saying or
    /// overrule it with a hex chosen four kilometres away — and an
    /// operational advance is not a tactical one. If they ever do map, it
    /// should be through the objectives, not around them.
    ///
    /// The orders go through `BattleState::apply` like everyone else's, so
    /// they travel at the signals net's speed and appear in the log and in
    /// any replay. A map that offers the side no exit produces nothing.
    pub fn inherit_missions(
        &self,
        registry: &DataRegistry,
        state: &mut BattleState,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        for principal in [self.attacker, self.defender] {
            let Some(force) = self
                .forces
                .iter()
                .find(|f| f.army == principal)
                .filter(|f| matches!(f.mission, Some(ArmyMission::Withdraw { .. })))
            else {
                continue;
            };
            let side = force.side;
            let ordered: Vec<usize> = state
                .formations()
                .iter()
                .enumerate()
                .filter(|(_, f)| f.side == side)
                .map(|(index, _)| index)
                .collect();
            for index in ordered {
                let Some(via) = formation_exit(state, index) else {
                    continue;
                };
                let name = state.formations()[index].id.clone();
                let order = Order::SetMission {
                    formation: FormationId(index as u32),
                    mission: Mission::Withdraw { via },
                    latitude: Latitude::Delegated,
                };
                if state.apply(registry, &order).is_ok() {
                    lines.push(format!("{name} is under orders to break contact."));
                }
            }
        }
        lines
    }
}

impl FieldBattle {
    /// Read a finished battle into what the campaign needs to hear.
    ///
    /// Pure over the battle, which is what lets a headless run fight a whole
    /// field battle into the roster.
    pub fn report(&self, registry: &DataRegistry, state: &BattleState) -> BattleReport {
        // Start every participating army at zero survivors so armies that
        // were wiped out are still reported, then hand each living unit
        // back to the army it marched in with.
        let mut survivors: Vec<(ElementId, Vec<ArmyUnit>)> = Vec::new();
        let mut slot_of = HashMap::new();
        for (_, army) in &self.origins {
            slot_of.entry(*army).or_insert_with(|| {
                survivors.push((*army, Vec::new()));
                survivors.len() - 1
            });
        }
        // `surviving_units`, not `alive_units`: a crew that drove off the map
        // by an exit is off the board but came home, and reading `alive` here
        // would hand the campaign a withdrawal as a burnt-out vehicle.
        for unit in state.surviving_units() {
            let Some(army) = self.origin(unit.id) else {
                continue;
            };
            let Some(&slot) = slot_of.get(&army) else {
                continue;
            };
            survivors[slot].1.push(ArmyUnit {
                vehicle: unit.vehicle.clone(),
                crew: unit.crew.clone(),
                name: Some(unit.name.clone()),
            });
        }

        // Everyone the battle hurt, in two kinds: pulled out of a wreck, and
        // found hurt in a seat that came home.
        let losses = CrewLoss::in_battle(registry, state);
        // An army whose every surviving vehicle drove off by an exit has
        // withdrawn: it is not beaten, and it is not here. An army that lost
        // everything has not withdrawn, whatever else it did, and neither has
        // one with a single vehicle still on the field — that one is still
        // standing on the ground.
        let mut withdrew: Vec<ElementId> = Vec::new();
        for (army, units) in &survivors {
            let mut exited = 0;
            let mut on_field = 0;
            for unit in state.units.iter() {
                if self.origin(unit.id) != Some(*army) {
                    continue;
                }
                match unit.fate {
                    Fate::Exited => exited += 1,
                    Fate::Fighting { .. } => on_field += 1,
                    Fate::Destroyed(_) => {}
                }
            }
            if !units.is_empty() && exited > 0 && on_field == 0 {
                withdrew.push(*army);
            }
        }
        BattleReport {
            attacker: self.attacker,
            defender: self.defender,
            winner: state.over.and_then(|r| r.winner),
            stalemate: matches!(
                state.over.map(|r| r.reason),
                Some(crate::battle::EndReason::Stalemate)
            ),
            survivors,
            losses,
            withdrew,
        }
    }

    /// The army a unit marched in with.
    pub fn origin(&self, unit: crate::battle::UnitId) -> Option<ElementId> {
        self.origins
            .binary_search_by_key(&unit, |(id, _)| *id)
            .ok()
            .map(|i| self.origins[i].1)
    }
}

/// Line the attacking armies up along the west edge and the defenders along
/// the east. Returns the placements, the crew each one carries, and the army
/// each came from, so casualties can be reported back to the right army.
#[allow(clippy::type_complexity)]
pub fn deploy(
    registry: &DataRegistry,
    roster: &Roster,
    field: &Battlefield,
    forces: &[BattleForce],
    attacker_side: u8,
) -> (Vec<UnitPlacement>, Vec<Vec<CadetId>>, Vec<ElementId>) {
    let map = &field.terrain;
    // Tiles a vehicle can actually sit on, nearest edge first. Taking spots
    // in this order lets a side deploy as deep inland as it needs to, so
    // three armies fit where one used to.
    let deployable = |west: bool| -> Vec<Hex> {
        let mut spots: Vec<(i32, i32, Hex)> = map
            .iter()
            .filter(|(_, tile)| {
                registry
                    .terrain(tile.terrain)
                    .is_some_and(|t| t.cost_for(MovementClass::Tracked).is_some())
            })
            // A side deploys at the shallowest tiles of its own edge, which
            // is precisely where that side's retreat lane is. Standing on an
            // exit means taking it, so without this the leading vehicles
            // would drive off the map on the first tick and the battle would
            // be over before anyone saw an enemy. Nobody forms up on the road
            // home.
            .filter(|(hex, _)| {
                !field
                    .scenario
                    .objectives()
                    .iter()
                    .any(|o| o.kind == ObjectiveKind::Exit && o.contains(*hex))
            })
            .map(|(hex, _)| {
                let [col, row] = hex_to_offset(hex);
                (if west { col } else { -col }, row, hex)
            })
            .collect();
        spots.sort_unstable_by_key(|(depth, row, _)| (*depth, *row));
        spots.into_iter().map(|(_, _, hex)| hex).collect()
    };

    // An army *is* a formation, which is what the design doc means by
    // "`ArmyPlacement` on the overworld maps naturally". The declarations
    // themselves belong to the battlefield's scenario — that is where its
    // order of battle is written — so an army fills the next-declared
    // formation of its own side, first army into the first-declared one, and
    // its first vehicle leads it. A map that declares none for a side leaves
    // that side the flat pool it has always been, so a field battle on a
    // formationless map is exactly the battle it was.
    let slots = |side: u8| -> Vec<String> {
        field
            .scenario
            .formations()
            .iter()
            .filter(|f| f.side == side)
            .map(|f| f.id.clone())
            .collect()
    };
    let mut taken: HashMap<u8, usize> = HashMap::new();
    let mut led: Vec<String> = Vec::new();

    let mut placements = Vec::new();
    let mut crews = Vec::new();
    let mut origins = Vec::new();
    for west in [true, false] {
        let mut spots = deployable(west).into_iter();
        for force in forces.iter().filter(|f| (f.side == attacker_side) == west) {
            // More armies than the map named formations is an ordinary muster
            // — three companies piling into a two-platoon map — and they wrap
            // round rather than being left out of the chain of command.
            let available = slots(force.side);
            let formation = (!available.is_empty()).then(|| {
                let next = taken.entry(force.side).or_default();
                let id = available[*next % available.len()].clone();
                *next += 1;
                id
            });
            // The army's senior cadet leads it: the vehicle carrying the
            // highest rank, and among equals the first in the army's list —
            // which, with no ranks declared, is the first vehicle, as it
            // always was. Succession then works by rank the same way.
            let senior = force
                .units
                .iter()
                .enumerate()
                .map(|(i, unit)| {
                    let rank = unit
                        .crew
                        .iter()
                        .filter_map(|c| roster.get(*c))
                        .filter_map(|cadet| registry.rank_index(cadet.rank.as_deref()))
                        .max();
                    (i, rank)
                })
                .fold(
                    None,
                    |best: Option<(usize, Option<usize>)>, (i, rank)| match best {
                        Some((_, b)) if rank <= b => best,
                        _ => Some((i, rank)),
                    },
                )
                .map(|(i, _)| i);
            for (i, unit) in force.units.iter().enumerate() {
                let Some(hex) = spots.next() else { break };
                let leads =
                    Some(i) == senior && formation.as_ref().is_some_and(|id| !led.contains(id));
                if leads {
                    led.push(formation.clone().expect("leads implies a formation"));
                }
                // The crew travels alongside as cadet handles rather than in
                // the placement: a `UnitPlacement` names crew by definition
                // id, and a campaign's crews are people, not definitions.
                placements.push(UnitPlacement {
                    aboard_at: None,
                    at: hex_to_offset(hex),
                    side: force.side,
                    vehicle: unit.vehicle.clone(),
                    crew: Vec::new(),
                    name: unit.name.clone(),
                    facing: None,
                    formation: formation.clone(),
                    leads,
                });
                crews.push(unit.crew.clone());
                origins.push(force.army);
            }
        }
    }
    (placements, crews, origins)
}
