//! The chain of command, as a battle carries it.
//!
//! A side is a flat pool of units that an all-seeing planner drives one at a
//! time. A *formation* is the unit of command that pool is missing: the thing
//! a mission is given to, that a leader can be lost from, and that can be out
//! of contact while the rest of the side is not. Maps declare them (see
//! [`crate::map::FormationDef`]); this is what a battle makes of the
//! declaration once the units it names actually exist.
//!
//! A formation carries three things now. Its **membership** (who answers to
//! whom, and which of them leads), its standing [`Mission`], and — once a mod
//! declares a [`crate::data::CommandRules`] block — the state of the wires:
//! which members can currently hear their leader, and any mission still in
//! transit toward her.
//!
//! Those last two exist only where a mod asked for them. With
//! `registry.command == None` the contact set is never computed and stays
//! empty, so [`Formation::in_contact`] answers `true` for everybody and no
//! mission ever travels: the game is exactly the one that was here before, and
//! the determinism baseline — fought on `river_crossing`, which declares four
//! formations — is what proves it rather than an argument.
//!
//! # Order
//!
//! Everything here is in declaration order (formations, as the map file wrote
//! them) or unit-id order (members, which is placement order because both
//! setup paths spawn placements into an empty unit list). Never a hash order.
//! This is not a stylistic preference: formations will be walked to produce
//! events and to make AI decisions, and an iteration-order dependency in that
//! walk is exactly the bug that hid in `fog::recompute` for months.

use super::{BattleState, Event, FireIntent, UnitId};
use crate::data::DataRegistry;
use crate::map::{FormationDef, Objective, ObjectiveKind, UnitPlacement};
use hexx::Hex;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// The nearest lane off the map that `side` is entitled to use, measured from
/// `from`. `None` when this map offers that side no way out — which is not a
/// failure: driving for an exit that does not exist is not a retreat, so a
/// side without one holds.
///
/// One rule with three callers, which is why it lives here rather than in any
/// of them: the commander brain ordering a beaten formation out, the player
/// pressing `W` on a formation of her own, and the campaign handing a
/// withdrawing army's battle its lane. They must agree — a player who orders a
/// withdrawal should get the lane her opposite number would have chosen, and
/// an army told on the map to fall back should leave by the same road its own
/// commander would have picked.
///
/// Ties go to the first-declared lane (strict less-than), so the answer cannot
/// flap between two equally distant exits from one round to the next.
pub fn nearest_exit(state: &BattleState, side: u8, from: Hex) -> Option<String> {
    let mut best: Option<(i32, &Objective)> = None;
    for objective in state.map.objectives() {
        if objective.kind != ObjectiveKind::Exit || !objective.open_to(side) {
            continue;
        }
        let dist = objective
            .hexes
            .iter()
            .map(|h| h.distance_to(from))
            .min()
            .unwrap_or(i32::MAX);
        if best.is_none_or(|(b, _)| dist < b) {
            best = Some((dist, objective));
        }
    }
    best.map(|(_, o)| o.id.clone())
}

/// The skill that decides how far a leader's orders carry.
///
/// A string rather than a data field, for the same reason `gunnery` and
/// `observation` are strings in [`crate::battle::stats`]: skill *ids* are the
/// engine's contract with the base mod, and a mod that renames them is
/// defining a different game. What the radius is worth per point of it is
/// data, in [`crate::data::CommandRules::radius_per_signals`].
const SIGNALS: &str = "signals";

/// Stable handle to a formation: an index into [`CommandState::formations`].
///
/// An index rather than the string id for the same reason `objective_held` is
/// parallel to the map's objective list — an index cannot disagree about
/// ordering, and orders travel through saves and replays where two copies of
/// a name would be two chances to drift. The string id survives on
/// [`Formation::id`] for the log and for map authors, which is exactly where a
/// name belongs: in what a person reads, not in what the engine matches on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FormationId(pub u32);

