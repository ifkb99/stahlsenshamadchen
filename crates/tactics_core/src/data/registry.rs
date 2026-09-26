//! Loads mod directories into a single validated [`DataRegistry`].

use super::defs::*;
use super::manifest::ModManifest;
use super::{
    AmmoClass, AmmoDef, Balance, Casualties, CommandRules, CoreDef, CoreIndex, ModuleDef,
    ModuleEffect, MoraleRules, PlannerRules, ReactionRules, RoleDef, STANDARD_MODULES, Scale,
    SkillDef, TraitDef,
};
use crate::map::{MapFile, MapKind};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum DataError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("parse error in {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("mod `{0}` depends on missing mod `{1}`")]
    MissingDependency(String, String),
    #[error("dependency cycle involving mod `{0}`")]
    DependencyCycle(String),
    #[error("no mods found under {0}")]
    NoMods(PathBuf),
}

/// Result of cross-referencing every loaded definition.
#[derive(Debug, Default, Clone)]
pub struct ValidationReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationReport {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    fn error(&mut self, msg: impl Into<String>) {
        self.errors.push(msg.into());
    }

    fn warn(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }
}

/// All game data, merged from every loaded mod in dependency order.
#[derive(Debug, Default, Clone)]
pub struct DataRegistry {
    pub mods: Vec<ModManifest>,
    /// What hexes, rounds and ticks mean. Unlike the definition maps this is
    /// a single value rather than a merge: see [`Self::load_dir`].
    pub scale: Scale,
    /// What a point of crew skill is worth. Single value, as [`Self::scale`].
    pub balance: Balance,
    /// What a battle costs the cadets who fought it. Single value, as
    /// [`Self::scale`].
    pub casualties: Casualties,
    /// How the AI thinks. Single value, as [`Self::scale`]. Defaulted rather
    /// than optional, unlike [`Self::command`]: there is no such thing as a
    /// battle with no planner numbers — somebody has to decide whether a
    /// march is worth making — so the neutral case is the shipped values
    /// rather than the absence of the block.
    pub planner: PlannerRules,
    /// How long crews take to act on orders.
    pub reaction: ReactionRules,
    /// What a crew can take before it stops doing as it is told.
    pub morale: MoraleRules,
    /// How far an order carries and how long it takes to arrive, or `None`
    /// when no mod declares a `command` block.
    ///
    /// The only rule block that is optional rather than defaulted, and the
    /// reason is the additivity rule read strictly: "no chain of command" is
    /// not a chain of command with generous numbers, it is the absence of a
    /// system — nothing is recomputed, nothing is delayed, and there is no
    /// per-tick cost to a battle that never asked for it. A block with zero
    /// coefficients must produce the same battle, and does; that is a pinned
    /// test rather than a hope.
    pub command: Option<CommandRules>,
    /// How a world is made, or `None` when no mod says. Optional for the
    /// reason [`Self::command`] is: a game with no world generator is not a
    /// generator with timid numbers, it is the absence of one.
    pub worldgen: Option<super::WorldGen>,
    /// How fast a column moves across a generated world. Defaulted, like
    /// [`Self::planner`]: a column that marches has a pace.
    pub march: super::March,
    /// The ladder of rank, lowest first ([`RankDef`]). Empty is no ranks at
    /// all: everybody is equal and command passes by arrival order.
    pub ranks: Vec<super::RankDef>,
    /// The axes of temperament this game has, in the order a cadet's values are
    /// stored. Declared by mod data, not by Rust.
    pub cores: Vec<CoreDef>,
    /// Core ids resolved to positions, so a check is an array index.
    pub core_index: CoreIndex,
    pub skills: HashMap<String, SkillDef>,
    pub roles: HashMap<String, RoleDef>,
    pub traits: HashMap<String, TraitDef>,
    pub characters: HashMap<String, CharacterDef>,
    pub vehicles: HashMap<String, VehicleDef>,
    pub weapons: HashMap<String, WeaponDef>,
    /// Every kind of round any weapon can chamber. No plural because there
    /// isn't one; the accessor beside it is [`Self::ammo`], the same
    /// map-plus-lookup pair as [`Self::radios`]/[`Self::radio`].
    pub ammo: HashMap<String, AmmoDef>,
    /// Every piece of breakable hardware a vehicle can carry. Accessor
    /// [`Self::module`]; what a given vehicle actually has aboard is
    /// [`Self::modules_for`], which is where the empty-list default lives.
    pub modules: HashMap<String, ModuleDef>,
    pub radios: HashMap<String, RadioDef>,
    pub terrain: HashMap<String, TerrainDef>,
    pub doctrines: HashMap<String, DoctrineDef>,
    /// Tactical templates by id ([`super::TemplateDef`]).
    pub templates: HashMap<String, super::TemplateDef>,
    pub maps: HashMap<String, MapFile>,
}

/// A definition file may hold one definition or an array of them.
#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> OneOrMany<T> {
    fn into_vec(self) -> Vec<T> {
        match self {
            Self::One(v) => vec![v],
            Self::Many(v) => v,
        }
    }
}

