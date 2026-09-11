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
//! decides. With it off, the worst a crew suffers is a long recovery. The
//! base mod declares it on: the shipped campaign means it.
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

    /// Which of two answers about the same cadet the campaign keeps: the one
    /// that keeps her out of a seat longer, and a grave over everything.
    ///
    /// A battle's fate is applied by writing it over what she was, which was
    /// harmless while everybody who could be a casualty of a battle was fit
    /// when it started. A cadet who is called up rides out hurt, so she can
    /// come back from one carrying a *shorter* recovery than she went in
    /// with — the tank she is riding in is destroyed, the wreck roll says
    /// `Unharmed`, and she is signed fit on the strength of having been shot
    /// at. She cannot get better by being shot at.
    ///
    /// The kinder reading — adding the two together — is wrong for the
    /// opposite reason: one wound would be charged twice, once by the battle
    /// that gave it to her and once by the next one she is dragged through.
    /// The worse of the two is the honest answer: her recovery is at least
    /// as long as it already was.
    ///
    /// A tie between a wound and a walk home keeps `other`, the fresher fact:
    /// she is on a road somewhere rather than in the infirmary, and that is
    /// the more recent truth about where she is.
    pub fn worse_of(self, other: Self) -> Self {
        let rank = |s: Self| (s.is_permanent(), s.days_out().unwrap_or(0));
        if rank(self) > rank(other) {
            self
        } else {
            other
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
/// Off by default *here*, and on in the shipped campaign, which is not a
/// contradiction: the engine's default is the benign one because every harsh
/// system in this game is an additive rule whose absence is the gentle
/// version, and the stakes are declared by content
/// ([`crate::data::Casualties::permadeath`], `true` in the base mod since
/// 2026-09-10). [`crate::overworld::OverworldState::from_map`] copies the
/// declaration onto this, and this is what is saved — so a campaign owns its
/// own answer and a settings screen can change it for one run without editing
/// anybody's mod, while a mod that wants a gentle game says so once and needs
/// no line of Rust.
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
    /// Whether her academy has put her on the roll for the battle about to be
    /// fought in spite of [`Self::status`].
    ///
    /// The muster's answer, and the only thing that ever writes it is a
    /// muster: the campaign hands each battle a *copy* of its roster, and the
    /// copy is where the roll is made. So this is false on the campaign's own
    /// cadets between battles, false again the next time somebody is asked,
    /// and a decision the player makes once does not quietly stand for the
    /// rest of the war.
    ///
    /// It lives on her rather than in an argument to
    /// [`crate::battle::BattleState::from_placements`] because the question
    /// it answers — is she climbing in — is asked in exactly one place, which
    /// reads the roster and nothing else. Threading a list of names through
    /// five signatures to reach one `if` would have been the same fact in
    /// five more places.
    ///
    /// **False is the game as it was**, which is the rule this whole
    /// subsystem is built to satisfy: an academy that never calls anybody up
    /// leaves every wounded cadet exactly where she was.
    #[serde(default)]
    pub called_up: bool,
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
            called_up: false,
        }
    }

    /// Whether she climbs into a vehicle at all: fit, or called up anyway.
    pub fn deploys(&self) -> bool {
        self.status.is_ready() || self.called_up
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
    aid: i32,
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
    // it; otherwise it is a long recovery instead — and whether this one is
    // depends on whether anybody beside her aboard knew what to do about it.
    let severe =
        rng.random_range(0..100) < aid_reduced(table.severe_percent, table.severe_per_aid, aid);
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
/// did not have to walk back. How much gentler is
/// [`Casualties::carried_fatal_percent`], which is a separate number from the
/// wreck case's [`Casualties::severe_percent`] precisely because "carried
/// home" and "dragged out of a fire" are not one situation.
/// A fatal chance with her crewmates' first aid taken off it.
///
/// One function because both chances are reduced the same way and by
/// different numbers, and because the clamp matters: no amount of skill makes
/// a wound safe, it makes it less likely to be the other kind. A crew with
/// nobody left to help passes [`crate::data::AVERAGE`] and changes nothing,
/// which is the rule's absence.
fn aid_reduced(percent: i32, per_aid: i32, aid: i32) -> i32 {
    // Only ever downward, the same way `athletics` only ever adds a level of
    // climb. Most of the roster is untrained in `first_aid` and an untrained
    // skill sits five points under its core base, so a two-sided rule would
    // make the shipped campaign *bury more cadets* the moment it was switched
    // on — measured: `severe_per_aid` at 5 put `buried` up 0.30 a battle
    // rather than down. That is `untrained_penalty` reaching a casualty
    // table through a side door, which is a different knob and not this one.
    (percent - per_aid * (aid - crate::data::AVERAGE).max(0)).max(0)
}

pub fn resolve_station_fate(
    rules: CasualtyRules,
    table: &Casualties,
    found: crate::battle::CrewCondition,
    aid: i32,
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
            // `carried_fatal_percent`, not `severe_percent`: what a wreck's
            // wound costs is a different question from what a homecoming
            // does, and until they were two numbers a fifth of everybody the
            // shipped campaign buried was a cadet whose tank drove back.
            let fatal = rules.permadeath
                && rng.random_range(0..100)
                    < aid_reduced(
                        table.carried_fatal_percent,
                        table.carried_fatal_per_aid,
                        aid,
                    );
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

    /// The roll for one battle: a copy of this roster with
    /// [`Cadet::called_up`] set on everybody named and on nobody else.
    ///
    /// The campaign hands each battle a copy of its roster anyway, and this
    /// is that copy. Going through here rather than reaching for `get_mut` is
    /// what makes the field's promise true by construction: the muster's
    /// answer never reaches the campaign's own cadets, so a decision made for
    /// one fight is not quietly standing at the next one.
    pub fn mustered(&self, called_up: &[CadetId]) -> Self {
        let mut roster = self.clone();
        for cadet in &mut roster.cadets {
            cadet.called_up = called_up.contains(&cadet.id);
        }
        roster
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
    ///
    /// `conditions` is the unit's [`crate::battle::Unit::crew_state`], seat
    /// for seat, and it is what decides who is working at all. A crew nobody
    /// has hurt and nobody stayed behind from carries an empty one, which is
    /// every battle written before either could happen; there the roster's
    /// own [`CadetStatus`] answers, exactly as it always did.
    pub fn crew_skill(
        &self,
        registry: &DataRegistry,
        vehicle: Option<&crate::data::VehicleDef>,
        crew: &[CadetId],
        conditions: &[crate::battle::CrewCondition],
        skill: &str,
        terrain: Option<&str>,
    ) -> i32 {
        let ctx = crate::data::CheckContext {
            terrain,
            vehicle_class: vehicle.map(|v| v.class.as_str()),
            crew_size: crew.iter().filter(|id| self.get(**id).is_some()).count(),
        };
        // What one seat is worth to this check: nothing at all if nobody is
        // working it, otherwise her level with what her condition costs her
        // already taken off.
        //
        // A wounded cadet is charged `substitution_penalty`, which is
        // deliberately the same number a stand-in pays rather than one of its
        // own: being hurt at your station is like doing somebody else's job,
        // and that sentence is the model. If the two ever need to differ they
        // are two fields, the way the two fatal chances became two.
        let working = |seat: usize, id: &CadetId| -> Option<i32> {
            use crate::battle::CrewCondition;
            let penalty = match conditions.get(seat) {
                Some(CrewCondition::Fine) => 0,
                Some(CrewCondition::Wounded) => registry.balance.substitution_penalty,
                Some(CrewCondition::Out | CrewCondition::Absent) => return None,
                // No condition list: the battle has not had to say anything
                // about this crew, so whether she is aboard is the roster's
                // question and the answer is the one it always gave.
                None if !self.get(*id).is_some_and(|c| c.status.is_ready()) => return None,
                None => 0,
            };
            Some(self.skill_level(registry, *id, skill, &ctx)? - penalty)
        };

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
                .enumerate()
                .filter_map(|(seat, id)| working(seat, id))
                .max()
                .unwrap_or_else(|| self.unspecified(registry, skill));
        }

        let specialist = responsible
            .iter()
            .filter_map(|seat| crew.get(*seat).map(|id| (*seat, id)))
            .filter_map(|(seat, id)| working(seat, id))
            .max();
        if let Some(level) = specialist {
            return level;
        }

        // Nobody in the seat: whoever else is aboard has a go at it.
        crew.iter()
            .enumerate()
            .filter(|(seat, _)| !responsible.contains(seat))
            .filter_map(|(seat, id)| working(seat, id))
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
            roster.crew_skill(&reg, Some(&tank), &[ace, novice], &[], "gunnery", None),
            8
        );
        // The same two cadets, seats swapped, shoot far better — which is what
        // makes moving a cadet between jobs a decision worth making.
        assert_eq!(
            roster.crew_skill(&reg, Some(&tank), &[novice, ace], &[], "gunnery", None),
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
            roster.crew_skill(&reg, Some(&tank), &[alone], &[], "gunnery", None),
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
            roster.crew_skill(&reg, Some(&tank), &crew, &[], "gunnery", None),
            15
        );

        roster.get_mut(gunner).unwrap().status = CadetStatus::Wounded { days: 2 };
        assert_eq!(
            roster.crew_skill(&reg, Some(&tank), &crew, &[], "gunnery", None),
            11 - reg.balance.substitution_penalty,
            "the commander takes the gun, and is worse at it"
        );

        roster.advance_day();
        roster.advance_day();
        assert!(roster.get(gunner).unwrap().status.is_ready());
        assert_eq!(
            roster.crew_skill(&reg, Some(&tank), &crew, &[], "gunnery", None),
            15
        );
    }

    /// A cadet hurt at her station keeps working it, and is worse at it.
    ///
    /// The middle rung of `CrewCondition` promised this from the day it was
    /// written — "hurt but working her station, at the substitution penalty's
    /// worth of worse" — and for as long as `crew_skill` could only see the
    /// roster, nothing charged it: a gunner could be carried out of her seat
    /// in round two and still lay the gun perfectly in round ten. Being hurt
    /// costs the same as doing somebody else's job, which is the whole
    /// sentence the model is built out of, and it is deliberately the same
    /// number.
    #[test]
    fn a_cadet_hurt_at_her_station_still_lays_the_gun_and_is_worse_at_it() {
        use crate::battle::CrewCondition::{Fine, Out, Wounded};

        let reg = registry();
        let tank = tank();
        let mut roster = Roster::new();
        let commander = roster.enlist(0, &def("commander", 10, Some(11)), &reg);
        let gunner = roster.enlist(0, &def("gunner", 10, Some(15)), &reg);
        let crew = [commander, gunner];
        let gunnery = |conditions: &[crate::battle::CrewCondition]| {
            roster.crew_skill(&reg, Some(&tank), &crew, conditions, "gunnery", None)
        };

        assert_eq!(gunnery(&[Fine, Fine]), 15, "a whole crew is what it was");
        assert_eq!(
            gunnery(&[Fine, Wounded]),
            15 - reg.balance.substitution_penalty,
            "she is still the gunner, and worse at it than she was"
        );
        // ...and still better than handing the gun over, which is why she
        // stays in the seat rather than being replaced by the arithmetic.
        assert!(gunnery(&[Fine, Wounded]) > gunnery(&[Fine, Out]));
        assert_eq!(
            gunnery(&[Fine, Out]),
            11 - reg.balance.substitution_penalty,
            "carried out of the fight, she lays nothing; the commander reaches over"
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
            roster.crew_skill(&reg, Some(&tank), &[a, b], &[], "unclaimed", None),
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
        assert_eq!(
            roster.crew_skill(&reg, None, &[], &[], "gunnery", None),
            AVERAGE
        );
        assert_eq!(
            roster.crew_skill(&reg, None, &[CadetId(99)], &[], "gunnery", None),
            AVERAGE
        );
    }
}
