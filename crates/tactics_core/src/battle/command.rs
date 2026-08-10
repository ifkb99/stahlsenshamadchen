//! The chain of command, as a battle carries it.
//!
//! A side is a flat pool of units that an all-seeing planner drives one at a
//! time. A *formation* is the unit of command that pool is missing: the thing
//! a mission is given to, that a leader can be lost from, and that can be out
//! of contact while the rest of the side is not. Maps declare them (see
//! [`crate::map::FormationDef`]); this is what a battle makes of the
//! declaration once the units it names actually exist.
//!
//! Nothing reads this yet. It is populated and serialized so that the shapes
//! everything after it depends on — ids, membership, seniority, and now the
//! standing [`Mission`] a formation is under — are settled and provably inert
//! first: the determinism baseline is fought on `river_crossing`, which now
//! declares formations, so an unchanged event stream is a proof that no
//! decision consults them. Missions can be *set* (through
//! [`crate::battle::Order::SetMission`], validated like any other order) and
//! are carried through saves; what reads one to move a tank is the executor
//! half of this chunk and lands next.
//!
//! # Order
//!
//! Everything here is in declaration order (formations, as the map file wrote
//! them) or unit-id order (members, which is placement order because both
//! setup paths spawn placements into an empty unit list). Never a hash order.
//! This is not a stylistic preference: formations will be walked to produce
//! events and to make AI decisions, and an iteration-order dependency in that
//! walk is exactly the bug that hid in `fog::recompute` for months.

use super::UnitId;
use crate::map::{FormationDef, UnitPlacement};
use hexx::Hex;
use serde::{Deserialize, Serialize};

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
/// Nothing reads a mission yet. The executors that will carry these out are
/// the other half of this build chunk; what is settled here is the vocabulary
/// and where it is kept, so that half has something to consume.
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
    /// `Option` because a formation can be *left* leaderless — the leader's
    /// vehicle is destroyed and succession has not happened yet — not because
    /// a fresh one ever is. A formation with members always starts with one.
    pub leader: Option<UnitId>,
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
}

impl Formation {
    /// Whether this unit answers to this formation.
    pub fn contains(&self, unit: UnitId) -> bool {
        self.members.contains(&unit)
    }
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
                    members: members.into_iter().map(|(id, _)| id).collect(),
                    doctrine: def.doctrine.clone(),
                    mission: None,
                })
            })
            .collect();
        Self { formations }
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
                true
            }
            None => false,
        }
    }
}
