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

use super::{BattleState, Event, FireIntent, OrderError, Unit, UnitId};
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
    for objective in state.scenario.objectives() {
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

/// The retreat lane a formation would take: [`nearest_exit`], measured from
/// its leader (or, leaderless, its first member still on the roll).
///
/// Two callers, and they are the reason it is here rather than in either:
/// the player pressing `W` on a formation of her own, and the campaign
/// handing a withdrawing army's battle its lane ([`crate::field::Clash`]).
/// It used to live in the game crate, which is where the second of those
/// lived too — so the campaign's half of the rule could not be reached by
/// any headless run.
pub fn formation_exit(state: &BattleState, formation: usize) -> Option<String> {
    let formation = state.formations().get(formation)?;
    let from = formation
        .leader
        .and_then(|id| state.unit(id))
        .or_else(|| formation.members.iter().find_map(|id| state.unit(*id)))
        .map(|u| u.pos)?;
    nearest_exit(state, formation.side, from)
}

/// Whose knowledge a question about the enemy is asked with. See
/// [`BattleState::known_enemies`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Knower {
    /// A side's commander: what has been reported to her.
    Commander(u8),
    /// One crew: what she has been told, if she can hear, and what she sees.
    Crew(UnitId),
}

/// Which net a message travels on. See `BattleState::informs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Net {
    /// A formation's own net: its leader and its members.
    Formation,
    /// The net formation leaders talk to each other on.
    Command,
}

/// One operational command: a senior officer and every leader she can reach
/// with orders. See [`BattleState::operational_commands`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationalCommand {
    /// Who commands: the most senior leader in the group.
    pub commander: UnitId,
    /// Every leader in the group, the commander included, in the order the
    /// map declared their formations and then by unit id.
    pub leaders: Vec<UnitId>,
    /// The formations those leaders lead, by index.
    pub formations: Vec<usize>,
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

/// How much latitude a crew has in carrying out a personal order: whether
/// she may break it off to keep herself alive.
///
/// This is the per-unit twin of the distinction [`Mission::Advance`] and
/// [`Mission::Assault`] already draw for a whole formation, and it is drawn
/// for the same reason. A commander who points at a ridge is usually saying
/// "get there, use your judgment on the way" — and a crew who drives a parade
/// route through effective fire to keep an appointment is not showing
/// initiative, she is dying stupidly. But sometimes the ridge is worth the
/// vehicle, and until now there was no way to say so to one crew. The order
/// went out, the battle drill quietly overrode it every round, and the
/// commander watched her tank shelter in a hedge without ever being told why.
///
/// So: the same sentence with two prices, and the caller says which one she
/// is paying. It rides in the order stream rather than living in the UI
/// because saves, replays and any future external brain have to carry it —
/// the same argument that makes a mission an [`crate::battle::Order`].
///
/// **[`Self::Delegated`] is the default everywhere**, which is what keeps
/// this additive: an AI side, a mod, a scenario and a save written before
/// latitude existed all mean exactly the game that was here before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Latitude {
    /// "Get there." She marches for the ground she was given, and breaks off
    /// for cover when she is under fire she has had time to take in — the
    /// battle drill, which every army since 1918 has trained and which this
    /// engine has always applied. She resumes the march when the shooting
    /// stops.
    #[default]
    Delegated,
    /// "Get there. I mean it." The drill does not preempt her: she drives on
    /// through fire and some crews do not arrive. Nothing else changes — she
    /// still shoots on her arc, still answers her morale, and a crew whose
    /// rung no longer obeys is exactly as frozen as the rung says. Binding
    /// buys the order priority over the crew's own judgment, never over her
    /// nerve.
    Binding,
}

impl Latitude {
    /// Whether an order at this latitude may be set aside by the battle
    /// drill. Named as a question about the drill rather than a bare bool
    /// match, because this is the only thing latitude decides and every
    /// caller should read as though it says so.
    pub fn yields_to_drill(self) -> bool {
        self == Self::Delegated
    }

    /// What marching at this latitude commits a crew to, in one clause —
    /// the per-unit twin of [`Mission::promise`], and there for exactly the
    /// same reason. A player choosing between clicking ground and insisting
    /// on it is making the advance-versus-assault decision one crew at a
    /// time, and is entitled to read the price of each before she pays it.
    pub fn promise(self) -> &'static str {
        match self {
            Self::Delegated => "she may break off for cover on the way",
            Self::Binding => "she drives on, and does not stop for cover",
        }
    }

    /// The same clause for a *formation's* orders, which commit a different
    /// thing and therefore have to say a different thing.
    ///
    /// What a mission's latitude buys is not the battle drill — that is the
    /// advance-versus-assault decision and has its own verb — it is whether
    /// the formation's doctrine may discount the order it was given. A loose
    /// doctrine currently reads "take the ford" as a suggestion worth about
    /// four fifths of what a tight one reads it as, which is the complaint
    /// this chunk answers: a commander's order should not be quietly worth
    /// less because of who she gave it to.
    pub fn mission_promise(self) -> &'static str {
        match self {
            Self::Delegated => "her doctrine decides how closely to hold to it",
            Self::Binding => "she holds to the letter of it, whatever her doctrine prefers",
        }
    }
}

/// The marching half of a commander's personal order: the ground, and how
/// hard she meant it.
///
/// The two travel in one struct because latitude is a property of *an order*
/// rather than of a crew — the same cadet is pressed on one ridge and given
/// her head on the next — and they used to be two fields on the unit that
/// could disagree. There is now no way to read the insistence without also
/// reading what is being insisted upon, and no way to set one without the
/// other, which is the whole reason this type exists.
///
/// A destination, never a path: she re-paths from wherever she stands each
/// round, marching across as many rounds as the ground demands. A movement
/// order does not expire for being far away; it is executed until arrival.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct March {
    /// Where her commander's order is taking her, until she gets there.
    pub to: Hex,
    /// How hard it was meant — whether the battle drill may set the march
    /// aside to keep her alive. `#[serde(default)]` so an order recorded
    /// before latitude existed reads as the delegated one every order in
    /// this engine used to be.
    #[serde(default)]
    pub latitude: Latitude,
}

