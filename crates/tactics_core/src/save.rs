//! Saving and restoring a game in progress.
//!
//! The simulation is deterministic given a seed, and a save has to preserve
//! that: a game reloaded from disk must produce the *same* future as the one
//! that would have happened without saving. That is a stronger requirement
//! than "the numbers look right", and it is the property the whole engine is
//! built around — replays, search-based AI and the determinism baseline all
//! rest on it. So the rng's stream position is part of the save, not just its
//! seed, and `tests/save.rs` holds the line by forking a battle in progress:
//! one copy carries straight on, the other goes through a save file first, and
//! the two must produce the same events for the rest of the fight.
//!
//! # What is deliberately not saved
//!
//! Two structures are skipped because they are caches rather than state:
//! [`crate::battle::SightGrid`], which resolves every tile's sight heights,
//! and the per-unit vision inside [`crate::battle::FogMap`]. Both are pure
//! functions of the map and the registry, both are large — the sight grid has
//! an entry per tile, so over a thousand on a battle map — and neither can
//! hold anything a player did.
//!
//! The cost of that is one obligation: [`rehydrate`] has to rebuild the sight
//! grid,
//! because an empty one answers every line-of-sight question wrongly rather
//! than loudly. That is why loading takes a registry.
//!
//! # Which mods were playing
//!
//! A save records the mods that produced it, because in this project the mods
//! *are* the rules. Difficulty is a mod: whether crews bail out, whether girls
//! can refuse an order, whether death is permanent. Loading a campaign under a
//! different set would silently change what game it is — a run started gentle
//! could come back lethal, and a permadeath run could quietly stop being one.
//!
//! Mismatched *ids* are refused, because the rules genuinely differ. Differing
//! *versions* of the same mods are allowed and reported, since that is an
//! ordinary content patch and refusing would make every balance tweak a
//! save-breaker.
//!
//! # Versioning
//!
//! [`SaveGame::version`] is checked on load. It exists now, while there is
//! nothing to migrate, because the alternative is discovering the need for it
//! from a player's corrupted campaign.

use crate::battle::{BattleState, SightGrid};
use crate::data::DataRegistry;
use crate::overworld::OverworldState;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Save format version. Bump when a field changes meaning rather than merely
/// being added — serde's `default` handles additions on its own.
pub const SAVE_VERSION: u32 = 1;

/// Which mod, at which version, was loaded when a save was written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModStamp {
    pub id: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("save is version {found}, this build reads version {expected}")]
    Version { found: u32, expected: u32 },
    #[error(
        "save was made with a different set of mods: {}{}",
        if missing.is_empty() { String::new() } else { format!("missing {}", missing.join(", ")) },
        if extra.is_empty() { String::new() } else { format!(" unexpected {}", extra.join(", ")) },
    )]
    Mods {
        /// In the save, not loaded now.
        missing: Vec<String>,
        /// Loaded now, not in the save.
        extra: Vec<String>,
    },
    #[error("save is not valid json: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A game in progress: the campaign, and the battle being fought inside it if
/// there is one.
///
/// Both are optional because the two layers are genuinely independent — a
/// scenario battle has no campaign behind it, and a campaign between battles
/// has no battle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveGame {
    pub version: u32,
    /// The mods in effect when this was written. Empty in saves from before
    /// this was recorded, which are then accepted without a mod check.
    #[serde(default)]
    pub mods: Vec<ModStamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overworld: Option<OverworldState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battle: Option<BattleState>,
}

impl SaveGame {
    /// Stamp a save with the mods currently loaded.
    pub fn new(
        registry: &DataRegistry,
        overworld: Option<OverworldState>,
        battle: Option<BattleState>,
    ) -> Self {
        Self {
            version: SAVE_VERSION,
            mods: registry
                .mods
                .iter()
                .map(|m| ModStamp {
                    id: m.id.clone(),
                    version: m.version.clone(),
                })
                .collect(),
            overworld,
            battle,
        }
    }

    pub fn to_json(&self) -> Result<String, SaveError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Parse a save and put back everything [`SaveGame`] chose not to store.
    ///
    /// The registry is needed for exactly that: the sight grid is rebuilt from
    /// the map and terrain rather than carried in the file.
    /// Parse a save, check it belongs to this game, and put back everything
    /// [`SaveGame`] chose not to store.
    ///
    /// Returns any warnings alongside — mirroring
    /// [`DataRegistry::load_dir`], which reports rather than refuses for
    /// anything survivable.
    pub fn from_json(
        registry: &DataRegistry,
        text: &str,
    ) -> Result<(Self, Vec<String>), SaveError> {
        let mut save: Self = serde_json::from_str(text)?;
        if save.version != SAVE_VERSION {
            return Err(SaveError::Version {
                found: save.version,
                expected: SAVE_VERSION,
            });
        }
        let warnings = save.check_mods(registry)?;
        if let Some(battle) = &mut save.battle {
            rehydrate(registry, battle);
        }
        Ok((save, warnings))
    }

    /// Compare the save's mods against what is loaded.
    fn check_mods(&self, registry: &DataRegistry) -> Result<Vec<String>, SaveError> {
        // A save written before mods were stamped cannot be checked, and
        // refusing it would be worse than trusting it.
        if self.mods.is_empty() {
            return Ok(Vec::new());
        }
        let loaded: Vec<&str> = registry.mods.iter().map(|m| m.id.as_str()).collect();
        let saved: Vec<&str> = self.mods.iter().map(|s| s.id.as_str()).collect();

        let missing: Vec<String> = saved
            .iter()
            .filter(|id| !loaded.contains(id))
            .map(|id| (*id).to_string())
            .collect();
        let extra: Vec<String> = loaded
            .iter()
            .filter(|id| !saved.contains(id))
            .map(|id| (*id).to_string())
            .collect();
        if !missing.is_empty() || !extra.is_empty() {
            return Err(SaveError::Mods { missing, extra });
        }

        // Same mods, different versions: an ordinary content patch. Say so and
        // carry on, because refusing would make every balance tweak break
        // saves.
        Ok(self
            .mods
            .iter()
            .filter_map(|stamp| {
                let now = registry.mods.iter().find(|m| m.id == stamp.id)?;
                (now.version != stamp.version).then(|| {
                    format!(
                        "mod `{}` was version {} when this was saved and is {} now",
                        stamp.id, stamp.version, now.version
                    )
                })
            })
            .collect())
    }
}

/// Rebuild what the save left out. Separate and public so a caller that
/// deserializes a `BattleState` by some other route cannot forget it.
pub fn rehydrate(registry: &DataRegistry, battle: &mut BattleState) {
    if battle.sight.is_empty() {
        battle.sight = Arc::new(SightGrid::build(registry, &battle.map));
    }
    // The fog's per-unit vision and per-side keys are skipped too, and the
    // key list is indexed by side during recompute, so an empty one panics
    // rather than simply recomputing.
    battle.fog.rehydrate();
}

/// Write a save to disk.
pub fn write(path: impl AsRef<std::path::Path>, save: &SaveGame) -> Result<(), SaveError> {
    let path = path.as_ref();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, save.to_json()?)?;
    Ok(())
}

/// Read a save from disk.
pub fn read(
    registry: &DataRegistry,
    path: impl AsRef<std::path::Path>,
) -> Result<(SaveGame, Vec<String>), SaveError> {
    SaveGame::from_json(registry, &std::fs::read_to_string(path)?)
}
