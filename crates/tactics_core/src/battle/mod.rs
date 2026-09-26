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
    AttackPreview, CounterPreview, HitBreakdown, HitFactor, HitModifier, ShellInFlight, ShotValue,
    best_weapon_from, blast_overmatches, crewed_reload, expected_damage, expected_pressure,
    expected_shot, flight_ticks, hit_breakdown, hit_chance, penetration_chance, penetration_share,
    preview_attack, round_worth, spent_share, struck_facing, weapon_ready,
};
pub use command::{
    CommandState, Contact, CutOff, Formation, FormationId, Goal, Knower, Latitude, March, Mission,
    MissionChange, OperationalCommand, PersonalOrder, Plan, PlanPhase, WaitingOrders,
    formation_exit, nearest_exit,
};
pub use danger::{
    Bearing, Incoming, drill_destination, fire_on, fire_on_as, incoming, incoming_from,
    noticed_threats, threatened, threats,
};
pub use fog::{FogMap, SideFog, SightGrid, los_clear, unit_vision};
pub use movement::{
    MoveGrid, Roads, along_the_bearing, destination_blocked, edge_cost as movement_edge_cost,
    move_points, path_to, reachable, roads, step_toward,
};
pub use orders::{Event, FireIntent, Order, OrderError, UnitIntent};

use crate::ai::AiConfig;
use crate::data::{DataError, DataRegistry, ModuleEffect, ValidationReport};
use crate::map::{Battlefield, HexMap, MapKind, Scenario, UnitPlacement};
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

/// What ended a vehicle, now that there are no hit points.
///
/// The variants are the four ways a fight finishes a machine, and they are
/// deliberately *not* four independent facts: a vehicle dies once, and this
/// is what she is remembered by. When more than one of them happens to the
/// same hull inside one tick — the crew bails out of a hull a later shell
/// then crushes — [`Self::supersedes`] decides which sticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Destruction {
    /// Blast overmatch crushed her: the round did not need the penetration
    /// gate's permission, and there is nothing left to fight from.
    Crushed,
    /// The crew left her under fire, rolled through the same discipline
    /// check that governs refusing orders. A wreck as far as the battle is
    /// concerned — reaped like a kill, scored like a loss — but the cadets
    /// are walking home, which the campaign's fate machinery treats very
    /// differently from burning.
    Abandoned,
    /// The ammunition went up. Instantly destroyed, and the fate rolls for
    /// everyone aboard carry the fire.
    BrewedUp,
    /// Nobody aboard can fight any more. Not a flag anything sets: it is
    /// what [`reap`](crate::battle::orders) concludes about a hull that is
    /// intact and has nobody left to work it.
    CrewSpent,
}

impl Destruction {
    /// Whether this end of hers is the one to remember, given one already
    /// recorded.
    ///
    /// **This is a read precedence turned into a write precedence.** The
    /// three flags this replaced were independent bools, all of which stayed
    /// set, and every reader had its own fixed order for consulting them —
    /// which is the same thing as an ordering, written out three times and
    /// able to disagree. Measured over 900 AI battles (9,131 vehicles lost),
    /// two of them land on one hull about 250 times, in all three pairings,
    /// so this is a case that happens rather than a case that is argued
    /// about.
    ///
    /// The order is *burning beats the crew leaving beats the hull being
    /// crushed*, and the middle rung is where the simulation depends on it:
    /// the bail-out check refuses to roll for a crew who has already gone,
    /// so an abandonment that a later shell's blast overwrote would let the
    /// same crew abandon the same tank twice.
    fn supersedes(self, existing: Destruction) -> bool {
        fn rank(d: Destruction) -> u8 {
            match d {
                Destruction::CrewSpent => 0,
                Destruction::Crushed => 1,
                Destruction::Abandoned => 2,
                Destruction::BrewedUp => 3,
            }
        }
        rank(self) > rank(existing)
    }
}

