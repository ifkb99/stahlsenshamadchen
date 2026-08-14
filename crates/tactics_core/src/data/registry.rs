//! Loads mod directories into a single validated [`DataRegistry`].

use super::defs::*;
use super::manifest::ModManifest;
use super::{
    AmmoClass, AmmoDef, Balance, CommandRules, CoreDef, CoreIndex, MoraleRules, ReactionRules,
    RoleDef, Scale, SkillDef, TraitDef,
};
use crate::map::MapFile;
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
    /// The axes of temperament this game has, in the order a girl's values are
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
    pub radios: HashMap<String, RadioDef>,
    pub terrain: HashMap<String, TerrainDef>,
    pub doctrines: HashMap<String, DoctrineDef>,
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
            if let Some(reaction) = &manifest.reaction {
                registry.reaction = reaction.clone();
            }
            if let Some(morale) = &manifest.morale {
                registry.morale = morale.clone();
            }
            if let Some(command) = &manifest.command {
                registry.command = Some(command.clone());
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

    pub fn radio(&self, id: &str) -> Option<&RadioDef> {
        self.radios.get(id)
    }

    pub fn weapon(&self, id: &str) -> Option<&WeaponDef> {
        self.weapons.get(id)
    }

    pub fn ammo(&self, id: &str) -> Option<&AmmoDef> {
        self.ammo.get(id)
    }

    pub fn character(&self, id: &str) -> Option<&CharacterDef> {
        self.characters.get(id)
    }

    pub fn doctrine(&self, id: &str) -> Option<&DoctrineDef> {
        self.doctrines.get(id)
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
        load_defs(&dir.join("radios"), report, |d: RadioDef| {
            self.radios.insert(d.id.clone(), d);
        })?;
        load_defs(&dir.join("terrain"), report, |d: TerrainDef| {
            self.terrain.insert(d.id.clone(), d);
        })?;
        load_defs(&dir.join("doctrines"), report, |d: DoctrineDef| {
            self.doctrines.insert(d.id.clone(), d);
        })?;
        load_defs(&dir.join("maps"), report, |d: MapFile| {
            self.maps.insert(d.id.clone(), d);
        })?;
        Ok(())
    }

    /// Cross-reference every definition and record problems in `report`.
    pub fn validate_into(&self, report: &mut ValidationReport) {
        self.validate_scale(report);
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
            if v.max_hp <= 0 {
                report.error(format!("vehicle `{}` has non-positive max_hp", v.id));
            }
            if let Some(radio) = &v.radio
                && !self.radios.contains_key(radio)
            {
                report.error(format!(
                    "vehicle `{}` references missing radio `{}`",
                    v.id, radio
                ));
            }
            self.validate_stowage(v, report);
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
            let bounded: [(&str, f32, f32, f32); 7] = [
                ("aggression", d.aggression, 0.0, 1.0),
                ("cover_value", d.cover_value, 0.0, 5.0),
                ("elevation_value", d.elevation_value, 0.0, 5.0),
                ("concentration", d.concentration, 0.0, 5.0),
                ("scouting", d.scouting, 0.0, 5.0),
                ("indirect_appetite", d.indirect_appetite, 0.0, 5.0),
                ("withdraw_threshold", d.withdraw_threshold, 0.0, 1.0),
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
            if !(0..=100).contains(&t.cover) {
                report.error(format!("terrain `{}` cover must be 0-100", t.id));
            }
            if parse_color(&t.color).is_none() {
                report.error(format!(
                    "terrain `{}` color `{}` is not #rrggbb",
                    t.id, t.color
                ));
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

    /// Check the scale block for values that would make the derived
    /// quantities meaningless, and the balance block for coefficients so
    /// large that crew skill would outweigh the vehicle.
    ///
    /// The battle-hexes-per-overworld-hex check is the one that earns its
    /// keep: the contract says an overworld hex *is* a battle map, and
    /// nothing else in the codebase would ever notice the two drifting apart.
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
                    dependencies: deps.iter().map(|s| s.to_string()).collect(),
                    scale: None,
                    balance: None,
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
