//! Isometric hex projection, view rotation, tile picking, and generated
//! placeholder art.
//!
//! The presentation is a squashed pointy-top hex grid (Tactics Ogre-style).
//! Rotating the view is a pure coordinate operation: every hex is rotated
//! 60 degrees around the map center before projection, so the same
//! projection math serves all six perspectives.

use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;
use tactics_core::Hex;
use tactics_core::data::{DataRegistry, parse_color};
use tactics_core::hexx;
use tactics_core::map::HexMap;

// Sprites are drawn at their native size, so one texel is one world unit.
// The hex is sized in whole texels — and, because a pointy-top hex tiles at
// exactly its width horizontally and three quarters of its height
// vertically, these values keep every tile centre on a whole texel. That is
// what makes tiles butt together seamlessly instead of showing ragged seams.

/// Width of a hex top face, in texels. Must be even.
pub const HEX_WIDTH: f32 = 64.0;
/// Height of a hex top face, in texels. Must be divisible by 4 so the
/// three-quarter row pitch stays whole.
pub const HEX_HEIGHT: f32 = 40.0;
/// Hex circumradius, derived so the face is exactly `HEX_WIDTH` across.
pub const HEX_SIZE: f32 = HEX_WIDTH / 1.732_050_8;
/// Vertical squash for the isometric look, derived from the face height.
pub const SQUASH: f32 = HEX_HEIGHT / (2.0 * HEX_SIZE);
/// Screen texels per elevation level. Must be even: prism sprites are
/// centred, so half of it becomes a position offset.
pub const ELEV_PX: f32 = 14.0;

/// Current view orientation, in 60-degree steps (0..6).
#[derive(Resource, Default)]
pub struct ViewRotation(pub u32);

/// Pivot for view rotation (the map center), set when a map is spawned.
#[derive(Resource, Default)]
pub struct ViewCenter(pub Hex);

pub fn layout() -> hexx::HexLayout {
    hexx::HexLayout {
        orientation: hexx::HexOrientation::Pointy,
        origin: hexx::Vec2::ZERO,
        // Negative y: map rows grow downward on screen.
        scale: hexx::Vec2::new(HEX_SIZE, -HEX_SIZE * SQUASH),
    }
}

/// Screen position of a tile's top-face center, plus its depth (z).
/// Depth sorts by the tile's *ground* row so raised tiles still occlude
/// correctly, with a small elevation bias for stacks on the same row.
///
/// The position is rounded to a whole texel: sprites sample cleanly only
/// when their centre sits on the texel grid, and a fractional centre makes
/// neighbouring tiles disagree about where their shared edge falls.
pub fn project(hex: Hex, elevation: i32, rotation: u32, center: Hex) -> (Vec2, f32) {
    let (pos, z) = project_exact(hex, elevation, rotation, center);
    (pos.round(), z)
}

/// Unrounded projection, for angle math where sub-texel precision matters.
pub fn project_exact(hex: Hex, elevation: i32, rotation: u32, center: Hex) -> (Vec2, f32) {
    let rotated = hex.rotate_cw_around(center, rotation);
    let ground = layout().hex_to_world_pos(rotated);
    let pos = Vec2::new(ground.x, ground.y + elevation as f32 * ELEV_PX);
    let z = 500.0 - ground.y * 0.02 + elevation as f32 * 0.001;
    (pos, z)
}

/// Inverse of [`project`]: which tile is under this world-space point?
/// Elevated tiles are tested first (front-most wins).
pub fn pick(map: &HexMap, world: Vec2, rotation: u32, center: Hex) -> Option<Hex> {
    let layout = layout();
    for elevation in (0..=9).rev() {
        let ground = hexx::Vec2::new(world.x, world.y - elevation as f32 * ELEV_PX);
        let rotated = layout.world_pos_to_hex(ground);
        let hex = rotated.rotate_ccw_around(center, rotation);
        if map.get(hex).is_some_and(|t| t.elevation == elevation) {
            return Some(hex);
        }
    }
    None
}

/// The screen-space direction a unit facing `dir` points in, given the
/// current view rotation. Used to orient unit sprites.
pub fn facing_angle(pos: Hex, dir: hexx::EdgeDirection, rotation: u32, center: Hex) -> f32 {
    let (from, _) = project_exact(pos, 0, rotation, center);
    let (to, _) = project_exact(pos.neighbor(dir), 0, rotation, center);
    (to - from).to_angle()
}