impl DataRegistry {
    /// Load every mod under `root` (each subdirectory containing a
    /// `mod.json`), in dependency order, then validate cross-references.
    ///
    /// Definitions merge by id — a later mod overriding one vehicle leaves
    /// the rest alone. Scale and balance do not merge, because they describe
    /// the game rather than a piece of content: the last mod in load order
    /// that declares a block replaces the previous one wholesale, and a mod
    /// that declares neither inherits what it is extending.
    pub fn load_dir(root: &Path) -> Result<(Self, ValidationReport), DataError> {
        let manifests = discover_mods(root)?;
        let ordered = topo_sort(manifests)?;

        let mut registry = Self::default();
        let mut report = ValidationReport::default();
        for (dir, manifest) in ordered {
            registry.load_mod_dir(&dir, &mut report)?;
            if let Some(scale) = manifest.scale {
                registry.scale = scale;
            }
            if let Some(balance) = manifest.balance {
                registry.balance = balance;
            }
            if let Some(casualties) = manifest.casualties {
                registry.casualties = casualties;
            }
            if let Some(planner) = manifest.planner {
                registry.planner = planner;
            }
            if let Some(reaction) = &manifest.reaction {
                registry.reaction = reaction.clone();
            }
            if let Some(morale) = &manifest.morale {
                registry.morale = morale.clone();
            }
            if let Some(command) = &manifest.command {
                registry.command = Some(command.clone());
            }
            if let Some(worldgen) = &manifest.worldgen {
                registry.worldgen = Some(worldgen.clone());
            }
            if let Some(march) = &manifest.march {
                registry.march = march.clone();
            }
            if let Some(ranks) = &manifest.ranks {
                registry.ranks = ranks.clone();
            }
            if let Some(cores) = &manifest.cores {
                registry.cores = cores.clone();
                registry.core_index = CoreIndex::build(cores);
            }
            if let Some(skills) = &manifest.skills {
                registry.skills = skills.iter().map(|s| (s.id.clone(), s.clone())).collect();
            }
            if let Some(roles) = &manifest.roles {
                registry.roles = roles.iter().map(|r| (r.id.clone(), r.clone())).collect();
            }
            if let Some(traits) = &manifest.traits {
                registry.traits = traits.iter().map(|t| (t.id.clone(), t.clone())).collect();
            }
            registry.mods.push(manifest);
        }
        registry.validate_into(&mut report);
        Ok((registry, report))
    }

    pub fn skill(&self, id: &str) -> Option<&SkillDef> {
        self.skills.get(id)
    }

    pub fn role(&self, id: &str) -> Option<&RoleDef> {
        self.roles.get(id)
    }

    pub fn trait_def(&self, id: &str) -> Option<&TraitDef> {
        self.traits.get(id)
    }

    pub fn terrain(&self, id: &str) -> Option<&TerrainDef> {
        self.terrain.get(id)
    }

    pub fn vehicle(&self, id: &str) -> Option<&VehicleDef> {
        self.vehicles.get(id)
    }

    /// Where a rank stands on the ladder: 0 is the lowest declared. `None`
    /// for no rank, or one the ladder does not have — both of which are the
    /// bottom, below every declared rank.
    pub fn rank_index(&self, id: Option<&str>) -> Option<usize> {
        let id = id?;
        self.ranks.iter().position(|r| r.id == id)
    }

    pub fn radio(&self, id: &str) -> Option<&RadioDef> {
        self.radios.get(id)
    }

    pub fn weapon(&self, id: &str) -> Option<&WeaponDef> {
        self.weapons.get(id)
    }

    pub fn ammo(&self, id: &str) -> Option<&AmmoDef> {
        self.ammo.get(id)
    }

    pub fn module(&self, id: &str) -> Option<&ModuleDef> {
        self.modules.get(id)
    }

    /// What is actually aboard `vehicle`, resolved against loaded content.
    ///
    /// The single place the empty-list rule is applied, so that spawning,
    /// validation and the roster table cannot disagree about what a vehicle
    /// carries. Two cases and no third:
    ///
    /// - the vehicle names modules, and this is those of them that exist;
    /// - the vehicle names none, and this is whichever of
    ///   [`STANDARD_MODULES`] the loaded content declares.
    ///
    /// Unknown ids are skipped rather than reported here — a lookup that
    /// returned errors would have to be called from places that have nowhere
    /// to put them. [`Self::validate_into`] is what refuses them, and it
    /// refuses them for the whole registry in one pass.
    ///
    /// Order follows the declaration (or [`STANDARD_MODULES`]) so that any
    /// listing built from this is stable; the per-unit state keyed off it is
    /// a `BTreeMap` regardless, because that is what reaches events.
    pub fn modules_for<'a>(&'a self, vehicle: &'a VehicleDef) -> Vec<&'a ModuleDef> {
        if vehicle.modules.is_empty() {
            STANDARD_MODULES
                .iter()
                .filter_map(|id| self.modules.get(*id))
                .collect()
        } else {
            vehicle
                .modules
                .iter()
                .filter_map(|id| self.modules.get(id))
                .collect()
        }
    }

    pub fn character(&self, id: &str) -> Option<&CharacterDef> {
        self.characters.get(id)
    }

    pub fn doctrine(&self, id: &str) -> Option<&DoctrineDef> {
        self.doctrines.get(id)
    }

    pub fn template(&self, id: &str) -> Option<&super::TemplateDef> {
        self.templates.get(id)
    }

    pub fn map(&self, id: &str) -> Option<&MapFile> {
        self.maps.get(id)
    }

