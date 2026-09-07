//! Cadets as instances rather than definitions.
//!
//! A [`crate::data::CharacterDef`] is mod data: a name, a portrait, a bio and
//! the stats someone *starts* with. It is immutable and shared, which is right
//! for content and wrong for a person. Until this module existed a unit's crew
//! was `Vec<String>` — keys into that static table — so there was nowhere to
//! record that Anka has been in nine battles, is carrying a wound, and has
//! learned to shoot better than the cadet she was defined as.
//!
//! [`Cadet`] is that missing object and [`Roster`] owns them. The distinction
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
//! Every cadet in the world lives in a single [`Roster`] and carries the
//! academy she belongs to in [`Cadet::owner`]. The alternative — a roster per
//! side — would make [`CadetId`] ambiguous without a side alongside it, which
//! would push side-indexing down into the battle layer for no gain.
//!
//! This shape is also the one a 4x mode wants. A campaign is a two-academy
//! case of the same thing, so cadets changing hands — recruited, poached,
//! captured, transferred between academies — is a field change here rather
//! than a data migration later.
//!
//! # Death is a rule, not a fact of the model
//!
//! [`CadetStatus::Dead`] is only reachable when [`CasualtyRules::permadeath`]
//! is on, which is a per-campaign option rather than something the engine
//! decides. With it off, the worst a crew suffers is a long recovery.
//!
//! Note that [`CadetStatus::Lost`] is *not* death and never was: it means she
//! bailed out and could not reach friendly lines before the fighting stopped,
//! and is making her own way back. It resolves on its own after a few days.

use crate::data::{Casualties, CharacterDef, DamageType, DataRegistry};
use rand::{Rng, RngExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Stable handle to a cadet in a [`Roster`].
///
/// Like [`crate::battle::UnitId`], entries are never removed — a cadet who is
/// lost is marked, not deleted — so an id stays valid for the life of a
/// campaign and can be stored in a save without a fixup pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CadetId(pub u32);

impl CadetId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Whether a cadet is available to crew a vehicle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CadetStatus {
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

impl CadetStatus {
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// Whether she will ever be available again. A wounded or lost cadet is
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
/// specific cadets, so permadeath is a decision a player (or a mode) makes,
/// not one the engine makes for them.
///
/// Off by default: the softer rule is the one that matches the genre, and a
/// player who wants the stakes can opt in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CasualtyRules {
    /// When off, [`CadetStatus::Dead`] is unreachable and what would have been
    /// a death becomes a long recovery instead.
    pub permadeath: bool,
}

/// What became of one crew member when her vehicle was destroyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrewFate {
    /// Got out and reached her own lines.
    Unharmed,
    Wounded {
        days: u32,
    },
    /// Got out, but not back — see [`CadetStatus::Lost`].
    Lost {
        days: u32,
    },
    Killed,
}

impl From<CrewFate> for CadetStatus {
    fn from(fate: CrewFate) -> Self {
        match fate {
            CrewFate::Unharmed => Self::Ready,
            CrewFate::Wounded { days } => Self::Wounded { days },
            CrewFate::Lost { days } => Self::Lost { days },
            CrewFate::Killed => Self::Dead,
        }
    }
}

/// One cadet, as she is now rather than as she was defined.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cadet {
    pub id: CadetId,
    /// The [`CharacterDef`] she was stamped from. Portrait and bio are still
    /// read through this, since those do not change; stats are not, because
    /// they do.
    pub def: String,
    /// Her name. Copied from the definition so it can diverge later without
    /// touching mod data — a nickname earned in play is exactly the sort of
    /// thing the academy half of the game should be able to do.
    pub name: String,
    /// Which academy she belongs to, as a side index. Mutable on purpose:
    /// cadets changing hands is a thing a 4x mode does.
    pub owner: u8,
    /// Temperament, positional in the registry's core order. Slow to change.
    pub cores: Vec<i32>,
    /// What she has been taught, by skill id. A skill missing here is
    /// untrained, which means it falls back to her cores at a penalty rather
    /// than to nothing.
    pub skills: HashMap<String, i32>,
    pub xp: u32,
    pub status: CadetStatus,
    /// Battles survived. The crudest possible history, kept because it costs
    /// nothing and because "how many times have you done this" is the first
    /// question any progression or support system asks.
    pub battles: u32,
    /// What is true about her that is not a number. Some she arrived with;
    /// others she will pick up from what happens to her.
    #[serde(default)]
    pub traits: Vec<String>,
}