impl FormationId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// What a formation has been told to do, until it is told something else.
///
/// The vocabulary every commander speaks: the built-in brain, the human
/// through the UI, and one day a net or a language model all issue these
/// through [`crate::battle::Order::SetMission`] and no other way. It is a
/// plain serde enum on purpose — a mission has to survive a save, a replay
/// and a round trip through something outside this process without losing
/// meaning.
///
/// What reads one is [`crate::ai::Evaluator::score_tile`], through the
/// formation a unit belongs to — and only when she is in contact to hear it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mission {
    /// Drive for ground and take it. The formation moves on `to` and means to
    /// be standing on it, which is a different thing from passing through.
    Advance { to: Hex },
    /// Stand where told. `None` means where the formation already is, which
    /// is the neutral order — it is what you give a reserve, and what a
    /// formation falls back to when nothing better has been said.
    Hold { at: Option<Hex> },
    /// Find them; do not die finding them. Move toward `toward` looking for
    /// the enemy, valuing information over ground and survival over both.
    Recon { toward: Hex },
    /// Leave by the named exit objective — the id of an [`crate::map::Objective`]
    /// with `kind: exit` that this formation's side is entitled to use. This
    /// is what a withdrawal ordered from above looks like, as opposed to a
    /// crew deciding for itself that it has had enough.
    Withdraw { via: String },
}

/// One formation with its declaration resolved against the units on the field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Formation {
    /// The id the map declared, and what a mission will name.
    pub id: String,
    /// Which side this formation belongs to. Every member agrees with it —
    /// map validation refuses a formation spanning two sides.
    pub side: u8,
    /// The girl in charge: the member whose placement said `leads`, else the
    /// first member in declaration order (seniority the map author controls).
    ///
    /// `Option` because a formation can be *left* leaderless — every member
    /// of it is dead or gone, so there is nobody left to take over — not
    /// because a fresh one ever is. A formation with members always starts
    /// with one, and keeps one for as long as anybody is still on the field.
    pub leader: Option<UnitId>,
    /// The girl who was in command when the battle opened.
    ///
    /// Kept beside [`Self::leader`] rather than derived from it because
    /// succession *overwrites* the current leader within a tick of her death,
    /// and a scenario's [`crate::map::LossTrigger::LeaderLost`] is a question
    /// about the girl the map named — "is the commanding officer dead" — not
    /// about whoever holds the job now. Without this, a decapitation condition
    /// would quietly retarget itself onto the successor the moment it should
    /// have fired.
    ///
    /// `#[serde(default)]` so a save written before succession existed opens
    /// as a battle whose formations have no founding leader on record, which
    /// is the honest answer: nothing in that save was tracking one.
    #[serde(default)]
    pub founding_leader: Option<UnitId>,
    /// Members in unit-id order, which is placement order.
    pub members: Vec<UnitId>,
    /// A doctrine of this formation's own, overriding its side's. Absent is
    /// the ordinary case and means the side's.
    pub doctrine: Option<String>,
    /// What this formation is currently trying to do, if anything.
    ///
    /// A *standing* order, which is the whole point of it: it is not cleared
    /// at the top of a round the way `UnitIntent` is, because a formation
    /// told to take the bridge is still taking the bridge next round and for
    /// as long as nobody says otherwise. `None` is an unordered formation —
    /// no `Default` impl, because "no mission" and "hold where you are" are
    /// genuinely different states and conflating them would decide by
    /// accident what a silent map means.
    #[serde(default)]
    pub mission: Option<Mission>,
    /// A mission still travelling, and how many ticks it has left to travel.
    ///
    /// Missions stop being instantaneous once a mod declares a `command`
    /// block: the order is *sent* when it is issued (that is the
    /// [`crate::battle::Event::MissionAssigned`] the log already carries) and
    /// *arrives* some ticks later, when it moves into [`Self::mission`] and is
    /// announced again as `MissionReceived`. A second order issued while the
    /// first is still in the air replaces it — countermanding is ordinary
    /// business, and two missions in transit at once is not a state anyone
    /// could act on.
    ///
    /// `None` for every battle whose mod declares no command rules, because
    /// the delay is then zero and a mission never travels at all.
    #[serde(default)]
    pub incoming: Option<(Mission, u32)>,
    /// Members who cannot currently hear their leader, in unit-id order,
    /// each carrying the orders she had when the wire went dead.
    ///
    /// The snapshot is the load-bearing half. A cut-off crew does not go
    /// rogue: she *continues her standing orders* — the design doc's words
    /// for what commander loss means — and what she cannot do is hear
    /// anything new. Snapshotting at the moment of losing contact is what
    /// makes both true at once: the formation's mission can change behind
    /// her back and she keeps soldiering on the plan she knows. It also
    /// keeps the whole system additive even while leaders die on the first
    /// contact (measured: some formation loses its leader in round 1 on
    /// effectively every seed) — with the old "cut off means unmissioned"
    /// model, declaring a command block silently deleted missions from half
    /// the map by round two.
    ///
    /// Recomputed each tick, and only when there are command rules to
    /// recompute it against: with none, this stays empty for the whole
    /// battle, [`Self::in_contact`] answers `true` for everybody, and nothing
    /// costs anything.
    #[serde(default)]
    pub out_of_contact: Vec<CutOff>,
}

