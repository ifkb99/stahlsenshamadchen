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
//! # What this module deliberately does not decide
//!
//! [`GirlStatus::Lost`] exists as a *representable state*, not a policy. What
//! actually happens to a crew when their vehicle is destroyed — bail out,
//! wounded pool, permadeath — is an open identity question for this game (see
//! TODO.md), and it is deliberately not answered here. This module only makes
//! sure that whatever is decided has somewhere to be recorded.

use crate::data::{CharacterDef, CrewStats, DataRegistry};
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
    /// Out of action for `days` more campaign turns. Zero means she is back
    /// next turn; the countdown is in overworld turns because that is the
    /// clock a campaign advances.
    Wounded { days: u32 },
    /// Gone for good. Present so the state is representable; nothing in the
    /// engine puts a girl here yet, because what happens to a crew when their
    /// tank dies is an open design question.
    Lost,
}

impl GirlStatus {
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
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
    pub fn from_def(id: GirlId, def: &CharacterDef) -> Self {
        Self {
            id,
            def: def.id.clone(),
            name: def.name.clone(),
            stats: def.stats,
            xp: 0,
            status: GirlStatus::Ready,
            battles: 0,
        }
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
    pub fn enlist(&mut self, def: &CharacterDef) -> GirlId {
        let id = GirlId(self.girls.len() as u32);
        self.girls.push(Girl::from_def(id, def));
        id
    }

    /// Add a girl by definition id. `None` if the mod does not define her,
    /// which a caller building from map data should report rather than panic
    /// on — content can be removed by a mod at any time.
    pub fn enlist_from_registry(
        &mut self,
        registry: &DataRegistry,
        def_id: &str,
    ) -> Option<GirlId> {
        registry.character(def_id).map(|def| {
            let def = def.clone();
            self.enlist(&def)
        })
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
                    .filter_map(|def_id| roster.enlist_from_registry(registry, def_id))
                    .collect()
            })
            .collect();
        (roster, crews)
    }

    /// Advance every wound by one campaign turn.
    pub fn advance_day(&mut self) {
        for girl in &mut self.girls {
            if let GirlStatus::Wounded { days } = girl.status {
                girl.status = match days.checked_sub(1) {
                    Some(0) | None => GirlStatus::Ready,
                    Some(remaining) => GirlStatus::Wounded { days: remaining },
                };
            }
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
        let id = roster.enlist(&def("anka", 3));
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
        let a = roster.enlist(&template);
        let b = roster.enlist(&template);
        assert_ne!(a, b);
        roster.get_mut(a).unwrap().stats.gunnery = 5;
        assert_eq!(roster.get(b).unwrap().stats.gunnery, 2);
    }

    #[test]
    fn a_wounded_crew_member_contributes_nothing_until_she_recovers() {
        let mut roster = Roster::new();
        let sharp = roster.enlist(&def("sharp", 5));
        let dull = roster.enlist(&def("dull", 1));
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
