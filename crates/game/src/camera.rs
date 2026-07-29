//! Camera controls: pan (WASD/arrows/middle-drag), zoom (wheel), and the
//! six-step view rotation (Q/E).

use crate::iso::ViewRotation;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewRotation>()
            .add_systems(Startup, setup_camera)
            .add_systems(Update, (pan_zoom, rotate_view));
    }
}

fn setup_camera(mut commands: Commands) {
    commands.spawn(Camera2d);
}

fn pan_zoom(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    time: Res<Time>,
    mut camera: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
) {
    let Ok((mut transform, mut projection)) = camera.single_mut() else {
        return;
    };
    let Projection::Orthographic(ortho) = &mut *projection else {
        return;
    };

    if scroll.delta.y.abs() > 0.0 {
        ortho.scale = (ortho.scale * (1.0 - scroll.delta.y * 0.1)).clamp(0.35, 4.0);
    }

    let mut delta = Vec2::ZERO;
    if keys.pressed(KeyCode::KeyW) || keys.pressed(KeyCode::ArrowUp) {
        delta.y += 1.0;
    }
    if keys.pressed(KeyCode::KeyS) || keys.pressed(KeyCode::ArrowDown) {
        delta.y -= 1.0;
    }
    if keys.pressed(KeyCode::KeyA) || keys.pressed(KeyCode::ArrowLeft) {
        delta.x -= 1.0;
    }
    if keys.pressed(KeyCode::KeyD) || keys.pressed(KeyCode::ArrowRight) {
        delta.x += 1.0;
    }
    let speed = 600.0 * ortho.scale;
    transform.translation += (delta * speed * time.delta_secs()).extend(0.0);

    if buttons.pressed(MouseButton::Middle) {
        let drag = motion.delta * ortho.scale;
        transform.translation += Vec3::new(-drag.x, drag.y, 0.0);
    }
}

fn rotate_view(keys: Res<ButtonInput<KeyCode>>, mut rotation: ResMut<ViewRotation>) {
    if keys.just_pressed(KeyCode::KeyQ) {
        rotation.0 = (rotation.0 + 5) % 6;
    }
    if keys.just_pressed(KeyCode::KeyE) {
        rotation.0 = (rotation.0 + 1) % 6;
    }
}