/// Where a vehicle stands in this battle: on the field, off it in a wreck, or
/// off it under her own power.
///
/// **Why one enum and not five bools.** This replaced `alive`, `exited`,
/// `abandoned`, `brewed` and `wrecked`, which between them could spell
/// thirty-two states of which four were meaningful, and a prose warning
/// never to classify a unit by `!alive` — because `!alive` is true of a
/// vehicle that withdrew successfully, and reading it as a loss records a
/// clean withdrawal as a massacre. The distinction the warning was defending
/// is now the difference between two variants, and the next fate anybody
/// wants — captured, immobilised and left behind — is a sixth *variant*,
/// which is a compile error at every reader rather than a sixth flag nobody
/// notices.
///
/// Note that [`Self::alive`] and "standing on a hex" remain two different
/// questions: a passenger is [`Self::Fighting`] and on no hex anybody may
/// interact with. Occupancy is asked through `unit_at` / `occupants`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fate {
    /// On the battlefield: targetable, blocking, and still shooting.
    ///
    /// `doom` is a destruction she has already taken and has not yet been
    /// reaped for. Damage lands during a tick and death is reaped at the end
    /// of it, which is the simultaneity bargain the whole resolver keeps, so
    /// between the shell and `reap` a vehicle is genuinely still on the
    /// board with her end already decided. It is inside this variant rather
    /// than beside it so that "is she on the field" stays one variant and
    /// cannot be got wrong by a match written later.
    Fighting { doom: Option<Destruction> },
    /// Reaped: off the board, and counted as a loss, with what ended her.
    Destroyed(Destruction),
    /// She drove off the map by an exit objective she was entitled to take.
    ///
    /// Off the board — nothing can see her, shoot her or be blocked by her —
    /// but emphatically *not* destroyed: the campaign counts her among the
    /// survivors and her crew walk home.
    Exited,
}

impl Default for Fate {
    /// Whole and on the field, which is what every vehicle spawns as and
    /// what a save that says nothing about her means. Written out rather
    /// than derived because `#[default]` cannot name a struct variant.
    fn default() -> Self {
        Self::Fighting { doom: None }
    }
}

impl Fate {
    /// Whether she is on the battlefield. False for a wreck *and* for a
    /// vehicle that withdrew, which is why it is never the question to ask
    /// about who came home.
    pub fn alive(self) -> bool {
        matches!(self, Self::Fighting { .. })
    }

    /// Whether she left under her own power by an exit she was entitled to.
    pub fn exited(self) -> bool {
        matches!(self, Self::Exited)
    }

    /// Whether this battle is going to record her as a loss — the complement
    /// of "came home", and the honest reading of what `!alive` used to be
    /// asked for.
    pub fn lost(self) -> bool {
        matches!(self, Self::Destroyed(_))
    }

