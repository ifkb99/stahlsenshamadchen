//! Shared hex map rendering used by both the battle and overworld screens.

use crate::iso::{self, ArtCache, ViewCenter, ViewRotation};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use tactics_core::Hex;
use tactics_core::map::HexMap;

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

    /// Top-face marker that wins where two land on the same tile.
    ///
    /// Two overlays at the same `z_bias` are ordered by nothing in
    /// particular, so a marker that must stay readable under another one
    /// cannot simply be spawned later. The radio-range ring is the case that
    /// wanted it: on the campaign an army's move range and its net horizon
    /// are both a handful of hexes out and land on each other, and the ring
    /// is the thinner, more precise of the two.
    pub fn face_over(hex: Hex) -> Self {
        Self {
            hex,
            anchor: Anchor::Face,
            z_bias: 0.65,
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
            Anchor::Prism => Vec3::new(pos.x, pos.y + prism_offset(elevation), z + self.z_bias),
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
            Transform::from_translation(Vec3::new(pos.x, pos.y + prism_offset(tile.elevation), z)),
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

/// Where the dev script says the cursor is, overriding the real mouse.
///
/// This exists because writing to `Window::cursor_position` makes `bevy_winit`
/// warp the operating system pointer, which is not acceptable for a harness
/// that has to run while someone else is using the machine. See
/// [`crate::devtools`].
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq)]
pub enum ScriptedCursor {
    /// The real mouse is in charge.
    #[default]
    None,
    /// Pretend the cursor is over this hex, whatever the camera is doing.
    Hex(Hex),
    /// Pretend the cursor is at this window position, in logical pixels.
    Pixel(Vec2),
}

/// How the world is currently being looked at, and what is under the cursor.
///
/// Rotation and centre are the two halves of the isometric projection and were
/// threaded as a pair through nine systems; the window and camera are what
/// picking needs; the scripted cursor is the dev harness's override. Every one
/// of those is a *view* question, so they travel together, and the systems that
/// used to take three or four parameters to ask "where is the mouse pointing"
/// now take one.
#[derive(SystemParam)]
pub struct View<'w, 's> {
    rotation: Res<'w, ViewRotation>,
    center: Res<'w, ViewCenter>,
    windows: Query<'w, 's, &'static Window>,
    camera: Query<'w, 's, (&'static Camera, &'static GlobalTransform)>,
    scripted: Option<Res<'w, ScriptedCursor>>,
}

impl View<'_, '_> {
    /// Which of the six view rotations is active.
    pub fn rotation(&self) -> u32 {
        self.rotation.0
    }

    /// The hex the view pivots around.
    pub fn center(&self) -> Hex {
        self.center.0
    }

    /// World translation for a top-face marker on `hex` — the projection every
    /// highlight system was open-coding as a local `face_at` closure.
    pub fn face_at(&self, map: &HexMap, hex: Hex) -> Vec3 {
        HexOverlay::face(hex).translation(map, self.rotation.0, self.center.0)
    }

    /// World-space cursor position, if the cursor is over the window.
    pub fn cursor_world(&self) -> Option<Vec2> {
        if let Some(ScriptedCursor::Pixel(pos)) = self.scripted.as_deref() {
            let (camera, transform) = self.camera.single().ok()?;
            return camera.viewport_to_world_2d(transform, *pos).ok();
        }
        let window = self.windows.single().ok()?;
        let cursor = window.cursor_position()?;
        let (camera, transform) = self.camera.single().ok()?;
        camera.viewport_to_world_2d(transform, cursor).ok()
    }

    /// The tile under the cursor right now.
    ///
    /// A scripted hex is returned directly rather than projected and picked
    /// back, so a script keeps meaning the same tile no matter how the view is
    /// rotated, zoomed or panned. A hex that is not on this map is treated as
    /// off-screen, which is what an out-of-bounds click would do anyway.
    pub fn hovered(&self, map: &HexMap) -> Option<Hex> {
        if let Some(ScriptedCursor::Hex(hex)) = self.scripted.as_deref() {
            return map.get(*hex).map(|_| *hex);
        }
        let world = self.cursor_world()?;
        iso::pick(map, world, self.rotation.0, map.center())
    }
}

/// One of several mutually exclusive `Text` slots in a HUD.
///
/// Bevy needs the `Without` filters to prove that two `&mut Text` queries in
/// the same system cannot alias, which makes the type long enough that clippy
/// asks for a name — and the battle HUD and the overworld HUD both spell out
/// the same shape three times each.
pub type TextSlot<'w, 's, A, B, C> =
    Query<'w, 's, &'static mut Text, (With<A>, Without<B>, Without<C>)>;

/// A hex marker sprite that moves and shows/hides — hover and selection
/// cursors, which must exclude each other for the same aliasing reason.
pub type MarkerQuery<'w, 's, A, B> =
    Query<'w, 's, (&'static mut Transform, &'static mut Visibility), (With<A>, Without<B>)>;

/// Complain, once, if a HUD widget has been spawned more than once.
///
/// Every HUD system reads its widgets with `single_mut()`, which returns `Err`
/// for *both* "not spawned yet" and "spawned twice" — and the systems treat
/// that as "nothing to do". A missing widget is normal for a frame or two
/// during setup; a duplicated one means the HUD has quietly stopped updating
/// and will never start again.
///
/// That exact failure cost a long debugging session: loading a save respawned
/// the world without despawning it, so there were two banners, and the
/// campaign loaded perfectly while appearing frozen on the old day. The fix
/// was elsewhere, but the reason it was slow to find was that nothing said
/// anything.
/// Takes a count rather than the query itself: the widget queries all have
/// different filter types, and threading those through a generic buys nothing
/// over `query.iter().count()` at the call site.
pub fn warn_if_duplicated(count: usize, what: &str, warned: &mut bool) {
    if !*warned && count > 1 {
        *warned = true;
        warn!(
            "{what} exists {count} times; HUD systems read it with single_mut() and will \
             silently stop updating. Something spawned the UI without despawning the old one."
        );
    }
}

/// UI color for a side index.
pub fn side_color(side: u8) -> Color {
    let c = iso::SIDE_COLORS[(side as usize) % iso::SIDE_COLORS.len()];
    Color::srgb_u8(c[0], c[1], c[2])
}
