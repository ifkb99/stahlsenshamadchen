//! Girls as instances rather than definitions.
//!
//! A [`crate::data::CharacterDef`] is mod data: a name, a portrait, a bio and
//! the stats someone *starts* with. It is immutable and shared, which is right
//! for content and wrong for a person. Until this module existed a unit's crew
//! was `Vec<String>` — keys into that static table — so there was nowhere to
//! record that Anka has been in nine battles, is carrying a wound, and has
//! learned to shoot better than the girl she was defined as.
//!
//! [`Girl`] is that missing object and [`Roster`] owns them. The distinction
//! matters in three places at once, which is why it is worth doing before any
//! of them are built:
//!
//! - **Progression.** XP and stat growth belong to the instance; the
//!   definition stays the starting point a new recruit is stamped from.
//! - **Wounds.** A hit that takes a crew member out has to mark *her*, not the
//!   vehicle, and has to still be true next battle.
//! - **Support conversations.** The academy half of the game wants to know who
//!   has fought alongside whom and how often, which is per-pair history hanging
//!   off the same instances.
//!
//! # Ownership, and why there is one roster rather than one per side
//!
//! Every girl in the world lives in a single [`Roster`] and carries the
//! academy she belongs to in [`Girl::owner`]. The alternative — a roster per
//! side — would make [`GirlId`] ambiguous without a side alongside it, which
//! would push side-indexing down into the battle layer for no gain.
//!
//! This shape is also the one a 4x mode wants. A campaign is a two-academy
//! case of the same thing, so girls changing hands — recruited, poached,
//! captured, transferred between academies — is a field change here rather
//! than a data migration later.
//!
//! # Death is a rule, not a fact of the model
//!
//! [`GirlStatus::Dead`] is only reachable when [`CasualtyRules::permadeath`]
//! is on, which is a per-campaign option rather than something the engine
//! decides. With it off, the worst a crew suffers is a long recovery.
//!
//! Note that [`GirlStatus::Lost`] is *not* death and never was: it means she
//! bailed out and could not reach friendly lines before the fighting stopped,
//! and is making her own way back. It resolves on its own after a few days.

use crate::data::{CharacterDef, CrewStats, DamageType, DataRegistry};
use rand::{Rng, RngExt};
use serde::{Deserialize, Serialize};

/// Stable handle to a girl in a [`Roster`].
///
/// Like [`crate::battle::UnitId`], entries are never removed — a girl who is
/// lost is marked, not deleted — so an id stays valid for the life of a
/// campaign and can be stored in a save without a fixup pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GirlId(pub u32);

impl GirlId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Whether a girl is available to crew a vehicle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GirlStatus {
    /// Fit to fight.
    Ready,
    /// Hurt, and out for `days` more campaign turns. The countdown is in
    /// overworld turns because that is the clock a campaign advances.
    Wounded { days: u32 },
    /// Bailed out and did not reach friendly lines before the fighting
    /// stopped. She is walking back, and turns up again in `days`.
    ///
    /// This is emphatically not a euphemism for dead — a crew whose tank
    /// brews up mostly gets out, and the interesting consequence is that they
    /// are unavailable for a while, not that they are gone.
    Lost { days: u32 },
    /// Killed. Only reachable with [`CasualtyRules::permadeath`] enabled.
    Dead,
}

impl GirlStatus {
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// Whether she will ever be available again. A wounded or lost girl is
    /// coming back; a dead one is not.
    pub fn is_permanent(self) -> bool {
        matches!(self, Self::Dead)
    }

    /// Campaign turns until she is fit again, if she is coming back at all.
    pub fn days_out(self) -> Option<u32> {
        match self {
            Self::Ready => Some(0),
            Self::Wounded { days } | Self::Lost { days } => Some(days),
            Self::Dead => None,
        }
    }
}