    /// What ended her, or is about to: the same answer either side of the
    /// reaping, because a doom taken this tick and a death recorded last
    /// tick are the same fact at different ages, and every caller that
    /// consults it wants it that way.
    pub fn destruction(self) -> Option<Destruction> {
        match self {
            Self::Fighting { doom } => doom,
            Self::Destroyed(how) => Some(how),
            Self::Exited => None,
        }
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
    /// Where she stands in this battle: on the field, destroyed and how, or
    /// withdrawn by an exit — the whole of what used to be `alive`,
    /// `exited`, `abandoned`, `brewed` and `wrecked`.
    ///
    /// Ask it through [`Self::alive`], [`Self::exited`] and
    /// [`Self::destruction`], or match it exhaustively; see [`Fate`] for
    /// why it is one value. `#[serde(default)]` is
    /// [`Fate::Fighting`] with no doom, which is what every unit spawns as.
    #[serde(default)]
    pub fate: Fate,
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
    /// Damage type of the last hit this unit took, if any. Read by the
    /// campaign when working out what became of the crew.
    pub last_hit_by: Option<crate::data::DamageType>,
    /// How much this crew has had to take. Walks them up the morale ladder;
    /// shed a little at the end of every round.
    pub pressure: u32,
    /// Ticks she has left closed up (`balance.buttoned_ticks`): hatches shut
    /// against small-arms fire, nearly blind to infantry close in and slow to
    /// react. Counted down at the top of every tick. `#[serde(default)]`,
    /// because a battle saved before anybody closed up had nobody closed up.
    #[serde(default)]
    pub buttoned: u32,
    /// Her commander's personal order, if she is under one: the whole of
    /// what used to be `detached`, `tasking` and `latitude`.
    ///
    /// `Some(_)` is *detached* — excused from her formation's standing
    /// mission until recalled, which is what makes a hand-placed vehicle
    /// stay where her commander put her instead of drifting back to the
    /// mission's axis at the next planning phase (the first playtest rightly
    /// read that as the game overriding the player). Set when a direct
    /// radioed order reaches her; cleared by an explicit recall
    /// (`ClearIntent`) or by a NEW mission being set for her formation — a
    /// fresh formation order collects everyone.
    ///
    /// [`PersonalOrder::Marching`] is *tasking*: a destination she re-paths
    /// toward each round, carrying the latitude it was given at. Reaching it
    /// turns her to [`PersonalOrder::Holding`] at the same latitude — she
    /// holds the ground she was sent to, still detached, still meaning it
    /// if her commander did — which is the one transition the three old
    /// flags had to perform in concert and now cannot get wrong.
    ///
    /// `#[serde(default)]` so a battle spawned or read without one is a crew
    /// answering to her formation, which is what every unit starts as.
    #[serde(default)]
    pub orders: Option<crate::battle::PersonalOrder>,
    /// What she has decided to do about it: her own goal, as opposed to her
    /// formation's mission or her commander's [`Self::orders`].
    ///
    /// Kept across rounds, which is the whole of its value — a planner that
    /// re-decides where it is going every round is a planner that never gets
    /// anywhere, and measuring that is what put this here. Cleared when the
    /// goal finishes ([`Goal::finished`]) and wherever [`Self::orders`]
    /// clears, because fresh orders end her own errand too.
    #[serde(default)]
    pub goal: Option<crate::battle::Goal>,
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

    /// Whether her commander has taken personal charge of this vehicle, and
    /// so whether her formation's standing mission still reaches her.
    ///
    /// Named as a question rather than left as `orders.is_some()` because
    /// "detached" is the word the doctrine, the evaluator and the bounding
    /// drill all use for it, and a reader should not have to know that the
    /// state is spelled as the presence of an order.
    pub fn detached(&self) -> bool {
        self.orders.is_some()
    }

    /// Whether she is on the battlefield: targetable, blocking, and still
    /// shooting.
    ///
    /// **Not the question to ask about who came home.** It is false for a
    /// wreck and equally false for a vehicle that withdrew by an exit, and
    /// a passenger answers `true` while standing on no hex anybody may
    /// interact with. Classify an outcome with
    /// [`BattleState::surviving_units`] / [`BattleState::lost_units`] and
    /// ask occupancy through `unit_at` / `occupants`.
    pub fn alive(&self) -> bool {
        self.fate.alive()
    }

    /// Whether she drove off the map by an exit objective she was entitled
    /// to take — off the board and not a loss.
    pub fn exited(&self) -> bool {
        self.fate.exited()
    }

    /// What ended her, or is about to before the tick is reaped.
    pub fn destruction(&self) -> Option<Destruction> {
        self.fate.destruction()
    }

    /// Record what has finished her, to be reaped at the end of the tick.
    ///
    /// Keeps whichever end is the more telling when she has already taken
    /// one ([`Destruction::supersedes`]); a vehicle already off the board is
    /// left alone, because nothing that happens to a wreck changes what
    /// killed her.
    pub fn doomed_by(&mut self, how: Destruction) {
        if let Fate::Fighting { doom } = &mut self.fate
            && doom.is_none_or(|existing| how.supersedes(existing))
        {
            *doom = Some(how);
        }
    }

    /// Take her off the board as a loss, remembered by whatever doomed her —
    /// or, if nothing did, by there being nobody left aboard to fight.
    ///
    /// The one door from [`Fate::Fighting`] to [`Fate::Destroyed`], so that
    /// the reaping is a transition rather than an assignment somebody could
    /// make from anywhere.
    pub fn destroy(&mut self) {
        if let Fate::Fighting { doom } = self.fate {
            self.fate = Fate::Destroyed(doom.unwrap_or(Destruction::CrewSpent));
        }
    }

    /// Take her off the board under her own power. Not a loss, and the
    /// campaign counts her among the survivors.
    pub fn withdraw(&mut self) {
        self.fate = Fate::Exited;
    }

    /// The march she is on, if her commander's order is taking her anywhere
    /// — the ground and the latitude together, or nothing.
    pub fn march(&self) -> Option<crate::battle::March> {
        self.orders.and_then(crate::battle::PersonalOrder::march)
    }

    /// Whether the battle drill may set her personal order aside: take her
    /// off the road for cover, or off the ground she was put on.
    ///
    /// **The one gate, and both drills ask it.** The planner's, at the top
    /// of a round, when it decides whether to plan cover for her instead of
    /// the next leg of her march (`ai/command.rs`); and the engine's own
    /// mid-round reflex (`run_crew_drill`), which fires the tick she notices
    /// a gun and used to read nothing at all, so a crew whose commander had
    /// said "I mean it" pressed on through the planning phase and was then
    /// pulled into the trees by the reflex five seconds later. Latitude buys
    /// an order priority over her *judgment* — that is all it buys, and the
    /// drill is her judgment wherever it runs. It buys nothing over her
    /// nerve: a crew whose rung no longer obeys does what the rung says,
    /// and that check comes before this one at both sites.
    ///
    /// A crew under no personal order yields: the drill is what every crew in
    /// this engine has always done, and the type is what makes "nobody is
    /// insisting on anything" unrepresentable as anything else.
    pub fn yields_to_drill(&self) -> bool {
        self.orders
            .is_none_or(|order| order.latitude().yields_to_drill())
    }
}

/// Why a battle stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndReason {
    /// One side (or every side) was wiped out.
    Eliminated,
    /// The sides lost each other: [`crate::data::Balance::stalemate_rounds`]
    /// rounds passed with no damage dealt and nobody holding an enemy in
    /// sight, so both disengage
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

/// A battle whose `#[serde(skip)]` caches have been rebuilt, said as a type.
///
/// The inner `()` is private, so the only places one can be made are the two
/// constructors below and [`SavedBattle::rehydrate`] — which is the whole
/// point: see [`Battle`].
#[derive(Debug, Clone, Copy)]
pub struct Built(());

/// A battle straight off disk, with every cache still empty.
///
/// Deliberately [`Default`] where [`Built`] is not, because that one
/// asymmetry is what decides which of the two serde is able to produce.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unbuilt;

/// The full battle simulation state, in one of two conditions.
///
/// `C` is not data. It is a marker saying whether the fields this struct
/// declares `#[serde(skip)]` — [`Self::sight`], [`Self::moves`], and the
/// per-unit vision inside [`Self::fog`] — hold anything. [`BattleState`] is
/// the playable battle and is what every signature in the engine names;
/// [`SavedBattle`] is what deserializing produces, and the only thing anybody
/// can do with one is [`SavedBattle::rehydrate`].
///
/// **Why a type rather than a comment.** Those caches are pure functions of
/// the map and the registry, so leaving them out of a save is right; the cost
/// used to be a list of manual duties in `save::rehydrate`, and forgetting one
/// is silent — an empty sight grid answers every line-of-sight question
/// wrongly rather than loudly, an empty move grid says nobody can drive, and
/// an empty `visible_key` panics. Rebuilding needs the [`DataRegistry`], which
/// serde has no way to hand a `Deserialize` impl, so the type system is used
/// to say "not yet": `Unbuilt` is `Default` and `Built` is not, and serde
/// fills a skipped field with `Default::default()`. That single fact makes
/// `Battle<Unbuilt>` deserializable and `Battle<Built>` not, so there is
/// exactly one door from a file to a playable battle and it takes a registry.
///
/// The second half of the obligation — that a cache added tomorrow is
/// rebuilt too — is [`SavedBattle::rehydrate`], which destructures this struct
/// by name. A new field there is a compile error until somebody has said
/// whether it comes off the disk or off the map.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(deserialize = "C: Default"))]
pub struct Battle<C = Built> {
    /// The ground: tiles, and nothing about the fight on them.
    pub map: Arc<HexMap>,
    /// What this fight is about — objectives, formations, loss conditions,
    /// the victory score — kept apart from the ground because the two have
    /// different owners once the ground is one world (WORLD.md, W0.1).
    /// Immutable for the battle, and behind an `Arc` for the reason the map
    /// is: search planners clone the whole state constantly.
    pub scenario: Arc<Scenario>,
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
    /// what it was — a battle with no ground worth taking. [`SavedBattle::rehydrate`]
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
    /// No longer inert: `ai::eval` reads it for the standing mission a crew is
    /// carrying, `ai::command` reads a formation's doctrine and delegation to
    /// decide what she is told and how literally, and rallying reads a
    /// formation's leader. It is state a decision depends on, so it saves and
    /// forks with everything else. `#[serde(default)]` so a save written
    /// before the chain of command existed opens as what it was: one flat pool
    /// per side.
    #[serde(default)]
    pub command: CommandState,
    /// Evidence that the skipped caches above hold something.
    ///
    /// Skipped itself, which is exactly what makes it work: serde reaches for
    /// `Default::default()` here, [`Built`] has no `Default`, and so a
    /// [`BattleState`] cannot be deserialized at all. See [`Battle`].
    #[serde(skip)]
    built: C,
}

/// A battle anybody may play: its caches are built.
pub type BattleState = Battle<Built>;

/// A battle as it comes off disk. See [`Battle`] for why it is a separate
/// type, and [`Self::rehydrate`] for the one thing to do with it.
pub type SavedBattle = Battle<Unbuilt>;

impl SavedBattle {
    /// Put back everything a save deliberately left out, and hand back a
    /// battle that can be asked a question.
    ///
    /// This is the only route from a deserialized battle to a playable one,
    /// and it destructures the struct field by field on purpose: a field
    /// added to [`Battle`] stops this compiling until its author has decided
    /// whether it travels in the file or is rebuilt here. That is the
    /// obligation `save::rehydrate` used to state in prose.
    pub fn rehydrate(self, registry: &DataRegistry) -> BattleState {
        let Battle {
            map,
            scenario,
            sight,
            moves,
            sides,
            roster,
            units,
            round,
            phase,
            mut fog,
            rng,
            over,
            last_contact_round,
            mut objective_held,
            mut score,
            shells,
            command,
            built: Unbuilt,
        } = self;

        // Both grids are pure functions of the map and the terrain
        // definitions. They are rebuilt rather than trusted because a save
        // written before a mod retuned its terrain would otherwise carry the
        // old answer; `is_empty` is checked first only so that a caller who
        // already holds a built grid — a fork inside the engine — pays
        // nothing.
        let sight = if sight.is_empty() {
            Arc::new(SightGrid::build(registry, &map))
        } else {
            sight
        };
        let moves = if moves.is_empty() {
            Arc::new(MoveGrid::build(registry, &map))
        } else {
            moves
        };
        // The fog's per-unit vision and per-side keys are skipped too, and the
        // key list is indexed by side during recompute, so an empty one panics
        // rather than simply recomputing.
        fog.rehydrate();
        // Objective control and the scoreboard are indexed positionally — by
        // objective and by side — and scoring writes through those indices. A
        // save written before objectives existed carries neither, so size them
        // from the map and the sides rather than trusting the file to agree.
        objective_held.resize(scenario.objectives().len(), None);
        score.resize(sides.len(), 0);

        Battle {
            map,
            scenario,
            sight,
            moves,
            sides,
            roster,
            units,
            round,
            phase,
            fog,
            rng,
            over,
            last_contact_round,
            objective_held,
            score,
            shells,
            command,
            built: Built(()),
        }
    }
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

/// Everything wrong with an order of battle that was assembled rather than
/// written down, as the same list of sentences [`crate::map::MapFile::validate_into`]
/// produces for a scenario.
///
/// This is [`BattleState::from_placements`]'s half of the check that
/// [`BattleState::from_map`] gets from map validation. It cannot simply *be*
/// that function: a placement here carries [`CadetId`]s into a roster that
/// already exists, where a map file carries character ids into the registry,
/// and the campaign's roster is the thing that can have lost somebody. So the
/// questions asked are the same four — is that hex on this map, does that
/// chassis exist, is that side real, is that cadet on the roll — and the last
/// two are asked of different books.
///
/// Every problem is collected rather than returned at the first, because a mod
/// that dropped a vehicle usually dropped it from every army that had one and
/// a player fixing content wants the whole list.
fn validate_placements(
    registry: &DataRegistry,
    map: &HexMap,
    sides: &[SideState],
    placements: &[UnitPlacement],
    crews: &[Vec<CadetId>],
    roster: &Roster,
) -> Vec<String> {
    let mut errors = Vec::new();
    for (i, placement) in placements.iter().enumerate() {
        let at = placement.at;
        if !map.contains(crate::offset_to_hex(at[0], at[1])) {
            errors.push(format!(
                "placement at [{}, {}] is outside the map",
                at[0], at[1]
            ));
        }
        if registry.vehicle(&placement.vehicle).is_none() {
            errors.push(format!(
                "placement at [{}, {}] references missing vehicle `{}`",
                at[0], at[1], placement.vehicle
            ));
        }
        if placement.side as usize >= sides.len() {
            errors.push(format!(
                "placement at [{}, {}] references side {} but only {} sides are in this battle",
                at[0],
                at[1],
                placement.side,
                sides.len()
            ));
        }
        // A cadet the roster does not know is not a cosmetic problem: her seat
        // would be empty, substance counts people aboard, and the vehicle
        // would go out about twice as easy to kill for a reason nobody could
        // see. She also cannot come home, because `apply_battle_result` drops
        // crew ids the campaign roster does not know.
        for cadet in crews.get(i).into_iter().flatten() {
            if roster.get(*cadet).is_none() {
                errors.push(format!(
                    "placement at [{}, {}] is crewed by cadet {}, who is not on this roster",
                    at[0], at[1], cadet.0
                ));
            }
        }
    }
    errors
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
        let Battlefield {
            terrain: map,
            scenario,
        } = Battlefield::from_map_file(file)?;
        let sides: Vec<SideState> = file
            .sides
            .iter()
            .map(|s| SideState {
                name: s.name.clone(),
                ai: s.ai.clone(),
            })
            .collect();
        let side_count = sides.len();
        let objective_count = scenario.objectives().len();
        let sight = Arc::new(SightGrid::build(registry, &map));
        let moves = Arc::new(MoveGrid::build(registry, &map));
        // Resolved before the map is moved into its `Arc`, and from the same
        // two things the units are spawned from, so membership cannot drift
        // from the roster it describes.
        let command = CommandState::from_placements(scenario.formations(), &file.units);
        // A scenario battle has no campaign behind it, so its cadets are
        // stamped fresh from mod data and forgotten afterwards.
        let (roster, crews) = Roster::stamp_for(registry, &file.units);
        let mut state = Self {
            map: Arc::new(map),
            scenario: Arc::new(scenario),
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
            built: Built(()),
        };
        for (placement, crew) in file.units.iter().zip(&crews) {
            // `validate_into` above has already said these placements name
            // content this registry has, so this cannot fail here. It is still
            // a `?` rather than an `expect`, because the day somebody adds a
            // check to `spawn_unit` that validation does not make, the error
            // should reach the caller rather than the player's screen.
            state.spawn_unit(registry, placement, crew.clone())?;
        }
        state.face_units_at_enemies(&file.units);
        state.board_mounted_starts(registry, &file.units);
        state.fog = FogMap::new(state.sides.len());
        fog::recompute(registry, &mut state);
        state.open_the_net(registry);
        Ok(state)
    }