// --- Placeholder art -----------------------------------------------------

/// Texel size of the hex face sprite. Exactly the face dimensions, with no
/// padding, so adjacent tiles interlock without gaps.
pub fn face_size() -> (u32, u32) {
    (HEX_WIDTH as u32, HEX_HEIGHT as u32)
}

/// Generated placeholder images, keyed so mods with real art can bypass
/// this entirely (terrain defs may name a sprite file instead).
#[derive(Resource, Default)]
pub struct ArtCache {
    /// (terrain id, elevation) -> prism sprite.
    pub tiles: HashMap<(String, i32), Handle<Image>>,
    /// Bare hex face used for highlights and fog, elevation 0.
    pub face: Handle<Image>,
    /// side index -> tank blob sprite, used when a vehicle ships no art.
    pub units: HashMap<u8, Handle<Image>>,
    /// (vehicle id, side index) -> the vehicle's three facing frames,
    /// recoloured for that academy. Absent for vehicles whose definition
    /// names no sprite.
    pub vehicles: HashMap<(String, u8), Vec<Handle<Image>>>,
    /// side index -> army banner sprite.
    pub armies: HashMap<u8, Handle<Image>>,
    /// character/vehicle id -> generated portrait.
    pub portraits: HashMap<String, Handle<Image>>,
}

/// Frames in a vehicle sheet: east, north-east, south-east, left to right.
///
/// Three rather than six because the other three directions are these
/// mirrored — west is east flipped, and so on round the hex. See
/// [`facing_frame`].
pub const VEHICLE_FRAMES: u32 = 3;

impl ArtCache {
    /// This vehicle's art for a side and facing frame, or the generated blob
    /// if it ships none.
    pub fn vehicle_sprite(&self, vehicle: &str, side: u8, frame: usize) -> Option<Handle<Image>> {
        match self.vehicles.get(&(vehicle.to_string(), side)) {
            Some(frames) => frames.get(frame).or_else(|| frames.first()).cloned(),
            None => self.units.get(&side).cloned(),
        }
    }

    /// Whether this vehicle has facing frames at all. Vehicles still on the
    /// generated blob keep the old behaviour of being rotated bodily.
    pub fn has_vehicle_frames(&self, vehicle: &str, side: u8) -> bool {
        self.vehicles.contains_key(&(vehicle.to_string(), side))
    }
}

/// Which sheet frame draws a unit facing `angle` on screen, and whether it
/// needs mirroring.
///
/// The six pointy-top directions land exactly 60 degrees apart, so rounding to
/// the nearest sixth of a turn is exact rather than a nearest-match.
pub fn facing_frame(angle: f32) -> (usize, bool) {
    let sixth = std::f32::consts::FRAC_PI_3;
    match ((angle / sixth).round() as i32).rem_euclid(6) {
        0 => (0, false), // east
        1 => (1, false), // north-east
        2 => (1, true),  // north-west, mirrored
        3 => (0, true),  // west, mirrored
        4 => (2, true),  // south-west, mirrored
        _ => (2, false), // south-east
    }
}

/// The magenta a sprite paints where its academy's colour belongs.
///
/// Pure magenta appears nowhere in the palette these are drawn from, which is
/// what makes it usable as a key. The alternative — tinting the whole sprite —
/// would wash out the muted military colours the art is built on, and the
/// point of the split is that the world stays drab while the sides stay
/// legible.
const TEAM_KEY: [u8; 3] = [255, 0, 255];

/// Decoded RGBA plus its dimensions.
struct RawSprite {
    width: u32,
    height: u32,
    data: Vec<u8>,
}

fn load_vehicle_sprite(asset_path: &str) -> Result<RawSprite, String> {
    // Sprite paths in mod data are relative to `assets/`, like every other
    // asset the game names.
    let full = std::path::Path::new("assets").join(asset_path);
    let img = image::open(&full).map_err(|e| e.to_string())?.to_rgba8();
    if img.width() % VEHICLE_FRAMES != 0 {
        return Err(format!(
            "sheet is {} px wide, which is not divisible into {VEHICLE_FRAMES} frames",
            img.width()
        ));
    }
    Ok(RawSprite {
        width: img.width(),
        height: img.height(),
        data: img.into_raw(),
    })
}