    fn load_mod_dir(&mut self, dir: &Path, report: &mut ValidationReport) -> Result<(), DataError> {
        load_defs(&dir.join("characters"), report, |d: CharacterDef| {
            self.characters.insert(d.id.clone(), d);
        })?;
        load_defs(&dir.join("vehicles"), report, |d: VehicleDef| {
            self.vehicles.insert(d.id.clone(), d);
        })?;
        load_defs(&dir.join("weapons"), report, |d: WeaponDef| {
            self.weapons.insert(d.id.clone(), d);
        })?;
        load_defs(&dir.join("ammo"), report, |d: AmmoDef| {
            self.ammo.insert(d.id.clone(), d);
        })?;
        load_defs(&dir.join("modules"), report, |d: ModuleDef| {
            self.modules.insert(d.id.clone(), d);
        })?;
        load_defs(&dir.join("radios"), report, |d: RadioDef| {
            self.radios.insert(d.id.clone(), d);
        })?;
        load_defs(&dir.join("terrain"), report, |d: TerrainDef| {
            self.terrain.insert(d.id.clone(), d);
        })?;
        load_defs(&dir.join("doctrines"), report, |d: DoctrineDef| {
            self.doctrines.insert(d.id.clone(), d);
        })?;
        load_defs(&dir.join("templates"), report, |d: super::TemplateDef| {
            self.templates.insert(d.id.clone(), d);
        })?;
        load_defs(&dir.join("maps"), report, |d: MapFile| {
            self.maps.insert(d.id.clone(), d);
        })?;
        Ok(())
    }