/// A crew under her commander's personal charge, and what she is doing about
/// it.
///
/// **Why one noun and not three flags.** This replaced `detached`, `tasking`
/// and `latitude` on [`crate::battle::Unit`], which were set and cleared as a
/// triple at five sites in `orders.rs` and could each be forgotten
/// individually. Every rule that used to be prose about keeping them in step
/// is now a fact about this type: an absent order is a crew back under her
/// formation's mission with no stale destination and no stale insistence
/// behind her, an arrival is one assignment rather than three, and latitude
/// cannot outlive the march it qualifies because it lives inside [`March`].
///
/// The field is an `Option<PersonalOrder>`, and `None` — deliberately not a
/// variant here — is "she answers to her formation like everybody else",
/// because that is the state every unit spawns in and the one a fresh
/// formation order returns her to. Detached is not idle either way: the
/// battle drill still applies and she still shoots on her arc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonalOrder {
    /// Holding the ground her commander put her on. Either she has arrived,
    /// or the order said nothing about where to go — a fire mission detaches
    /// her from the standing mission exactly as a march does, because the
    /// commander has taken personal charge of this vehicle either way.
    ///
    /// **The latitude arrives with her.** A binding march that reaches its
    /// ground becomes a binding hold: "get there, I mean it" was never
    /// "get there and then use your judgment", and the first draft of this
    /// type dropped the insistence at the last hex, so the battle drill
    /// backed a crew off the ordered ground the tick after she reached it —
    /// the designer's ruling (2026-09-09) is that the same gate that keeps
    /// her on the road keeps her on the ground. An order that said nothing
    /// about where to go holds at [`Latitude::Delegated`], because there was
    /// no march for an insistence to ride in on.
    Holding {
        /// How hard the order she is holding under was meant.
        latitude: Latitude,
    },
    /// Still on her way, at the latitude she was sent under.
    Marching(March),
}

impl PersonalOrder {
    /// Where this order is taking her, if it is taking her anywhere.
    pub fn march(self) -> Option<March> {
        match self {
            Self::Holding { .. } => None,
            Self::Marching(march) => Some(march),
        }
    }

    /// How hard this order is meant, marching or holding.
    ///
    /// Read by [`crate::battle::Unit::yields_to_drill`] and by nothing else
    /// that decides anything: latitude answers one question — may the battle
    /// drill set this order aside — and one accessor on the unit is where it
    /// is answered.
    pub fn latitude(self) -> Latitude {
        match self {
            Self::Holding { latitude } => latitude,
            Self::Marching(march) => march.latitude,
        }
    }
}

/// What one crew means to do next, as opposed to what her formation was
/// told.
///
/// This is the unit of decision the planners work in, and it is deliberately
/// small: a handful of statements, each stable enough to outlive the round
/// that produced it. The shape is borrowed from the options framework in
/// reinforcement learning — an *option* is a temporally extended action with
/// three parts, and a goal has all three: the set of goals worth considering
/// is the initiation set ([`crate::ai::goal::candidates`]), the executor that
/// walks toward one is the policy, and [`Goal::finished`] is the termination
/// condition.
///
/// That is not decoration. It is the seam this codebase is meant to be
/// replaceable at: a learned policy chooses among a handful of goals, while
/// pathing, boarding, dismounting, opportunity fire and defiance stay in the
/// executor where they are already written and already tested. A policy that
/// had to emit orders directly would be choosing among every hex on a
/// 1261-tile board and relearning rules the engine already knows.
///
/// Lives here rather than in `ai` for two reasons. It is a fact about a unit
/// in the same family as [`Mission`] and `tasking` — what she is trying to do
/// — so it belongs where those are; and the presentation layer reads it, so
/// that a vehicle driving somewhere can say where. It rides on the unit and
/// therefore through saves, which a planner's private memory would not: that
/// is not a style preference, it is what `tests/save.rs` requires, since a
/// battle that forks through a save file has to play the same afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Goal {
    /// Get to this ground and be standing on it. The workhorse: an
    /// objective, a piece of cover, a firing position, the ground her orders
    /// named.
    Take(Hex),
    /// Stay here and watch. Not the absence of a goal — a crew who has
    /// decided this piece of ground is where she should be is doing
    /// something, and the difference matters to anyone reading the log.
    Hold,
}

impl Goal {
    /// Whether this goal is over — achieved, or no longer worth carrying.
    ///
    /// The termination condition, and a *list* rather than a threshold on
    /// purpose. A threshold ("abandon it if something else is now much
    /// better") re-opens the question every round and needs a number nobody
    /// can defend; a list can be read, and each entry is a sentence about the
    /// world rather than about the arithmetic.
    ///
    /// `Hold` never finishes on its own. It is ended by the things that end
    /// every goal from outside — fresh orders, a recall — which is right: a
    /// crew told to sit somewhere sits there until told otherwise.
    pub fn finished(
        &self,
        registry: &crate::data::DataRegistry,
        state: &crate::battle::BattleState,
        unit: crate::battle::UnitId,
    ) -> bool {
        let Some(me) = state.unit(unit) else {
            return true;
        };
        match self {
            Self::Hold => false,
            Self::Take(hex) => {
                // Arrived.
                me.pos == *hex
                    // Or somebody else filled it. Two crews driving for one
                    // hex is the queue the plateau rule was invented to break
                    // up, and saying it here says it once instead of as a
                    // tie-break buried in a sweep. What counts as "filled" is
                    // now the capacity rule rather than "anybody at all",
                    // because a wood a friend is standing in may still have
                    // room for a platoon — and on terrain that declares no
                    // capacity the two statements are the same one.
                    || (state.occupants(*hex).any(|u| u.id != unit && u.side == me.side)
                        && !state.room_for(registry, me, *hex))
            }
        }
    }