/// Cut one frame out of a sheet and swap the team key for a side's colour.
fn team_recolored(src: &RawSprite, frame: u32, color: [u8; 3]) -> Image {
    let fw = src.width / VEHICLE_FRAMES;
    let mut data = Vec::with_capacity((fw * src.height * 4) as usize);
    let lit = shade(color, 1.15);
    for y in 0..src.height {
        for x in 0..fw {
            let i = (((y * src.width) + frame * fw + x) * 4) as usize;
            let px = &src.data[i..i + 4];
            if px[0] == TEAM_KEY[0] && px[1] == TEAM_KEY[1] && px[2] == TEAM_KEY[2] {
                data.extend_from_slice(&[lit[0], lit[1], lit[2], px[3]]);
            } else {
                data.extend_from_slice(px);
            }
        }
    }
    make_image(fw, src.height, data)
}

fn inside_hex(px: f32, py: f32) -> bool {
    // Point-in-polygon for the squashed pointy-top hex centered at origin.
    // Corners at angle 30 + 60k degrees, radius HEX_SIZE, y scaled by SQUASH.
    let corners: Vec<Vec2> = (0..6)
        .map(|i| {
            let a = std::f32::consts::FRAC_PI_6 + i as f32 * std::f32::consts::FRAC_PI_3;
            Vec2::new(HEX_SIZE * a.cos(), HEX_SIZE * SQUASH * a.sin())
        })
        .collect();
    let p = Vec2::new(px, py);
    let mut sign = 0.0f32;
    for i in 0..6 {
        let a = corners[i];
        let b = corners[(i + 1) % 6];
        let cross = (b - a).perp_dot(p - a);
        if cross.abs() > f32::EPSILON {
            if sign == 0.0 {
                sign = cross.signum();
            } else if cross.signum() != sign {
                return false;
            }
        }
    }
    true
}

fn put(data: &mut [u8], w: u32, x: u32, y: u32, c: [u8; 4]) {
    let i = ((y * w + x) * 4) as usize;
    data[i..i + 4].copy_from_slice(&c);
}

fn shade(c: [u8; 3], f: f32) -> [u8; 4] {
    [
        (c[0] as f32 * f).clamp(0.0, 255.0) as u8,
        (c[1] as f32 * f).clamp(0.0, 255.0) as u8,
        (c[2] as f32 * f).clamp(0.0, 255.0) as u8,
        255,
    ]
}