/// One member out of contact, and the orders she is soldiering on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutOff {
    pub unit: UnitId,
    /// The formation mission as of the tick she lost contact — possibly
    /// `None`, because being cut off with no orders is its own state: she
    /// fights by her own judgment, exactly as an unmissioned unit does.
    pub orders: Option<Mission>,
}

impl Formation {
    /// Whether this unit answers to this formation.
    pub fn contains(&self, unit: UnitId) -> bool {
        self.members.contains(&unit)
    }

    /// Whether this unit can currently hear her chain of command.
    ///
    /// True for a unit that is not in this formation at all, which is the
    /// right answer for the question this is asked: callers want to know
    /// whether a mission reaches her, and a mission that is not hers reaches
    /// her either way.
    pub fn in_contact(&self, unit: UnitId) -> bool {
        !self.out_of_contact.iter().any(|c| c.unit == unit)
    }

    /// The orders this member is acting on: the formation's standing mission
    /// if she can hear it, else whatever she was carrying when contact was
    /// lost. This — not [`Self::mission`] — is what execution consults, and
    /// the difference between the two is the whole point of the wire.
    pub fn mission_for(&self, unit: UnitId) -> Option<&Mission> {
        match self.out_of_contact.iter().find(|c| c.unit == unit) {
            Some(cut) => cut.orders.as_ref(),
            None => self.mission.as_ref(),
        }
    }

    /// The most recent thing this formation has been told, whether or not it
    /// has arrived yet.
    ///
    /// What a commander should compare against before issuing anything: an
    /// order already in the air is one she has given, and re-sending it every
    /// round of the transit window would fill the log with the same sentence
    /// and reset the clock each time. What the *executors* act on is
    /// [`Self::mission`] — deliberately different, because the whole point of
    /// latency is that the formation does not yet know.
    pub fn latest_mission(&self) -> Option<&Mission> {
        self.incoming
            .as_ref()
            .map(|(mission, _)| mission)
            .or(self.mission.as_ref())
    }
}

/// A commander's direct order to one crew, held at the radio until she can be
/// reached.
///
/// The **destination**, never the path. She re-paths from wherever she is when
/// the order finally arrives, which is the only honest thing to store: a route
/// computed from the hex she stood on when the order was given would walk her
/// through positions that stopped existing while the wire was dead. That is
/// the same reason a [`Mission`] names ground rather than a route.
///
/// Both halves accumulate in one slot, because "drive there and shoot that"
/// is one instruction to a person even when the UI sends it as two keystrokes.
/// A newer order of either kind replaces the older half of its own kind and
/// leaves the other standing — countermanding the route does not countermand
/// the target.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaitingOrders {
    /// Where she is to end up, re-pathed on delivery.
    #[serde(default)]
    pub destination: Option<Hex>,
    /// What she is to shoot at. Dropped on delivery if the target has since
    /// died — an order to engage a wreck is not an order.
    #[serde(default)]
    pub fire: Option<FireIntent>,
}

/// One entry in a side's command picture: an enemy as last *reported*, which
/// is a different thing from an enemy as currently seen. `fresh` is whether
/// somebody in contact can see it right now; a stale contact is a ghost at
/// the last reported position, and its age is `round` measured against the
/// battle's current one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub unit: UnitId,
    pub at: Hex,
    /// Who filed the report — a person, so the log can say "Kesselring
    /// reports armor at the bridge" instead of an anonymous marker moving.
    pub reporter: UnitId,
    /// The round the report was last refreshed.
    pub round: u32,
    pub fresh: bool,
}