    /// Build a battle directly from placements (used by the overworld when
    /// two armies clash on a generated or terrain-picked map).
    ///
    /// Validated, and for the same reasons [`Self::from_map`] is. This is the
    /// campaign's path into a battle, and its order of battle is assembled at
    /// run time out of armies rather than read from a file an author could be
    /// shown warnings about — so a mod that dropped a vehicle between one save
    /// and the next, or a campaign carrying a cadet the roster no longer
    /// knows, arrives here. It used to arrive as a panic inside
    /// [`Self::spawn_unit`]; it is now the same [`BattleSetupError::Invalid`]
    /// a bad scenario map produces, and the presentation layer can say so and
    /// decline to start the battle.
    pub fn from_placements(
        registry: &DataRegistry,
        field: Battlefield,
        sides: Vec<SideState>,
        placements: &[UnitPlacement],
        crews: &[Vec<CadetId>],
        roster: Arc<Roster>,
        seed: u64,
    ) -> Result<Self, BattleSetupError> {
        let Battlefield {
            terrain: map,
            scenario,
        } = field;
        let errors = validate_placements(registry, &map, &sides, placements, crews, &roster);
        if !errors.is_empty() {
            return Err(BattleSetupError::Invalid(errors));
        }
        let side_count = sides.len();
        let objective_count = scenario.objectives().len();
        let sight = Arc::new(SightGrid::build(registry, &map));
        let moves = Arc::new(MoveGrid::build(registry, &map));
        // The formations travel with the scenario for exactly this reason: a
        // field battle the overworld assembles picks them up without this
        // signature growing, the same trip objectives already make. A
        // declaration whose members are not among these placements is dropped
        // rather than carried empty — see `CommandState::from_placements`.
        let command = CommandState::from_placements(scenario.formations(), placements);
        let mut state = Self {
            map: Arc::new(map),
            scenario: Arc::new(scenario),
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
            built: Built(()),
        };
        for (i, placement) in placements.iter().enumerate() {
            state.spawn_unit(
                registry,
                placement,
                crews.get(i).cloned().unwrap_or_default(),
            )?;
        }
        state.face_units_at_enemies(placements);
        state.board_mounted_starts(registry, placements);
        state.fog = FogMap::new(state.sides.len());
        fog::recompute(registry, &mut state);
        state.open_the_net(registry);
        Ok(state)
    }