    /// Cross-reference every definition and record problems in `report`.
    pub fn validate_into(&self, report: &mut ValidationReport) {
        self.validate_scale(report);
        self.validate_worldgen(report);
        // A rank the ladder does not have would put her at the bottom without
        // a word, which is a demotion nobody wrote.
        let mut ranked: Vec<&CharacterDef> = self.characters.values().collect();
        ranked.sort_by(|a, b| a.id.cmp(&b.id));
        for c in ranked {
            if let Some(rank) = &c.rank
                && !self.ranks.iter().any(|r| &r.id == rank)
            {
                report.error(format!(
                    "character `{}` holds rank `{rank}`, which no mod's `ranks` ladder declares",
                    c.id
                ));
            }
        }
        // A template nobody declared is a play nobody can run: refused, like
        // an undeclared rank, rather than quietly never chosen.
        let mut taught: Vec<(String, String)> = self
            .doctrines
            .values()
            .flat_map(|d| {
                d.teaches
                    .iter()
                    .map(|t| (format!("doctrine `{}`", d.id), t.clone()))
            })
            .chain(self.characters.values().flat_map(|c| {
                c.templates
                    .iter()
                    .map(|t| (format!("character `{}`", c.id), t.clone()))
            }))
            .collect();
        taught.sort();
        for (who, template) in taught {
            if !self.templates.contains_key(&template) {
                report.error(format!(
                    "{who} knows template `{template}`, which no mod declares"
                ));
            }
        }
        let mut ids: Vec<&str> = self.ranks.iter().map(|r| r.id.as_str()).collect();
        ids.sort_unstable();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            report.error("the `ranks` ladder names the same rank twice".to_string());
        }
        for v in self.vehicles.values() {
            if v.weapons.is_empty() {
                report.warn(format!("vehicle `{}` has no weapons", v.id));
            }
            for w in &v.weapons {
                if !self.weapons.contains_key(w) {
                    report.error(format!(
                        "vehicle `{}` references missing weapon `{}`",
                        v.id, w
                    ));
                }
            }
            if v.movement.points == 0 {
                report.warn(format!("vehicle `{}` has 0 movement points", v.id));
            }
            // `max_hp` was deprecated by the ballistics rewrite and is read by
            // nothing: what a vehicle can lose is now her crew and her
            // modules. The check used to demand a positive number, which was
            // right while the pool existed and became a trap the moment the
            // field went optional — the first chassis written without one
            // failed validation for omitting a number no code consults.
            // Absent (zero) is now simply "not stated"; a *negative* pool is
            // still a typo worth naming, since nobody omits a field by
            // writing `-1` in it.
            if v.max_hp < 0 {
                report.error(format!("vehicle `{}` has negative max_hp", v.id));
            }
            if let Some(radio) = &v.radio
                && !self.radios.contains_key(radio)
            {
                report.error(format!(
                    "vehicle `{}` references missing radio `{}`",
                    v.id, radio
                ));
            }
            // Concealment scales a spotter's range against her. A hundred
            // percent would be a unit nobody can ever see at any range, which
            // is not a stealth setting but a broken battle, and anything past
            // about ninety is close enough to that to be worth saying out
            // loud. A warning rather than an error because where exactly the
            // line sits is a balance opinion and a mod is allowed to disagree
            // with ours.
            if v.concealment > 90 {
                report.warn(format!(
                    "vehicle `{}` has concealment {}%, and nothing on this battlefield is invisible",
                    v.id, v.concealment
                ));
            }
            // Lift is for vehicles. A platoon that could carry another platoon
            // would need a whole second reading of what "aboard" means — who
            // is walking, who is being carried, and what happens to either
            // when the ground disagrees — and that is not a game this project
            // is playing. Warned rather than refused because a mod about
            // porters or pack animals is a perfectly reasonable thing for
            // somebody else to want, and the engine will simply not honour it.
            if v.capacity > 0 && v.movement.class == MovementClass::Foot {
                report.warn(format!(
                    "vehicle `{}` walks and lifts {} unit(s); infantry carrying infantry is not this game yet",
                    v.id, v.capacity
                ));
            }
            self.validate_stowage(v, report);
            self.validate_modules(v, report);
        }
        for m in self.modules.values() {
            // Zero hits remaining is the state the outcome engine will read as
            // "destroyed", so a module written with `toughness: 0` drives out
            // already broken. That is never what anyone meant.
            if m.toughness == 0 {
                report.error(format!(
                    "module `{}` has toughness 0, so it would spawn already destroyed; the minimum is 1",
                    m.id
                ));
            }
            // Size is a share of the draw a penetration makes. A zero share is
            // well-formed and unreachable: the module is aboard, cannot be
            // hit, and will never do anything. Almost always an unfilled
            // field rather than deliberate armour plating.
            if m.size == 0 {
                report.warn(format!(
                    "module `{}` has size 0, so nothing can ever hit it",
                    m.id
                ));
            }
        }
        for w in self.weapons.values() {
            if w.range[0] > w.range[1] {
                report.error(format!(
                    "weapon `{}` has min range {} greater than max range {}",
                    w.id, w.range[0], w.range[1]
                ));
            }
            if w.range[1] == 0 {
                report.error(format!("weapon `{}` has max range 0", w.id));
            }
            match w.reload_ticks {
                Some(0) => report.error(format!(
                    "weapon `{}` has reload_ticks 0; a weapon must take at least one tick to reload",
                    w.id
                )),
                Some(ticks) if ticks > self.scale.ticks_per_round => report.warn(format!(
                    "weapon `{}` reloads in {} ticks ({}), longer than the {}-tick round, so it cannot fire every round",
                    w.id,
                    ticks,
                    self.scale.format_duration(ticks),
                    self.scale.ticks_per_round
                )),
                _ => {}
            }
            for ammo in &w.ammo {
                if !self.ammo.contains_key(ammo) {
                    report.error(format!(
                        "weapon `{}` references missing ammo `{}`",
                        w.id, ammo
                    ));
                }
            }
        }
        for a in self.ammo.values() {
            // Flight time is `distance / velocity`, so a round that declares
            // no velocity is a division waiting to happen rather than a
            // slow shell.
            if a.velocity == 0 {
                report.error(format!("ammo `{}` has velocity 0", a.id));
            }
            match a.class {
                // A shaped charge forms its jet on impact and does not care
                // how fast it arrived, so two different numbers here are
                // almost always a copied kinetic entry rather than a
                // deliberate curve.
                AmmoClass::Chemical if a.penetration[0] != a.penetration[1] => report.warn(format!(
                    "ammo `{}` is chemical but its penetration falls {} -> {} with range; a shaped charge does not lose penetration downrange",
                    a.id, a.penetration[0], a.penetration[1]
                )),
                // The pair is written near-first. A kinetic round that gains
                // penetration as it flies has had its entries swapped.
                AmmoClass::Kinetic if a.penetration[0] < a.penetration[1] => report.warn(format!(
                    "ammo `{}` gains penetration with range ({} -> {}); the pair is [near, far], so these look swapped",
                    a.id, a.penetration[0], a.penetration[1]
                )),
                _ => {}
            }
        }
        for d in self.doctrines.values() {
            // Weights are multipliers; wildly out-of-band numbers are more
            // likely a typo than a design choice, so warn rather than reject.
            let bounded: [(&str, f32, f32, f32); 9] = [
                ("aggression", d.aggression, 0.0, 1.0),
                ("cover_value", d.cover_value, 0.0, 5.0),
                ("elevation_value", d.elevation_value, 0.0, 5.0),
                ("concentration", d.concentration, 0.0, 5.0),
                ("scouting", d.scouting, 0.0, 5.0),
                ("indirect_appetite", d.indirect_appetite, 0.0, 5.0),
                ("withdraw_threshold", d.withdraw_threshold, 0.0, 1.0),
                // These two are points off a goal's score per round of march
                // rather than multipliers, and `IMPATIENCE` charges 0.35 a
                // round for the march itself — so anything past 5 is a
                // doctrine that would rather sit still than take any ground
                // at all, which is a typo far more often than a design.
                ("route_caution", d.route_caution, 0.0, 5.0),
                ("contest_aversion", d.contest_aversion, 0.0, 5.0),
            ];
            for (field, value, lo, hi) in bounded {
                if !(lo..=hi).contains(&value) {
                    report.warn(format!(
                        "doctrine `{}` {field} is {value}, outside the usual {lo}..={hi}",
                        d.id
                    ));
                }
            }
        }
        for t in self.terrain.values() {
            // Retired with the treasury it paid into; see `TerrainDef::value`.
            // Warned rather than carried across, because a mod that set
            // `income` to pay for something now wants to decide what the
            // campaign planner should make of this ground instead.
            if let Some(was) = t.retired_income {
                report.warn(format!(
                    "terrain `{}` declares income {was}, which is no longer read: there are \
                     no funds. What the campaign planner wants from this ground is `value` \
                     (the base mod's city is 3 and factory 5, the numbers income had)",
                    t.id
                ));
            }
            if !(0..=100).contains(&t.cover) {
                report.error(format!("terrain `{}` cover must be 0-100", t.id));
            }
            if !(0..=100).contains(&t.concealment) {
                report.error(format!("terrain `{}` concealment must be 0-100", t.id));
            }
            // Ground that hides somebody at the far edge of a crew's reach is
            // a perfectly good thing to declare — the base mod's deep forest
            // does exactly that, and a crew is still given away by driving or
            // firing. Ground that hides her from a crew standing next to her
            // is not: `detection_certain_percent` exists precisely so that
            // what is plainly in front of somebody is seen, and a terrain
            // that beats it has made a hex nobody can ever be found on.
            // Warned rather than refused, because where that line sits is a
            // balance opinion and a mod is allowed to disagree with ours.
            if self.balance.detection_chance(t.concealment, 0, 1, 0) <= 0 {
                report.warn(format!(
                    "terrain `{}` conceals {}%, which is more than a crew standing on \
                     the next hex can see through",
                    t.id, t.concealment
                ));
            }
            if parse_color(&t.color).is_none() {
                report.error(format!(
                    "terrain `{}` color `{}` is not #rrggbb",
                    t.id, t.color
                ));
            }
            // The link from ground to battlefield is the one piece of terrain
            // data whose failure mode is silence: a campaign clash on a
            // terrain naming a map that does not exist does not stop, it
            // falls through to whichever battle map iterated first and the
            // mountains are fought out on a plain. An error rather than a
            // warning for that reason — there is no reading of a dangling
            // `battlefield` under which the modder got what she asked for.
            if let Some(id) = &t.battlefield {
                match self.maps.get(id) {
                    None => report.error(format!(
                        "terrain `{}` is fought on `{id}`, which no mod ships",
                        t.id
                    )),
                    Some(m) if m.kind != MapKind::Battle => report.error(format!(
                        "terrain `{}` is fought on `{id}`, which is a {} map rather than a battlefield",
                        t.id,
                        match m.kind {
                            MapKind::Overworld => "campaign",
                            MapKind::Battle => "battle",
                        }
                    )),
                    Some(_) => {}
                }
            }
        }
        for m in self.maps.values() {
            m.validate_into(self, report);
        }
    }

    /// Check that what a vehicle carries matches what it can fire.
    ///
    /// Two of these are errors and one is a warning, and the split is the
    /// point. A rack of shells no gun aboard can chamber is simply wrong —
    /// the vehicle would drive to the battle carrying dead weight it can
    /// never use, and there is no reading of the content under which that was
    /// meant. Sending a gun to war with nothing to feed it is different: it
    /// is how you write a vehicle whose coaxial is decorative, or a chassis
    /// whose loadout a scenario fills in later, and the day the pipeline
    /// makes an empty rack mean "cannot fire" a modder will want to have been
    /// told rather than stopped.
    fn validate_stowage(&self, v: &VehicleDef, report: &mut ValidationReport) {
        // Gathered once so both checks read the same answer. A `BTreeSet`
        // because it is walked to produce messages and nothing this project
        // prints should depend on a hash seed.
        let chamberable: std::collections::BTreeSet<&str> = v
            .weapons
            .iter()
            .filter_map(|w| self.weapons.get(w))
            .flat_map(|w| w.ammo.iter().map(String::as_str))
            .collect();

        for ammo in v.stowage.keys() {
            if !self.ammo.contains_key(ammo) {
                report.error(format!("vehicle `{}` stows missing ammo `{}`", v.id, ammo));
            } else if !chamberable.contains(ammo.as_str()) {
                report.error(format!(
                    "vehicle `{}` stows `{}`, which no weapon aboard can chamber",
                    v.id, ammo
                ));
            }
        }

        for weapon in v.weapons.iter().filter_map(|w| self.weapons.get(w)) {
            // A declared-but-empty rack counts as no rounds: the entry says a
            // space exists, not that anything is in it.
            let fed = weapon
                .ammo
                .iter()
                .any(|a| v.stowage.get(a).is_some_and(|n| *n > 0));
            if !weapon.ammo.is_empty() && !fed {
                report.warn(format!(
                    "vehicle `{}` carries `{}` but stows none of the ammo it fires ({})",
                    v.id,
                    weapon.id,
                    weapon.ammo.join(", ")
                ));
            }
        }
    }

    /// Check that what a vehicle says is aboard her exists, and that the
    /// hardware she claims matches the hardware she has.
    ///
    /// One error and two warnings, and as with stowage the split follows
    /// intent. A module id no mod declares is an error: the vehicle would
    /// drive out with a hole in her equipment list and the shot that went
    /// looking for it would find nothing, silently. The same id written
    /// twice is a warning rather than an error because the meaning is
    /// obvious and harmless — per-module state is keyed by id, so the second
    /// entry collapses into the first — but it is worth saying out loud,
    /// since a modder writing it almost certainly wanted two *different*
    /// modules sharing an effect and needs to know that is spelt with two
    /// ids.
    ///
    /// Two modules with the same effect is explicitly fine and goes
    /// unremarked: a hull machine gun beside a coaxial is two guns, and the
    /// day someone writes an auxiliary engine it is two mobility modules.
    /// Effects are behaviours, not slots.
    fn validate_modules(&self, v: &VehicleDef, report: &mut ValidationReport) {
        let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        for id in &v.modules {
            if !self.modules.contains_key(id) {
                report.error(format!(
                    "vehicle `{}` carries missing module `{}`",
                    v.id, id
                ));
            }
            if !seen.insert(id.as_str()) {
                report.warn(format!(
                    "vehicle `{}` lists module `{}` twice; a module is carried once, so the repeat does nothing",
                    v.id, id
                ));
            }
        }

        // A wireless set is the one module whose presence the rest of the
        // content already has an opinion about: `radio` is what the chain of
        // command reads to decide whether she can be reached at all. A
        // vehicle with a radio module and no radio hardware is a crew who
        // can lose a set they never had — harmless today, and wrong the
        // moment losing it means something. A warning rather than an error
        // because the reverse pairing (hardware, no module) is a legitimate
        // way to write a set that cannot be shot off, and neither direction
        // should stop a mod from loading.
        //
        // Asked of `modules_for` rather than of the declared list, so it
        // reports what the vehicle will actually spawn carrying: a chassis
        // that declares nothing inherits the standard four, and inheriting a
        // wireless set she does not mount is exactly the mismatch worth
        // hearing about.
        if v.radio.is_none() {
            for m in self.modules_for(v) {
                if m.effect == ModuleEffect::Radio {
                    report.warn(format!(
                        "vehicle `{}` carries module `{}` but mounts no radio, so there is no set aboard to lose",
                        v.id, m.id
                    ));
                }
            }
        }

        // Troops are the one module kind that needs somebody named. A platoon
        // is one piece on one hex precisely because its leadership is two or
        // three cadets in the ordinary crew seats and the rest is abstracted
        // into the module — so a chassis carrying troops and declaring no
        // crew slots is a body of soldiers with nobody to lead them, which
        // the interior roll, the casualty machinery and the chain of command
        // all have opinions about and none of them good ones. A warning
        // rather than an error because it is well-formed content the engine
        // will happily spawn: she is simply a unit whose only occupants are
        // riflemen, and the day a mod wants exactly that it should be told
        // once and then left alone.
        if v.crew_slots.is_empty() {
            for m in self.modules_for(v) {
                if m.effect == ModuleEffect::Troops {
                    report.warn(format!(
                        "vehicle `{}` carries troops module `{}` but declares no crew slots, so nobody is named to lead them",
                        v.id, m.id
                    ));
                }
            }
        }
    }

    /// Check the scale block for values that would make the derived
    /// quantities meaningless, and the balance block for coefficients so
    /// large that crew skill would outweigh the vehicle.
    ///
    /// The battle-hexes-per-overworld-hex check is the one that earns its
    /// keep: the contract says an overworld hex *is* a battle map, and
    /// nothing else in the codebase would ever notice the two drifting apart.
    /// A world generator that writes a terrain nobody declares makes a
    /// world nobody can stand on; one whose relief shares do not add to a
    /// hundred makes a world that is not the one its author described.
    fn validate_worldgen(&self, report: &mut ValidationReport) {
        let Some(wg) = &self.worldgen else {
            return;
        };
        let p = &wg.terrain;
        for (kind, id) in [
            ("open", &p.open),
            ("wood", &p.wood),
            ("hedge", &p.hedge),
            ("wet", &p.wet),
            ("water", &p.water),
            ("road", &p.road),
            ("town", &p.town),
        ] {
            if self.terrain(id).is_none() {
                report.error(format!(
                    "worldgen terrain.{kind} is `{id}`, which no mod declares"
                ));
            }
        }
        for rule in &wg.summary {
            if self.terrain(&rule.terrain).is_none() {
                report.error(format!(
                    "worldgen summary names `{}`, which no mod declares",
                    rule.terrain
                ));
            }
            if let Some(share) = &rule.share
                && self.terrain(&share.terrain).is_none()
            {
                report.error(format!(
                    "worldgen summary counts `{}`, which no mod declares",
                    share.terrain
                ));
            }
        }
        if wg.summary.last().is_none_or(|r| {
            r.feature.is_some() || r.mean_elevation_tenths.is_some() || r.share.is_some()
        }) {
            report.warn(
                "worldgen summary does not end with a rule that always holds; a campaign hex no \
                 rule names is called by terrain.open",
            );
        }
        let shares = &wg.relief.shares;
        if shares.is_empty() || shares.len() > 10 {
            report.error(format!(
                "worldgen relief.shares names {} levels; a world has 1 to 10",
                shares.len()
            ));
        }
        let total: u32 = shares.iter().sum();
        if total != 100 {
            report.error(format!("worldgen relief.shares adds to {total}, not 100"));
        }
        if wg.towns.factories > wg.towns.count {
            report.error(format!(
                "worldgen asks for {} factories in {} towns",
                wg.towns.factories, wg.towns.count
            ));
        }
        if wg.cover.wood_percent + wg.cover.hedge_percent > 100 {
            report.error(
                "worldgen cover: wood and hedge together are more than the land".to_string(),
            );
        }
    }

    fn validate_scale(&self, report: &mut ValidationReport) {
        let s = &self.scale;
        for (field, value) in [
            ("hex_meters", s.hex_meters),
            ("round_seconds", s.round_seconds),
            ("elevation_meters", s.elevation_meters),
            ("overworld_hex_meters", s.overworld_hex_meters),
            ("overworld_turn_hours", s.overworld_turn_hours),
        ] {
            if !(value.is_finite() && value > 0.0) {
                report.error(format!(
                    "scale {field} must be a positive number, got {value}"
                ));
            }
        }
        if s.ticks_per_round == 0 {
            report.error("scale ticks_per_round must be at least 1".to_string());
        }

        let per_hex = s.battle_hexes_per_overworld_hex();
        if per_hex.is_finite() && !(16.0..=64.0).contains(&per_hex) {
            report.warn(format!(
                "one overworld hex is {per_hex:.0} battle hexes across, which is not a plausible battle map; \
                 the contract is that an overworld hex is one map, so hex_meters and overworld_hex_meters have drifted apart"
            ));
        }

        for (field, value) in [
            (
                "vision_per_observation",
                self.balance.vision_per_observation,
            ),
            ("speed_per_driving", self.balance.speed_per_driving),
            ("accuracy_per_gunnery", self.balance.accuracy_per_gunnery),
        ] {
            // Stats run 0..=5, so anything past 20 lets a crew more than
            // double their vehicle and makes the hardware decorative.
            if !(0..=20).contains(&value) {
                report.warn(format!(
                    "balance {field} is {value}; crew stats run 0-5, so this is outside the usual 0-20"
                ));
            }
        }

        // The casualty table is the dial between "an armoured skirmish costs
        // nobody anything" and "half the school is in the infirmary by
        // Tuesday", which is exactly why it wants checking: a percentage
        // typed as a fraction reads as a mod that never hurts anybody, and
        // does so silently.
        let c = &self.casualties;
        for (field, value) in [
            ("harm_kinetic", c.harm_kinetic),
            ("harm_explosive", c.harm_explosive),
            ("harm_small_arms", c.harm_small_arms),
            ("harm_unattributed", c.harm_unattributed),
            ("harm_floor", c.harm_floor),
            ("harm_ceiling", c.harm_ceiling),
            ("adrift_percent", c.adrift_percent),
            ("severe_percent", c.severe_percent),
        ] {
            if !(0..=100).contains(&value) {
                report.error(format!(
                    "casualties {field} is {value}; it is a chance in 100"
                ));
            }
        }
        if c.harm_floor > c.harm_ceiling {
            report.error(format!(
                "casualties harm_floor ({}) is above harm_ceiling ({})",
                c.harm_floor, c.harm_ceiling
            ));
        }
        for (field, range) in [
            ("adrift_days", c.adrift_days),
            ("severe_days", c.severe_days),
            ("light_days", c.light_days),
            ("carried_days", c.carried_days),
            ("grazed_days", c.grazed_days),
        ] {
            // A range typed backwards is forgiven by `Casualties::days` and
            // said out loud here, because content should be readable and a
            // mod author should still hear about it.
            if range[0] > range[1] {
                report.warn(format!(
                    "casualties {field} is [{}, {}], which reads backwards; it will be used as [{}, {}]",
                    range[0], range[1], range[1], range[0]
                ));
            }
        }

        // The planner numbers. Only a handful have a value that is *wrong*
        // rather than merely aggressive, and those are quiet failures rather
        // than loud ones, which is the whole reason to check them: a game
        // that plays subtly badly looks exactly like a game whose balance you
        // disagree with.
        let p = &self.planner;
        if p.horizon_rounds == 0 {
            report.error(
                "planner horizon_rounds is 0, so no goal has a road at all and every crew \
                 prices every march as if the ground beyond her were unreachable"
                    .to_string(),
            );
        }
        if p.impatience < 0.0 {
            report.error(format!(
                "planner impatience is {}; a negative price pays a crew to drive, so she \
                 marches away from the ground she wants",
                p.impatience
            ));
        }
        // Warnings rather than errors, because both are legitimate things for
        // a mod to say and neither is silent when it happens: `devolved`
        // outside the doctrine range means every commander devolves or none
        // does, which is a decision a difficulty mod might well make.
        if !(0.0..=1.0).contains(&p.devolved) {
            report.warn(format!(
                "planner devolved is {}; doctrine delegation runs 0-1, so no commander will \
                 change her mind about assigning ground",
                p.devolved
            ));
        }
        if p.exit_urgency < 0.0 {
            report.warn(format!(
                "planner exit_urgency is {}; a broken crew is repelled by the lane she is \
                 trying to leave through",
                p.exit_urgency
            ));
        }
        if p.boarding_rounds < 0.0 {
            report.warn(format!(
                "planner boarding_rounds is {}; mounting up saves time in itself, so a \
                 platoon will board to be carried nowhere",
                p.boarding_rounds
            ));
        }
        // A negative slope is the one genuinely inverted value in the
        // order-versus-terrain family: every objective and every mission
        // would then be worth *more* the further off it is, so a crew drives
        // away from the ground she was sent to take and is behaving
        // perfectly rationally about it. Nothing else in the game would
        // report a fault.
        if p.distance_decay < 0.0 {
            report.error(format!(
                "planner distance_decay is {}; a negative slope makes every objective and \
                 every order worth more the further away it is, so a crew marches away from \
                 the ground she was given",
                p.distance_decay
            ));
        }
        if p.plateau < 0.0 {
            report.error(format!(
                "planner plateau is {}; the band is a width, and a negative one excludes the \
                 best tile from the choice between the best tiles",
                p.plateau
            ));
        }
        if p.order_worth < 0.0 {
            report.warn(format!(
                "planner order_worth is {}; a formation under orders is repelled by the \
                 ground its commander named",
                p.order_worth
            ));
        }
        if !(0.0..=1.0).contains(&p.order_complement) {
            report.warn(format!(
                "planner order_complement is {}; below zero a worn crew values an order at \
                 less than a share of what she has left, and above one at more than a \
                 fresh crew would",
                p.order_complement
            ));
        }
        if p.score_worth < 0.0 {
            report.warn(format!(
                "planner score_worth is {}; every objective on every map is then worth \
                 marching away from, and a side wins by conceding the ground",
                p.score_worth
            ));
        }
        // A field that has been removed is the one mod error nothing else in
        // this machinery can report: serde ignores what it does not
        // recognise, so a modder who had tuned `mission_weight` would load
        // cleanly and play a different game from the one they wrote. A
        // warning rather than an error, for the same reason `devolved` out of
        // range is one — carrying a stale key from an older engine is a thing
        // a mod may legitimately do, and this is the tool that tells them.
        if let Some(was) = p.retired_mission_weight {
            report.warn(format!(
                "planner mission_weight is {was} and is no longer read. An order is priced \
                 as a share of what the crew has left now: set planner.order_worth (0.25 \
                 ships, and 0.25 x a typical eleven-point chassis is the 3.0 that \
                 mission_weight 2.0 used to pay)"
            ));
        }
        if let Some(was) = self.balance.retired_speed_per_athletics {
            report.warnings.push(format!(
                "balance speed_per_athletics is {was} and is no longer read. A foot unit \
                 has one movement point and one hex a round is already walking pace, so \
                 no percentage of it can say anything; athletics buys steep ground \
                 instead, through athletics_per_climb_level"
            ));
        }
        // Warned rather than errored because 1.0 is a coherent thing for a
        // mod to say — it just is not the thing it looks like. See the field.
        if !(0.0..1.0).contains(&p.pull_under_fire) {
            report.warn(format!(
                "planner pull_under_fire is {}; at 1 or more an advance presses on through \
                 fire, which is what an assault already is, so the two orders stop differing \
                 in anything",
                p.pull_under_fire
            ));
        }
    }

    /// Convenience wrapper producing a fresh report.
    pub fn validate(&self) -> ValidationReport {
        let mut report = ValidationReport::default();
        self.validate_into(&mut report);
        report
    }
}