/// Everything a battle knows about who answers to whom.
///
/// Empty is a meaningful and common value: a map that declares no formations
/// fights as one flat pool per side, which is every battle this engine has
/// ever run. That is the additivity rule — the absence of the system is
/// today's game, with no branch in Rust to switch it off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandState {
    formations: Vec<Formation>,
    /// Each side's command picture, indexed by side and sorted by the
    /// contact's unit id. Empty until command rules exist; sized lazily by
    /// the first recompute so a save from before pictures opens unchanged.
    #[serde(default)]
    pictures: Vec<Vec<Contact>>,
    /// Radioed orders that have not reached the girl they were meant for,
    /// in unit-id order.
    ///
    /// Only [`crate::battle::Order::Radio`] ever fills this — the commander's
    /// own voice, which needs a wire. A planner's `SetMove` is a crew's own
    /// judgment about her own tank and never queues, which is what keeps an
    /// AI-driven battle bit-identical to the one before this existed.
    ///
    /// Empty for the whole battle where no mod declares command rules:
    /// nobody is ever out of contact then, so a radioed order applies on the
    /// spot and there is nothing to hold.
    #[serde(default)]
    waiting: Vec<(UnitId, WaitingOrders)>,
}

impl CommandState {
    /// Resolve a map's declarations against the placements a battle spawned.
    ///
    /// `placements` is indexed by [`UnitId`]: both setup paths spawn
    /// placements in order into an empty unit list, which is the same
    /// assumption `face_units_at_enemies` already documents and relies on.
    ///
    /// A declared formation that no placement joined is dropped rather than
    /// carried empty. Map validation already refuses that case for a
    /// scenario's own units, so the only way to reach it is the overworld
    /// path, where the terrain map's declarations meet an army's units — and
    /// there the honest answer is that a formation nobody is in does not
    /// exist in this battle. Carrying it would mean a mission could later be
    /// issued to nobody and silently do nothing.
    pub fn from_placements(defs: &[FormationDef], placements: &[UnitPlacement]) -> Self {
        let formations = defs
            .iter()
            .filter_map(|def| {
                // Placement order throughout: `members` is therefore sorted by
                // unit id, and the fallback leader is the first-declared
                // member rather than whichever one a hash happened to yield.
                let members: Vec<(UnitId, &UnitPlacement)> = placements
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.formation.as_deref() == Some(def.id.as_str()))
                    .map(|(i, p)| (UnitId(i as u32), p))
                    .collect();
                let leader = members
                    .iter()
                    .find(|(_, p)| p.leads)
                    .or_else(|| members.first())
                    .map(|(id, _)| *id)?;
                Some(Formation {
                    id: def.id.clone(),
                    side: def.side,
                    leader: Some(leader),
                    founding_leader: Some(leader),
                    members: members.into_iter().map(|(id, _)| id).collect(),
                    doctrine: def.doctrine.clone(),
                    mission: None,
                    incoming: None,
                    out_of_contact: Vec::new(),
                })
            })
            .collect();
        Self {
            formations,
            pictures: Vec::new(),
            waiting: Vec::new(),
        }
    }

    /// Every formation in this battle, in the order its map declared them.
    pub fn formations(&self) -> &[Formation] {
        &self.formations
    }

    /// The formation a unit answers to, if any.
    ///
    /// A linear scan over both lists. Formations number in the single digits
    /// and members likewise, so an index would cost more in the invalidation
    /// it needs — membership changes when a unit is destroyed or transferred
    /// — than it saves. Revisit if it ever appears in a profile.
    pub fn formation_of(&self, unit: UnitId) -> Option<&Formation> {
        self.formations.iter().find(|f| f.contains(unit))
    }

    /// One formation by handle, or `None` if this battle has no such index.
    ///
    /// The handle comes in from outside — an order that travelled through a
    /// save, a replay, or a brain that is not this process — so a bad one is
    /// an ordinary refusal rather than a bug to panic on.
    pub fn get(&self, formation: FormationId) -> Option<&Formation> {
        self.formations.get(formation.index())
    }

    /// Give a formation its standing mission, replacing whatever it was
    /// doing. Returns whether the formation exists.
    ///
    /// Replacement is silent and deliberate: an order countermanding an
    /// earlier one is the normal business of command, and a formation holding
    /// two missions at once has no meaning anyone could act on. What *is*
    /// news — that a mission was given at all — is the caller's
    /// [`crate::battle::Event::MissionAssigned`], because this type has no
    /// business deciding what reaches the log.
    pub fn set_mission(&mut self, formation: FormationId, mission: Mission) -> bool {
        match self.formations.get_mut(formation.index()) {
            Some(f) => {
                f.mission = Some(mission);
                // Anything still travelling has been overtaken by this. It
                // cannot be allowed to land afterwards and quietly countermand
                // the order that arrived first.
                f.incoming = None;
                true
            }
            None => false,
        }
    }

    /// Every radioed order still waiting for its girl, in unit-id order.
    /// What the formation panel reads to say "orders waiting" beside her name.
    pub fn waiting(&self) -> &[(UnitId, WaitingOrders)] {
        &self.waiting
    }

    /// The orders held for one unit, if any.
    pub fn waiting_for(&self, unit: UnitId) -> Option<&WaitingOrders> {
        self.waiting
            .iter()
            .find(|(id, _)| *id == unit)
            .map(|(_, orders)| orders)
    }

    /// Merge a radioed order into this unit's slot, creating it if she has
    /// none. Each half replaces its own kind and leaves the other alone;
    /// `None` for a half means "this order said nothing about that".
    ///
    /// Kept sorted by unit id, because this list is walked to deliver orders
    /// and to emit events, and an insertion order is a hash order in disguise
    /// the moment anything reorders the callers.
    pub(super) fn hold_orders(
        &mut self,
        unit: UnitId,
        destination: Option<Hex>,
        fire: Option<FireIntent>,
    ) {
        if !self.waiting.iter().any(|(id, _)| *id == unit) {
            self.waiting.push((unit, WaitingOrders::default()));
            self.waiting.sort_by_key(|(id, _)| *id);
        }
        let slot = &mut self
            .waiting
            .iter_mut()
            .find(|(id, _)| *id == unit)
            .expect("present or just pushed")
            .1;
        if destination.is_some() {
            slot.destination = destination;
        }
        if fire.is_some() {
            slot.fire = fire;
        }
    }

    /// Forget what was being held for this unit. Returns whether there was
    /// anything to forget.
    pub(super) fn drop_orders(&mut self, unit: UnitId) -> bool {
        let before = self.waiting.len();
        self.waiting.retain(|(id, _)| *id != unit);
        self.waiting.len() != before
    }

    /// Take the whole queue for delivery; the caller puts back whatever could
    /// not be delivered. Order is preserved, so what goes back is still sorted.
    pub(super) fn take_waiting(&mut self) -> Vec<(UnitId, WaitingOrders)> {
        std::mem::take(&mut self.waiting)
    }

    /// Put undelivered slots back, keeping the list sorted by unit id.
    pub(super) fn keep_waiting(&mut self, waiting: Vec<(UnitId, WaitingOrders)>) {
        self.waiting = waiting;
        self.waiting.sort_by_key(|(id, _)| *id);
    }

    /// Put a mission in the air: it will replace the formation's standing one
    /// in `ticks` ticks, not now. Returns whether the formation exists.
    ///
    /// Only ever called with `ticks > 0` — a delay of zero is a mission that
    /// arrived, and goes through [`Self::set_mission`] like any other, which
    /// is what makes the no-rules game the *same code path* rather than the
    /// same code path plus a branch.
    pub fn set_incoming(&mut self, formation: FormationId, mission: Mission, ticks: u32) -> bool {
        match self.formations.get_mut(formation.index()) {
            Some(f) => {
                f.incoming = Some((mission, ticks));
                true
            }
            None => false,
        }
    }
}

