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
//! The cost of that is an obligation to put them back, and it used to be a
//! list of manual duties in a function called `rehydrate` — the kind of rule
//! that is only ever stated in prose and is therefore only ever remembered.
//! Forgetting one is silent: an empty sight grid answers every line-of-sight
//! question wrongly rather than loudly, an empty move grid says nobody can
//! drive, and an empty `visible_key` panics.
//!
//! # The obligation is a type
//!
//! Rebuilding needs the [`DataRegistry`], which serde cannot hand to a
//! `Deserialize` impl, so the shape here says "not yet" in the type system
//! instead. [`crate::battle::Battle`] carries a marker saying whether its
//! caches hold anything; the unbuilt marker is [`Default`] and the built one
//! is not, and serde fills a `#[serde(skip)]` field with `Default::default()`.
//! So [`crate::battle::SavedBattle`] deserializes and
//! [`crate::battle::BattleState`] does not, and the only bridge between them
//! is [`crate::battle::SavedBattle::rehydrate`], which takes a registry and
//! destructures the battle field by field — so a cache added tomorrow stops
//! the build until somebody has said where it comes from.
//!
//! That reaches this module the same way: [`SavedGame`] is what comes off the
//! disk, [`SaveGame`] is what a caller gets, and there is no route from one to
//! the other except [`SaveGame::from_json`]. A battle whose caches nobody
//! rebuilt is not a value this crate can produce.
//!
//! Chosen over the two alternatives — a `Caches` struct owning the skipped
//! fields, or a hand-written mirror of the saved shape — because both would
//! have meant either a second copy of `BattleState`'s field list, which
//! drifts, or renaming `state.sight` and `state.moves` at every call site in
//! the engine. This costs one type parameter with a default, so every
//! signature in the codebase still says `BattleState` and reads as it did.
//!
//! # Which mods were playing
//!
//! A save records the mods that produced it, because in this project the mods
//! *are* the rules. Difficulty is a mod: whether crews bail out, whether cadets
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

use crate::battle::{BattleState, SavedBattle};
use crate::data::DataRegistry;
use crate::overworld::OverworldState;
use serde::{Deserialize, Serialize};

/// Save format version. Bump when a field changes meaning rather than merely
/// being added — serde's `default` handles additions on its own.
///
/// Version 2 renamed the roster's people from *girls* to *cadets*, which is a
/// rename of serde field names (`Roster::cadets`, `CrewLoss::cadet`) and so a
/// break even though nothing about the format's meaning moved.
///
/// Version 3 gave a formation's orders a [`crate::battle::Latitude`].
/// Everything it added defaults, but `MissionChange` went from tuple variants
/// to struct ones, so an order caught in transit in an older save no longer
/// reads — and an order that fails to read is a formation that silently
/// forgets what it was told.
///
/// Version 4 folded a unit's `detached` / `tasking` / `latitude` into one
/// [`crate::battle::PersonalOrder`], and `WaitingOrders`' destination and
/// latitude into one [`crate::battle::March`]. An older save is **refused**
/// rather than migrated, on the grounds that the failure it would otherwise
/// produce is silent: the new field defaults to `None`, so a version-3 file
/// would open with every crew quietly back under her formation's mission,
/// and a hand-placed tank drifting off in the first planning phase is
/// exactly the bug detachment exists to prevent. `SaveError::Version` says
/// which version was found, which is the loud failure this is worth.
pub const SAVE_VERSION: u32 = 4;

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
///
/// `B` is the battle's *condition*, not a choice a caller makes: `SaveGame` is
/// the ordinary one and carries a playable [`BattleState`], while
/// [`SavedGame`] carries a battle whose caches are still empty and is the only
/// one serde can produce. See the module docs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(deserialize = "B: Deserialize<'de>"))]
pub struct SaveGame<B = BattleState> {
    pub version: u32,
    /// The mods in effect when this was written. Empty in saves from before
    /// this was recorded, which are then accepted without a mod check.
    #[serde(default)]
    pub mods: Vec<ModStamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overworld: Option<OverworldState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battle: Option<B>,
}

/// A save exactly as it comes off the disk: its battle's caches are empty, so
/// it is not yet a game anybody can play. [`SaveGame::from_json`] is the only
/// thing that reads one, and what it hands back is a `SaveGame`.
pub type SavedGame = SaveGame<SavedBattle>;

impl SaveGame<BattleState> {
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

    /// Parse a save, check it belongs to this game, and put back everything
    /// [`SaveGame`] chose not to store.
    ///
    /// The registry is needed for exactly that: the sight and movement grids
    /// are rebuilt from the map and its terrain rather than carried in the
    /// file. It is also the only door: what serde reads is a [`SavedGame`],
    /// which nothing can play until its battle has been through
    /// [`SavedBattle::rehydrate`] here.
    ///
    /// Returns any warnings alongside — mirroring
    /// [`DataRegistry::load_dir`], which reports rather than refuses for
    /// anything survivable.
    pub fn from_json(
        registry: &DataRegistry,
        text: &str,
    ) -> Result<(Self, Vec<String>), SaveError> {
        let saved: SavedGame = serde_json::from_str(text)?;
        if saved.version != SAVE_VERSION {
            return Err(SaveError::Version {
                found: saved.version,
                expected: SAVE_VERSION,
            });
        }
        let warnings = saved.check_mods(registry)?;
        // Destructured rather than field-assigned so that a field added to
        // `SaveGame` has to be carried across deliberately: the two halves of
        // the save differ only in the battle's condition, and anything that
        // silently failed to make the crossing would be state a player lost by
        // loading.
        let SaveGame {
            version,
            mods,
            overworld,
            battle,
        } = saved;
        Ok((
            Self {
                version,
                mods,
                overworld,
                battle: battle.map(|b| b.rehydrate(registry)),
            },
            warnings,
        ))
    }
}

impl<B> SaveGame<B> {
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
