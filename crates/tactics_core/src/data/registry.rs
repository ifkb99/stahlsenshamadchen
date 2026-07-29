//! Loads mod directories into a single validated [`DataRegistry`].

use super::defs::*;
use super::manifest::ModManifest;
use crate::map::MapFile;
use serde::de::DeserializeOwned;
use serde::Deserialize;
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
    pub characters: HashMap<String, CharacterDef>,
    pub vehicles: HashMap<String, VehicleDef>,
    pub weapons: HashMap<String, WeaponDef>,
    pub terrain: HashMap<String, TerrainDef>,
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
    pub fn load_dir(root: &Path) -> Result<(Self, ValidationReport), DataError> {
        let manifests = discover_mods(root)?;
        let ordered = topo_sort(manifests)?;

        let mut registry = Self::default();
        let mut report = ValidationReport::default();
        for (dir, manifest) in ordered {
            registry.load_mod_dir(&dir, &mut report)?;
            registry.mods.push(manifest);
        }
        registry.validate_into(&mut report);
        Ok((registry, report))
    }

    pub fn terrain(&self, id: &str) -> Option<&TerrainDef> {
        self.terrain.get(id)
    }

    pub fn vehicle(&self, id: &str) -> Option<&VehicleDef> {
        self.vehicles.get(id)
    }

    pub fn weapon(&self, id: &str) -> Option<&WeaponDef> {
        self.weapons.get(id)
    }

    pub fn character(&self, id: &str) -> Option<&CharacterDef> {
        self.characters.get(id)
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
        load_defs(&dir.join("terrain"), report, |d: TerrainDef| {
            self.terrain.insert(d.id.clone(), d);
        })?;
        load_defs(&dir.join("maps"), report, |d: MapFile| {
            self.maps.insert(d.id.clone(), d);
        })?;
        Ok(())
    }

    /// Cross-reference every definition and record problems in `report`.
    pub fn validate_into(&self, report: &mut ValidationReport) {
        for v in self.vehicles.values() {
            if v.weapons.is_empty() {
                report.warn(format!("vehicle `{}` has no weapons", v.id));
            }
            for w in &v.weapons {
                if !self.weapons.contains_key(w) {
                    report.error(format!("vehicle `{}` references missing weapon `{}`", v.id, w));
                }
            }
            if v.movement.points == 0 {
                report.warn(format!("vehicle `{}` has 0 movement points", v.id));
            }
            if v.max_hp <= 0 {
                report.error(format!("vehicle `{}` has non-positive max_hp", v.id));
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
fn topo_sort(
    mods: Vec<(PathBuf, ModManifest)>,
) -> Result<Vec<(PathBuf, ModManifest)>, DataError> {
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
                    dependencies: deps.iter().map(|s| s.to_string()).collect(),
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
