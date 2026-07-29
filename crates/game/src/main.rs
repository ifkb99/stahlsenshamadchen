//! Stahlsenshamädchen: a hex-based tactics roguelike engine.

mod battle;
mod camera;
mod campaign;
mod iso;
mod map_render;
mod mods;
mod overworld;

use bevy::prelude::*;

#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    /// Load mods, build art caches.
    #[default]
    Boot,
    /// Strategic layer.
    Overworld,
    /// Tactical layer.
    Battle,
}

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(primary_window()),
                    ..default()
                })
                // Sprites are pixel art drawn at native size; smoothing them
                // would only blur the texels.
                .set(ImagePlugin::default_nearest()),
        )
        .insert_resource(ClearColor(Color::srgb(0.09, 0.10, 0.13)))
        .init_state::<AppState>()
        .init_resource::<iso::ViewCenter>()
        .add_plugins((
            mods::ModsPlugin,
            campaign::CampaignPlugin,
            camera::CameraPlugin,
            battle::BattlePlugin,
            overworld::OverworldPlugin,
        ))
        .add_systems(Update, (map_render::reposition_tiles, dev_screenshot))
        .run();
}

fn primary_window() -> Window {
    let mut resolution = bevy::window::WindowResolution::default();
    // Dev tool: STAHL_SCALE=1.5 fakes a HiDPI display so pixel-grid
    // regressions can be reproduced on an ordinary 1x monitor.
    if let Some(scale) = std::env::var("STAHL_SCALE")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        resolution.set_scale_factor_override(Some(scale));
    }
    Window {
        title: "Stahlsenshamädchen".into(),
        resolution,
        ..default()
    }
}

/// Dev tool: STAHL_SCREENSHOT=out.png captures the window a few seconds in
/// (delay adjustable with STAHL_SCREENSHOT_AT=<seconds>).
fn dev_screenshot(mut commands: Commands, time: Res<Time>, mut done: Local<bool>) {
    let at = std::env::var("STAHL_SCREENSHOT_AT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4.0);
    if *done || time.elapsed_secs() < at {
        return;
    }
    *done = true;
    if let Ok(path) = std::env::var("STAHL_SCREENSHOT") {
        commands
            .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(path));
    }
}
