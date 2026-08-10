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
//! everything after it depends on — ids, membership, seniority — are settled
//! and provably inert first: the determinism baseline is fought on
//! `river_crossing`, which now declares formations, so an unchanged event
//! stream is a proof that no decision consults them.
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
use serde::{Deserialize, Serialize};

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
}