/// Whether a campaign is willing to kill its characters.
///
/// Deliberately a rule rather than a constant: Girls und Panzer is famously
/// non-lethal and this game has an academy half that invests the player in
/// specific girls, so permadeath is a decision a player (or a mode) makes,
/// not one the engine makes for them.
///
/// Off by default: the softer rule is the one that matches the genre, and a
/// player who wants the stakes can opt in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CasualtyRules {
    /// When off, [`GirlStatus::Dead`] is unreachable and what would have been
    /// a death becomes a long recovery instead.
    pub permadeath: bool,
}

/// What became of one crew member when her vehicle was destroyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrewFate {
    /// Got out and reached her own lines.
    Unharmed,
    Wounded {
        days: u32,
    },
    /// Got out, but not back — see [`GirlStatus::Lost`].
    Lost {
        days: u32,
    },
    Killed,
}

impl From<CrewFate> for GirlStatus {
    fn from(fate: CrewFate) -> Self {
        match fate {
            CrewFate::Unharmed => Self::Ready,
            CrewFate::Wounded { days } => Self::Wounded { days },
            CrewFate::Lost { days } => Self::Lost { days },
            CrewFate::Killed => Self::Dead,
        }
    }
}

/// One girl, as she is now rather than as she was defined.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Girl {
    pub id: GirlId,
    /// The [`CharacterDef`] she was stamped from. Portrait and bio are still
    /// read through this, since those do not change; stats are not, because
    /// they do.
    pub def: String,
    /// Her name. Copied from the definition so it can diverge later without
    /// touching mod data — a nickname earned in play is exactly the sort of
    /// thing the academy half of the game should be able to do.
    pub name: String,
    /// Which academy she belongs to, as a side index. Mutable on purpose:
    /// girls changing hands is a thing a 4x mode does.
    pub owner: u8,
    /// Current ability, seeded from the definition and grown by play.
    pub stats: CrewStats,
    pub xp: u32,
    pub status: GirlStatus,
    /// Battles survived. The crudest possible history, kept because it costs
    /// nothing and because "how many times have you done this" is the first
    /// question any progression or support system asks.
    pub battles: u32,
}

impl Girl {
    /// Stamp a new girl from a definition.
    pub fn from_def(id: GirlId, owner: u8, def: &CharacterDef) -> Self {
        Self {
            id,
            def: def.id.clone(),
            name: def.name.clone(),
            owner,
            stats: def.stats,
            xp: 0,
            status: GirlStatus::Ready,
            battles: 0,
        }
    }
}

/// Decide what became of one crew member whose vehicle was destroyed.
///
/// Two inputs beyond the dice, which is what makes this a model rather than a
/// coin flip:
///
/// - **What hit them.** A kinetic penetration puts a spall of hot metal
///   through the fighting compartment; high explosive is more likely to
///   disable the vehicle than the people in it; small arms that finish off a
///   vehicle have barely touched the crew at all.
/// - **How survivable the vehicle is** ([`crate::data::VehicleDef::safety`]) —
///   hatches, layout, where the ammunition lives.
///
/// The bail-out case is the common one and the interesting one: most crews get
/// out. Whether they get *back* is a separate question, which is what
/// [`GirlStatus::Lost`] records.
///
/// Takes the rng by reference so the caller owns determinism; the campaign
/// resolves these in girl-id order.
pub fn resolve_crew_fate(
    rules: CasualtyRules,
    safety: i32,
    killed_by: Option<DamageType>,
    rng: &mut impl Rng,
) -> CrewFate {
    // Chance in 100 that this girl is hurt at all, before safety is applied.
    let base_harm = match killed_by {
        Some(DamageType::Kinetic) => 55,
        Some(DamageType::Explosive) => 40,
        Some(DamageType::SmallArms) => 20,
        // Nothing recorded — burned out, abandoned, or a source the sim did
        // not attribute. Treat it as the middling case.
        None => 40,
    };
    // Each point of safety takes eight points off, so a 0-safety deathtrap is
    // meaningfully worse than a 5-safety one without ever reaching certainty.
    let harm = (base_harm - safety * 8).clamp(5, 95);

    if rng.random_range(0..100) >= harm {
        // Out clean — but possibly on the wrong side of the fighting.
        return if rng.random_range(0..100) < 25 {
            CrewFate::Lost {
                days: rng.random_range(1..=3),
            }
        } else {
            CrewFate::Unharmed
        };
    }

    // Hurt. A quarter of those are bad enough to be fatal if the campaign
    // allows it; otherwise it is a long recovery instead.
    let severe = rng.random_range(0..100) < 25;
    match (severe, rules.permadeath) {
        (true, true) => CrewFate::Killed,
        (true, false) => CrewFate::Wounded {
            days: rng.random_range(5..=10),
        },
        (false, _) => CrewFate::Wounded {
            days: rng.random_range(1..=4),
        },
    }
}