impl BattleState {
    /// Ticks a mission spends travelling to this formation.
    ///
    /// Priced on the *leader's* crew, because getting an order out clearly is
    /// her job: her skill at whatever the rules name (the base game's
    /// `command`) is the check, taken where she is standing so terrain and
    /// traits reach it like every other check. Zero when no mod declares
    /// command rules, which is the whole of the additivity story for latency.
    ///
    /// A formation whose leader is off the board is priced at an ordinary
    /// level rather than refused. That is a narrow window now that
    /// [`Self::pass_command`] exists — it lasts from the tick she is lost to
    /// the next one, plus the planning phase in between if she went in the
    /// round's last tick — and a formation with nobody left at all, where
    /// "somebody passed it on, at ordinary speed" is the honest placeholder
    /// and the mission has nobody to reach anyway.
    pub(super) fn mission_delay(&self, registry: &DataRegistry, formation: FormationId) -> u32 {
        let Some(rules) = registry.command.as_ref() else {
            return 0;
        };
        let Some(f) = self.command.get(formation) else {
            return 0;
        };
        let level = f
            .leader
            .and_then(|id| self.unit(id))
            .map(|u| {
                self.roster.crew_skill(
                    registry,
                    registry.vehicle(&u.vehicle),
                    &u.crew,
                    &rules.latency.skill,
                    self.terrain_at(u.pos),
                )
            })
            .unwrap_or(crate::data::AVERAGE);
        rules.delay(level)
    }