/// Parse `#rrggbb` into linear byte components.
pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let hex = s.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(hex, 16).ok()?;
    Some([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

fn discover_mods(root: &Path) -> Result<Vec<(PathBuf, ModManifest)>, DataError> {
    let entries = std::fs::read_dir(root).map_err(|source| DataError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    let mut mods = Vec::new();
    for entry in entries.flatten() {
        let manifest_path = entry.path().join("mod.json");
        if manifest_path.is_file() {
            let manifest: ModManifest = read_json(&manifest_path)?;
            mods.push((entry.path(), manifest));
        }
    }
    if mods.is_empty() {
        return Err(DataError::NoMods(root.to_path_buf()));
    }
    mods.sort_by(|a, b| a.1.id.cmp(&b.1.id));
    Ok(mods)
}

/// Order mods so that dependencies load before dependents.
fn topo_sort(mods: Vec<(PathBuf, ModManifest)>) -> Result<Vec<(PathBuf, ModManifest)>, DataError> {
    let index: HashMap<String, usize> = mods
        .iter()
        .enumerate()
        .map(|(i, (_, m))| (m.id.clone(), i))
        .collect();
    // 0 = unvisited, 1 = visiting, 2 = done
    let mut state = vec![0u8; mods.len()];

    fn visit(
        i: usize,
        mods: &[(PathBuf, ModManifest)],
        index: &HashMap<String, usize>,
        state: &mut [u8],
        ordered: &mut Vec<usize>,
    ) -> Result<(), DataError> {
        match state[i] {
            2 => return Ok(()),
            1 => return Err(DataError::DependencyCycle(mods[i].1.id.clone())),
            _ => {}
        }
        state[i] = 1;
        for dep in &mods[i].1.dependencies {
            let Some(&j) = index.get(dep) else {
                return Err(DataError::MissingDependency(
                    mods[i].1.id.clone(),
                    dep.clone(),
                ));
            };
            visit(j, mods, index, state, ordered)?;
        }
        state[i] = 2;
        ordered.push(i);
        Ok(())
    }

    let mut order_indices = Vec::new();
    for i in 0..mods.len() {
        visit(i, &mods, &index, &mut state, &mut order_indices)?;
    }
    let mut slots: Vec<Option<(PathBuf, ModManifest)>> = mods.into_iter().map(Some).collect();
    Ok(order_indices
        .into_iter()
        .map(|i| slots[i].take().expect("each index visited once"))
        .collect())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, DataError> {
    let text = std::fs::read_to_string(path).map_err(|source| DataError::Io {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&text).map_err(|source| DataError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn load_defs<T: DeserializeOwned>(
    dir: &Path,
    report: &mut ValidationReport,
    mut insert: impl FnMut(T),
) -> Result<(), DataError> {
    if !dir.is_dir() {
        return Ok(());
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|source| DataError::Io {
            path: dir.to_path_buf(),
            source,
        })?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    paths.sort();
    for path in paths {
        match read_json::<OneOrMany<T>>(&path) {
            Ok(defs) => {
                for def in defs.into_vec() {
                    insert(def);
                }
            }
            // Malformed individual files are validation errors, not fatal:
            // modders should see every problem in one pass.
            Err(err) => report.error(err.to_string()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_parsing() {
        assert_eq!(parse_color("#ff8000"), Some([255, 128, 0]));
        assert_eq!(parse_color("ff8000"), None);
        assert_eq!(parse_color("#ff80"), None);
    }

    #[test]
    fn topo_sort_orders_dependencies_first() {
        let m = |id: &str, deps: &[&str]| {
            (
                PathBuf::from(id),
                ModManifest {
                    id: id.into(),
                    name: id.into(),
                    version: String::new(),
                    description: String::new(),
                    cores: None,
                    skills: None,
                    roles: None,
                    traits: None,
                    reaction: None,
                    morale: None,
                    command: None,
                    worldgen: None,
                    march: None,
                    ranks: None,
                    dependencies: deps.iter().map(|s| s.to_string()).collect(),
                    scale: None,
                    balance: None,
                    casualties: None,
                    planner: None,
                },
            )
        };
        let sorted = topo_sort(vec![m("addon", &["base"]), m("base", &[])]).unwrap();
        let ids: Vec<_> = sorted.iter().map(|(_, m)| m.id.as_str()).collect();
        assert_eq!(ids, vec!["base", "addon"]);

        assert!(matches!(
            topo_sort(vec![m("a", &["b"]), m("b", &["a"])]),
            Err(DataError::DependencyCycle(_))
        ));
        assert!(matches!(
            topo_sort(vec![m("a", &["nope"])]),
            Err(DataError::MissingDependency(..))
        ));
    }
}