    /// How this reads over the radio, given a scenario that can name its
    /// ground.
    ///
    /// A phrase rather than a sentence, and in the crew's own voice, because
    /// the log is traffic rather than narration: the presentation layer puts
    /// her call sign in front of it. Ground with a name is called by its
    /// name — "moving to the Great Glade" is what somebody would actually
    /// say, where a grid reference is what she falls back on when the ground
    /// has none.
    pub fn describe(&self, scenario: &crate::map::Scenario) -> String {
        match self {
            Self::Hold => "holding here".to_string(),
            Self::Take(hex) => match scenario.objectives().iter().find(|o| o.hexes.contains(hex)) {
                Some(objective) => format!("moving to {}", objective.name),
                None => {
                    let [col, row] = crate::hex_to_offset(*hex);
                    format!("moving to ({col}, {row})")
                }
            },
        }
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
    /// Movement to contact: drive for ground and take it, but fight what you
    /// meet on the way. The formation moves on `to` and means to be standing
    /// on it, which is a different thing from passing through — and the march
    /// is suspended for as long as somebody is shooting at her, because
    /// halting and fighting on contact is what this order *means*. The pull
    /// resumes on its own when the threat is dead or lost.
    ///
    /// This is the order a commander gives when she wants ground and has not
    /// decided what it is worth. If she has decided, she says
    /// [`Self::Assault`].
    Advance { to: Hex },
    /// Press on to the ground whatever is firing: the deliberate attack, as
    /// opposed to the movement to contact [`Self::Advance`] is. Identical to
    /// an advance in what it is worth to be there and which way that is —
    /// and unlike an advance it is *never* suspended by contact, which is
    /// precisely the cost of it. Crews under this order drive through
    /// effective fire and some of them do not arrive.
    ///
    /// A commander orders it anyway because ground is sometimes worth more
    /// than the vehicles it costs, and because an attack that halts at every
    /// contact gives the defender the one thing he needs, which is time. The
    /// two orders are the same sentence with different prices, so the
    /// evaluator scores them identically and only the damping tells them
    /// apart.
    Assault { to: Hex },
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
    /// Base of fire: stand off at supporting distance from the named
    /// formation and put fire on what threatens them. The mission of the
    /// overwatching half of "base of fire and maneuver" — she is not going
    /// where they are going, she is where she can shoot for them.
    ///
    /// The supported formation is named by its **string id** rather than a
    /// [`FormationId`], which is the one place in this module a name beats an
    /// index. A mission travels through saves, replays and logs; an id is
    /// stable across all three and reads as itself in a sentence, and the
    /// live formation is resolved at scoring time — which is also the honest
    /// shape, because who is left in that formation changes minute by minute
    /// while the order does not.
    Support { formation: String },
}

impl Mission {
    /// Whether anything can follow this mission in a plan. A stand-fast has
    /// no end to reach and a retreat has no afterwards worth planning on
    /// this battlefield, so queueing behind either is refused out loud
    /// rather than left to sit as a leg that can never begin.
    ///
    /// A base of fire is terminal for the same reason a stand-fast is: it is
    /// a posture rather than a leg, held for as long as the people it covers
    /// need covering. "Advance to the ridge, then support the platoon" is a
    /// perfectly good plan — it simply *ends* in support, and nothing follows
    /// it.
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Hold { .. } | Self::Withdraw { .. } | Self::Support { .. }
        )
    }

    /// The verb a person would use for this order, one word.
    pub fn verb(&self) -> &'static str {
        VOCABULARY[self.slot()].0
    }

    /// What ordering this commits a formation to, in one clause.
    ///
    /// It lives here rather than in the UI because it is a claim about the
    /// *rules*, and the rules are here. The distinction between an advance
    /// and an assault is the sharpest example in the game and was, until
    /// this existed, invisible from the keyboard: both keys move a platoon
    /// toward ground, one of them stops when somebody shoots, and the player
    /// who meant the second and pressed the first watched her attack die
    /// halfway for reasons the game never stated. A promise beside the enum
    /// is the cheapest possible fix and the one that cannot drift, because
    /// whoever changes what an order *does* is looking straight at the
    /// sentence claiming what it does.
    ///
    /// Deliberately about consequences rather than mechanism. "Halt and
    /// fight whatever shoots at you" is something a player can plan around;
    /// "damped to a quarter on contact" is the same fact written for
    /// somebody who has read [`crate::ai::Evaluator`].
    pub fn promise(&self) -> &'static str {
        VOCABULARY[self.slot()].1
    }

    /// Every order a commander can give, as verb and promise, in the order a
    /// briefing would list them.
    ///
    /// For a menu of orders that have not been aimed at anything yet — which
    /// is exactly when a player needs to read what they mean. It shares its
    /// table with [`Self::verb`] and [`Self::promise`] so a listed order and
    /// a given one can never say different things.
    pub fn vocabulary() -> &'static [(&'static str, &'static str)] {
        VOCABULARY
    }

    /// Which row of [`VOCABULARY`] describes this order. Written as an
    /// exhaustive match rather than a discriminant cast so that adding a
    /// mission without giving it a promise fails to compile.
    fn slot(&self) -> usize {
        match self {
            Self::Advance { .. } => 0,
            Self::Assault { .. } => 1,
            Self::Hold { .. } => 2,
            Self::Recon { .. } => 3,
            Self::Withdraw { .. } => 4,
            Self::Support { .. } => 5,
        }
    }
}

/// What each mission verb promises, in the order [`Mission`] declares them.
///
/// One table rather than a match arm per accessor, because the two things a
/// caller wants — "what is this order called" and "what does it commit me
/// to" — are two columns of one fact and drift the moment they are written
/// twice.
const VOCABULARY: &[(&str, &str)] = &[
    ("advance", "take it, halting to fight what shoots"),
    ("assault", "take it through fire, and pay for it"),
    ("hold", "stand fast and hold what you have"),
    ("reconnoitre", "find them without getting pinned"),
    ("withdraw", "break contact and leave the field"),
    ("support", "shoot for them instead of going with them"),
];

/// One formation with its declaration resolved against the units on the field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Formation {
    /// The id the map declared, and what a mission will name.
    pub id: String,
    /// Which side this formation belongs to. Every member agrees with it —
    /// map validation refuses a formation spanning two sides.
    pub side: u8,
    /// The cadet in charge: the member whose placement said `leads`, else the
    /// first member in declaration order (seniority the map author controls).
    ///
    /// `Option` because a formation can be *left* leaderless — every member
    /// of it is dead or gone, so there is nobody left to take over — not
    /// because a fresh one ever is. A formation with members always starts
    /// with one, and keeps one for as long as anybody is still on the field.
    pub leader: Option<UnitId>,
    /// The cadet who was in command when the battle opened.
    ///
    /// Kept beside [`Self::leader`] rather than derived from it because
    /// succession *overwrites* the current leader within a tick of her death,
    /// and a scenario's [`crate::map::LossTrigger::LeaderLost`] is a question
    /// about the cadet the map named — "is the commanding officer dead" — not
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
    /// The latitude the standing orders were given with: whether the
    /// formation's own doctrine is allowed to discount them.
    ///
    /// The formation-scale half of [`Latitude`], and it governs a different
    /// thing from the per-unit half on purpose. A crew's latitude answers
    /// *will she break off for cover*; a formation's answers *may her
    /// doctrine bend how hard she is pulled toward the ground she was
    /// given*. The first question already has a formation-scale verb —
    /// [`Mission::Assault`] is "press on through fire", and giving latitude
    /// that job too would make a binding [`Mission::Advance`] an exact
    /// synonym for it. Two idioms for one sentence is the thing this whole
    /// chunk exists to avoid.
    ///
    /// **It belongs to the orders as a whole, not to one leg.** A plan is
    /// one intention: a commander who wants the third bound and the first
    /// loose is a commander who countermands when it is time, which is what
    /// she would do on the day. So an amendment sets it as a replacement
    /// does, and a promoted leg inherits it.
    ///
    /// `#[serde(default)]` — and [`Latitude::Delegated`] is the default — so
    /// a save, a scenario and an AI side that never say otherwise mean
    /// exactly the game that was here before.
    #[serde(default)]
    pub latitude: Latitude,
    /// The rest of the plan: missions queued behind the standing one, in the
    /// order they will be taken up.
    ///
    /// Transmitted once and executed locally — the leader has known the
    /// whole plan since it arrived, so promotion from one leg to the next
    /// costs no wire and no latency, which is the Auftragstaktik shape and
    /// also the cheap one. Promotion happens when the standing mission
    /// *completes* (see `promote_missions`); `Hold` and `Withdraw` never
    /// complete, so nothing may be queued behind them and validation says so
    /// out loud rather than letting an impossible plan sit silently.
    #[serde(default)]
    pub plan: std::collections::VecDeque<Mission>,
    /// An order still travelling, what it will do to the plan when it lands,
    /// and how many ticks it has left to travel.
    ///
    /// Orders stop being instantaneous once a mod declares a `command`
    /// block: sent when issued (the
    /// [`crate::battle::Event::MissionAssigned`] the log already carries),
    /// arriving some ticks later as `MissionReceived`. A second order issued
    /// while one is in the air replaces it — countermanding is ordinary
    /// business, and two orders in transit at once is not a state anyone
    /// could act on. An amendment (`Append`) travels exactly like a
    /// replacement: the wire does not care what the envelope says.
    ///
    /// `None` for every battle whose mod declares no command rules, because
    /// the delay is then zero and an order never travels at all.
    #[serde(default)]
    pub incoming: Option<(MissionChange, u32)>,
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

