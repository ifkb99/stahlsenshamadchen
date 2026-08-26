//! Stahlsenshamädchen: a hex-based tactics roguelike engine.

mod battle;
mod camera;
mod campaign;
mod devtools;
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

/// Returning [`AppExit`] rather than `()` is what lets a failing dev script
/// fail the process. `App::run` has always handed back an exit status and
/// this dropped it, so a scripted tour could watch every assertion fail and
/// still exit 0 — which is worse than having no assertions, because it looks
/// like a pass. Bevy's `AppExit` implements `Termination`, so returning it
/// here is the whole fix; a normal run still exits 0.
fn main() -> AppExit {
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
            devtools::DevToolsPlugin,
        ))
        .add_systems(Update, map_render::reposition_map)
        .run()
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
        present_mode: present_mode(),
        ..default()
    }
}

/// How frames are handed to the compositor. Vsync by default, because this is
/// pixel art and tearing is the one artefact it cannot hide.
///
/// `STAHL_PRESENT=immediate|mailbox|no_vsync|fifo` overrides it, and exists
/// because of a driver bug rather than a preference. On this project's Linux
/// box — two RTX A4500s, NVIDIA 580.126.20, X11 — the Vulkan FIFO present path
/// loses the device after a few seconds:
///
/// ```text
/// Caught DeviceLost error: Unknown Unexpected error variant
///   (driver implementation is at fault)
/// ```
///
/// It is not this game's bug: a stock Bevy app with none of our systems
/// reproduces it 3 runs out of 3, and the same app under `immediate` survives
/// 3 out of 3. `crates/game/examples/minimal_window.rs` is that experiment if
/// it needs re-running after a driver update.
fn present_mode() -> bevy::window::PresentMode {
    use bevy::window::PresentMode;
    match std::env::var("STAHL_PRESENT").as_deref() {
        Ok("immediate") => PresentMode::Immediate,
        Ok("mailbox") => PresentMode::Mailbox,
        Ok("no_vsync") => PresentMode::AutoNoVsync,
        Ok("fifo") => PresentMode::Fifo,
        _ => PresentMode::AutoVsync,
    }
}