    /// Bring every travelling mission one tick closer, and hand over the ones
    /// that have arrived.
    ///
    /// Run at the top of a tick, before anything moves, so that a one-tick
    /// delay means "it reaches them as the round starts moving" rather than
    /// "a round late". Formations are walked in declaration order, so what
    /// this says cannot depend on a hash.
    pub(super) fn deliver_missions(&mut self, events: &mut Vec<Event>) {
        for formation in &mut self.command.formations {
            let arrived = match &mut formation.incoming {
                Some((_, ticks)) => {
                    *ticks = ticks.saturating_sub(1);
                    *ticks == 0
                }
                None => false,
            };
            if arrived && let Some((mission, _)) = formation.incoming.take() {
                formation.mission = Some(mission.clone());
                events.push(Event::MissionReceived {
                    formation: formation.id.clone(),
                    mission,
                });
            }
        }
    }

    /// Hand command to the next girl in the order of battle wherever the
    /// leader is off the field.
    ///
    /// **Ungated, and that is the point.** Every other rule in this module
    /// exists only where a mod declares a `command` block, because a radius
    /// and a latency are a signals net and a game without one has neither.
    /// Leadership is not like that: it belongs to the *formation*, and a map
    /// that declares formations has said who is in charge whether or not
    /// anybody priced the radio. A formation whose leader burned and whose
    /// command never passed would be a hole in the chain that no mod asked
    /// for, so this runs on every battle that has formations at all — and on
    /// one that has none it walks an empty list, which is the additivity rule
    /// satisfied by there being nothing to do rather than by a branch.
    ///
    /// Succession is by **lowest living unit id**, which is placement order,
    /// which is the order the map author wrote her formation down in. That is
    /// the authorable rule the `leads` flag already follows for the first
    /// leader: seniority is something a scenario writer states, not something
    /// the engine infers from stats.
    ///
    /// The successor is worse at the job and no code here makes her so. Every
    /// price the chain of command charges — [`BattleState::mission_delay`] and
    /// the contact radius below — is already read off *the current leader's*
    /// crew, at the place she is standing, so promoting a girl with a weaker
    /// `command` skill lengthens her formation's latencies and promoting one
    /// with weaker `signals` shrinks its net, for free and for the right
    /// reason. Building a separate penalty on top would be pricing the same
    /// thing twice.
    ///
    /// Formations are walked in declaration order and members in id order, so
    /// what this emits cannot depend on a hash.
    pub(super) fn pass_command(&mut self, events: &mut Vec<Event>) {
        for index in 0..self.command.formations.len() {
            let formation = &self.command.formations[index];
            // `unit()` filters on `alive`, which is false for the destroyed
            // and for anyone who drove off by an exit — and both are reasons
            // somebody else has to take over. Whether she *died* is a
            // different question, asked by the scenario's loss conditions.
            let Some(gone) = formation.leader.filter(|id| self.unit(*id).is_none()) else {
                continue;
            };
            let successor = formation
                .members
                .iter()
                .copied()
                .find(|id| self.unit(*id).is_some());
            self.command.formations[index].leader = successor;
            // A formation with nobody left is left leaderless and silent.
            // There is no promotion to announce and nobody to hear it.
            if let Some(to) = successor {
                events.push(Event::CommandPassed {
                    formation: self.command.formations[index].id.clone(),
                    from: gone,
                    to,
                });
            }
        }
    }

