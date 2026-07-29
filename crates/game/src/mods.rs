//! Loads the mod registry at boot and builds the placeholder art cache.

use crate::iso::ArtCache;
use crate::AppState;
use bevy::prelude::*;
use std::sync::Arc;
use tactics_core::data::DataRegistry;

/// The merged, validated game data. Immutable after boot; cheap to share.
#[derive(Resource, Clone)]
pub struct Mods(pub Arc<DataRegistry>);

pub struct ModsPlugin;

impl Plugin for ModsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Boot), load_mods);
    }
}

fn load_mods(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut next: ResMut<NextState<AppState>>,
) {
    let root = std::path::PathBuf::from("assets/mods");
    let (registry, report) = match DataRegistry::load_dir(&root) {
        Ok(r) => r,
        Err(err) => {
            error!("failed to load mods from {}: {err}", root.display());
            std::process::exit(1);
        }
    };
    for warning in &report.warnings {
        warn!("mod validation: {warning}");
    }
    if !report.is_ok() {
        for e in &report.errors {
            error!("mod validation: {e}");
        }
        error!("mods failed validation; run `cargo run --bin validate-mods` for details");
        std::process::exit(1);
    }
    info!(
        "loaded {} mod(s): {} characters, {} vehicles, {} maps",
        registry.mods.len(),
        registry.characters.len(),
        registry.vehicles.len(),
        registry.maps.len()
    );

    let art = ArtCache::build(&registry, &mut images);
    commands.insert_resource(art);
    commands.insert_resource(Mods(Arc::new(registry)));

    // Dev shortcut: STAHL_BATTLE=<map_id> boots straight into a battle.
    if let Ok(map_id) = std::env::var("STAHL_BATTLE") {
        commands.insert_resource(crate::battle::PendingBattle::Scenario { map_id });
        next.set(AppState::Battle);
    } else {
        next.set(AppState::Overworld);
    }
}