    /// Pass up what the side can already see, before anybody plans round
    /// one.
    ///
    /// Both setup paths used to compute the fog and stop, so under command
    /// rules the picture did not exist until the first tick had run: the
    /// commander planned round one with an empty picture, and the player's
    /// screen — which draws the picture — showed no enemy at all, even one
    /// standing in plain sight of her whole formation (on `river_crossing`,
    /// four or five of them). Nobody noticed because the AI read the pooled
    /// fog instead; once the brains read the picture too, a blind round one
    /// is a blind AI.
    ///
    /// Only the picture and who can speak to it. Who has *lost contact* is
    /// still settled on the first tick, because a crew who starts cut off is
    /// news and that is where she is announced. The `ContactReported`
    /// events are dropped: a sighting in hand when the battle opens is the
    /// state of things, not something that just happened. With no `command`
    /// block both calls return at once, which is the game as it was.
    fn open_the_net(&mut self, registry: &DataRegistry) {
        self.recompute_voices(registry);
        self.recompute_picture(registry, &mut Vec::new());
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

    /// Put one placement on the board.
    ///
    /// Fallible rather than panicking on a chassis the registry has never
    /// heard of. Both setup paths validate before they get here, so in
    /// practice this only fires if a check is added to one and not the other —
    /// but it used to be an `expect`, and the campaign's path did not validate
    /// at all, so a mod that dropped a vehicle took the game down instead of
    /// declining the battle.
    pub fn spawn_unit(
        &mut self,
        registry: &DataRegistry,
        placement: &UnitPlacement,
        crew: Vec<CadetId>,
    ) -> Result<UnitId, BattleSetupError> {
        let id = UnitId(self.units.len() as u32);
        let vehicle = registry.vehicle(&placement.vehicle).ok_or_else(|| {
            BattleSetupError::Invalid(vec![format!(
                "placement at [{}, {}] references missing vehicle `{}`",
                placement.at[0], placement.at[1], placement.vehicle
            )])
        })?;
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
            fate: Fate::default(),
            aboard: None,
            boarding: None,
            dismounting: false,
            last_hit_by: None,
            pressure: 0,
            buttoned: 0,
            orders: None,
            goal: None,
        });
        Ok(id)
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
    /// Three answers, not two, since the muster existed: she is fit, she is
    /// not fit and staying behind, or she is not fit and her academy has
    /// called her up anyway — [`crate::roster::Cadet::called_up`], which only
    /// a muster ever writes and only on the copy of the roster it hands the
    /// battle. A cadet called up rides as [`CrewCondition::Wounded`]: at her
    /// station, and worse at it, which is what that rung has always meant.
    /// Nobody called up is the game as it was.
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
        let seat = |id: &CadetId| match self.roster.get(*id) {
            // Nobody the campaign owns — an anonymous crew, a scenario
            // battle — climbs in as she always did.
            None => CrewCondition::Fine,
            Some(cadet) if cadet.status.is_ready() => CrewCondition::Fine,
            // Her academy put her on the roll anyway. She rides with her
            // wound, which is the one thing `CrewCondition::Wounded` has
            // always meant: at her station, and worse at it.
            Some(cadet) if cadet.called_up => CrewCondition::Wounded,
            Some(_) => CrewCondition::Absent,
        };
        let states: Vec<CrewCondition> = crew.iter().map(seat).collect();
        // Nobody was kept out of a seat, so there is nothing to record and
        // every battle written before wounds could keep anybody out is byte
        // for byte what it was.
        if states.iter().all(|c| *c == CrewCondition::Fine) {
            return Vec::new();
        }
        // ...and if the answer is that nobody at all climbs in, the walking
        // wounded go out anyway, exactly as they did before there was
        // anything to call up: the engine refusing to produce an empty
        // vehicle is not the academy deciding to send anybody, so nothing is
        // charged for it.
        if states.iter().all(|c| *c == CrewCondition::Absent) {
            return Vec::new();
        }
        states
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
        self.units.get(id.index()).filter(|u| u.alive())
    }