    /// Work out who can still hear their leader, and say so when the answer
    /// changes.
    ///
    /// Cheap and total rather than incremental, for the reason fog settled on
    /// the same shape: contact depends on where everybody is standing, which
    /// half the units change every tick, so a diff would cost more than the
    /// answer. There are single digits of formations and of members.
    ///
    /// The graph is a breadth-first walk from the leader. Without `relay` she
    /// is the only anchor and everyone must be inside *her* radius; with it,
    /// anybody already in contact passes the signal on at her own radius,
    /// which is what makes a well-crewed radio vehicle worth a seat in the
    /// order of battle. Members are visited in unit-id order at every step, so
    /// the events and the resulting set are the same on every machine.
    ///
    /// A formation with nobody left to speak — every member dead or gone — is
    /// entirely out of contact. That is now the only way to reach that state:
    /// [`Self::pass_command`] runs first and hands the net to the next girl,
    /// so a leader dying costs her formation a tick of nothing rather than the
    /// rest of the battle in silence.
    ///
    /// Does nothing at all when no mod declares command rules. Not an
    /// optimisation: it is what makes `out_of_contact` empty forever in a
    /// battle that never asked for a chain of command, so every caller reading
    /// [`Formation::in_contact`] gets `true` and the old game back.
    pub(super) fn recompute_contact(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        let Some(rules) = registry.command.as_ref() else {
            return;
        };
        // The net is two media, walked side-wide in one breadth-first pass.
        //
        // **Radio follows the chain of command**: a transmitter reaches only
        // her own formation, at her vehicle's radio worked by her crew's
        // signals — dissemination up and down the squad, never sideways to a
        // neighbour's platoon. **Visual signalling is promiscuous**: a flag,
        // a hand, a shout carries `visual_range` hexes to ANY friendly with
        // a clear line to see it, formation be damned — which is exactly why
        // the walk is per side rather than per formation, and why a platoon
        // hugging its neighbour stays on the net with every radio out of
        // reach. Roots are the formation leaders (each implicitly wired to
        // the side's command) and every unit in no formation at all; roots
        // always transmit, everyone else extends the net only where `relay`
        // says a net is run that way.
        //
        // Roots, queue and candidate scans are all in unit-id order, so the
        // reached set and the events below cannot depend on a hash.
        for side in 0..self.sides.len() as u8 {
            let living: Vec<UnitId> = self.side_units(side).map(|u| u.id).collect();
            let mut roots: Vec<UnitId> = living
                .iter()
                .copied()
                .filter(|id| match self.command.formation_of(*id) {
                    Some(f) => f.leader == Some(*id),
                    None => true,
                })
                .collect();
            roots.sort_unstable();

            let mut heard: Vec<UnitId> = roots.clone();
            let mut anchors: VecDeque<UnitId> = roots.into();
            while let Some(anchor) = anchors.pop_front() {
                let Some(unit) = self.unit(anchor) else {
                    continue;
                };
                let signals = self.roster.crew_skill(
                    registry,
                    registry.vehicle(&unit.vehicle),
                    &unit.crew,
                    SIGNALS,
                    self.terrain_at(unit.pos),
                );
                let radio_base = registry
                    .vehicle(&unit.vehicle)
                    .and_then(|v| v.radio)
                    .unwrap_or(rules.radius);
                let radio = rules.radio_range(radio_base, signals) as i32;
                let from = unit.pos;
                let squad = self.command.formation_of(anchor).map(|f| f.id.clone());
                for id in &living {
                    if heard.contains(id) {
                        continue;
                    }
                    let Some(other) = self.unit(*id) else {
                        continue;
                    };
                    let dist = other.pos.distance_to(from);
                    let by_radio = dist <= radio
                        && squad.is_some()
                        && self.command.formation_of(*id).map(|f| f.id.as_str())
                            == squad.as_deref();
                    let by_sight = rules.visual_range > 0
                        && dist <= rules.visual_range as i32
                        && self.sight.clear(from, other.pos);
                    if by_radio || by_sight {
                        heard.push(*id);
                        if rules.relay {
                            anchors.push_back(*id);
                        }
                    }
                }
            }

            self.settle_contact(side, &heard, events);
        }
    }

    /// Write one side's reached set back onto its formations, snapshotting
    /// standing orders for the newly cut off and saying every change once.
    fn settle_contact(&mut self, side: u8, heard: &[UnitId], events: &mut Vec<Event>) {
        for index in 0..self.command.formations.len() {
            if self.command.formations[index].side != side {
                continue;
            }
            // Only units still on the field can be in or out of contact. The
            // dead and the departed are neither, and saying so about them
            // would be noise in the log at the worst possible moment.
            let living: Vec<UnitId> = self.command.formations[index]
                .members
                .iter()
                .copied()
                .filter(|id| self.unit(*id).is_some())
                .collect();

            // `living` is in unit-id order, so this is too, and so are the
            // events below it. A member newly cut off snapshots the standing
            // mission as her carried orders; one who was already cut off
            // keeps the snapshot she has — the wire has been dead the whole
            // time, so nothing newer can have reached her.
            let formation = &self.command.formations[index];
            let cut_off: Vec<CutOff> = living
                .iter()
                .filter(|id| !heard.contains(id))
                .map(|id| CutOff {
                    unit: *id,
                    orders: formation
                        .out_of_contact
                        .iter()
                        .find(|c| c.unit == *id)
                        .map(|c| c.orders.clone())
                        .unwrap_or_else(|| formation.mission.clone()),
                })
                .collect();
            let was = std::mem::replace(
                &mut self.command.formations[index].out_of_contact,
                cut_off.clone(),
            );
            for cut in &cut_off {
                if !was.iter().any(|c| c.unit == cut.unit) {
                    events.push(Event::OutOfContact { unit: cut.unit });
                }
            }
            for cut in &was {
                // Still on the field: a crew that came back into contact by
                // dying is not news anybody wants twice.
                if !cut_off.iter().any(|c| c.unit == cut.unit) && self.unit(cut.unit).is_some() {
                    events.push(Event::ContactRestored { unit: cut.unit });
                }
            }
        }
    }