/// Every girl a side has, wounded and lost ones included.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Roster {
    girls: Vec<Girl>,
}

impl Roster {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a girl stamped from a definition, returning her handle.
    pub fn enlist(&mut self, owner: u8, def: &CharacterDef) -> GirlId {
        let id = GirlId(self.girls.len() as u32);
        self.girls.push(Girl::from_def(id, owner, def));
        id
    }

    /// Add a girl by definition id. `None` if the mod does not define her,
    /// which a caller building from map data should report rather than panic
    /// on — content can be removed by a mod at any time.
    pub fn enlist_from_registry(
        &mut self,
        registry: &DataRegistry,
        owner: u8,
        def_id: &str,
    ) -> Option<GirlId> {
        registry.character(def_id).map(|def| {
            let def = def.clone();
            self.enlist(owner, &def)
        })
    }

    /// Every girl belonging to one academy, in id order.
    pub fn of_side(&self, side: u8) -> impl Iterator<Item = &Girl> {
        self.girls.iter().filter(move |g| g.owner == side)
    }

    pub fn get(&self, id: GirlId) -> Option<&Girl> {
        self.girls.get(id.index())
    }

    pub fn get_mut(&mut self, id: GirlId) -> Option<&mut Girl> {
        self.girls.get_mut(id.index())
    }

    /// Every girl, in id order. Ordered because the simulation must not depend
    /// on iteration order anywhere.
    pub fn iter(&self) -> impl Iterator<Item = &Girl> {
        self.girls.iter()
    }

    /// Girls fit to be assigned to a vehicle.
    pub fn ready(&self) -> impl Iterator<Item = &Girl> {
        self.girls.iter().filter(|g| g.status.is_ready())
    }

    pub fn len(&self) -> usize {
        self.girls.len()
    }

    pub fn is_empty(&self) -> bool {
        self.girls.is_empty()
    }

    /// The best value any of `crew` has for some stat, which is how a
    /// vehicle's crew bonus is decided: one gunner lays the gun, one driver
    /// drives, and the sharpest pair of eyes is the one that spots.
    ///
    /// Girls who are not [`GirlStatus::Ready`] contribute nothing, so a
    /// wounded gunner costs her vehicle its gunnery bonus without needing a
    /// separate code path.
    pub fn best(&self, crew: &[GirlId], stat: impl Fn(&CrewStats) -> i32) -> i32 {
        crew.iter()
            .filter_map(|id| self.get(*id))
            .filter(|g| g.status.is_ready())
            .map(|g| stat(&g.stats))
            .max()
            .unwrap_or(0)
    }