impl Cadet {
    /// Stamp a new cadet from a definition.
    ///
    /// Takes the registry because cores are positional and only it knows the
    /// order — which is the price of letting a mod decide what the cores are.
    pub fn from_def(id: CadetId, owner: u8, def: &CharacterDef, registry: &DataRegistry) -> Self {
        Self {
            id,
            def: def.id.clone(),
            name: def.name.clone(),
            owner,
            cores: registry.core_index.values_from(&def.cores),
            skills: def.skills.clone(),
            traits: def.traits.clone(),
            xp: 0,
            status: CadetStatus::Ready,
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
/// [`CadetStatus::Lost`] records.
///
/// Takes the rng by reference so the caller owns determinism; the campaign
/// resolves these in cadet-id order.
pub fn resolve_crew_fate(
    rules: CasualtyRules,
    table: &Casualties,
    safety: i32,
    killed_by: Option<DamageType>,
    rng: &mut impl Rng,
) -> CrewFate {
    // Chance in 100 that this cadet is hurt at all, before safety is applied.
    let base_harm = match killed_by {
        Some(DamageType::Kinetic) => table.harm_kinetic,
        Some(DamageType::Explosive) => table.harm_explosive,
        Some(DamageType::SmallArms) => table.harm_small_arms,
        None => table.harm_unattributed,
    };
    let harm = (base_harm - safety * table.harm_per_safety)
        .clamp(table.harm_floor.min(table.harm_ceiling), table.harm_ceiling);

    if rng.random_range(0..100) >= harm {
        // Out clean — but possibly on the wrong side of the fighting.
        return if rng.random_range(0..100) < table.adrift_percent {
            CrewFate::Lost {
                days: rng.random_range(Casualties::days(table.adrift_days)),
            }
        } else {
            CrewFate::Unharmed
        };
    }

    // Hurt. Some of those are bad enough to be fatal if the campaign allows
    // it; otherwise it is a long recovery instead.
    let severe = rng.random_range(0..100) < table.severe_percent;
    match (severe, rules.permadeath) {
        (true, true) => CrewFate::Killed,
        (true, false) => CrewFate::Wounded {
            days: rng.random_range(Casualties::days(table.severe_days)),
        },
        (false, _) => CrewFate::Wounded {
            days: rng.random_range(Casualties::days(table.light_days)),
        },
    }
}

/// Decide what one cadet takes home from a vehicle that came home with her.
///
/// The other half of [`resolve_crew_fate`], and the half that did not exist:
/// until this function the only way a wound survived a battle was for the
/// vehicle to be destroyed, so a gunner knocked out at her station in a tank
/// that drove home was fit again by the time the campaign screen drew — the
/// whole in-battle crew model evaporated at the door. Whether a cadet is hurt
/// is the battle's question and it has already answered it in
/// [`crate::battle::CrewCondition`]; all that is left is how long it keeps
/// her out.
///
/// Deliberately much gentler than the destroyed case, and never fatal on its
/// own without permadeath: her tank came home, so somebody got her to a
/// doctor within the hour. There is no [`CrewFate::Lost`] here at all — she
/// did not have to walk back.
pub fn resolve_station_fate(
    rules: CasualtyRules,
    table: &Casualties,
    found: crate::battle::CrewCondition,
    rng: &mut impl Rng,
) -> CrewFate {
    use crate::battle::CrewCondition;
    match found {
        // Untouched, or never in the vehicle: nothing to record. Callers are
        // expected not to report these at all, and answering rather than
        // panicking keeps the report a filter rather than a contract.
        CrewCondition::Fine | CrewCondition::Absent => CrewFate::Unharmed,
        CrewCondition::Wounded => CrewFate::Wounded {
            days: rng.random_range(Casualties::days(table.grazed_days)),
        },
        CrewCondition::Out => {
            let fatal = rules.permadeath && rng.random_range(0..100) < table.severe_percent;
            if fatal {
                CrewFate::Killed
            } else {
                CrewFate::Wounded {
                    days: rng.random_range(Casualties::days(table.carried_days)),
                }
            }
        }
    }
}

/// Every cadet a side has, wounded and lost ones included.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Roster {
    cadets: Vec<Cadet>,
}

impl Roster {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a cadet stamped from a definition, returning her handle.
    pub fn enlist(&mut self, owner: u8, def: &CharacterDef, registry: &DataRegistry) -> CadetId {
        let id = CadetId(self.cadets.len() as u32);
        self.cadets.push(Cadet::from_def(id, owner, def, registry));
        id
    }

    /// Add a cadet by definition id. `None` if the mod does not define her,
    /// which a caller building from map data should report rather than panic
    /// on — content can be removed by a mod at any time.
    pub fn enlist_from_registry(
        &mut self,
        registry: &DataRegistry,
        owner: u8,
        def_id: &str,
    ) -> Option<CadetId> {
        registry.character(def_id).map(|def| {
            let def = def.clone();
            self.enlist(owner, &def, registry)
        })
    }

    /// Every cadet belonging to one academy, in id order.
    pub fn of_side(&self, side: u8) -> impl Iterator<Item = &Cadet> {
        self.cadets.iter().filter(move |g| g.owner == side)
    }

    pub fn get(&self, id: CadetId) -> Option<&Cadet> {
        self.cadets.get(id.index())
    }

    pub fn get_mut(&mut self, id: CadetId) -> Option<&mut Cadet> {
        self.cadets.get_mut(id.index())
    }

    /// Every cadet, in id order. Ordered because the simulation must not depend
    /// on iteration order anywhere.
    pub fn iter(&self) -> impl Iterator<Item = &Cadet> {
        self.cadets.iter()
    }

    /// Cadets fit to be assigned to a vehicle.
    pub fn ready(&self) -> impl Iterator<Item = &Cadet> {
        self.cadets.iter().filter(|g| g.status.is_ready())
    }

    pub fn len(&self) -> usize {
        self.cadets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cadets.is_empty()
    }

    /// How well one cadet performs a skill, trained or not.
    ///
    /// This is the only way to ask what someone can do. There is no stored
    /// ability to read: a trained skill is her level, and an untrained one
    /// falls back to the weighted mean of the skill's controlling cores minus
    /// its penalty.
    pub fn skill_level(
        &self,
        registry: &DataRegistry,
        cadet: CadetId,
        skill: &str,
        ctx: &crate::data::CheckContext,
    ) -> Option<i32> {
        let cadet = self.get(cadet)?;
        let def = registry.skill(skill)?;
        let base = def.level_for(
            &registry.core_index,
            &cadet.cores,
            cadet.skills.get(skill).copied(),
        );
        // Traits arrive here rather than being baked into a stored number,
        // which is what lets them be conditional on where she is and who she
        // is with.
        let from_traits: i32 = cadet
            .traits
            .iter()
            .filter_map(|id| registry.trait_def(id))
            .map(|t| t.modifier(skill, ctx))
            .sum();
        Some(base + from_traits)
    }

    /// How well this crew performs a skill, given who is sitting where.
    ///
    /// The crew is positional: cadet *i* fills the vehicle's *i*th crew slot,
    /// so the gunner's gunnery is what lays the gun rather than the best
    /// gunnery aboard. That is the difference between a crew and a bag of
    /// numbers, and it is what makes moving a cadet between tanks a decision.
    ///
    /// Three cases, in order:
    ///
    /// 1. **Somebody whose job this is.** If more than one seat answers for
    ///    the skill — a heavy tank has a commander *and* a radio operator —
    ///    the better of them is used.
    /// 2. **Somebody covering.** With ten cadets and four seats a tank, an
    ///    empty seat is the normal case, so the best remaining crew member
    ///    takes it at [`crate::data::Balance::substitution_penalty`]. A
    ///    commander can lay a gun; she is simply not the gunner.
    /// 3. **Nobody fit.** An untrained average, because the vehicle has not
    ///    stopped existing just because its crew is down.
    ///
    /// A skill no seat claims — discipline, athletics — is everybody's
    /// business, and takes the best aboard with no penalty.
    pub fn crew_skill(
        &self,
        registry: &DataRegistry,
        vehicle: Option<&crate::data::VehicleDef>,
        crew: &[CadetId],
        skill: &str,
        terrain: Option<&str>,
    ) -> i32 {
        let ctx = crate::data::CheckContext {
            terrain,
            vehicle_class: vehicle.map(|v| v.class.as_str()),
            crew_size: crew.iter().filter(|id| self.get(**id).is_some()).count(),
        };
        let ready = |id: &CadetId| self.get(*id).is_some_and(|g| g.status.is_ready());
        let level = |id: &CadetId| self.skill_level(registry, *id, skill, &ctx);

        // Which seats answer for this skill, as indices into the crew.
        let responsible: Vec<usize> = vehicle
            .map(|v| {
                v.crew_slots
                    .iter()
                    .enumerate()
                    .filter(|(_, role)| {
                        registry
                            .role(role)
                            .is_some_and(|r| r.skills.iter().any(|s| s == skill))
                    })
                    .map(|(i, _)| i)
                    .collect()
            })
            .unwrap_or_default();

        // Nobody's job in particular: everyone's business, best aboard.
        if responsible.is_empty() {
            return crew
                .iter()
                .filter(|id| ready(id))
                .filter_map(level)
                .max()
                .unwrap_or_else(|| self.unspecified(registry, skill));
        }

        let specialist = responsible
            .iter()
            .filter_map(|i| crew.get(*i))
            .filter(|id| ready(id))
            .filter_map(level)
            .max();
        if let Some(level) = specialist {
            return level;
        }

        // Nobody in the seat: whoever else is aboard has a go at it.
        crew.iter()
            .enumerate()
            .filter(|(i, _)| !responsible.contains(i))
            .map(|(_, id)| id)
            .filter(|id| ready(id))
            .filter_map(level)
            .max()
            .map(|best| best - registry.balance.substitution_penalty)
            .unwrap_or_else(|| self.unspecified(registry, skill))
    }

    /// What a vehicle manages when nobody named is aboard.
    ///
    /// Ordinary, not untrained. A placement that names no crew — a test
    /// fixture, a map that does not care, a generated skirmish — should get a
    /// vehicle that performs exactly as its data says, because that is what
    /// the data is *for*. Treating unspecified as untrained made every such
    /// vehicle quietly slower and blinder than its own definition, which is a
    /// nasty thing to debug from the outside.
    ///
    /// Named cadets then modify from there, in both directions.
    fn unspecified(&self, _registry: &DataRegistry, _skill: &str) -> i32 {
        crate::data::AVERAGE
    }

    /// Stamp a throwaway roster for a set of placements, returning it
    /// alongside each placement's crew in the same order.
    ///
    /// This is what a scenario battle uses: it has no campaign behind it, so
    /// the cadets it fields exist for the length of the fight. A campaign
    /// battle passes its own roster instead, which is the whole point of the
    /// distinction — the same cadet carries her wounds and her experience from
    /// one battle to the next only if somebody owns her between them.
    ///
    /// A crew id the mods do not define is skipped rather than fatal: content
    /// can be removed by a mod, and a missing gunner should cost a bonus, not
    /// crash a battle.
    pub fn stamp_for(
        registry: &DataRegistry,
        placements: &[crate::map::UnitPlacement],
    ) -> (Self, Vec<Vec<CadetId>>) {
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
        for cadet in &mut self.cadets {
            cadet.status = match cadet.status {
                CadetStatus::Wounded { days } if days > 1 => {
                    CadetStatus::Wounded { days: days - 1 }
                }
                CadetStatus::Lost { days } if days > 1 => CadetStatus::Lost { days: days - 1 },
                // The last day of either brings her back.
                CadetStatus::Wounded { .. } | CadetStatus::Lost { .. } => CadetStatus::Ready,
                other => other,
            };
        }
    }

    /// Record that a cadet came through a battle.
    pub fn credit_battle(&mut self, id: CadetId) {
        if let Some(cadet) = self.get_mut(id) {
            cadet.battles += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::{AVERAGE, CoreDef, CoreIndex, SkillDef};

    /// A two-core, one-skill world, so the tests exercise the model rather
    /// than the base mod's content.
    fn registry() -> DataRegistry {
        let cores = vec![
            CoreDef {
                id: "hands".into(),
                name: "Hands".into(),
                description: String::new(),
            },
            CoreDef {
                id: "nerve".into(),
                name: "Nerve".into(),
                description: String::new(),
            },
        ];
        let mut reg = DataRegistry {
            core_index: CoreIndex::build(&cores),
            cores,
            ..Default::default()
        };
        reg.skills.insert(
            "gunnery".into(),
            SkillDef {
                id: "gunnery".into(),
                name: "Gunnery".into(),
                cores: HashMap::from([("hands".into(), 1)]),
                untrained_penalty: 4,
            },
        );
        reg.roles.insert(
            "gunner".into(),
            crate::data::RoleDef {
                id: "gunner".into(),
                name: "Gunner".into(),
                skills: vec!["gunnery".into()],
            },
        );
        reg.roles.insert(
            "commander".into(),
            crate::data::RoleDef {
                id: "commander".into(),
                name: "Commander".into(),
                skills: vec!["command".into()],
            },
        );
        reg
    }

    /// Commander in seat 0, gunner in seat 1.
    fn tank() -> crate::data::VehicleDef {
        serde_json::from_value(serde_json::json!({
            "id": "test_tank", "name": "Test Tank", "max_hp": 10,
            "movement": { "class": "tracked", "points": 5 },
            "armor": { "front": 5, "side": 3, "rear": 2 },
            "vision_range": 10, "weapons": [],
            "crew_slots": ["commander", "gunner"]
        }))
        .expect("test vehicle")
    }

    fn def(id: &str, hands: i32, gunnery: Option<i32>) -> CharacterDef {
        CharacterDef {
            id: id.into(),
            name: format!("{id} the tester"),
            portrait: None,
            bio: String::new(),
            cores: HashMap::from([("hands".into(), hands)]),
            skills: gunnery
                .map(|g| HashMap::from([("gunnery".into(), g)]))
                .unwrap_or_default(),
            traits: Vec::new(),
        }
    }

    #[test]
    fn a_girl_starts_as_her_definition_but_is_not_bound_to_it() {
        let reg = registry();
        let mut roster = Roster::new();
        let id = roster.enlist(0, &def("anka", 12, Some(13)), &reg);
        assert_eq!(
            roster.skill_level(&reg, id, "gunnery", &Default::default()),
            Some(13)
        );

        // The whole point: the instance moves and the definition does not.
        roster
            .get_mut(id)
            .unwrap()
            .skills
            .insert("gunnery".into(), 14);
        roster.get_mut(id).unwrap().xp += 100;
        assert_eq!(
            roster.skill_level(&reg, id, "gunnery", &Default::default()),
            Some(14)
        );
        assert_eq!(def("anka", 12, Some(13)).skills["gunnery"], 13);
    }

    #[test]
    fn an_untrained_girl_falls_back_to_her_cores_rather_than_to_nothing() {
        let reg = registry();
        let mut roster = Roster::new();
        let gifted = roster.enlist(0, &def("gifted", 16, None), &reg);
        let ordinary = roster.enlist(0, &def("ordinary", 10, None), &reg);

        // Untrained is core minus the penalty, so temperament is nearly all
        // there is to go on.
        assert_eq!(
            roster.skill_level(&reg, gifted, "gunnery", &Default::default()),
            Some(12)
        );
        assert_eq!(
            roster.skill_level(&reg, ordinary, "gunnery", &Default::default()),
            Some(6)
        );

        // Train the ordinary one and she overtakes the natural outright, which
        // is the relationship between talent and experience the model wants.
        roster
            .get_mut(ordinary)
            .unwrap()
            .skills
            .insert("gunnery".into(), 13);
        assert!(
            roster.skill_level(&reg, ordinary, "gunnery", &Default::default())
                > roster.skill_level(&reg, gifted, "gunnery", &Default::default())
        );
    }

    #[test]
    fn two_girls_from_one_definition_are_separate_people() {
        let reg = registry();
        let mut roster = Roster::new();
        let template = def("recruit", 10, Some(9));
        let a = roster.enlist(0, &template, &reg);
        let b = roster.enlist(0, &template, &reg);
        assert_ne!(a, b);
        roster
            .get_mut(a)
            .unwrap()
            .skills
            .insert("gunnery".into(), 14);
        assert_eq!(
            roster.skill_level(&reg, b, "gunnery", &Default::default()),
            Some(9)
        );
    }

    #[test]
    fn an_unknown_skill_is_none_rather_than_a_panic() {
        let reg = registry();
        let mut roster = Roster::new();
        let id = roster.enlist(0, &def("anka", 10, None), &reg);
        assert_eq!(
            roster.skill_level(&reg, id, "telepathy", &Default::default()),
            None
        );
    }

    #[test]
    fn the_gunner_lays_the_gun_not_the_best_shot_aboard() {
        // The point of roles. A brilliant commander does not make her tank
        // shoot well if the cadet in the gunner's seat cannot.
        let reg = registry();
        let tank = tank();
        let mut roster = Roster::new();
        let ace = roster.enlist(0, &def("ace", 10, Some(15)), &reg);
        let novice = roster.enlist(0, &def("novice", 10, Some(8)), &reg);

        // Ace commanding, novice on the gun.
        assert_eq!(
            roster.crew_skill(&reg, Some(&tank), &[ace, novice], "gunnery", None),
            8
        );
        // The same two cadets, seats swapped, shoot far better — which is what
        // makes moving a cadet between jobs a decision worth making.
        assert_eq!(
            roster.crew_skill(&reg, Some(&tank), &[novice, ace], "gunnery", None),
            15
        );
    }

    #[test]
    fn somebody_covers_an_empty_seat_at_a_penalty() {
        // Short-handed crews are the normal case: ten cadets, four seats a tank.
        let reg = registry();
        let tank = tank();
        let mut roster = Roster::new();
        let alone = roster.enlist(0, &def("alone", 10, Some(13)), &reg);
        assert_eq!(
            roster.crew_skill(&reg, Some(&tank), &[alone], "gunnery", None),
            13 - reg.balance.substitution_penalty,
            "commanding with nobody on the gun, she reaches over and is worse at it"
        );
    }

    #[test]
    fn a_wounded_specialist_is_covered_rather_than_replaced() {
        let reg = registry();
        let tank = tank();
        let mut roster = Roster::new();
        let commander = roster.enlist(0, &def("commander", 10, Some(11)), &reg);
        let gunner = roster.enlist(0, &def("gunner", 10, Some(15)), &reg);
        let crew = [commander, gunner];
        assert_eq!(
            roster.crew_skill(&reg, Some(&tank), &crew, "gunnery", None),
            15
        );

        roster.get_mut(gunner).unwrap().status = CadetStatus::Wounded { days: 2 };
        assert_eq!(
            roster.crew_skill(&reg, Some(&tank), &crew, "gunnery", None),
            11 - reg.balance.substitution_penalty,
            "the commander takes the gun, and is worse at it"
        );

        roster.advance_day();
        roster.advance_day();
        assert!(roster.get(gunner).unwrap().status.is_ready());
        assert_eq!(
            roster.crew_skill(&reg, Some(&tank), &crew, "gunnery", None),
            15
        );
    }

    #[test]
    fn a_skill_no_seat_claims_is_everybodys_business() {
        // Discipline and athletics belong to no job, so they take the best
        // aboard with no substitution penalty.
        let reg = registry();
        let tank = tank();
        let mut roster = Roster::new();
        let a = roster.enlist(0, &def("a", 10, None), &reg);
        let b = roster.enlist(0, &def("b", 16, None), &reg);
        assert_eq!(
            roster.crew_skill(&reg, Some(&tank), &[a, b], "unclaimed", None),
            AVERAGE,
            "an unknown skill falls back to an ordinary showing rather than panicking"
        );
    }

    #[test]
    fn a_vehicle_with_nobody_named_performs_exactly_as_its_data_says() {
        // Ordinary, not untrained. A placement that names no crew should get
        // the vehicle its definition describes; anything else means the paper
        // stats quietly lie.
        let reg = registry();
        let roster = Roster::new();
        assert_eq!(roster.crew_skill(&reg, None, &[], "gunnery", None), AVERAGE);
        assert_eq!(
            roster.crew_skill(&reg, None, &[CadetId(99)], "gunnery", None),
            AVERAGE
        );
    }
}