    /// The command picture: what `side`'s commander has been *told* is out
    /// there, as opposed to what her units can currently see. Empty for a
    /// battle whose mod declares no command rules — the display and the
    /// brains then read the live fog, which is today's game.
    pub fn picture(&self, side: u8) -> &[Contact] {
        self.command
            .pictures
            .get(side as usize)
            .map(|p| p.as_slice())
            .unwrap_or(&[])
    }

    /// Rebuild every side's command picture from what its in-contact units
    /// can see, and say when something new is reported.
    ///
    /// The rule that earns this its place: **seeing is not reporting.** The
    /// side's fog may spot an enemy through any unit's eyes, but the picture
    /// only learns of it when a unit *in contact* sees it — a cut-off scout
    /// discovers things nobody else knows, which is recon wasted, which is
    /// what makes the wires worth protecting. A contact nobody currently
    /// re-reports goes stale rather than vanishing: the commander keeps a
    /// ghost at the last reported position, because "we lost sight of it" is
    /// information and "it was never there" is a lie.
    ///
    /// Reports are instantaneous once a reporter exists; pricing them in
    /// signals-check ticks the way outbound orders are priced is future
    /// work, noted in the design doc rather than half-built here.
    ///
    /// Sides, enemies and reporters are all walked in index/id order, so the
    /// events and the picture are the same on every machine. Does nothing
    /// when no mod declares command rules.
    pub(super) fn recompute_picture(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        if registry.command.is_none() {
            return;
        }
        let sides = self.sides.len();
        let mut pictures = std::mem::take(&mut self.command.pictures);
        pictures.resize(sides, Vec::new());

        for side in 0..sides as u8 {
            // Sorted, because a HashSet's order must never reach an event.
            let mut spotted: Vec<UnitId> = self.fog.side(side).spotted.iter().copied().collect();
            spotted.sort_unstable();

            let prior = std::mem::take(&mut pictures[side as usize]);
            let mut next: Vec<Contact> = Vec::new();

            for enemy in spotted {
                let Some(target) = self.unit(enemy) else {
                    continue;
                };
                // The lowest-id unit in contact that can see it files the
                // report. Units outside any formation answer directly to
                // their side and always report.
                let reporter = self
                    .side_units(side)
                    .filter(|u| {
                        self.command
                            .formation_of(u.id)
                            .is_none_or(|f| f.in_contact(u.id))
                    })
                    .find(|u| super::fog::sees(registry, self, u.id, target.pos))
                    .map(|u| u.id);
                if let Some(by) = reporter {
                    let known = prior.iter().find(|c| c.unit == enemy);
                    if known.is_none_or(|c| !c.fresh) {
                        events.push(Event::ContactReported {
                            unit: enemy,
                            by,
                            at: target.pos,
                        });
                    }
                    next.push(Contact {
                        unit: enemy,
                        at: target.pos,
                        reporter: by,
                        round: self.round,
                        fresh: true,
                    });
                }
            }

            // Everything previously known and not freshly reported stays as
            // a ghost — unless we watched it die: a contact that was fresh
            // while its vehicle was destroyed was seen going up, and keeping
            // a ghost of something the whole net saw burn would be the
            // picture lying in the other direction.
            for old in prior {
                if next.iter().any(|c| c.unit == old.unit) {
                    continue;
                }
                let gone = self.units.get(old.unit.index()).is_none_or(|u| !u.alive);
                if gone && old.fresh {
                    continue;
                }
                next.push(Contact {
                    fresh: false,
                    ..old
                });
            }
            next.sort_unstable_by_key(|c| c.unit);
            pictures[side as usize] = next;
        }
        self.command.pictures = pictures;
    }
}