fn make_image(w: u32, h: u32, data: Vec<u8>) -> Image {
    Image::new(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// A hex prism: colored top face plus `elevation` levels of darker side
/// walls extruded downward, like an FFT tile stack.
pub fn tile_image(color: [u8; 3], elevation: i32) -> Image {
    let (w, face_h) = face_size();
    let extra = (elevation.max(0) as f32 * ELEV_PX).ceil() as u32;
    let h = face_h + extra;
    let mut data = vec![0u8; (w * h * 4) as usize];

    // Rasterize the top face into a mask first, so the outline can be
    // derived from it. Probing the hex shape at fixed offsets instead
    // leaves the outline dotted along the shallow diagonal edges.
    let mut mask = vec![false; (w * face_h) as usize];
    let mut bottom = vec![None::<u32>; w as usize];
    for y in 0..face_h {
        for x in 0..w {
            let px = x as f32 - w as f32 / 2.0 + 0.5;
            let py = face_h as f32 / 2.0 - y as f32 - 0.5;
            if inside_hex(px, py) {
                mask[(y * w + x) as usize] = true;
                bottom[x as usize] = Some(y);
            }
        }
    }
    let filled = |x: i64, y: i64| -> bool {
        x >= 0
            && y >= 0
            && x < w as i64
            && y < face_h as i64
            && mask[(y as u32 * w + x as u32) as usize]
    };
    for y in 0..face_h {
        for x in 0..w {
            if !filled(x as i64, y as i64) {
                continue;
            }
            // A rim texel is one missing an orthogonal neighbour, which
            // gives a uniformly thick outline all the way around.
            let rim = !filled(x as i64 - 1, y as i64)
                || !filled(x as i64 + 1, y as i64)
                || !filled(x as i64, y as i64 - 1)
                || !filled(x as i64, y as i64 + 1);
            put(
                &mut data,
                w,
                x,
                y,
                shade(color, if rim { 0.82 } else { 1.0 }),
            );
        }
    }
    // Extrude walls below the face; left side darker than right for a fake
    // light direction.
    for x in 0..w {
        if let Some(b) = bottom[x as usize] {
            let f = if (x as f32) < w as f32 / 2.0 {
                0.45
            } else {
                0.6
            };
            for y in (b + 1)..(b + 1 + extra).min(h) {
                let stripe = if (y - b) % ELEV_PX as u32 == 0 {
                    0.8
                } else {
                    1.0
                };
                put(&mut data, w, x, y, shade(color, f * stripe));
            }
        }
    }
    make_image(w, h, data)
}

/// Bare hex face in white, tinted at use sites (highlights, fog).
pub fn face_image() -> Image {
    tile_image([255, 255, 255], 0)
}

/// A little tank: hull plus turret and a barrel pointing +x, tinted per
/// side. Sprites rotate to match unit facing.
pub fn unit_image(color: [u8; 3]) -> Image {
    let (w, h) = (44u32, 30u32);
    let mut data = vec![0u8; (w * h * 4) as usize];
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    for y in 0..h {
        for x in 0..w {
            let px = (x as f32 - cx) / (w as f32 * 0.42);
            let py = (y as f32 - cy) / (h as f32 * 0.40);
            let hull = px * px + py * py * 1.6;
            let turret = (px + 0.05) * (px + 0.05) * 4.0 + py * py * 5.0;
            let barrel = px > 0.2 && px < 1.05 && py.abs() < 0.13;
            if barrel {
                put(&mut data, w, x, y, shade(color, 0.35));
            } else if turret < 1.0 {
                put(&mut data, w, x, y, shade(color, 1.25));
            } else if hull < 1.0 {
                let edge = hull > 0.72;
                put(
                    &mut data,
                    w,
                    x,
                    y,
                    shade(color, if edge { 0.5 } else { 0.9 }),
                );
            }
        }
    }
    make_image(w, h, data)
}

/// Army banner for the overworld: a pennant on a pole.
pub fn army_image(color: [u8; 3]) -> Image {
    let (w, h) = (30u32, 40u32);
    let mut data = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let pole = (4..7).contains(&x) && y > 4;
            let flag = x >= 7 && (6..20).contains(&y) && (x as i32 - 7) < (26 - y as i32);
            if pole {
                put(&mut data, w, x, y, [70, 60, 50, 255]);
            } else if flag {
                let edge = !(8..18).contains(&y);
                put(
                    &mut data,
                    w,
                    x,
                    y,
                    shade(color, if edge { 0.7 } else { 1.1 }),
                );
            }
        }
    }
    make_image(w, h, data)
}

/// Deterministic placeholder portrait: tinted background and a simple
/// silhouette, colored from the id hash so every character looks distinct.
pub fn portrait_image(id: &str) -> Image {
    let mut hash: u32 = 2166136261;
    for b in id.bytes() {
        hash = (hash ^ b as u32).wrapping_mul(16777619);
    }
    let bg = [
        96 + (hash & 0x3F) as u8,
        96 + ((hash >> 8) & 0x3F) as u8,
        96 + ((hash >> 16) & 0x3F) as u8,
    ];
    let hair = [
        60 + ((hash >> 6) & 0x7F) as u8,
        40 + ((hash >> 14) & 0x7F) as u8,
        40 + ((hash >> 22) & 0x7F) as u8,
    ];
    let skin = [222, 190, 160];
    let (w, h) = (48u32, 48u32);
    let mut data = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let px = (x as f32 - 24.0) / 11.0;
            let py = (y as f32 - 22.0) / 13.0;
            let head = px * px + py * py < 1.0;
            let fringe = head && py < -0.15;
            let shoulders = y > 36 && (x as i32 - 24).abs() < 18;
            let c = if fringe {
                hair
            } else if head {
                skin
            } else if shoulders {
                shade(hair, 0.8)[..3].try_into().unwrap()
            } else {
                bg
            };
            put(&mut data, w, x, y, [c[0], c[1], c[2], 255]);
        }
    }
    make_image(w, h, data)
}

