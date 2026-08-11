//! Loads the mod registry at boot and builds the placeholder art cache.

use crate::AppState;
use crate::iso::ArtCache;
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
    let registry = Arc::new(registry);
    commands.insert_resource(Mods(Arc::clone(&registry)));

    // Straight into a battle, skipping the campaign map: `--battle` on the
    // command line (optionally `--battle <map_id>` / `--battle=<map_id>`),
    // or the `STAHL_BATTLE=<map_id>` env var the dev scripts already use.
    // The flag exists because "let me just fight one" is a player-shaped
    // request, not only a tooling one, and an env var is a poor front door.
    match battle_shortcut(&registry) {
        Some(map_id) => {
            info!("booting straight into battle on `{map_id}`");
            commands.insert_resource(crate::battle::PendingBattle::Scenario { map_id });
            next.set(AppState::Battle);
        }
        None => next.set(AppState::Overworld),
    }
}

/// The battle map the command line or environment asks to boot into, if any.
///
/// A bare `--battle` takes the first battle map in id order — with one map
/// shipped that is simply "the battle map", and with several it is at least
/// deterministic. A name that matches nothing is an error worth stopping for:
/// the player asked for a specific fight, and silently starting the campaign
/// instead would look like the flag doing nothing.
fn battle_shortcut(registry: &tactics_core::data::DataRegistry) -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut asked: Option<Option<String>> = std::env::var("STAHL_BATTLE").ok().map(Some);
    for (i, arg) in args.iter().enumerate() {
        if let Some(id) = arg.strip_prefix("--battle=") {
            asked = Some(Some(id.to_string()));
        } else if arg == "--battle" {
            asked = Some(
                args.get(i + 1)
                    .filter(|next| !next.starts_with('-'))
                    .cloned(),
            );
        }
    }
    let asked = asked?;
    let mut battles: Vec<&String> = registry
        .maps
        .iter()
        .filter(|(_, m)| m.kind == tactics_core::map::MapKind::Battle)
        .map(|(id, _)| id)
        .collect();
    battles.sort();
    match asked {
        Some(id) if battles.iter().any(|b| **b == id) => Some(id),
        Some(id) => {
            error!(
                "no battle map named `{id}`; shipped battle maps: {}",
                battles
                    .iter()
                    .map(|b| b.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            std::process::exit(1);
        }
        None => battles.first().map(|id| (*id).clone()),
    }
}
