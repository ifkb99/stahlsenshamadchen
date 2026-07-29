//! Isometric hex projection, view rotation, tile picking, and generated
//! placeholder art.
//!
//! The presentation is a squashed pointy-top hex grid (Tactics Ogre-style).
//! Rotating the view is a pure coordinate operation: every hex is rotated
//! 60 degrees around the map center before projection, so the same
//! projection math serves all six perspectives.

use bevy::prelude::*;
use bevy::asset::RenderAssetUsages;
use bevy::image::Image;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::HashMap;
use tactics_core::data::{parse_color, DataRegistry};
use tactics_core::hexx;
use tactics_core::map::HexMap;
use tactics_core::Hex;

/// Hex circumradius in pixels (before the isometric squash).
pub const HEX_SIZE: f32 = 36.0;
/// Vertical squash factor for the isometric look.
pub const SQUASH: f32 = 0.55;
/// Screen pixels per elevation level.
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
pub fn project(hex: Hex, elevation: i32, rotation: u32, center: Hex) -> (Vec2, f32) {
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
    let (from, _) = project(pos, 0, rotation, center);
    let (to, _) = project(pos.neighbor(dir), 0, rotation, center);
    (to - from).to_angle()
}

// --- Placeholder art -----------------------------------------------------

/// Pixel size of the hex face sprite.
pub fn face_size() -> (u32, u32) {
    let w = (3.0f32.sqrt() * HEX_SIZE).ceil() as u32 + 2;
    let h = (2.0 * HEX_SIZE * SQUASH).ceil() as u32 + 2;
    (w, h)
}

/// Generated placeholder images, keyed so mods with real art can bypass
/// this entirely (terrain defs may name a sprite file instead).
#[derive(Resource, Default)]
pub struct ArtCache {
    /// (terrain id, elevation) -> prism sprite.
    pub tiles: HashMap<(String, i32), Handle<Image>>,
    /// Bare hex face used for highlights and fog, elevation 0.
    pub face: Handle<Image>,
    /// side index -> tank blob sprite.
    pub units: HashMap<u8, Handle<Image>>,
    /// side index -> army banner sprite.
    pub armies: HashMap<u8, Handle<Image>>,
    /// character/vehicle id -> generated portrait.
    pub portraits: HashMap<String, Handle<Image>>,
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

    // Rasterize the top face and record each column's lowest face pixel.
    let mut bottom = vec![None::<u32>; w as usize];
    for y in 0..face_h {
        for x in 0..w {
            let px = x as f32 - w as f32 / 2.0 + 0.5;
            let py = face_h as f32 / 2.0 - y as f32 - 0.5;
            if inside_hex(px, py) {
                // Edge detection for a subtle outline.
                let edge = !inside_hex(px - 1.5, py)
                    || !inside_hex(px + 1.5, py)
                    || !inside_hex(px, py - 1.5)
                    || !inside_hex(px, py + 1.5);
                put(&mut data, w, x, y, shade(color, if edge { 0.72 } else { 1.0 }));
                bottom[x as usize] = Some(y);
            }
        }
    }
    // Extrude walls below the face; left side darker than right for a fake
    // light direction.
    for x in 0..w {
        if let Some(b) = bottom[x as usize] {
            let f = if (x as f32) < w as f32 / 2.0 { 0.45 } else { 0.6 };
            for y in (b + 1)..(b + 1 + extra).min(h) {
                let stripe = if (y - b) % ELEV_PX as u32 == 0 { 0.8 } else { 1.0 };
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
                put(&mut data, w, x, y, shade(color, if edge { 0.5 } else { 0.9 }));
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
            let pole = x >= 4 && x < 7 && y > 4;
            let flag = x >= 7 && y >= 6 && y < 20 && (x as i32 - 7) < (26 - y as i32);
            if pole {
                put(&mut data, w, x, y, [70, 60, 50, 255]);
            } else if flag {
                let edge = y < 8 || y >= 18;
                put(&mut data, w, x, y, shade(color, if edge { 0.7 } else { 1.1 }));
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
    [64, 120, 200],  // player blue
    [204, 70, 60],   // enemy red
    [220, 180, 60],  // yellow
    [90, 170, 90],   // green
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
        for character in registry.characters.values() {
            cache
                .portraits
                .insert(character.id.clone(), images.add(portrait_image(&character.id)));
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