pub const SIDE_COLORS: [[u8; 3]; 4] = [
    [64, 120, 200], // player blue
    [204, 70, 60],  // enemy red
    [220, 180, 60], // yellow
    [90, 170, 90],  // green
];

impl ArtCache {
    /// Build every image the loaded mods can ask for.
    pub fn build(registry: &DataRegistry, images: &mut Assets<Image>) -> Self {
        let mut cache = Self {
            face: images.add(face_image()),
            ..Default::default()
        };
        for terrain in registry.terrain.values() {
            let color = parse_color(&terrain.color).unwrap_or([128, 128, 128]);
            for elevation in 0..=9 {
                cache.tiles.insert(
                    (terrain.id.clone(), elevation),
                    images.add(tile_image(color, elevation)),
                );
            }
        }
        for (i, color) in SIDE_COLORS.iter().enumerate() {
            cache.units.insert(i as u8, images.add(unit_image(*color)));
            cache.armies.insert(i as u8, images.add(army_image(*color)));
        }
        for vehicle in registry.vehicles.values() {
            let Some(path) = &vehicle.sprite else {
                continue;
            };
            match load_vehicle_sprite(path) {
                Ok(sheet) => {
                    for (i, color) in SIDE_COLORS.iter().enumerate() {
                        let frames = (0..VEHICLE_FRAMES)
                            .map(|f| images.add(team_recolored(&sheet, f, *color)))
                            .collect();
                        cache.vehicles.insert((vehicle.id.clone(), i as u8), frames);
                    }
                }
                // A missing or broken sprite falls back to the generated blob
                // rather than failing the boot: art is the most likely thing
                // for a mod to get wrong, and the game is still playable.
                Err(err) => warn!("vehicle `{}` sprite `{path}`: {err}", vehicle.id),
            }
        }
        for character in registry.characters.values() {
            cache.portraits.insert(
                character.id.clone(),
                images.add(portrait_image(&character.id)),
            );
        }
        for vehicle in registry.vehicles.values() {
            cache
                .portraits
                .insert(vehicle.id.clone(), images.add(portrait_image(&vehicle.id)));
        }
        cache
    }

    pub fn tile(&self, terrain: &str, elevation: i32) -> Handle<Image> {
        self.tiles
            .get(&(terrain.to_string(), elevation))
            .cloned()
            .unwrap_or_else(|| self.face.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// The six pointy-top directions have to land on three frames and a
    /// mirror, with nothing falling between two frames — which is what makes
    /// three drawings enough for six facings.
    #[test]
    fn every_hex_direction_maps_to_a_frame_and_a_flip() {
        let sixth = PI / 3.0;
        let expected = [
            (0, false), // east
            (1, false), // north-east
            (1, true),  // north-west
            (0, true),  // west
            (2, true),  // south-west
            (2, false), // south-east
        ];
        for (i, want) in expected.iter().enumerate() {
            assert_eq!(facing_frame(i as f32 * sixth), *want, "direction {i}");
            // Angles are computed from projected positions, so they arrive
            // with float slop and in either winding; both must round home.
            assert_eq!(facing_frame(i as f32 * sixth + 0.2), *want);
            assert_eq!(facing_frame(i as f32 * sixth - 0.2), *want);
            assert_eq!(facing_frame(i as f32 * sixth - 2.0 * PI), *want);
        }
    }

    /// Each drawing is shared by a pair of directions that mirror about the
    /// vertical axis, which is what makes three frames cover six facings.
    ///
    /// Note those pairs are *mirrors*, not opposites: the mirror of north-east
    /// is north-west, while its opposite is south-west and needs a different
    /// drawing entirely. Getting that backwards was the first version of this
    /// test, and it failed against correct code.
    #[test]
    fn mirrored_directions_share_a_drawing() {
        let sixth = PI / 3.0;
        for (a, b) in [(0, 3), (1, 2), (5, 4)] {
            let (fa, flip_a) = facing_frame(a as f32 * sixth);
            let (fb, flip_b) = facing_frame(b as f32 * sixth);
            assert_eq!(fa, fb, "directions {a} and {b} should share a drawing");
            assert_ne!(flip_a, flip_b, "one of {a}/{b} must be mirrored");
        }
        // ...and opposites genuinely differ, since a vehicle driving away is
        // not a mirror of one driving towards you.
        assert_ne!(facing_frame(sixth), facing_frame(4.0 * sixth));
    }
}