    pub fn unit_mut(&mut self, id: UnitId) -> Option<&mut Unit> {
        self.units.get_mut(id.index()).filter(|u| u.alive())
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
            .filter(move |u| u.alive() && u.aboard.is_none() && u.pos == hex)
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
        let mine = registry
            .vehicle(&unit.vehicle)
            .map(|v| v.footprint())
            .unwrap_or(1);
        let taken: u32 = others
            .iter()
            .filter_map(|o| registry.vehicle(&o.vehicle))
            .map(|v| v.footprint())
            .sum();
        // The rule itself lives in `movement`, shared with the index a
        // whole-map sweep gathers, so there is one answer to "is there room"
        // however the counting was done.
        movement::fits(others.len() as u32, taken, capacity, mine)
    }

    /// Everyone riding in `carrier`, in id order.
    pub fn passengers(&self, carrier: UnitId) -> Vec<UnitId> {
        self.units
            .iter()
            .filter(|u| u.alive() && u.aboard == Some(carrier))
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
        self.map.get(hex).map(|t| t.terrain)
    }

    pub fn alive_units(&self) -> impl Iterator<Item = &Unit> {
        self.units.iter().filter(|u| u.alive())
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
        self.units.iter().filter(|u| !u.fate.lost())
    }

    /// Vehicles actually destroyed — the complement of
    /// [`Self::surviving_units`], and never merely `!alive`.
    pub fn lost_units(&self) -> impl Iterator<Item = &Unit> {
        self.units.iter().filter(|u| u.fate.lost())
    }

