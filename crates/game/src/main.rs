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

/// The phases a screen's frame runs in, in order.
///
/// Both screens do the same seven things in the same sequence, which is why
/// this is one enum and not two: the battle and the campaign map differ in
/// *which* systems they put in each phase, never in what the phases are or
/// what order they come in.
///
/// **This replaced a `.chain()` of twelve and fourteen systems.** A chain is a
/// total order expressed by adjacency, so where a system sat in a tuple was
/// load-bearing and invisible — two of the orderings were load-bearing enough
/// to carry paragraphs explaining them, and this layer has already lost time
/// to an ordering fault (`until idle` coming true a frame early, four `Enter`
/// presses advancing one round, every screenshot after them describing the
/// wrong turn while the script reported success). A new system now names the
/// phase it belongs to and inherits that phase's place.
///
/// The order is not the obvious one and it is worth knowing why:
/// **animation comes first**. The paced event queue gates everything behind
/// it — the simulation refuses to advance while anything is still animating,
/// and the screen refuses keystrokes for the same reason — so the queue has to
/// be drained before anyone asks whether it is empty.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScreenSet {
    /// Drain the paced event queue and advance sprite animation. First
    /// because everything after it asks whether this has finished.
    Animate,
    /// Let the simulation take a step: AI planning, round resolution.
    Simulate,
    /// Read the keyboard and the mouse. After the simulation so a keystroke
    /// is answered against the state the player was actually looking at.
    Input,
    /// Carry the simulation's answer out to the entities that draw it.
    Sync,
    /// Everything derived purely for the eye: fog, highlights, markers,
    /// panels, decaying effects.
    Present,
    /// Leaving the screen, and anything that can ask for a state transition.
    /// Late, so a frame that ends the screen has already been drawn.
    Lifecycle,
    /// The dev harness's window onto the frame. Last, so `ScriptFacts`
    /// describes a settled frame rather than a half-applied one — an `until
    /// idle` that came true early is exactly the fault above.
    Facts,
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
        // Declared once for the whole app rather than per screen. Configuring
        // the same set twice with two `run_if`s would AND them and the phase
        // would never run at all, so the run conditions stay on the systems
        // and the *order* lives here.
        .configure_sets(
            Update,
            (
                ScreenSet::Animate,
                ScreenSet::Simulate,
                ScreenSet::Input,
                ScreenSet::Sync,
                ScreenSet::Present,
                ScreenSet::Lifecycle,
                ScreenSet::Facts,
            )
                .chain(),
        )
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