/// What an order in transit will do to the formation's plan when it lands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionChange {
    /// This mission becomes the standing one and everything queued behind
    /// the old one is off — a countermand replaces the plan, not a line of
    /// it.
    Replace {
        mission: Mission,
        /// Carried with the mission rather than applied when it was sent,
        /// for the same reason [`WaitingOrders`] carries a unit's: an order
        /// held on the wire has to arrive meaning what it meant when it was
        /// given, not what the commander happens to mean by the time it
        /// lands.
        #[serde(default)]
        latitude: Latitude,
    },
    /// This mission joins the end of the plan: "…and then this."
    Append {
        mission: Mission,
        #[serde(default)]
        latitude: Latitude,
    },
}

impl MissionChange {
    pub fn mission(&self) -> &Mission {
        match self {
            Self::Replace { mission, .. } | Self::Append { mission, .. } => mission,
        }
    }

    pub fn latitude(&self) -> Latitude {
        match self {
            Self::Replace { latitude, .. } | Self::Append { latitude, .. } => *latitude,
        }
    }
}

/// One member out of contact, and the orders she is soldiering on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutOff {
    pub unit: UnitId,
    /// The formation mission as of the tick she lost contact — possibly
    /// `None`, because being cut off with no orders is its own state: she
    /// fights by her own judgment, exactly as an unmissioned unit does.
    pub orders: Option<Mission>,
    /// And the latitude they were given with, snapshotted at the same
    /// moment and for the same reason. `#[serde(default)]` so a save written
    /// before missions had latitude opens as one where nobody was ever held
    /// to the letter of anything, which is what it was.
    #[serde(default)]
    pub latitude: Latitude,
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

    /// The latitude those orders came with — the twin of [`Self::mission_for`]
    /// and resolved the same way, because an order and how hard it was meant
    /// travel together or they do not travel at all. A cut-off crew soldiers
    /// on the orders she was given *as she was given them*; the formation's
    /// latitude may have changed twice behind her back and she has heard
    /// none of it.
    pub fn latitude_for(&self, unit: UnitId) -> Latitude {
        match self.out_of_contact.iter().find(|c| c.unit == unit) {
            Some(cut) => cut.latitude,
            None => self.latitude,
        }
    }

    /// The tail of the pipeline: the last thing this formation will end up
    /// doing once everything sent, queued and standing has run its course.
    ///
    /// What a commander should compare against before issuing anything: an
    /// order already in the air or in the plan is one she has given, and
    /// re-sending it would fill the log with the same sentence and reset
    /// the clock each time. It is also what queue-validation tests for a
    /// terminal mission, because "what would this follow" is a question
    /// about the tail. What the *executors* act on is [`Self::mission`] —
    /// deliberately different, because the whole point of latency is that
    /// the formation does not yet know.
    pub fn latest_mission(&self) -> Option<&Mission> {
        match &self.incoming {
            // A replacement in the air supersedes the whole plan; an
            // amendment in the air lands at the end of it. Either way the
            // travelling order is the tail.
            Some((change, _)) => Some(change.mission()),
            None => self.plan.back().or(self.mission.as_ref()),
        }
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
    /// Where she is to end up and how hard it was meant, re-pathed on
    /// delivery. One [`March`] rather than a destination beside a latitude,
    /// for the reason that type exists: an order that waited at the radio
    /// arrives meaning what it meant when it was sent, and a commander who
    /// said "press on" and could not be heard has still said it.
    #[serde(default)]
    pub march: Option<March>,
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
    /// Per side, the units whose reports cannot currently reach command —
    /// a receive-only set with no flag route to a transmitter, or a
    /// transmitter masked by the ground. Sorted by unit id, sized lazily
    /// like the pictures. Hearing and speaking came apart when radios
    /// became hardware: `out_of_contact` is the first, this is the second.
    #[serde(default)]
    voiceless: Vec<Vec<UnitId>>,
    /// Radioed orders that have not reached the cadet they were meant for,
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
    /// The plans commanders have in flight, one per commander at most,
    /// sorted by side and then commander.
    ///
    /// On the battle rather than in the planner that made them, for the
    /// reason a `Goal` lives on the unit: a battle forked through a save file
    /// has to have the same future, and a planner's private memory does not
    /// survive that. `#[serde(default)]`: a save from before plans opens with
    /// nobody planning anything, which is what was true of it.
    #[serde(default)]
    plans: Vec<Plan>,
}

/// A commander's plan in flight: a template matched onto ground, and how far
/// through it she is. See `crate::ai::plan` for how one is chosen and
/// PLANNING.md for why plans exist at all.
///
/// A record of intent, not a rule: nothing in the engine reads it to decide
/// what happens. The orders a plan produces are ordinary missions, carried by
/// the ordinary net at its ordinary speed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    pub side: u8,
    /// Whose plan it is: the unit carrying the commanding cadet.
    pub commander: UnitId,
    /// The [`crate::data::TemplateDef`] id.
    pub template: String,
    /// The formation that takes the firing position and holds the enemy.
    pub fix: FormationId,
    /// The formation that goes round.
    pub manoeuvre: FormationId,
    /// The enemy being fixed and flanked.
    pub target: UnitId,
    pub fire_position: Hex,
    /// Where the manoeuvre element waits, unseen if the ground allows, for
    /// the word to go.
    pub assembly: Hex,
    /// The ground off the enemy's frontal arc it goes in to.
    pub flank: Hex,
    pub phase: PlanPhase,
    /// What the plan scored when it was chosen, in hundredths — an integer so
    /// that comparing it is exact on every machine.
    pub score: i32,
    /// The round by which the word to go is given whatever else has
    /// happened: when the flankers should have arrived, and two rounds'
    /// grace. A plan that waits for ever leaves half the force sitting at an
    /// assembly point, which the first measurements showed it doing.
    pub go_by: u32,
}

