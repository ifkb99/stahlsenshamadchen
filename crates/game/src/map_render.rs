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

/// The fog overlay covering a tile (same silhouette, tinted black).
#[derive(Component)]
pub struct FogOverlay {
    pub hex: Hex,
}

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
            commands.spawn((
                Sprite {
                    image,
                    color: Color::srgba(0.02, 0.02, 0.05, 1.0),
                    ..default()
                },
                Transform::from_translation(Vec3::new(
                    pos.x,
                    pos.y + prism_offset(tile.elevation),
                    z + 0.4,
                )),
                FogOverlay { hex },
                scope.clone(),
            ));
        }
    }
}

/// Re-project every tile and fog overlay when the view rotates.
pub fn reposition_tiles(
    rotation: Res<ViewRotation>,
    center: Res<ViewCenter>,
    mut tiles: Query<(&MapTile, &mut Transform), Without<FogOverlay>>,
    mut fog: Query<(&FogOverlay, &mut Transform), With<FogOverlay>>,
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
    for (overlay, mut transform) in &mut fog {
        let elevation = maps.0.get(overlay.hex).map(|t| t.elevation).unwrap_or(0);
        let (pos, z) = iso::project(overlay.hex, elevation, rotation.0, center.0);
        transform.translation = Vec3::new(pos.x, pos.y + prism_offset(elevation), z + 0.4);
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
