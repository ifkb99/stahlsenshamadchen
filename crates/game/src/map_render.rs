//! Shared hex map rendering used by both the battle and overworld screens.

use crate::iso::{self, ArtCache, ViewCenter, ViewRotation};
use bevy::prelude::*;
use tactics_core::map::HexMap;
use tactics_core::Hex;

/// Marks a spawned tile sprite.
#[derive(Component)]
pub struct MapTile {
    pub hex: Hex,
    pub elevation: i32,
}

/// Where a hex overlay sits relative to the tile prism.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    /// Centered on the tile's top face (range markers, plan routes).
    Face,
    /// Aligned with the prism silhouette (fog).
    Prism,
}

/// Shared data for any sprite anchored to a hex. Kind-specific markers
/// (`FogOverlay`, battle plan/move highlights, …) live alongside this.
#[derive(Component, Clone, Copy)]
pub struct HexOverlay {
    pub hex: Hex,
    pub anchor: Anchor,
    pub z_bias: f32,
}

impl HexOverlay {
    /// Top-face marker, slightly above the tile so it draws over the prism.
    pub fn face(hex: Hex) -> Self {
        Self {
            hex,
            anchor: Anchor::Face,
            z_bias: 0.6,
        }
    }

    /// Fog-style marker, sitting on the prism body.
    pub fn fog(hex: Hex) -> Self {
        Self {
            hex,
            anchor: Anchor::Prism,
            z_bias: 0.4,
        }
    }

    /// World translation for this overlay under the current view.
    pub fn translation(&self, map: &HexMap, rotation: u32, center: Hex) -> Vec3 {
        let elevation = map.get(self.hex).map(|t| t.elevation).unwrap_or(0);
        let (pos, z) = iso::project(self.hex, elevation, rotation, center);
        match self.anchor {
            Anchor::Face => Vec3::new(pos.x, pos.y, z + self.z_bias),
            Anchor::Prism => {
                Vec3::new(pos.x, pos.y + prism_offset(elevation), z + self.z_bias)
            }
        }
    }
}

/// The fog overlay covering a tile (same silhouette, tinted black).
#[derive(Component)]
pub struct FogOverlay;

/// Vertical pixel offset from a tile's top-face center for the fog/tile
/// sprite: the prism image is taller than the face, so the sprite center
/// sits half the wall height lower.
pub fn prism_offset(elevation: i32) -> f32 {
    -(elevation.max(0) as f32 * iso::ELEV_PX) / 2.0
}

/// Spawn tile sprites (and fog overlays) for a map. Extra components in
/// `scope` tag entities for state-scoped cleanup.
pub fn spawn_map<B: Bundle + Clone>(
    commands: &mut Commands,
    art: &ArtCache,
    map: &HexMap,
    rotation: u32,
    with_fog: bool,
    scope: B,
) {
    let center = map.center();
    for (hex, tile) in map.iter() {
        let (pos, z) = iso::project(hex, tile.elevation, rotation, center);
        let image = art.tile(&tile.terrain, tile.elevation);
        commands.spawn((
            Sprite {
                image: image.clone(),
                ..default()
            },
            Transform::from_translation(Vec3::new(
                pos.x,
                pos.y + prism_offset(tile.elevation),
                z,
            )),
            MapTile {
                hex,
                elevation: tile.elevation,
            },
            scope.clone(),
        ));
        if with_fog {
            let overlay = HexOverlay::fog(hex);
            commands.spawn((
                Sprite {
                    image,
                    color: Color::srgba(0.02, 0.02, 0.05, 1.0),
                    ..default()
                },
                Transform::from_translation(overlay.translation(map, rotation, center)),
                overlay,
                FogOverlay,
                scope.clone(),
            ));
        }
    }
}

/// Re-project every tile and hex-anchored overlay when the view rotates.
pub fn reposition_map(
    rotation: Res<ViewRotation>,
    center: Res<ViewCenter>,
    mut tiles: Query<(&MapTile, &mut Transform), Without<HexOverlay>>,
    mut overlays: Query<(&HexOverlay, &mut Transform)>,
    maps: Option<Res<CurrentMap>>,
) {
    if !rotation.is_changed() && !center.is_changed() {
        return;
    }
    let Some(maps) = maps else { return };
    for (tile, mut transform) in &mut tiles {
        let (pos, z) = iso::project(tile.hex, tile.elevation, rotation.0, center.0);
        transform.translation = Vec3::new(pos.x, pos.y + prism_offset(tile.elevation), z);
    }
    for (overlay, mut transform) in &mut overlays {
        transform.translation = overlay.translation(&maps.0, rotation.0, center.0);
    }
}

/// The map currently on screen (battle or overworld), for picking and
/// re-projection.
#[derive(Resource)]
pub struct CurrentMap(pub std::sync::Arc<HexMap>);

/// World-space cursor position, if the cursor is over the window.
pub fn cursor_world(
    windows: &Query<&Window>,
    camera: &Query<(&Camera, &GlobalTransform)>,
) -> Option<Vec2> {
    let window = windows.single().ok()?;
    let cursor = window.cursor_position()?;
    let (camera, transform) = camera.single().ok()?;
    camera.viewport_to_world_2d(transform, cursor).ok()
}

/// The tile under the cursor right now.
pub fn hovered_tile(
    windows: &Query<&Window>,
    camera: &Query<(&Camera, &GlobalTransform)>,
    map: &HexMap,
    rotation: u32,
) -> Option<Hex> {
    let world = cursor_world(windows, camera)?;
    iso::pick(map, world, rotation, map.center())
}

/// UI color for a side index.
pub fn side_color(side: u8) -> Color {
    let c = iso::SIDE_COLORS[(side as usize) % iso::SIDE_COLORS.len()];
    Color::srgb_u8(c[0], c[1], c[2])
}
