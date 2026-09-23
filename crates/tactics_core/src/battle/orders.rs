//! Intents in, events out: the planning and resolution boundary.
//!
//! During planning, [`BattleState::apply`] records what units are told to do
//! without moving anything. When every side has committed, resolution runs in
//! ticks via [`BattleState::step_tick`], and everyone's orders play out
//! together.

use super::{
    BattleResult, BattleState, EndReason, FormationId, Goal, Latitude, March, Mission,
    PersonalOrder, Phase, Unit, UnitId, combat, fog, movement,
};
use crate::data::{ArmorFacing, DataRegistry, ShotFelt};
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
    /// Whether this route is the crew's own idea rather than an order.
    ///
    /// Only two things lay one: the battle drill breaking a crew off for
    /// cover, and a frightened crew reversing out of contact. Both matter
    /// here for the same reason — **a crew cannot refuse her own decision**.
    /// Without this flag a crew who broke and ran would find her own flight
    /// path sitting in her intent on the next tick, be selected by the
    /// refusal check as somebody with orders she will not follow, throw the
    /// path away and lay it again, forever.
    ///
    /// `#[serde(default)]` so a save written before this field loads as what
    /// every route in it was: an order.
    #[serde(default)]
    pub own_idea: bool,
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
    /// Record what this crew is trying to achieve.
    ///
    /// It moves nothing on its own — the movement toward a goal is an
    /// ordinary [`Self::SetMove`] — and it exists so that the intention
    /// travels the same road every other decision in this engine travels: in
    /// through the order stream, out through the event log, into the save
    /// file, onto the replay. A planner that kept its goals privately would
    /// be a planner whose behaviour changed when the player saved the game,
    /// which is what `tests/save.rs` is there to catch.
    SetGoal { unit: UnitId, goal: Goal },
    /// The commander's own order to one crew, carried by the wire.
    ///
    /// Identical to [`Self::SetMove`] plus [`Self::SetFire`] for a cadet who
    /// can hear it, and *held at the radio* for one who cannot: it waits in
    /// her formation's queue and is delivered at the first planning phase she
    /// is back in contact for.
    ///
    /// The distinction between this and the two orders above is the whole
    /// point of it, and it is a distinction about **who is speaking** rather
    /// than about what is said. A planner issuing `SetMove` is a crew's own
    /// judgment about her own tank — she does not need to be radioed her own
    /// decision, and gating it on contact would make a cut-off cadet freeze
    /// instead of soldiering on. This is the *commander* talking, and a
    /// commander who cannot be heard has not given an order yet. The engine
    /// cannot tell one caller from another, so the caller says which it is by
    /// which order it sends.
    ///
    /// Either half may be `None`: an order about the route says nothing about
    /// the target, and vice versa.
    ///
    /// `latitude` is how hard the commander meant the *route* — whether the
    /// crew may break the march off for cover when she comes under fire.
    /// [`Latitude::Delegated`] is what every order in this engine has always
    /// been and remains the default; see [`Latitude`] for why the choice
    /// belongs to the commander rather than to a doctrine weight.
    Radio {
        unit: UnitId,
        to: Option<Hex>,
        fire: Option<FireIntent>,
        latitude: Latitude,
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
        /// How hard the order is meant — see [`crate::battle::Latitude`] and
        /// [`crate::battle::Formation::latitude`]. `#[serde(default)]` and
        /// defaulting to `Delegated`, so every order stream, save and replay
        /// written before missions had latitude means what it always meant.
        #[serde(default)]
        latitude: Latitude,
    },
    /// "…and then this": add a mission to the end of a formation's plan
    /// instead of replacing it. Refused behind a terminal mission — nothing
    /// follows a stand-fast or a retreat — and travels the wire exactly as a
    /// replacement does; the radio does not care what the envelope says.
    QueueMission {
        formation: FormationId,
        mission: Mission,
        #[serde(default)]
        latitude: Latitude,
    },
    /// Go and board this carrier: a standing order — she marches toward it
    /// round after round and steps aboard the tick she arrives alongside.
    /// Refused for anything that is not a foot unit boarding a transport of
    /// her own side with room left.
    Mount { unit: UnitId, into: UnitId },
    /// Get off, onto the first free tile beside the carrier, at the next
    /// transport pass. Dismounting into an ambush is a thing that happens.
    Dismount { unit: UnitId },
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
        /// The gun was laid from a vehicle under way this round.
        ///
        /// A flag beside `blind` and `opportunity` because it is the same
        /// kind of fact — a circumstance of the shot that the player is
        /// entitled to see in the log and the balance harness is entitled
        /// to count. Everything it costs is already priced into the hit
        /// chance; this only says so out loud, so that "half my shots
        /// missed" can be answered with *why*.
        #[serde(default)]
        moving: bool,
    },
    /// A shell fired one or more ticks ago has arrived on the ground it was
    /// aimed at.
    ///
    /// Side-blind on purpose: a shellburst is a column of earth and smoke
    /// that everybody on the field can see, whoever fired it and whoever it
    /// was meant for. What follows in the same tick is the ordinary
    /// vocabulary — a penetration or a bounce for whoever was standing
    /// there, module hits for the neighbours — so nothing downstream has to
    /// learn a second way to read damage. There is no `ShotMissed` twin:
    /// artillery cannot miss a hex, it can only be aimed at the wrong one.
    ShellLanded {
        attacker: UnitId,
        at: Hex,
        /// The round, by [`crate::data::AmmoDef`] id, so the log can name
        /// what came down.
        ammo: String,
    },
    /// The round penetrated. There is no hit-point arithmetic behind this
    /// any more: `damage` is the behind-armor budget the outcome engine
    /// spent on the crew and modules, and the events that follow this one
    /// in the same tick — crew hits, module hits, a brew-up, an
    /// abandonment — are what it actually did.
    ShotHit {
        attacker: UnitId,
        target: UnitId,
        damage: i32,
        /// What the round could have spent — its datasheet budget, before
        /// the partial-penetration band and the platoon's muster took their
        /// shares. `damage` over this is what the morale ladder charges the
        /// outcome price by, so a shell that scraped through frightens less
        /// than one that came through clean, and the log can say "4 of 6".
        #[serde(default)]
        budget: i32,
        facing: ArmorFacing,
        /// The round that arrived, by [`crate::data::AmmoDef`] id. `None` on
        /// the legacy path, where a weapon with no ammunition list fires
        /// something the mod never named.
        ///
        /// Here because pressure is priced off the round: what a shot costs
        /// a crew's nerve is the ladder's price for the outcome plus the
        /// round's own [`crate::data::AmmoDef::suppression`], and
        /// `apply_pressure` reads the events rather than being called from
        /// inside the shooting code. Naming the round is also the honest
        /// record — a log that can say *which* shell came through is worth
        /// more than one that says a shell did.
        #[serde(default)]
        ammo: Option<String>,
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
        /// The round that struck, by [`crate::data::AmmoDef`] id — see
        /// [`Self::ShotHit`]'s field of the same name. A belt that declares
        /// suppression frightens a crew through this even though `rattled`
        /// is false, which is the designer overruling the plinking rule per
        /// round rather than the engine deciding it per class.
        #[serde(default)]
        ammo: Option<String>,
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
    /// A cadet aboard was hit. Named — the who/what/why rule at its most
    /// important, since permadeath without a name is just a number going
    /// down. `out` false is wounded and still at her station; `out` true is
    /// the battle's whole verdict, with dead-or-unconscious resolved by the
    /// roster when the shooting stops.
    CrewHit {
        unit: UnitId,
        cadet: crate::roster::CadetId,
        out: bool,
    },
    /// Something inside (or, for blast against the hull, outside) broke.
    /// `destroyed` false is damaged-but-working-worse; the module id names
    /// what, so the log can say "the tracks" rather than "a subsystem".
    ModuleHit {
        unit: UnitId,
        module: String,
        destroyed: bool,
    },
    /// Somebody aboard got a broken module working again between rounds.
    ///
    /// The other half of `ModuleHit { destroyed: true }`, and the reason
    /// `Unit::modules` has always distinguished a module at zero hits from
    /// one that is absent: a crew who lost her wireless set and a crew who
    /// never had one look the same until there is a rule that can give one
    /// of them hers back.
    ModuleRepaired {
        unit: UnitId,
        module: String,
    },
    /// The ammunition went up. The vehicle is destroyed on the spot, and
    /// the crew's fate rolls carry the fire.
    BrewedUp {
        unit: UnitId,
    },
    /// The crew has had enough and left the vehicle. A wreck for scoring —
    /// the side has lost a tank — but the cadets are walking home, which is
    /// a different day entirely from burning in it, and the log must never
    /// let the two read alike.
    Abandoned {
        unit: UnitId,
    },
    /// She stepped aboard. From this tick she is off the map in every sense
    /// the enemy cares about, and shares whatever comes through the
    /// carrier's armor.
    Mounted {
        unit: UnitId,
        into: UnitId,
    },
    /// She stepped off, at `at` — by order, or because the ride ended the
    /// hard way and this is where the survivors picked themselves up.
    Dismounted {
        unit: UnitId,
        at: Hex,
    },
    /// The round missed what it was aimed at and found somebody else standing
    /// on the same ground.
    ///
    /// Its own event rather than a flag on [`Self::ShotHit`] because the two
    /// facts a reader needs are *who was shot at* and *who was hit*, and a
    /// hit that quietly named a different unit than the `ShotFired` before it
    /// would read as the log contradicting itself. A `ShotMissed` for the
    /// intended target still precedes it: she was missed, and that is true.
    ShotStrayed {
        attacker: UnitId,
        intended: UnitId,
        onto: UnitId,
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
    /// A crew set out for somewhere on her own initiative.
    ///
    /// Emitted when the goal *changes*, not every round she is carrying one,
    /// because "she is still driving to the ford" is not news. Carries the
    /// wording rather than the hex so a log can be read without a map in the
    /// other hand: [`crate::battle::Goal::describe`] names the objective when
    /// the ground has a name.
    ///
    /// This is the first thing the AI in this game has ever done that is
    /// explainable in a sentence, which is most of the argument for having
    /// it. A vehicle driving across the map with nothing in the log behind it
    /// is indistinguishable from a bug — the same bargain the drill's
    /// provenance flag was plumbed for.
    SetOut {
        unit: UnitId,
        goal: Goal,
        doing: String,
    },
    /// A crew stopped doing what she was told, and what she did instead.
    ///
    /// Never silent, for two different reasons. An order that quietly fails
    /// is indistinguishable from a bug — that was always true. And a vehicle
    /// reversing out of the line with no order behind it is worse than a
    /// silent failure: it looks like the game moving her for no reason,
    /// which is the exact complaint the direction memo was written about.
    ///
    /// It was called `OrderRefused` while freezing was the only thing a
    /// broken crew could do, and the name became a lie the moment one could
    /// act without having been told anything.
    Defied {
        unit: UnitId,
        rung: String,
        /// The [`crate::data::DefianceDef::name`] she acted on — "falls
        /// back", "fights on", "goes to ground". A fragment that reads after
        /// her name.
        doing: String,
        /// Where she took herself, when her defiance was the moving kind.
        to: Option<Hex>,
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
    /// A radioed order could not reach this cadet and is waiting at the radio
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
    /// commander: `unit` is the enemy, `by` is the cadet who filed it, `at` is
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
    /// A formation's leader is off the field and the next cadet in its order of
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

impl Event {
    /// Whether `side` is entitled to know this happened.
    ///
    /// **This is a rule about knowledge, not about presentation**, which is
    /// why it lives here beside the events rather than in whichever screen
    /// happens to be drawing them. Two divisions run through it:
    ///
    /// - **Fighting is side-blind.** A shot, a spot, a wreck, a brew-up, a
    ///   crew baling out — anybody on the field can see those, and the fog
    ///   already decides whether a given unit is visible at all. Nothing here
    ///   needs to re-ask that question.
    /// - **Command traffic is a side's own business.** Orders, contact
    ///   troubles, the radio queue, where a crew has decided to go, and what
    ///   is broken or bleeding inside her hull are all on her own net.
    ///   Listening to the enemy's is the electronic-warfare future, not a
    ///   freebie.
    ///
    /// **The match is exhaustive on purpose.** A catch-all would give every
    /// event added tomorrow an audience by default instead of by decision,
    /// and that is not hypothetical: three variants rode a `_ => true` for
    /// months, two of which were printing an enemy crew's morale rung into
    /// the player's log. A new event should fail to compile until somebody
    /// has said who hears it — the same bargain `Mission::slot` makes.
    ///
    /// It has **two callers that must not drift**, and the second is not
    /// tidiness: the log filters with this after draining, and the game's AI
    /// driver filters with it *before queueing*, because planning events go
    /// through the same paced animation queue as combat and an event nobody
    /// will print still costs the player a beat of not being able to give
    /// orders.
    ///
    /// An id this battle does not know is heard by everybody. That is the
    /// safe direction for a stray event: saying too much in a log is a bug,
    /// and silently dropping an event because a lookup missed is a bug that
    /// looks like the game freezing.
    pub fn heard_by(&self, state: &BattleState, side: u8) -> bool {
        let own_formation = |formation: &str| {
            state
                .formations()
                .iter()
                .find(|f| f.id == formation)
                .is_none_or(|f| f.side == side)
        };
        let own_unit = |unit: &UnitId| state.units.get(unit.index()).is_none_or(|u| u.side == side);
        match self {
            // The clock, and the end of it. Both sides fight the same battle.
            Event::RoundStarted { .. } | Event::TickStarted { .. } | Event::BattleEnded { .. } => {
                true
            }

            // Things that happen in the open. Whether the *unit* can be seen
            // is the fog's question and it has already been asked; a crew
            // driving, bogging down, shooting, being hit, burning, baling
            // out, mounting, dismounting, dying or driving off the board is
            // not a secret from anybody who can see her.
            Event::UnitMoved { .. }
            | Event::UnitTrapped { .. }
            | Event::ShotFired { .. }
            | Event::ShellLanded { .. }
            | Event::ShotHit { .. }
            | Event::ShotBounced { .. }
            | Event::ShotStrayed { .. }
            | Event::ShotMissed { .. }
            | Event::BrewedUp { .. }
            | Event::Abandoned { .. }
            | Event::Mounted { .. }
            | Event::Dismounted { .. }
            | Event::UnitDestroyed { .. }
            | Event::UnitExited { .. } => true,

            // Who holds the bridge is not a secret from the side that does
            // not: an objective changing hands is a fact about the ground.
            Event::ObjectiveTaken { .. } => true,

            // Her formation's net: what it was told, what it heard, what it
            // finished, and who is commanding it now.
            Event::MissionAssigned { formation, .. }
            | Event::MissionReceived { formation, .. }
            | Event::MissionCompleted { formation, .. }
            | Event::CommandPassed { formation, .. } => own_formation(formation),

            // Her own net, and her own hull. How much ammunition she has
            // left is her quartermaster's secret, not something the sound of
            // her gun gives away; what is broken or bleeding inside her even
            // more so. Where she has decided to go and whether her radio is
            // working are the same kind of fact.
            Event::TookCover { unit, .. }
            | Event::WeaponDry { unit, .. }
            | Event::CrewHit { unit, .. }
            | Event::ModuleHit { unit, .. }
            | Event::ModuleRepaired { unit, .. }
            | Event::SetOut { unit, .. }
            | Event::OutOfContact { unit }
            | Event::OrdersWaiting { unit }
            | Event::OrdersDelivered { unit }
            | Event::ContactRestored { unit }
            // Her nerve is inside the hull with everything else. You can see
            // her tank reverse out of the line — `UnitMoved` is side-blind
            // and the sprite does it in front of you — and you may draw your
            // own conclusion from that; what you cannot do is read the rung
            // she is standing on. This is the same rule `CrewHit` and
            // `ModuleHit` already follow, and morale was the lone exception
            // to it: you could not see inside her tank but you could read her
            // nerve.
            | Event::MoraleChanged { unit, .. }
            | Event::Defied { unit, .. } => own_unit(unit),

            // A spot report belongs to whoever made it. `by` and not `unit`:
            // the interesting party is the crew doing the reporting, and the
            // unit being reported is by definition the other side's.
            Event::ContactReported { by, .. } => own_unit(by),

            // A spot belongs to the side that made it. Finding somebody is
            // the reward for looking, and being found is not something the
            // found party gets told — she learns it when the shooting starts.
            //
            // This used to be answered in the renderer instead, by asking
            // whether `by_side` was a side with no AI on it. That is a
            // different question wearing the same clothes — "was the spotter
            // human-controlled" rather than "was the spotter mine" — and the
            // two part company the moment more than one side has `ai: None`,
            // which the game crate's own tests construct and which a field
            // battle with reinforcing neighbours makes reachable in play.
            Event::UnitSpotted { by_side, .. } => *by_side == side,

        }
    }
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
    #[error("that vehicle carries nobody")]
    NotATransport,
    #[error("the carrier is full")]
    TransportFull,
    #[error("only foot units ride")]
    CannotRide,
    #[error("she is already aboard a carrier")]
    AlreadyAboard,
    #[error("she is not aboard anything")]
    NotAboard,
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
            Order::SetGoal { unit, goal } => {
                self.planning_unit_side(*unit)?;
                let changed = self.unit(*unit).is_some_and(|u| u.goal != Some(*goal));
                if let Some(u) = self.unit_mut(*unit) {
                    u.goal = Some(*goal);
                }
                Ok(if changed {
                    vec![Event::SetOut {
                        unit: *unit,
                        goal: *goal,
                        doing: goal.describe(&self.map),
                    }]
                } else {
                    Vec::new()
                })
            }
            Order::Radio {
                unit,
                to,
                fire,
                latitude,
            } => self.radio(registry, *unit, *to, *fire, *latitude),
            Order::ClearIntent { unit } => {
                self.planning_unit_side(*unit)?;
                // Not sending is free, so taking back what has not gone out
                // needs no contact at all: the message never leaves the radio.
                self.command.drop_orders(*unit);
                // What it cannot do is reach *her*. A cadet off the net is
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
                // The recall: she rejoins her formation's tasking. The
                // destination and the insistence go with the order they
                // belonged to, because they *are* the order — a recalled
                // crew is under nobody's, and there is no longer any way to
                // drop one of the three and leave the others standing.
                u.orders = None;
                Ok(Vec::new())
            }
            Order::Mount { unit, into } => {
                self.planning_unit_side(*unit)?;
                let rider = self.unit(*unit).ok_or(OrderError::NoSuchUnit)?;
                let carrier = self.unit(*into).ok_or(OrderError::NoSuchUnit)?;
                if rider.aboard.is_some() {
                    return Err(OrderError::AlreadyAboard);
                }
                if carrier.side != rider.side {
                    return Err(OrderError::FriendlyTarget);
                }
                let vehicle = registry
                    .vehicle(&carrier.vehicle)
                    .ok_or(OrderError::NoSuchUnit)?;
                if vehicle.capacity == 0 {
                    return Err(OrderError::NotATransport);
                }
                if self.passengers(*into).len() as u32 >= vehicle.capacity {
                    return Err(OrderError::TransportFull);
                }
                let on_foot = registry
                    .vehicle(&rider.vehicle)
                    .is_some_and(|v| v.movement.class == crate::data::MovementClass::Foot);
                if !on_foot {
                    return Err(OrderError::CannotRide);
                }
                let carrier_pos = carrier.pos;
                if let Some(u) = self.unit_mut(*unit) {
                    u.boarding = Some(*into);
                    u.dismounting = false;
                    u.planned = true;
                }
                // Start walking now: the standing order re-paths her every
                // round after this, exactly as a personal tasking does.
                self.march_toward(registry, *unit, carrier_pos);
                Ok(Vec::new())
            }
            Order::Dismount { unit } => {
                self.planning_unit_side(*unit)?;
                let rider = self.unit(*unit).ok_or(OrderError::NoSuchUnit)?;
                if rider.aboard.is_none() {
                    return Err(OrderError::NotAboard);
                }
                if let Some(u) = self.unit_mut(*unit) {
                    u.dismounting = true;
                    u.planned = true;
                }
                Ok(Vec::new())
            }
            Order::SetMission {
                formation,
                mission,
                latitude,
            } => self.set_mission(registry, *formation, mission.clone(), *latitude),
            Order::QueueMission {
                formation,
                mission,
                latitude,
            } => self.push_mission(registry, *formation, mission.clone(), *latitude),
            Order::Commit { side } => self.commit(*side),
        }
    }

    /// Whether a cadet can currently hear her chain of command.
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
        if let Some(step) = movement::step_toward(registry, self, id, destination) {
            let _ = self.set_move(registry, id, step);
        }
    }

    fn radio(
        &mut self,
        registry: &DataRegistry,
        id: UnitId,
        to: Option<Hex>,
        fire: Option<FireIntent>,
        latitude: Latitude,
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
                    // The destination and the latitude land as one value, so
                    // the rule that they travel together is no longer
                    // something this line has to remember.
                    unit.orders = Some(PersonalOrder::Marching(March { to, latitude }));
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
            //
            // `get_or_insert` and not an assignment, because an order that
            // says nothing about where she is going has said nothing about
            // her march: a fire mission to a crew already marching leaves the
            // march — and so the insistence it carries — exactly as it was.
            if let Some(unit) = self.unit_mut(id) {
                unit.orders.get_or_insert(PersonalOrder::Holding {
                    latitude: Latitude::Delegated,
                });
            }
            return Ok(Vec::new());
        }
        self.command
            .hold_orders(id, to.map(|to| March { to, latitude }), fire);
        Ok(vec![Event::OrdersWaiting { unit: id }])
    }

    /// Hand out every radioed order whose cadet is back on the net.
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
            // a cadet who is not coming back is not news, it is an epitaph.
            if self.unit(unit).is_none() {
                continue;
            }
            if !self.hears_orders(unit) {
                undelivered.push((unit, orders));
                continue;
            }
            if let Some(march) = orders.march {
                // The promise of the queue, kept: however far she has come
                // in the meantime, the delivered order is a destination she
                // now marches for — this round as far as the round allows,
                // and every round after until she arrives — under the
                // latitude it was sent with, which travelled inside it.
                if let Some(u) = self.unit_mut(unit) {
                    u.orders = Some(PersonalOrder::Marching(march));
                }
                self.march_toward(registry, unit, march.to);
            }
            // The target may have burned while the order was in the drawer.
            if let Some(fire) = orders.fire
                && self.check_fire(registry, unit, fire).is_ok()
            {
                let _ = self.set_fire(registry, unit, fire);
            }
            // Delivery is when the personal tasking takes hold — until the
            // order reached her she was soldiering the standing mission,
            // which is exactly what the queue promised. `get_or_insert` for
            // the same reason as in `radio`: a fire order arriving for a crew
            // already marching detaches her without touching her march.
            if let Some(u) = self.unit_mut(unit) {
                u.orders.get_or_insert(PersonalOrder::Holding {
                    latitude: Latitude::Delegated,
                });
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
        latitude: Latitude,
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
            self.command
                .set_mission(formation, mission.clone(), latitude);
        } else {
            self.command.set_incoming(
                formation,
                crate::battle::MissionChange::Replace {
                    mission: mission.clone(),
                    latitude,
                },
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
                unit.orders = None;
                // Fresh orders end her own errand too. A crew still driving
                // to ground her *old* mission made sense of is the failure
                // mode a committed goal introduces, and this is where it is
                // shut.
                unit.goal = None;
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
        latitude: Latitude,
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
            self.command
                .queue_mission(formation, mission.clone(), latitude);
        } else {
            self.command.set_incoming(
                formation,
                crate::battle::MissionChange::Append {
                    mission: mission.clone(),
                    latitude,
                },
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
        // Somebody told her to go there, so it is refusable again — an order
        // arriving over the top of a crew's own dash is still an order.
        unit.intent.own_idea = false;
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
    /// before it is queued: an order held for a cadet who cannot hear it has to
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
        for unit in self.units.iter_mut().filter(|u| u.alive()) {
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
        // Boarding, dismounting, and keeping every passenger's position
        // mirrored on her carrier — after movement so a rider who walked
        // alongside this tick steps up this tick, and before the fog
        // recompute so she vanishes from (or reappears in) the enemy's
        // picture in the same slice of time she changed state.
        self.resolve_transport(registry, &mut events);

        // The shells fired one or more ticks ago come down here, AFTER
        // movement: a shell lands on whoever is standing on the hex once
        // this tick's driving is done, so even the shortest flight gives a
        // moving target one real chance to be somewhere else.
        //
        // This placement was measured against the alternative, not assumed.
        // Shooting is aimed at the end of a tick, so a shell brought down at
        // the top of tick T + n before movement gives its target only n - 1
        // chances to leave — and at 470 m/s the flight rounds to ONE tick
        // inside 2.4 km, meaning zero chances: with the to-hit roll gone
        // from the shell path, top-of-tick impact made artillery *better*
        // than the hitscan sniper it was (36 games: 53 kills before flight
        // time, 63 with impact before movement, 23 with this line here).
        // The design doc's claim — flight time turns artillery into an area
        // weapon that punishes standing still — is a claim about THIS
        // placement, and 23 kills against 13 losses is an area weapon,
        // still the best gun on the field but no longer a rifle. B4 tunes
        // from here.
        combat::resolve_shells(registry, self, &mut events);
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

        // The stalemate clock measures PROGRESS, not proximity. It used to
        // reset on mere mutual spotting, and the post-hit-point physics
        // made that a livelock: two survivors neither can kill hold each
        // other in sight forever — the loader happily harassing tracks
        // with high explosive — and a battle that will never produce
        // another loss never ends. Now only fighting that changes
        // something resets it: hits, anything breaking or burning, anyone
        // dying or leaving. Crews staring at each other across a field
        // with dry racks or hopeless guns wind the clock down exactly like
        // crews that lost contact, and the score decides what the staring
        // was worth.
        //
        // A bounce is deliberately NOT on that list, and it used to be, on
        // the reading that the guns are still trying. Trying is not
        // progress. The playthrough review caught what that costs: a
        // howitzer lobbing at a plate it cannot beat reset this clock every
        // round for twenty-three rounds after the last thing its bursts
        // could reach was already broken, so one AI mispricing bought eight
        // extra rounds of wandering on top of the barrage itself. Note the
        // livelock this list was written against is still shut out, because
        // a bounce that achieves anything announces the achievement
        // separately — `ModuleHit` and `CrewHit` are both still here, and
        // overpressure raises them from outside the plate. The rule this
        // now states is the honest one: a gun that is *accomplishing*
        // something keeps the battle alive, a gun that is merely firing
        // does not.
        let progress = events.iter().any(|e| {
            matches!(
                e,
                Event::ShotHit { .. }
                    | Event::CrewHit { .. }
                    | Event::ModuleHit { .. }
                    | Event::BrewedUp { .. }
                    | Event::Abandoned { .. }
                    | Event::UnitDestroyed { .. }
                    | Event::UnitExited { .. }
            )
        });
        if progress {
            self.last_contact_round = self.round;
        }
        self.check_victory(registry, &mut events);
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

    /// The ground a frightened crew reverses onto: as far from everything
    /// she can see that can hurt her as this round's movement allows, and
    /// among the hexes that tie on that, the quietest.
    ///
    /// Distance from *threats*, not toward a lane. A retreat lane is a
    /// destination, and a crew who has stopped listening is not navigating —
    /// she is getting away from the thing that is shooting at her. It is
    /// also what survives the map growing: there will not always be an edge
    /// to run off, and "away" needs no map furniture at all.
    ///
    /// Ties break toward the ground the guns can do least on, then the
    /// cheapest drive, then coordinates, so two identical crews in identical
    /// fear still bolt somewhere a replay agrees about.
    ///
    /// **Distance is the first key and the second one is the currency.** The
    /// tiebreak used to be terrain `cover` — a number in a model of its own,
    /// which can prefer a wood the gun sees into perfectly well over bare
    /// ground it cannot see at all. It is
    /// [`incoming`](crate::battle::incoming) now, so among the hexes that put
    /// the same distance between her and the thing shooting at her she takes
    /// the one where a round of that fire is worth least, cover and elevation
    /// and facing and range included. That keeps the rout and the drill
    /// underneath pricing ground the same way the evaluator and the player's
    /// overlay do, which is the point of there being one currency.
    ///
    /// What it does **not** do is become a search for safety: distance stays
    /// the first key, because this is the designer's rout and a crew who has
    /// stopped listening is not choosing a firing position — she is getting
    /// away. The orderly version of the same reflex is [`Self::run_crew_drill`],
    /// which minimises fire and does not care about distance at all, and the
    /// difference between the two is exactly what "drill versus rout" means.
    /// Pricing only the tiles that tie on the first key also keeps the walk
    /// cheap: on a full move range that is a handful of hexes rather than
    /// ninety.
    fn flight_destination(&self, registry: &DataRegistry, unit: UnitId) -> Option<Hex> {
        let me = self.unit(unit)?;
        let threatening = super::danger::threats(registry, self, unit);
        let threats: Vec<Hex> = threatening
            .iter()
            .filter_map(|id| self.unit(*id).map(|u| u.pos))
            .collect();
        if threats.is_empty() {
            return None;
        }
        let clearance = |hex: Hex| {
            threats
                .iter()
                .map(|t| hex.distance_to(*t))
                .min()
                .unwrap_or(0)
        };
        let here = clearance(me.pos);
        let away: Vec<(Hex, u32)> = movement::reachable(registry, self, unit)
            .into_iter()
            .filter(|&(hex, _)| hex != me.pos && clearance(hex) > here)
            .collect();
        let furthest = away.iter().map(|&(hex, _)| clearance(hex)).max()?;
        away.into_iter()
            .filter(|&(hex, _)| clearance(hex) == furthest)
            .max_by_key(|&(hex, cost)| {
                (
                    std::cmp::Reverse(super::danger::worth_key(
                        crate::battle::incoming_from(registry, self, unit, hex, &threatening).worth,
                    )),
                    std::cmp::Reverse(cost),
                    std::cmp::Reverse(hex.x),
                    std::cmp::Reverse(hex.y),
                )
            })
            .map(|(hex, _)| hex)
    }

    /// How this crew's defiance reads in a log, from the mod's own wording.
    fn defiance_name(&self, registry: &DataRegistry, unit: &Unit) -> String {
        let response = self.defiance(registry, unit);
        registry
            .morale
            .defiance
            .iter()
            .find(|d| d.response == response)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| "will not advance".into())
    }

    /// The mid-round half of the battle drill: nobody under fire waits for
    /// the next planning phase to survive.
    ///
    /// The planning-table drill (the delegation layer's `executor_only`)
    /// covers a crew who is already threatened when the round is planned.
    /// This covers the one who is ambushed at tick four: an idle crew — no
    /// route left to drive, no target she was told to watch — who has had
    /// time to take in a threat breaks for the ground those guns can do
    /// least on. Return fire needs no twin here because opportunity fire
    /// already is one; this is its movement half, and it prices time with
    /// the same clock. `spotted_since` says when her side first laid eyes on
    /// each enemy, her `reactions` delay says how long she needs to catch
    /// up, so a tank she has watched crawl toward her for three rounds is
    /// answered the instant it comes into range while a gun that appears out
    /// of a treeline costs her the same stunned ticks it costs her gunner.
    ///
    /// **This is the orderly reaction and [`Self::flight_destination`] is
    /// the rout**, which is the designer's own split: a crew who has noticed
    /// a threat minimises the fire on her, and a crew whose nerve has gone
    /// simply gets away from it. So this one does not care about distance at
    /// all — a hex closer to the gun that the gun cannot see is a better
    /// answer than a hex further off in the open — and the other takes
    /// distance first.
    ///
    /// What it will never touch: a crew with a route in hand keeps driving
    /// it (delaying plans already given is the sin the reaction-latency
    /// post-mortem forbids), a crew with a fire order is on deliberate
    /// overwatch and trusts her gun, and a crew whose morale rung no longer
    /// obeys does what her temperament says instead. A crew already on the
    /// quietest ground she can reach stands her ground — the comparison is
    /// strict, so equal danger never causes a pointless shuffle and each
    /// dash is to strictly better ground, which is what makes the drill
    /// settle instead of oscillate.
    fn run_crew_drill(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        let ids: Vec<UnitId> = self
            .units
            .iter()
            // A passenger has no ground to break for; her cover is the
            // carrier, for better and much worse.
            .filter(|u| u.alive() && u.intent.is_empty() && u.aboard.is_none())
            .map(|u| u.id)
            .collect();
        for id in ids {
            let Some(unit) = self.unit(id) else { continue };
            // A crew who has stopped listening does not take the ordinary
            // drill's advice — she does what her nerve tells her. Flight is
            // the only one of the three that moves; fight and freeze are both
            // "stay here", for opposite reasons, and are settled elsewhere
            // (fight in what she will shoot at, freeze in what she will not).
            //
            // This used to be a bare `continue` under a comment saying she is
            // exactly as frozen as the rung says she is. That reading is what
            // made a broken crew unable to take cover from the fire that
            // broke her.
            if !self.obeys(registry, unit) {
                if self.defiance(registry, unit) == crate::data::DefianceResponse::Flight
                    && let Some(dest) = self.flight_destination(registry, id)
                    && let Some((path, _)) = movement::path_to(registry, self, id, dest)
                {
                    let (rung, doing) = (
                        registry.morale.rung(unit.pressure).name.clone(),
                        self.defiance_name(registry, unit),
                    );
                    if let Some(unit) = self.unit_mut(id) {
                        unit.intent.path = path.into_iter().skip(1).collect();
                        unit.intent.own_idea = true;
                    }
                    events.push(Event::Defied {
                        unit: id,
                        rung,
                        doing,
                        to: Some(dest),
                    });
                }
                continue;
            }
            // Nerve first, then latitude, in that order at both drills: a
            // commander who said "I mean it" has answered the drill's
            // objection in advance, and this reflex is the same judgment the
            // planner's drill is, five seconds sooner. Before this gate a
            // binding crew pressed on through the planning phase and was
            // pulled into the trees by the reflex on the first tick she
            // noticed the gun — one gate read, one not.
            if !unit.yields_to_drill() {
                continue;
            }
            // The orderly reaction, which the planning table's drill asks as
            // well: the guns she has caught up with on her reaction clock,
            // and the reachable ground where they can do least to her.
            let noticed = super::danger::noticed_threats(registry, self, id);
            if noticed.is_empty() {
                continue;
            }
            let Some(dest) = super::danger::drill_destination(registry, self, id, &noticed) else {
                continue;
            };
            let Some((path, _)) = movement::path_to(registry, self, id, dest) else {
                continue;
            };
            if let Some(unit) = self.unit_mut(id) {
                unit.intent.path = path.into_iter().skip(1).collect();
                unit.intent.own_idea = true;
            }
            events.push(Event::TookCover { unit: id, at: dest });
        }
    }

    /// Boarding, dismounting, and the passenger position mirror.
    fn resolve_transport(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        // Dismounts first, in id order: the tile beside the carrier that a
        // dismounting rider takes may be the tile a boarding one crosses.
        let dismounting: Vec<UnitId> = self
            .units
            .iter()
            .filter(|u| u.alive() && u.dismounting && u.aboard.is_some())
            .map(|u| u.id)
            .collect();
        for id in dismounting {
            let Some(carrier_pos) = self
                .unit(id)
                .and_then(|u| u.aboard)
                .and_then(|c| self.unit(c))
                .map(|c| c.pos)
            else {
                continue;
            };
            let Some(ground) = self.dismount_tile(registry, id, carrier_pos) else {
                // Nowhere to stand: she stays aboard and tries again next
                // tick rather than being silently teleported or dropped.
                continue;
            };
            if let Some(u) = self.unit_mut(id) {
                u.aboard = None;
                u.dismounting = false;
                u.pos = ground;
            }
            events.push(Event::Dismounted {
                unit: id,
                at: ground,
            });
        }

        // Boardings, in id order.
        let boarding: Vec<(UnitId, UnitId)> = self
            .units
            .iter()
            .filter(|u| u.alive() && u.aboard.is_none())
            .filter_map(|u| u.boarding.map(|c| (u.id, c)))
            .collect();
        for (id, carrier) in boarding {
            let Some(c) = self.unit(carrier).filter(|c| c.alive()) else {
                // The ride she was walking to is gone; the order dies with
                // it and she is simply a platoon standing where she stands.
                if let Some(u) = self.unit_mut(id) {
                    u.boarding = None;
                }
                continue;
            };
            let capacity = registry
                .vehicle(&c.vehicle)
                .map(|v| v.capacity)
                .unwrap_or(0);
            let (c_pos, adjacent) = (c.pos, {
                let u = self.unit(id).expect("collected above");
                u.pos.distance_to(c.pos) <= 1
            });
            if !adjacent {
                continue;
            }
            if (self.passengers(carrier).len() as u32) < capacity {
                if let Some(u) = self.unit_mut(id) {
                    u.aboard = Some(carrier);
                    u.boarding = None;
                    u.pos = c_pos;
                    // Whatever she was doing on foot ends at the tailgate.
                    u.intent = UnitIntent::default();
                }
                events.push(Event::Mounted {
                    unit: id,
                    into: carrier,
                });
            }
        }

        // The mirror: passengers ride where the carrier is, every tick,
        // so nothing downstream ever reads a stale hex.
        let rides: Vec<(UnitId, Hex)> = self
            .units
            .iter()
            .filter(|u| u.alive())
            .filter_map(|u| u.aboard.and_then(|c| self.unit(c)).map(|c| (u.id, c.pos)))
            .collect();
        for (id, pos) in rides {
            if let Some(u) = self.unit_mut(id) {
                u.pos = pos;
            }
        }
    }

    /// Where a dismounting foot unit steps off: on the map, with room for her,
    /// and priced for foot movement.
    ///
    /// **The carrier's own hex first.** That is what happens on the day — a
    /// section gets out of the back of the vehicle it was riding in, it does
    /// not walk a hundred metres first — and until a hex could hold more than
    /// one crew it was the one place she could not go. It is also what makes
    /// stacking a thing the game actually does rather than a rule it merely
    /// permits: a platoon standing with its taxi in a wood is the ordinary
    /// case, and it is the case the stray rule was written for.
    ///
    /// Then the neighbours, lowest `(x, y)` so replays agree where she stepped
    /// off. A coordinate order is safe here in a way it is not in a tiebreak
    /// about *direction* — this is a search for the first hex that works, not
    /// a choice between equally good ones.
    fn dismount_tile(
        &self,
        registry: &DataRegistry,
        unit: UnitId,
        carrier_pos: Hex,
    ) -> Option<Hex> {
        let mut tiles: Vec<Hex> = vec![carrier_pos];
        let mut around: Vec<Hex> = carrier_pos.all_neighbors().into();
        around.sort_unstable_by_key(|h| (h.x, h.y));
        tiles.extend(around);
        let tiles = tiles;
        let u = self.unit(unit)?;
        tiles.into_iter().find(|hex| {
            self.map.contains(*hex)
                && self.room_for(registry, u, *hex)
                // Getting out where she already is costs no movement and needs
                // no price; only a step to a neighbour does.
                && (*hex == carrier_pos
                    || movement::edge_cost_for(registry, self, u, carrier_pos, *hex).is_some())
        })
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
            .filter(|u| {
                u.alive()
                    && !u.intent.path.is_empty()
                    && !u.intent.own_idea
                    && !self.obeys(registry, u)
            })
            .map(|u| u.id)
            .collect();
        for id in refusing {
            // She gets to try to hold. Discipline is the skill that resists,
            // rolled three-dice against her level, so training buys reliability
            // rather than a coin flip — and disobedience becomes something a
            // player can train away instead of a fact about the cadet.
            let level = self.unit(id).map(|u| {
                self.roster.crew_skill(
                    registry,
                    registry.vehicle(&u.vehicle),
                    &u.crew,
                    &u.crew_state,
                    &registry.morale.skill,
                    self.terrain_at(u.pos),
                )
            });
            if let Some(level) = level
                && crate::data::holds_together(&mut self.rng, level)
            {
                continue;
            }
            let (rung, doing, response) = match self.unit(id) {
                Some(u) => (
                    registry.morale.rung(u.pressure).name.clone(),
                    self.defiance_name(registry, u),
                    self.defiance(registry, u),
                ),
                None => continue,
            };
            // Said out loud, and the order is cleared so the unit does not sit
            // silently failing the same instruction for twelve ticks.
            if let Some(unit) = self.unit_mut(id) {
                unit.intent.path.clear();
            }
            // Refusing the order and choosing her own ground are one decision
            // and happen in one place, which is why flight is laid here rather
            // than left for the drill: the drill only visits crews with an
            // empty intent, and hers was emptied a line ago inside a loop that
            // has not finished. Doing it here also means she reverses in the
            // same tick she refuses, instead of standing still for one.
            let mut ran_to = None;
            if response == crate::data::DefianceResponse::Flight
                && let Some(dest) = self.flight_destination(registry, id)
                && let Some((path, _)) = movement::path_to(registry, self, id, dest)
            {
                if let Some(unit) = self.unit_mut(id) {
                    unit.intent.path = path.into_iter().skip(1).collect();
                    unit.intent.own_idea = true;
                }
                ran_to = Some(dest);
            }
            events.push(Event::Defied {
                unit: id,
                rung,
                doing,
                to: ran_to,
            });
        }

        let ids: Vec<UnitId> = self
            .units
            .iter()
            .filter(|u| u.alive())
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
                // An enemy on the next hex is the end of the advance either
                // way, but only one nobody had spotted counts as an ambush.
                // Checked before crowding, because bumping into an enemy is
                // an event and running out of room is just traffic.
                if let Some(other) = self.occupants(next).find(|o| o.side != side) {
                    if self.fog.side(side).spotted.contains(&other.id) {
                        blocked = true;
                    } else {
                        trapped_at = Some(unit.pos);
                    }
                    break;
                }
                // No room is traffic: a friend in the way will probably have
                // driven on by the next tick, so hold and try again rather
                // than abandoning the route. On terrain that declares no
                // capacity this is the old one-unit-per-hex rule exactly.
                let me = self.unit(id).expect("alive above");
                if !self.room_for(registry, me, next) {
                    break;
                }
                let unit = self.unit_mut(id).expect("alive above");
                unit.move_credit -= price;
                // Counted here, at the one place a unit changes hex, so that
                // every way of getting somewhere — an order, the battle
                // drill, a crew running away — costs the same accuracy. A
                // second increment anywhere else would mean two answers to
                // whether she is under way.
                unit.moved += 1;
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
        // Two phases on purpose: every crew decides against the tick's
        // opening state, then everything resolves. One phase reintroduced
        // loop order as a rule of the game the moment outcomes started
        // landing mid-tick — the first crew processed could shoot the gun
        // out of the second's hands and cancel a reply that was already
        // coming. See `fire_decision` for the full argument.
        let decisions: Vec<(UnitId, combat::FireAction)> = self
            .units
            .iter()
            .filter(|u| u.alive())
            .map(|u| u.id)
            .collect::<Vec<_>>()
            .into_iter()
            .filter_map(|id| combat::fire_decision(registry, self, id).map(|a| (id, a)))
            .collect();
        for (id, action) in decisions {
            combat::execute_fire(registry, self, id, action, events);
        }
        combat::reap(registry, self, events);
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
            .filter(|u| u.alive())
            .map(|u| (u.id, rules.rung(u.pressure).id.clone()))
            .collect();

        let add = |state: &mut BattleState, id: UnitId, amount: u32| {
            if let Some(unit) = state.unit_mut(id) {
                unit.pressure = unit.pressure.saturating_add(amount);
            }
        };

        // What the round that arrived costs a crew's nerve, over and above
        // the outcome. Looked up here, off the event, because the event is
        // the record of what was fired and this pass is the one place that
        // turns a tick's news into pressure.
        let felt = |ammo: &Option<String>, small_arms: bool| crate::data::RoundPressure {
            small_arms,
            suppression: ammo
                .as_ref()
                .and_then(|id| registry.ammo(id))
                .map(|a| a.suppression)
                .unwrap_or(0),
        };

        // Collected first: the borrow of `events` has to end before units are
        // touched, and iterating in event order keeps this deterministic.
        let hits: Vec<(UnitId, ShotFelt, crate::data::RoundPressure)> = events
            .iter()
            .filter_map(|e| match e {
                // `small_arms: false` is not read on this branch — a
                // penetration is priced by `hit + penetrated` whatever came
                // through — but it is stated rather than defaulted so that a
                // future price list which does read it gets the truth.
                Event::ShotHit {
                    target,
                    ammo,
                    damage,
                    budget,
                    ..
                } => Some((
                    *target,
                    ShotFelt::Penetrated {
                        spent: super::combat::spent_share(*damage, *budget),
                    },
                    felt(ammo, false),
                )),
                _ => None,
            })
            .collect();
        // A shell that strikes and fails to get through still rings the
        // hull like a bell; small-arms fire does not (`rattled` is false),
        // or the ladder's `bounced` price would quietly rebuild the damage
        // floor's defect in morale instead of hit points. Every bounce is
        // collected now rather than only the rattling ones, because a round
        // that declares suppression is charged for it either way and
        // `pressure_for` is the one place that knows which is which.
        let clangs: Vec<(UnitId, crate::data::RoundPressure)> = events
            .iter()
            .filter_map(|e| match e {
                Event::ShotBounced {
                    target,
                    rattled,
                    ammo,
                    ..
                } => Some((*target, felt(ammo, !rattled))),
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

        // The price list itself lives on `MoraleRules`, and it lives there
        // rather than here because the analytic twin `combat::round_pressure`
        // spends the same three lines to tell a planner what a shot is
        // expected to be worth. Two copies would be a gunner aiming at a
        // number no resolver honours.
        // Rounded here, once, at the ledger: the price list is a real
        // number because the expectation is one, and a crew's pressure is
        // whole points because the rungs are.
        for (id, outcome, round) in hits {
            // Every hit in the stream is a penetration now — the bounces
            // file separately below — so the shell that came through costs
            // both the old price of being hit and the new price of knowing
            // the armor did not hold, by the share of itself it spent.
            add(self, id, rules.pressure_for(outcome, round).round() as u32);
        }
        for (id, round) in clangs {
            add(
                self,
                id,
                rules.pressure_for(ShotFelt::Bounced, round).round() as u32,
            );
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
                .filter(|u| {
                    u.alive() && u.side == side && self.fog.side(side).visible.contains(&at)
                })
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
    /// One broken thing per crew per round, if anybody aboard can fix it.
    ///
    /// `maintenance` was one of the skills no rule read, and the seat that
    /// answers for it is the driver's — so a tank that has lost her driver is
    /// also a tank nobody can get the tracks back on, which is the kind of
    /// consequence the crew model is for.
    ///
    /// The shape is deliberately small. **One module**, the first in key
    /// order, because a crew fixes one thing at a time and because the key
    /// order is a `BTreeMap`'s and therefore the same on every machine. Back
    /// to **one hit**, not to full: she is working again, not as new. And
    /// only a crew who is *on the field* — a passenger has nothing of her own
    /// to mend.
    ///
    /// **No die is thrown when `field_repair_percent` is zero**, which is the
    /// rule's absence down to the rng stream, the same contract
    /// `detection_certain_percent` keeps.
    fn field_repairs(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        if registry.balance.field_repair_percent <= 0 {
            return;
        }
        let ids: Vec<UnitId> = self
            .units
            .iter()
            .filter(|u| u.alive() && u.aboard.is_none())
            .map(|u| u.id)
            .collect();
        for id in ids {
            let Some(unit) = self.units.get(id.index()) else {
                continue;
            };
            let Some(module) = unit
                .modules
                .iter()
                .find(|(_, hits)| **hits == 0)
                .map(|(id, _)| id.clone())
            else {
                continue;
            };
            let level =
                super::stats::maintenance(registry, &self.roster, unit, self.terrain_at(unit.pos));
            let chance = registry.balance.repair_chance(level);
            if chance <= 0 {
                continue;
            }
            if rand::RngExt::random_range(&mut self.rng, 0..100) >= chance {
                continue;
            }
            if let Some(unit) = self.units.get_mut(id.index())
                && let Some(hits) = unit.modules.get_mut(&module)
            {
                *hits = 1;
            }
            events.push(Event::ModuleRepaired { unit: id, module });
        }
    }

    fn begin_round(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        self.round += 1;
        self.field_repairs(registry, events);
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
                    &u.crew_state,
                    &registry.morale.skill,
                    self.terrain_at(u.pos),
                )
            });
            // ...and settle faster with her officer in sight. This is the
            // half of the chain of command that had never paid anybody
            // anything: losing a leader has cost a formation its nerve since
            // `leader_lost` was added, and still having one bought nothing at
            // all. It is what makes rallying something a leader *does*
            // rather than something that happens to a crew who got far
            // enough away — a routed platoon comes back because somebody is
            // there to bring it back.
            //
            // **Sight, not the radio net**, and the first draft had it the
            // other way round. Two reasons it moved. It is the more
            // believable rule: a commander steadies a frightened crew by
            // being visibly still in the fight, which is not something that
            // travels down a wire — the wire carries orders, and this is not
            // an order. And it is the only version that is additive. A zeroed
            // `command` block still puts a crew with no radio out of contact
            // while a mod with no block at all has nobody out of contact, so
            // hanging a rule on `in_contact` makes the two configurations
            // differ in deeds, which
            // `a_zeroed_command_block_is_the_game_without_one...` exists to
            // forbid. Fog is computed the same either way.
            //
            // It also gives the flight response a real cost that needed no
            // inventing: a crew who reverses far enough loses sight of her
            // officer, and rallies at the ordinary rate until somebody comes
            // for her.
            let rallied = self
                .command
                .formation_of(id)
                .and_then(|f| f.leader.filter(|l| *l != id))
                .and_then(|leader| self.unit(leader))
                // Her own eyes, not her side's. The first draft asked the
                // side's visible set, which is vacuous: that set is the union
                // of every friendly unit's vision and every unit sees the hex
                // she is standing on, so a side can always see its own
                // officer and the condition was true for everybody, always.
                // `fog::sees` is the per-unit question, answered off the
                // cache the last recompute left warm.
                .filter(|leader| leader.alive() && fog::sees(registry, self, id, leader.pos))
                .map(|_| registry.morale.recovery_near_leader)
                .unwrap_or(0);
            let shed = level.map(|l| registry.morale.recovered(l)).unwrap_or(0) + rallied;
            if let Some(unit) = self.units.get_mut(id.index()) {
                unit.pressure = unit.pressure.saturating_sub(shed);
            }
        }
        for unit in self.units.iter_mut() {
            unit.intent = UnitIntent::default();
            unit.planned = false;
            unit.move_credit = 0;
            unit.moved = 0;
        }
        self.phase = Phase::Planning {
            committed: vec![false; self.sides.len()],
        };
        events.push(Event::RoundStarted { round: self.round });
        // A standing boarding order is a march that renews itself: she
        // re-paths toward wherever her ride now stands, every round, until
        // she is aboard or the ride is gone.
        let boarders: Vec<(UnitId, Hex)> = self
            .units
            .iter()
            .filter(|u| u.alive() && u.aboard.is_none())
            .filter_map(|u| {
                u.boarding
                    .and_then(|c| self.unit(c))
                    .filter(|c| c.alive())
                    .map(|c| (u.id, c.pos))
            })
            .collect();
        for (id, to) in boarders {
            self.march_toward(registry, id, to);
            if let Some(u) = self.unit_mut(id) {
                u.planned = true;
            }
        }

        // A personal destination reached is a personal destination done:
        // she holds the ground she was sent to, still detached, and the
        // panel stops saying she is on her way.
        for unit in self.units.iter_mut().filter(|u| u.alive()) {
            if let Some(march) = unit.march()
                && march.to == unit.pos
            {
                // One assignment, and the insistence arrives with her: a
                // binding march becomes a binding hold, or the drill would
                // take her off the ground the tick after she reached it.
                unit.orders = Some(PersonalOrder::Holding {
                    latitude: march.latitude,
                });
            }
        }
        // A goal that is over is cleared here rather than by whoever notices,
        // so that the panel, the log and the planner all stop believing in it
        // at the same moment. `finished` needs the whole state, hence the
        // second pass.
        let done: Vec<UnitId> = self
            .units
            .iter()
            .filter(|u| u.alive())
            .filter(|u| u.goal.is_some_and(|g| g.finished(registry, self, u.id)))
            .map(|u| u.id)
            .collect();
        for id in done {
            if let Some(u) = self.unit_mut(id) {
                u.goal = None;
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
                if !unit.alive() || !objective.open_to(unit.side) || !objective.contains(unit.pos) {
                    continue;
                }
                let (id, side, at) = (unit.id, unit.side, unit.pos);
                let unit = &mut self.units[index];
                // Off the board but not destroyed. `Fate::Exited` is what keeps
                // the campaign from mourning her, and it is one transition
                // rather than two flags that had to be set together.
                unit.withdraw();
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
                .filter(|u| u.alive() && objective.contains(u.pos))
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

    fn check_victory(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
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
        if self.round.saturating_sub(self.last_contact_round) >= registry.balance.stalemate_rounds {
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
            // `Fate::lost` throughout, never `!alive`: a vehicle that drove
            // off by an exit is off the board but home, and reading `alive`
            // here would turn every ordered withdrawal into a decapitation.
            // The two are different variants now rather than a flag and a
            // qualifier, so the wrong reading is at least visible.
            let lost = |id: &UnitId| self.units.get(id.index()).is_some_and(|u| u.fate.lost());
            let fallen = match condition.when {
                LossTrigger::LeaderLost => formation.founding_leader.iter().any(lost),
                LossTrigger::Wiped => {
                    formation.members.iter().any(lost)
                        && formation
                            .members
                            .iter()
                            .all(|id| self.units.get(id.index()).is_some_and(|u| !u.alive()))
                }
            };
            if fallen {
                return Some(condition.side);
            }
        }
        None
    }

    fn finish(&mut self, winner: Option<u8>, reason: EndReason, events: &mut Vec<Event>) {
        self.over = Some(BattleResult { winner, reason });
        events.push(Event::BattleEnded { winner, reason });
    }
}
