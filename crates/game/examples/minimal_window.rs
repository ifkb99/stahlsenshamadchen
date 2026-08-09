//! Is it us, or is it the driver?
//!
//! A stock Bevy window with none of this game's systems in it. When the
//! renderer misbehaves, run this first: a failure here is the environment, a
//! pass here means the problem is ours and worth chasing.
//!
//! It earned its keep once already. The overworld was dying a few seconds in
//! with
//!
//! ```text
//! Caught DeviceLost error: Unknown Unexpected error variant
//!   (driver implementation is at fault)
//! ```
//!
//! which looked like a rendering bug in the campaign screen. This reproduced
//! it 3 runs out of 3 with no game code involved, which settled that question,
//! and then `PRESENT=` isolated it further: the vsync present modes
//! (`autovsync`, `fifo`) fail every run and the others (`immediate`,
//! `mailbox`, `nosync`) survive every run. Hence `STAHL_PRESENT` in
//! `main.rs`. Re-run after a driver update to find out whether that knob is
//! still needed:
//!
//! ```sh
//! for m in fifo immediate; do PRESENT=$m cargo run -p stahlsenshamädchen \
//!   --example minimal_window; done
//! ```
//!
//! Beware one false trail: `WGPU_BACKEND=gl` appears to fix it, but only
//! because the GL backend cannot find a device on this machine at all and
//! panics before rendering anything. Check for the panic before believing a
//! clean run.

use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(
            DefaultPlugins
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "minimal".into(),
                        present_mode: match std::env::var("PRESENT").as_deref() {
                            Ok("fifo") => bevy::window::PresentMode::Fifo,
                            Ok("immediate") => bevy::window::PresentMode::Immediate,
                            Ok("mailbox") => bevy::window::PresentMode::Mailbox,
                            Ok("nosync") => bevy::window::PresentMode::AutoNoVsync,
                            _ => bevy::window::PresentMode::AutoVsync,
                        },
                        ..default()
                    }),
                    ..default()
                })
                .set(ImagePlugin::default_nearest()),
        )
        .insert_resource(ClearColor(Color::srgb(0.09, 0.10, 0.13)))
        .add_systems(Startup, setup)
        .add_systems(Update, quit_after)
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn((Camera2d, Msaa::Off));
    // A few hundred sprites, in the same ballpark as a map screen.
    for i in 0..400 {
        let x = (i % 20) as f32 * 32.0 - 320.0;
        let y = (i / 20) as f32 * 32.0 - 320.0;
        commands.spawn((
            Sprite {
                color: Color::srgb(0.3, 0.6, 0.3),
                custom_size: Some(Vec2::splat(28.0)),
                ..default()
            },
            Transform::from_xyz(x, y, 0.0),
        ));
    }
}

fn quit_after(time: Res<Time>, mut exit: MessageWriter<AppExit>) {
    if time.elapsed_secs() > 30.0 {
        exit.write(AppExit::Success);
    }
}