    pub fn is_over(&self) -> bool {
        self.over.is_some()
    }

    /// Every objective on this map paired with the side currently holding it.
    pub fn objectives(&self) -> impl Iterator<Item = (&crate::map::Objective, Option<u8>)> {
        self.scenario
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
            &unit.crew_state,
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
                &unit.crew_state,
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
            &unit.crew_state,
            &rules.skill,
            terrain,
        );
        // Closed up, she sees the world through glass and is late to it.
        let buttoned = if unit.buttoned > 0 {
            registry.balance.buttoned_reaction_ticks
        } else {
            0
        };
        rules.delay(level) + buttoned
    }

    /// How well this crew moves on her own feet, used by
    /// [`super::move_points`] for a chassis that walks.
    ///
    /// A separate question from [`driving`], and not a nicety: a
    /// `rifle_platoon` fields a platoon leader and a section leader and no
    /// driver at all, so asking her for `driving` took `crew_skill`'s
    /// "nobody is in that seat" path and charged her the stand-in penalty for
    /// a seat her chassis has never had.
    pub fn athletics(
        registry: &DataRegistry,
        roster: &Roster,
        unit: &Unit,
        terrain: Option<&str>,
    ) -> i32 {
        roster.crew_skill(
            registry,
            registry.vehicle(&unit.vehicle),
            &unit.crew,
            &unit.crew_state,
            "athletics",
            terrain,
        )
    }

    /// How well this crew hides, which scales her chassis's own
    /// `concealment` rather than standing on its own.
    pub fn fieldcraft(
        registry: &DataRegistry,
        roster: &Roster,
        unit: &Unit,
        terrain: Option<&str>,
    ) -> i32 {
        roster.crew_skill(
            registry,
            registry.vehicle(&unit.vehicle),
            &unit.crew,
            &unit.crew_state,
            "fieldcraft",
            terrain,
        )
    }

    /// How well this platoon's riflemen shoot.
    pub fn small_arms(
        registry: &DataRegistry,
        roster: &Roster,
        unit: &Unit,
        terrain: Option<&str>,
    ) -> i32 {
        roster.crew_skill(
            registry,
            registry.vehicle(&unit.vehicle),
            &unit.crew,
            &unit.crew_state,
            "small_arms",
            terrain,
        )
    }

    /// How quickly this crew gets the next round into the breech.
    ///
    /// Read in exactly one place — [`super::combat::crewed_reload`] — because
    /// a reload reaches the game as a cooldown *and* as a cadence, and those
    /// two must not be able to disagree.
    pub fn loading(
        registry: &DataRegistry,
        roster: &Roster,
        unit: &Unit,
        terrain: Option<&str>,
    ) -> i32 {
        roster.crew_skill(
            registry,
            registry.vehicle(&unit.vehicle),
            &unit.crew,
            &unit.crew_state,
            "loading",
            terrain,
        )
    }

    /// How well this crew mends what is broken, used by the field-repair
    /// rule at the top of every round.
    pub fn maintenance(
        registry: &DataRegistry,
        roster: &Roster,
        unit: &Unit,
        terrain: Option<&str>,
    ) -> i32 {
        roster.crew_skill(
            registry,
            registry.vehicle(&unit.vehicle),
            &unit.crew,
            &unit.crew_state,
            "maintenance",
            terrain,
        )
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
            &unit.crew_state,
            "driving",
            terrain,
        )
    }
}
