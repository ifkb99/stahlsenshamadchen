use serde::{Deserialize, Serialize};

/// `mod.json` at the root of every mod directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModManifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Ids of mods that must load before this one. Later mods override
    /// definitions with the same id.
    #[serde(default)]
    pub dependencies: Vec<String>,
}
