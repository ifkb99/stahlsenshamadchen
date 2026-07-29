//! Camera controls: pan (WASD/arrows/middle-drag), zoom (wheel), and the
//! six-step view rotation (Q/E).
//!
//! The camera is pixel-perfect on purpose. Sprites are drawn at native size,
//! so one texel is one world unit, and the art only stays crisp if a texel
//! covers a whole number of physical pixels. Two things would break that:
//! a fractional display scale factor (this is common — 1.25, 1.5, and so on)
//! feeding straight into the projection, and a camera parked on a fractional
//! position. So zoom is quantised to whole pixels per texel, and the
//! transform is snapped to the pixel grid while panning tracks the exact
//! position separately.

use crate::iso::ViewRotation;
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;

/// Zoom, in physical pixels per texel. Always a whole number.
#[derive(Resource, Clone, Copy)]
pub struct PixelZoom(pub u32);

impl Default for PixelZoom {
    fn default() -> Self {
        // Replaced on the first frame by a scale-factor-appropriate default.
        Self(0)
    }
}

pub const MAX_ZOOM: u32 = 4;

/// Exact camera position. Panning accumulates here rather than in the
/// `Transform`, which is snapped: rounding every frame would swallow slow
/// pans whose per-frame delta is under one pixel.
#[derive(Resource, Default)]
pub struct CameraFocus(pub Vec2);

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewRotation>()
            .init_resource::<PixelZoom>()
            .init_resource::<CameraFocus>()
            .add_systems(Startup, setup_camera)
            .add_systems(Update, (pan_zoom, rotate_view, apply_camera).chain());
    }
}

fn setup_camera(mut commands: Commands) {
    // MSAA antialiases sprite quad edges, which only softens crisp pixel
    // art (and costs bandwidth for nothing in a 2D scene).
    commands.spawn((Camera2d, Msaa::Off));
}

fn pan_zoom(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    time: Res<Time>,
    mut focus: ResMut<CameraFocus>,
    mut zoom: ResMut<PixelZoom>,
) {
    let current = zoom.0.max(1);
    if scroll.delta.y > 0.0 {
        zoom.0 = (current + 1).min(MAX_ZOOM);
    } else if scroll.delta.y < 0.0 {
        zoom.0 = current.saturating_sub(1).max(1);
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
    // Pan at a constant on-screen speed regardless of zoom.
    let speed = 600.0 / current as f32;
    focus.0 += delta * speed * time.delta_secs();

    if buttons.pressed(MouseButton::Middle) {
        let drag = motion.delta / current as f32;
        focus.0 += Vec2::new(-drag.x, drag.y);
    }
}

/// Push the zoom and the snapped focus into the camera each frame.
fn apply_camera(
    windows: Query<&Window>,
    focus: Res<CameraFocus>,
    mut zoom: ResMut<PixelZoom>,
    mut camera: Query<(&mut Transform, &mut Projection), With<Camera2d>>,
) {
    let Ok(window) = windows.single() else { return };
    let Ok((mut transform, mut projection)) = camera.single_mut() else {
        return;
    };
    let Projection::Orthographic(ortho) = &mut *projection else {
        return;
    };

    let scale_factor = window.resolution.scale_factor().max(1.0);
    if zoom.0 == 0 {
        // Default to the zoom closest to the size the desktop asks for, so
        // the game looks right on both 1x and HiDPI displays.
        zoom.0 = (scale_factor.round() as u32).clamp(1, MAX_ZOOM);
    }
    let pixels_per_texel = zoom.0 as f32;

    // `ScalingMode::WindowSize` maps one world unit to `scale_factor`
    // physical pixels before `scale` divides it, so this lands the ratio on
    // exactly `pixels_per_texel`.
    ortho.scale = scale_factor / pixels_per_texel;

    // Snap to whole physical pixels. A viewport with an odd dimension puts
    // its centre on a half pixel, so shift by half a texel to compensate.
    let odd = |n: u32| if n % 2 == 1 { 0.5 } else { 0.0 };
    let half = Vec2::new(
        odd(window.resolution.physical_width()),
        odd(window.resolution.physical_height()),
    ) / pixels_per_texel;
    let snapped = (focus.0 * pixels_per_texel).round() / pixels_per_texel + half;
    transform.translation.x = snapped.x;
    transform.translation.y = snapped.y;
}

fn rotate_view(keys: Res<ButtonInput<KeyCode>>, mut rotation: ResMut<ViewRotation>) {
    if keys.just_pressed(KeyCode::KeyQ) {
        rotation.0 = (rotation.0 + 5) % 6;
    }
    if keys.just_pressed(KeyCode::KeyE) {
        rotation.0 = (rotation.0 + 1) % 6;
    }
}