/// How far through a plan its commander is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanPhase {
    /// The elements are moving to their positions.
    Forming,
    /// The word has been given: the manoeuvre element is going in.
    Going,
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
                    latitude: Latitude::default(),
                    plan: std::collections::VecDeque::new(),
                    incoming: None,
                    out_of_contact: Vec::new(),
                })
            })
            .collect();
        Self {
            formations,
            pictures: Vec::new(),
            waiting: Vec::new(),
            voiceless: Vec::new(),
            plans: Vec::new(),
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
    pub fn set_mission(
        &mut self,
        formation: FormationId,
        mission: Mission,
        latitude: Latitude,
    ) -> bool {
        match self.formations.get_mut(formation.index()) {
            Some(f) => {
                f.mission = Some(mission);
                f.latitude = latitude;
                // Anything still travelling or still queued has been
                // overtaken by this: a countermand replaces the plan, and
                // letting an old leg land or begin afterwards would quietly
                // undo the order that arrived last.
                f.incoming = None;
                f.plan.clear();
                true
            }
            None => false,
        }
    }

    /// Every radioed order still waiting for its cadet, in unit-id order.
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
        march: Option<March>,
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
        if march.is_some() {
            // Latitude travels inside the march, so a later order that says
            // nothing about where she is going cannot say anything about how
            // hard she is to press either — it is one assignment now rather
            // than two that had to be remembered together.
            slot.march = march;
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

    /// Put an order in the air: it will change the formation's plan in
    /// `ticks` ticks, not now. Returns whether the formation exists.
    ///
    /// Only ever called with `ticks > 0` — a delay of zero is an order that
    /// arrived, and goes through [`Self::set_mission`] or
    /// [`Self::queue_mission`] like any other, which is what makes the
    /// no-rules game the *same code path* rather than the same code path
    /// plus a branch.
    pub fn set_incoming(
        &mut self,
        formation: FormationId,
        change: MissionChange,
        ticks: u32,
    ) -> bool {
        match self.formations.get_mut(formation.index()) {
            Some(f) => {
                f.incoming = Some((change, ticks));
                true
            }
            None => false,
        }
    }

    /// Add a mission to the end of a formation's plan — or make it the
    /// standing one, if the formation had nothing to do: an amendment to an
    /// empty plan is simply the first order. Returns whether the formation
    /// exists.
    pub fn queue_mission(
        &mut self,
        formation: FormationId,
        mission: Mission,
        latitude: Latitude,
    ) -> bool {
        match self.formations.get_mut(formation.index()) {
            Some(f) => {
                if f.mission.is_none() {
                    f.mission = Some(mission);
                } else {
                    f.plan.push_back(mission);
                }
                // An amendment speaks for the whole plan — see the note on
                // [`Formation::latitude`]. The alternative is a latitude per
                // leg, which buys a distinction ("take the ford loosely, and
                // then the ridge come what may") that no commander issues in
                // one breath and that a countermand already expresses.
                f.latitude = latitude;
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
                    &u.crew_state,
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
            if arrived && let Some((change, _)) = formation.incoming.take() {
                let mission = change.mission().clone();
                match change {
                    MissionChange::Replace { mission, latitude } => {
                        formation.mission = Some(mission);
                        formation.latitude = latitude;
                        formation.plan.clear();
                    }
                    MissionChange::Append { mission, latitude } => {
                        if formation.mission.is_none() {
                            formation.mission = Some(mission);
                        } else {
                            formation.plan.push_back(mission);
                        }
                        formation.latitude = latitude;
                    }
                }
                events.push(Event::MissionReceived {
                    formation: formation.id.clone(),
                    mission,
                });
            }
        }
    }

    /// Hand command to the next cadet in the order of battle wherever the
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
    /// Succession is by **rank** — the highest rank still fighting aboard
    /// any member ([`Self::unit_rank`]) — and among equals by **lowest living
    /// unit id**, which is placement order, which is the order the map author
    /// wrote her formation down in. With no ladder, or nobody ranked, that is
    /// placement order alone, which is how command passed before ranks
    /// existed. Seniority is something content states — a rank, a place in
    /// the order of battle — never something the engine infers from stats.
    ///
    /// The successor is worse at the job and no code here makes her so. Every
    /// price the chain of command charges — [`BattleState::mission_delay`] and
    /// the contact radius below — is already read off *the current leader's*
    /// crew, at the place she is standing, so promoting a cadet with a weaker
    /// `command` skill lengthens her formation's latencies and promoting one
    /// with weaker `signals` shrinks its net, for free and for the right
    /// reason. Building a separate penalty on top would be pricing the same
    /// thing twice.
    ///
    /// Formations are walked in declaration order and members in id order, so
    /// what this emits cannot depend on a hash.
    pub(super) fn pass_command(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        for index in 0..self.command.formations.len() {
            let formation = &self.command.formations[index];
            // `unit()` filters on `alive`, which is false for the destroyed
            // and for anyone who drove off by an exit — and both are reasons
            // somebody else has to take over. Whether she *died* is a
            // different question, asked by the scenario's loss conditions.
            let Some(gone) = formation.leader.filter(|id| self.unit(*id).is_none()) else {
                continue;
            };
            // The highest rank still in the fight, and among equals the first
            // in the order the map wrote the formation down — so with no
            // ranks at all this is exactly the arrival order it always was.
            let mut successor: Option<(UnitId, Option<usize>)> = None;
            for id in formation.members.iter().copied() {
                if self.unit(id).is_none() {
                    continue;
                }
                let rank = self.unit_rank(registry, id);
                if successor.is_none_or(|(_, best)| rank > best) {
                    successor = Some((id, rank));
                }
            }
            let successor = successor.map(|(id, _)| id);
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

    /// Move every formation whose standing mission is *done* on to the next
    /// leg of its plan, and say so.
    ///
    /// Runs at the top of a round, so the new leg steers this round's
    /// planning. Promotion costs no wire and no latency on purpose: the plan
    /// was transmitted once and the formation has known all of it since —
    /// which is also why a cut-off member does not stall the plan; she is
    /// soldiering on her snapshot while the rest move on, and catches up
    /// when the net does.
    ///
    /// Completion is the mission's own meaning: an advance is done when a
    /// member stands on the ground (within one hex), a reconnaissance when a
    /// member has eyes on the target tile. The terminal missions — `Hold`,
    /// `Withdraw` and `Support`, the postures rather than the legs — never
    /// complete; validation refuses to queue behind them, so a plan can
    /// only ever be waiting behind a leg that can actually end. A completed
    /// mission with nothing queued stands: the reward that pulled the
    /// formation there is the same one that keeps it there, and announcing a
    /// completion nobody acts on every round would be noise.
    pub(super) fn promote_missions(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        for index in 0..self.command.formations.len() {
            let formation = &self.command.formations[index];
            if formation.plan.is_empty() {
                continue;
            }
            let done = match &formation.mission {
                // A plan behind no standing mission begins immediately; the
                // orders layer prevents the state, but a save edited by hand
                // should start marching rather than sit wedged.
                None => true,
                // An advance and an assault end the same way: somebody is
                // standing on the ground. What they cost to get there is not
                // a question about completion.
                Some(Mission::Advance { to }) | Some(Mission::Assault { to }) => formation
                    .members
                    .iter()
                    .filter_map(|id| self.unit(*id))
                    .any(|u| u.pos.distance_to(*to) <= 1),
                Some(Mission::Recon { toward }) => formation
                    .members
                    .iter()
                    .any(|id| super::fog::sees(registry, self, *id, *toward)),
                Some(Mission::Hold { .. })
                | Some(Mission::Withdraw { .. })
                | Some(Mission::Support { .. }) => false,
            };
            if !done {
                continue;
            }
            let formation = &mut self.command.formations[index];
            let completed = formation.mission.take();
            let next = formation.plan.pop_front().expect("checked non-empty");
            formation.mission = Some(next.clone());
            let id = formation.id.clone();
            if let Some(completed) = completed {
                events.push(Event::MissionCompleted {
                    formation: id.clone(),
                    mission: completed,
                });
            }
            events.push(Event::MissionAssigned {
                formation: id,
                mission: next,
            });
        }
    }

    /// How far this crew's own transmissions carry, in hexes: her vehicle's
    /// radio worked by her crew's `signals`, falling back to the command
    /// block's `radius` for a vehicle that declares no set.
    ///
    /// **Presentation's window into the net.** The contact graph below walks
    /// exactly this sum for every transmitter it visits, and it is public
    /// only so that the range ring the battle screen draws round a leader is
    /// *the same number the engine used* rather than the game crate's copy of
    /// the formula — a copy that would silently start lying the day a mod
    /// changed `radius_per_signals` or a vehicle grew a better radio.
    ///
    /// `None` where there is nothing to draw: a mod that prices no chain of
    /// command has no net, and a unit no longer on the field is not
    /// transmitting.
    pub fn radio_reach(&self, registry: &DataRegistry, unit: UnitId) -> Option<u32> {
        let rules = registry.command.as_ref()?;
        let unit = self.unit(unit)?;
        let vehicle = registry.vehicle(&unit.vehicle);
        // What the set can do is hardware; whether it can send at all is the
        // set's nature. A vehicle naming no set keeps the block's symmetric
        // radius, so content from before radios were things is unchanged; a
        // receive-only set answers `None` here, which is also exactly what
        // the range ring should draw for her — nothing.
        // A destroyed radio module is a set that no longer exists, whatever
        // its paper range was. Content without a radio module keeps working
        // untouched — `module_ok` answers true when nothing of the effect
        // was ever declared.
        if !unit.module_ok(registry, crate::data::ModuleEffect::Radio) {
            return None;
        }
        let hardware = match vehicle.and_then(|v| v.radio.as_deref()) {
            Some(set) => registry.radio(set).and_then(|r| r.send)?,
            None => rules.radius,
        };
        let signals = self.roster.crew_skill(
            registry,
            vehicle,
            &unit.crew,
            &unit.crew_state,
            SIGNALS,
            self.terrain_at(unit.pos),
        );
        Some(rules.radio_range(hardware, signals))
    }

    /// Whether a radio wave gets from `a` to `b`: VHF is line-of-sight-ish,
    /// so *terrain* stands in the way — and only terrain. Forests do not
    /// mask a radio the way they mask an eye, and the check is priced more
    /// generously than gun sight because a mast clears what a gunsight
    /// cannot: both ends get an antenna's worth of extra height.
    ///
    /// Walked over raw map elevation rather than through the sight grid,
    /// because the grid's heights bake in `vision_block` — the forest
    /// canopy — which is exactly the part radio does not care about.
    fn radio_clear(&self, a: Hex, b: Hex) -> bool {
        /// Elevation levels of mast, granted to each end.
        const ANTENNA: f32 = 1.5;
        let (Some(from), Some(to)) = (self.world.get(a), self.world.get(b)) else {
            return false;
        };
        let h_a = from.elevation as f32 + ANTENNA;
        let h_b = to.elevation as f32 + ANTENNA;
        let line = a.line_to(b).collect::<Vec<_>>();
        let steps = line.len().saturating_sub(1).max(1) as f32;
        for (i, hex) in line.iter().enumerate() {
            if *hex == a || *hex == b {
                continue;
            }
            let Some(tile) = self.world.get(*hex) else {
                // Off-map gaps in the line do not block a wave.
                continue;
            };
            let along = i as f32 / steps;
            let ray = h_a + (h_b - h_a) * along;
            if tile.elevation as f32 > ray {
                return false;
            }
        }
        true
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
    /// [`Self::pass_command`] runs first and hands the net to the next cadet,
    /// so a leader dying costs her formation a tick of nothing rather than the
    /// rest of the battle in silence.
    ///
    /// Does nothing at all when no mod declares command rules. Not an
    /// optimisation: it is what makes `out_of_contact` empty forever in a
    /// battle that never asked for a chain of command, so every caller reading
    /// [`Formation::in_contact`] gets `true` and the old game back.
    pub(super) fn recompute_contact(&mut self, registry: &DataRegistry, events: &mut Vec<Event>) {
        self.walk_net(registry, Some(events));
    }

    /// The upward half of [`Self::recompute_contact`] alone: whose reports
    /// can reach command (`voiceless`), without settling who has lost or
    /// regained contact.
    ///
    /// For battle setup, which needs the picture before round one is planned
    /// and so needs to know who can speak — but must not settle contact,
    /// because a crew who *starts* cut off is news, and the first tick is
    /// where she is announced (`a_scout_out_of_contact_reports_nothing`).
    pub(super) fn recompute_voices(&mut self, registry: &DataRegistry) {
        self.walk_net(registry, None);
    }

    /// Whether `speaker` can tell `listener` anything, on `net`.
    ///
    /// Radio needs the speaker's set, a listener whose set works, and a path
    /// the wave survives (VHF is line-of-sight-ish: hills mask, forests do
    /// not); on a formation's own net it also needs the two to share that
    /// formation, and on the command net it does not — that net is the one
    /// formation leaders talk to each other on. A flag needs only eyes, on
    /// either net: visual signalling is promiscuous and symmetric.
    fn informs(
        &self,
        registry: &DataRegistry,
        rules: &crate::data::CommandRules,
        speaker: UnitId,
        listener: UnitId,
        net: Net,
    ) -> bool {
        let (Some(s), Some(l)) = (self.unit(speaker), self.unit(listener)) else {
            return false;
        };
        let dist = s.pos.distance_to(l.pos);
        let by_radio = self
            .radio_reach(registry, speaker)
            .is_some_and(|reach| dist <= reach as i32)
            // Hearing needs a working set too: a listener whose radio was
            // shot out is off the net however loudly her leader transmits.
            // Visual signalling below is what she has left, which is exactly
            // the early-war fallback.
            && l.module_ok(registry, crate::data::ModuleEffect::Radio)
            && match net {
                Net::Command => true,
                Net::Formation => {
                    let squad = self.command.formation_of(speaker).map(|f| f.id.as_str());
                    squad.is_some()
                        && self.command.formation_of(listener).map(|f| f.id.as_str()) == squad
                }
            }
            && self.radio_clear(s.pos, l.pos);
        let by_sight = rules.visual_range > 0
            && dist <= rules.visual_range as i32
            && self.world.sight().clear(s.pos, l.pos);
        by_radio || by_sight
    }

    /// Her rank, for command: the highest rank of any cadet aboard her and
    /// still in the fight, as a place on the mod's ladder. `None` — below
    /// every declared rank — for an anonymous crew, a crew nobody ranked, or
    /// a mod that declares no ladder, which is everybody equal.
    ///
    /// Of anybody aboard rather than of one seat, because the senior cadet in
    /// a hull commands it whatever seat she is in; and of those *fighting*,
    /// because a lieutenant carried out unconscious commands nothing.
    pub fn unit_rank(&self, registry: &DataRegistry, unit: UnitId) -> Option<usize> {
        let u = self.unit(unit)?;
        u.crew
            .iter()
            .enumerate()
            .filter(|(i, _)| u.crew_state.get(*i).is_none_or(|c| c.fighting()))
            .filter_map(|(_, id)| self.roster.get(*id))
            .filter_map(|cadet| registry.rank_index(cadet.rank.as_deref()))
            .max()
    }

    /// Who commands whom on `side` at this moment: one [`OperationalCommand`]
    /// per group of leaders the most senior of them can reach with orders.
    ///
    /// **Derived every time it is asked, never declared** — the designer's
    /// ruling that command is a role held by whoever is present, senior and
    /// connected, not a title written on the map. The leaders are every
    /// formation's leader and every unit in no formation. The most senior of
    /// them (rank, then formations in the order the map declared them, then
    /// unit id) starts a command and takes every leader her orders reach on
    /// the command net (`informs`, [`Net::Command`]) — through leaders she
    /// has already reached, if the mod's command block relays. The most
    /// senior leader left over starts the next one, and so on.
    ///
    /// So a connected army is one command under its senior officer; a column
    /// cut off behind a hill is a second command under *its* senior; and a
    /// junior who can reach the commander does not command a senior who
    /// cannot — they are two commands. Orders care about rank; reports do not,
    /// and this is not where reports are decided.
    ///
    /// With no `command` block every leader reaches every other, so a side is
    /// one command under its most senior leader — the game before, where one
    /// brain spoke for the side.
    pub fn operational_commands(
        &self,
        registry: &DataRegistry,
        side: u8,
    ) -> Vec<OperationalCommand> {
        // (leader, the formation she leads if any) in declaration order, then
        // the unattached by id.
        let mut leaders: Vec<(UnitId, Option<usize>)> = self
            .command
            .formations
            .iter()
            .enumerate()
            .filter(|(_, f)| f.side == side)
            .filter_map(|(i, f)| {
                f.leader
                    .filter(|id| self.unit(*id).is_some())
                    .map(|l| (l, Some(i)))
            })
            .collect();
        let mut loose: Vec<UnitId> = self
            .side_units(side)
            .filter(|u| u.aboard.is_none() && self.command.formation_of(u.id).is_none())
            .map(|u| u.id)
            .collect();
        loose.sort_unstable();
        leaders.extend(loose.into_iter().map(|id| (id, None)));
        let seniority: Vec<Option<usize>> = leaders
            .iter()
            .map(|(id, _)| self.unit_rank(registry, *id))
            .collect();
        // Most senior first; the declared order breaks ties, never a coordinate.
        let mut order: Vec<usize> = (0..leaders.len()).collect();
        order.sort_by_key(|i| (std::cmp::Reverse(seniority[*i]), *i));

        let tells = |speaker: UnitId, listener: UnitId| match registry.command.as_ref() {
            None => true,
            Some(rules) => self.informs(registry, rules, speaker, listener, Net::Command),
        };
        let relay = registry.command.as_ref().is_none_or(|r| r.relay);
        let mut assigned = vec![false; leaders.len()];
        let mut commands = Vec::new();
        for &head in &order {
            if assigned[head] {
                continue;
            }
            assigned[head] = true;
            let mut members = vec![head];
            let mut anchors = VecDeque::from([head]);
            while let Some(anchor) = anchors.pop_front() {
                for &next in &order {
                    if assigned[next] || !tells(leaders[anchor].0, leaders[next].0) {
                        continue;
                    }
                    assigned[next] = true;
                    members.push(next);
                    if relay {
                        anchors.push_back(next);
                    }
                }
            }
            members.sort_unstable();
            commands.push(OperationalCommand {
                commander: leaders[head].0,
                leaders: members.iter().map(|i| leaders[*i].0).collect(),
                formations: members.iter().filter_map(|i| leaders[*i].1).collect(),
            });
        }
        commands
    }

    fn walk_net(&mut self, registry: &DataRegistry, mut events: Option<&mut Vec<Event>>) {
        let Some(rules) = registry.command.as_ref() else {
            return;
        };
        // The net is two media and, since radios became hardware, two
        // *directions*, walked side-wide in a pair of breadth-first passes.
        //
        // **Radio follows the chain of command and the nature of the set**:
        // a transmitter reaches her own formation, at her set's send range
        // worked by her crew's signals, over a terrain-clear path (VHF is
        // line-of-sight-ish: hills mask, forests do not, and everyone gets
        // an antenna's grace) — and a receive-only set transmits nothing,
        // which is the early-war fit: she hears everything and answers with
        // her tracks. **Visual signalling is promiscuous and symmetric**: a
        // flag carries `visual_range` hexes to ANY friendly with a clear
        // sight line, formation be damned.
        //
        // Downward, from the roots (formation leaders and everyone in no
        // formation), the walk answers "who can HEAR her orders" — that is
        // `out_of_contact`, and standing orders soldier on where it fails.
        // Upward, along the reversed edges, it answers "whose REPORTS can
        // reach command" — that is `voiceless`, and the picture learns
        // nothing from a cadet who cannot speak: a receiver-only scout must
        // flag her sighting to somebody with a set, or it dies with her
        // silence. Roots, queues and candidate scans are all in unit-id
        // order, so neither set nor any event can depend on a hash.
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

            // An edge is "speaker informs listener", on her formation's own
            // net: see `informs`.
            let informs = |speaker: UnitId, listener: UnitId| -> bool {
                self.informs(registry, rules, speaker, listener, Net::Formation)
            };
            // One walk, both directions: `down` grows the set orders reach,
            // `up` the set reports escape from.
            let walk = |down: bool| -> Vec<UnitId> {
                let mut reached: Vec<UnitId> = roots.clone();
                let mut anchors: VecDeque<UnitId> = roots.clone().into();
                while let Some(anchor) = anchors.pop_front() {
                    for id in &living {
                        if reached.contains(id) {
                            continue;
                        }
                        let linked = if down {
                            informs(anchor, *id)
                        } else {
                            informs(*id, anchor)
                        };
                        if linked {
                            reached.push(*id);
                            if rules.relay {
                                anchors.push_back(*id);
                            }
                        }
                    }
                }
                reached
            };
            let heard = walk(true);
            let speaking = walk(false);

            if let Some(events) = events.as_deref_mut() {
                self.settle_contact(side, &heard, events);
            }
            let mut voiceless: Vec<UnitId> = living
                .iter()
                .copied()
                .filter(|id| !speaking.contains(id))
                .collect();
            voiceless.sort_unstable();
            if self.command.voiceless.len() <= side as usize {
                self.command.voiceless.resize(self.sides.len(), Vec::new());
            }
            self.command.voiceless[side as usize] = voiceless;
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
                .map(|id| {
                    let already = formation.out_of_contact.iter().find(|c| c.unit == *id);
                    CutOff {
                        unit: *id,
                        orders: already
                            .map(|c| c.orders.clone())
                            .unwrap_or_else(|| formation.mission.clone()),
                        latitude: already.map_or(formation.latitude, |c| c.latitude),
                    }
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

    /// The plans in flight on `side`, by commander.
    pub fn plans(&self, side: u8) -> impl Iterator<Item = &Plan> {
        self.command.plans.iter().filter(move |p| p.side == side)
    }

    /// The plan `commander` has in flight, if any.
    pub fn plan_of(&self, commander: UnitId) -> Option<&Plan> {
        self.command.plans.iter().find(|p| p.commander == commander)
    }

    /// Record, replace or drop `commander`'s plan, and say so to her side.
    pub(super) fn set_plan(
        &mut self,
        side: u8,
        commander: UnitId,
        plan: Option<Plan>,
    ) -> Result<Vec<Event>, OrderError> {
        let own = self.unit(commander).ok_or(OrderError::NoSuchUnit)?.side;
        if own != side
            || plan
                .as_ref()
                .is_some_and(|p| p.side != side || p.commander != commander)
        {
            return Err(OrderError::NoSuchSide);
        }
        if self.has_committed(side) {
            return Err(OrderError::AlreadyCommitted);
        }
        let before = self
            .command
            .plans
            .iter()
            .position(|p| p.commander == commander);
        let old = before.map(|i| self.command.plans.remove(i));
        let mut events = Vec::new();
        match (&old, &plan) {
            (Some(was), None) => {
                let done = was.phase == PlanPhase::Going
                    && self
                        .command
                        .formations
                        .get(was.manoeuvre.index())
                        .and_then(|f| f.leader)
                        .and_then(|id| self.unit(id))
                        .is_some_and(|u| u.pos.distance_to(was.flank) <= 1)
                    || self.unit(was.target).is_none();
                events.push(if done {
                    Event::PlanDone {
                        commander,
                        template: was.template.clone(),
                    }
                } else {
                    Event::PlanDropped {
                        commander,
                        template: was.template.clone(),
                    }
                });
            }
            (Some(was), Some(now))
                if was.template == now.template
                    && was.fix == now.fix
                    && was.manoeuvre == now.manoeuvre
                    && was.phase == PlanPhase::Forming
                    && now.phase == PlanPhase::Going =>
            {
                events.push(Event::PlanGoing {
                    commander,
                    template: now.template.clone(),
                });
            }
            (Some(was), Some(now)) if was == now => {}
            (_, Some(now)) => events.push(Event::PlanAdopted {
                commander,
                template: now.template.clone(),
                fix: self.command.formations[now.fix.index()].id.clone(),
                manoeuvre: self.command.formations[now.manoeuvre.index()].id.clone(),
            }),
            (None, None) => {}
        }
        if let Some(plan) = plan {
            self.command.plans.push(plan);
            self.command
                .plans
                .sort_by_key(|p| (p.side, p.commander.index()));
        }
        Ok(events)
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

    /// The enemies `who` knows are out there, in unit-id order: the one
    /// answer to that question for every brain and every screen.
    ///
    /// Two knowers, because the picture above exists to make them differ:
    ///
    /// - **A commander** — the player, or the AI's mission review — knows
    ///   what has been *reported*: the fresh contacts in her picture. A ghost
    ///   is where somebody was, not somebody to plan against.
    /// - **A crew** — the executor planning her round, the drill, the rout,
    ///   the danger she is priced against — knows that picture *if she can
    ///   hear the net*, and whatever she has acquired with her own eyes
    ///   whether or not she could report it — her eyes, and a muzzle flash,
    ///   which the fog gives the whole side. A scout out of contact sees
    ///   things her commander is never told, and she still acts on them.
    ///
    /// With no `command` block both are the side's fog, every crew's eyes
    /// pooled — the game exactly as it was, with no switch in Rust to say so.
    ///
    /// This used to be `ai::visible_enemies`, which read the pooled fog
    /// everywhere, command rules or not. So under a chain of command the
    /// player was shown the picture while every AI crew planned against
    /// everything any crew on her side had seen, reported or not — the radio
    /// cost the human and nobody else — and the player's danger overlay,
    /// which read the same pooled list, could name a gun her screen drew as
    /// a ghost or not at all.
    pub fn known_enemies(&self, registry: &DataRegistry, who: Knower) -> Vec<&Unit> {
        let side = match who {
            Knower::Commander(side) => side,
            Knower::Crew(id) => match self.unit(id) {
                Some(u) => u.side,
                None => return Vec::new(),
            },
        };
        let fog = self.fog.side(side);
        let spotted = self
            .alive_units()
            .filter(move |u| u.side != side && fog.spotted.contains(&u.id));
        if registry.command.is_none() {
            return spotted.collect();
        }
        let reported = |enemy: UnitId| {
            self.picture(side)
                .iter()
                .any(|c| c.unit == enemy && c.fresh)
        };
        match who {
            Knower::Commander(_) => spotted.filter(|e| reported(e.id)).collect(),
            Knower::Crew(id) => {
                let hears = self.hears_orders(id);
                spotted
                    .filter(|e| {
                        (hears && reported(e.id))
                            || fog.revealed.contains(&e.id)
                            || super::fog::sees(registry, self, id, e.pos)
                    })
                    .collect()
            }
        }
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
                // Seeing is not reporting, and — since radios became
                // hardware — hearing is not speaking either: the report
                // needs a route to command, which a receive-only set does
                // not provide on its own.
                //
                // A gun that gave herself away by firing is the one exception
                // to "somebody has to see her hex": the fog spots her for the
                // whole side off the muzzle flash, and so any crew who can
                // speak can say where it came from. Without this the picture
                // was stricter than the fog it is built from — a howitzer
                // shelling the line from beyond everybody's sight was
                // spotted and never reported, even on a perfect net.
                let flashed = self.fog.side(side).revealed.contains(&enemy);
                let speaking = || {
                    self.side_units(side).filter(|u| {
                        !self
                            .command
                            .voiceless
                            .get(side as usize)
                            .is_some_and(|v| v.contains(&u.id))
                    })
                };
                // An eyewitness files it when there is one; the flash is
                // only the fallback, so a report still names who saw her.
                let reporter = speaking()
                    .find(|u| super::fog::sees(registry, self, u.id, target.pos))
                    .or_else(|| speaking().find(|_| flashed))
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
                let gone = self.units.get(old.unit.index()).is_none_or(|u| !u.alive());
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