    /// Stamp a throwaway roster for a set of placements, returning it
    /// alongside each placement's crew in the same order.
    ///
    /// This is what a scenario battle uses: it has no campaign behind it, so
    /// the girls it fields exist for the length of the fight. A campaign
    /// battle passes its own roster instead, which is the whole point of the
    /// distinction — the same girl carries her wounds and her experience from
    /// one battle to the next only if somebody owns her between them.
    ///
    /// A crew id the mods do not define is skipped rather than fatal: content
    /// can be removed by a mod, and a missing gunner should cost a bonus, not
    /// crash a battle.
    pub fn stamp_for(
        registry: &DataRegistry,
        placements: &[crate::map::UnitPlacement],
    ) -> (Self, Vec<Vec<GirlId>>) {
        let mut roster = Self::new();
        let crews = placements
            .iter()
            .map(|placement| {
                placement
                    .crew
                    .iter()
                    .filter_map(|def_id| {
                        roster.enlist_from_registry(registry, placement.side, def_id)
                    })
                    .collect()
            })
            .collect();
        (roster, crews)
    }

    /// Advance every recovery and every long walk home by one campaign turn.
    pub fn advance_day(&mut self) {
        for girl in &mut self.girls {
            girl.status = match girl.status {
                GirlStatus::Wounded { days } if days > 1 => GirlStatus::Wounded { days: days - 1 },
                GirlStatus::Lost { days } if days > 1 => GirlStatus::Lost { days: days - 1 },
                // The last day of either brings her back.
                GirlStatus::Wounded { .. } | GirlStatus::Lost { .. } => GirlStatus::Ready,
                other => other,
            };
        }
    }

    /// Record that a girl came through a battle.
    pub fn credit_battle(&mut self, id: GirlId) {
        if let Some(girl) = self.get_mut(id) {
            girl.battles += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(id: &str, gunnery: i32) -> CharacterDef {
        CharacterDef {
            id: id.into(),
            name: format!("{id} the tester"),
            portrait: None,
            bio: String::new(),
            stats: CrewStats {
                gunnery,
                ..Default::default()
            },
        }
    }

    #[test]
    fn a_girl_starts_as_her_definition_but_is_not_bound_to_it() {
        let mut roster = Roster::new();
        let id = roster.enlist(0, &def("anka", 3));
        assert_eq!(roster.get(id).unwrap().stats.gunnery, 3);

        // The whole point: the instance can move and the definition cannot.
        roster.get_mut(id).unwrap().stats.gunnery = 4;
        roster.get_mut(id).unwrap().xp += 100;
        assert_eq!(roster.get(id).unwrap().stats.gunnery, 4);
        assert_eq!(def("anka", 3).stats.gunnery, 3);
    }

    #[test]
    fn two_girls_from_one_definition_are_separate_people() {
        let mut roster = Roster::new();
        let template = def("recruit", 2);
        let a = roster.enlist(0, &template);
        let b = roster.enlist(0, &template);
        assert_ne!(a, b);
        roster.get_mut(a).unwrap().stats.gunnery = 5;
        assert_eq!(roster.get(b).unwrap().stats.gunnery, 2);
    }

    #[test]
    fn a_wounded_crew_member_contributes_nothing_until_she_recovers() {
        let mut roster = Roster::new();
        let sharp = roster.enlist(0, &def("sharp", 5));
        let dull = roster.enlist(0, &def("dull", 1));
        let crew = [sharp, dull];
        assert_eq!(roster.best(&crew, |s| s.gunnery), 5);

        roster.get_mut(sharp).unwrap().status = GirlStatus::Wounded { days: 2 };
        assert_eq!(
            roster.best(&crew, |s| s.gunnery),
            1,
            "the vehicle should fall back to whoever is still fit"
        );

        roster.advance_day();
        assert_eq!(
            roster.get(sharp).unwrap().status,
            GirlStatus::Wounded { days: 1 }
        );
        roster.advance_day();
        assert!(roster.get(sharp).unwrap().status.is_ready());
        assert_eq!(roster.best(&crew, |s| s.gunnery), 5);
    }

    #[test]
    fn an_empty_or_unknown_crew_is_worth_nothing_rather_than_panicking() {
        let roster = Roster::new();
        assert_eq!(roster.best(&[], |s| s.gunnery), 0);
        assert_eq!(roster.best(&[GirlId(99)], |s| s.gunnery), 0);
    }
}
