//! The CDDA-inspired hex map format and its runtime representation.
//!
//! Map files are JSON: a palette mapping single-character glyphs to terrain
//! ids, ASCII `rows` of glyphs, and a parallel `elevation` grid of digits.
//! Rows are offset coordinates (see [`crate::offset_to_hex`]); a space in a
//! row means "no tile there", allowing non-rectangular maps.

use crate::ai::AiConfig;
use crate::data::{DataRegistry, ValidationReport};
use hexx::Hex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapKind {
    #[default]
    Battle,
    Overworld,
}

/// What shape a map's tiles are expected to form.
///
/// A battle is fought on the ground an overworld tile depicts, and the
/// overworld draws that tile as a hexagon — so a battle map is a hexagon
/// too, sized by the scale. Shaping it that way is what stops "one overworld
/// hex = one battle map" from being a slogan the rectangle quietly violated
/// by 20%.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapShape {
    /// One overworld tile: the hexagon of [`crate::data::Scale::battle_map_radius`].
    /// The default for battle maps, and checked during validation.
    #[default]
    Tile,
    /// Whatever the rows happen to describe. For scenario maps that are
    /// deliberately not a whole tile, for overworld maps, and for the small
    /// fixtures tests build by hand.
    Free,
}

/// A side participating in the scenario a map describes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SideSpec {
    pub name: String,
    /// `None` = human controlled; otherwise which AI planner runs this side.
    #[serde(default)]
    pub ai: Option<AiConfig>,
    /// Overworld starting funds.
    #[serde(default)]
    pub funds: i32,
}

/// Which way a vehicle is pointing, in compass terms.
///
/// Hexes are pointy-top, so a vehicle faces one of six edges: due east and
/// west, and four diagonals. This exists rather than serialising hexx's
/// `EdgeDirection` directly because that is an index, and `"facing": 4` in a
/// map file tells a modder nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Facing {
    East,
    NorthEast,
    NorthWest,
    West,
    SouthWest,
    SouthEast,
}

impl From<Facing> for hexx::EdgeDirection {
    fn from(facing: Facing) -> Self {
        use hexx::EdgeDirection as D;
        match facing {
            Facing::East => D::POINTY_EAST,
            Facing::NorthEast => D::POINTY_NORTH_EAST,
            Facing::NorthWest => D::POINTY_NORTH_WEST,
            Facing::West => D::POINTY_WEST,
            Facing::SouthWest => D::POINTY_SOUTH_WEST,
            Facing::SouthEast => D::POINTY_SOUTH_EAST,
        }
    }
}

/// A unit placed by a battle map (scenario-style).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitPlacement {
    /// `[column, row]` offset coordinates into the rows grid.
    pub at: [i32; 2],
    /// Index into the map's `sides` list.
    pub side: u8,
    pub vehicle: String,
    #[serde(default)]
    pub crew: Vec<String>,
    /// Display name override; defaults to the first crew member's name.
    #[serde(default)]
    pub name: Option<String>,
    /// Which way this vehicle starts pointing. Absent means "at the enemy",
    /// which is what a unit deployed to fight would do; set it explicitly for
    /// a scenario that wants someone caught looking the wrong way.
    #[serde(default)]
    pub facing: Option<Facing>,
}

/// An army placed by an overworld map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmyPlacement {
    pub at: [i32; 2],
    pub side: u8,
    pub name: String,
    /// Units travelling with this army, spawned into battles it fights.
    pub units: Vec<UnitPlacement>,
    /// Overworld movement points per turn.
    #[serde(default = "default_army_movement")]
    pub movement: u32,
}

fn default_army_movement() -> u32 {
    3
}

/// A map file as stored in `mods/<mod>/maps/*.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MapFile {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub kind: MapKind,
    /// Expected tile shape. Absent means "whatever suits the kind" — see
    /// [`Self::shape`] — so an ordinary field battle says nothing and gets
    /// checked against the scale, while a map that means to be some other
    /// shape says `"free"` and is left alone.
    #[serde(default)]
    pub shape: Option<MapShape>,
    /// Single-character glyph -> terrain id.
    pub palette: HashMap<String, String>,
    pub rows: Vec<String>,
    /// Parallel digit grid, `0`-`9`. Missing/short rows default to 0.
    #[serde(default)]
    pub elevation: Vec<String>,
    #[serde(default)]
    pub sides: Vec<SideSpec>,
    #[serde(default)]
    pub units: Vec<UnitPlacement>,
    #[serde(default)]
    pub armies: Vec<ArmyPlacement>,
}

/// One tile of a parsed map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    /// Terrain definition id.
    pub terrain: String,
    /// Elevation level, 0-9.
    pub elevation: i32,
}

/// A parsed, playable hex map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HexMap {
    tiles: HashMap<Hex, Tile>,
}

#[derive(Debug, thiserror::Error)]
pub enum MapError {
    #[error("map `{map}` row {row}: glyph `{glyph}` not in palette")]
    UnknownGlyph {
        map: String,
        row: usize,
        glyph: char,
    },
    #[error("map `{map}` elevation row {row}: `{glyph}` is not a digit")]
    BadElevation {
        map: String,
        row: usize,
        glyph: char,
    },
    #[error("map `{map}` palette glyph `{glyph}` must be a single character")]
    BadPaletteKey { map: String, glyph: String },
}

impl HexMap {
    pub fn from_map_file(file: &MapFile) -> Result<Self, MapError> {
        let palette = file.parsed_palette()?;
        let mut tiles = HashMap::new();
        for (row, line) in file.rows.iter().enumerate() {
            let elev_line = file.elevation.get(row).map(String::as_str).unwrap_or("");
            let mut elev_chars = elev_line.chars();
            for (col, glyph) in line.chars().enumerate() {
                let elev_char = elev_chars.next().unwrap_or('0');
                if glyph == ' ' {
                    continue;
                }
                let terrain = palette.get(&glyph).ok_or(MapError::UnknownGlyph {
                    map: file.id.clone(),
                    row,
                    glyph,
                })?;
                let elevation = match elev_char {
                    ' ' => 0,
                    c => c.to_digit(10).ok_or(MapError::BadElevation {
                        map: file.id.clone(),
                        row,
                        glyph: c,
                    })? as i32,
                };
                tiles.insert(
                    crate::offset_to_hex(col as i32, row as i32),
                    Tile {
                        terrain: terrain.clone(),
                        elevation,
                    },
                );
            }
        }
        Ok(Self { tiles })
    }

    pub fn get(&self, hex: Hex) -> Option<&Tile> {
        self.tiles.get(&hex)
    }

    pub fn contains(&self, hex: Hex) -> bool {
        self.tiles.contains_key(&hex)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Hex, &Tile)> {
        self.tiles.iter().map(|(h, t)| (*h, t))
    }

    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Centre of mass, used as the pivot for view rotation.
    ///
    /// Averaged in floating point and rounded through [`Hex::round`], which
    /// respects the cube constraint `x + y + z == 0`. The previous version
    /// averaged `x` and `y` with integer division and ignored `z`, which was
    /// merely cosmetic on a rectangle and is not on a hexagon: truncating
    /// each axis independently can name a hex that is not the centre and, on
    /// a sparse map, is not on the map at all.
    pub fn center(&self) -> Hex {
        if self.tiles.is_empty() {
            return Hex::ZERO;
        }
        let (sx, sy) = self.tiles.keys().fold((0i64, 0i64), |(sx, sy), h| {
            (sx + h.x as i64, sy + h.y as i64)
        });
        let n = self.tiles.len() as f32;
        Hex::round([sx as f32 / n, sy as f32 / n])
    }
}

impl MapFile {
    fn parsed_palette(&self) -> Result<HashMap<char, String>, MapError> {
        let mut out = HashMap::new();
        for (glyph, terrain) in &self.palette {
            let mut chars = glyph.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return Err(MapError::BadPaletteKey {
                    map: self.id.clone(),
                    glyph: glyph.clone(),
                });
            };
            out.insert(c, terrain.clone());
        }
        Ok(out)
    }

    /// The shape this map's tiles are expected to form.
    ///
    /// A battle map is one overworld tile unless it says otherwise; an
    /// overworld map is a region of them and has no such constraint.
    pub fn shape(&self) -> MapShape {
        self.shape.unwrap_or(match self.kind {
            MapKind::Battle => MapShape::Tile,
            MapKind::Overworld => MapShape::Free,
        })
    }

    /// Check that a tile-shaped map really is the scale's hexagon.
    ///
    /// Both halves matter. A map with the right number of tiles in the wrong
    /// arrangement is still not a tile, and a hexagon of the wrong radius is
    /// the exact drift this shape exists to prevent — so this compares the
    /// tile set against the hexagon centred on the map's own centroid rather
    /// than just counting.
    fn validate_shape(&self, map: &HexMap, registry: &DataRegistry, report: &mut ValidationReport) {
        if self.shape() != MapShape::Tile {
            return;
        }
        let radius = registry.scale.battle_map_radius();
        let expected: std::collections::HashSet<Hex> = map.center().range(radius).collect();
        let actual: std::collections::HashSet<Hex> = map.iter().map(|(h, _)| h).collect();
        if actual == expected {
            return;
        }
        let missing = expected.difference(&actual).count();
        let extra = actual.difference(&expected).count();
        report.errors.push(format!(
            "map `{}` is a battle map, so its tiles must form the hexagon of one overworld tile \
             (radius {radius}, {} tiles at the current scale) — it has {} tiles, {missing} short \
             and {extra} outside. Set `\"shape\": \"free\"` if it is deliberately not a whole tile.",
            self.id,
            registry.scale.battle_map_tiles(),
            map.len(),
        ));
    }

    /// Validate this map against loaded terrain/vehicle/character defs.
    pub fn validate_into(&self, registry: &DataRegistry, report: &mut ValidationReport) {
        let palette = match self.parsed_palette() {
            Ok(p) => p,
            Err(e) => {
                report.errors.push(e.to_string());
                return;
            }
        };
        for terrain in palette.values() {
            if registry.terrain(terrain).is_none() {
                report.errors.push(format!(
                    "map `{}`: palette references missing terrain `{}`",
                    self.id, terrain
                ));
            }
        }
        let map = match HexMap::from_map_file(self) {
            Ok(m) => m,
            Err(e) => {
                report.errors.push(e.to_string());
                return;
            }
        };
        if map.is_empty() {
            report
                .errors
                .push(format!("map `{}` has no tiles", self.id));
        }
        self.validate_shape(&map, registry, report);
        for (i, row) in self.elevation.iter().enumerate() {
            match self.rows.get(i) {
                Some(r) if r.chars().count() == row.chars().count() => {}
                _ => report.warnings.push(format!(
                    "map `{}`: elevation row {} does not match rows grid",
                    self.id, i
                )),
            }
        }
        if self.elevation.len() > self.rows.len() {
            report.warnings.push(format!(
                "map `{}`: more elevation rows than terrain rows",
                self.id
            ));
        }
        for (i, side) in self.sides.iter().enumerate() {
            let Some(ai) = &side.ai else { continue };
            // A doctrine that does not exist would silently become the
            // balanced default, quietly discarding the side's character.
            if let Some(doctrine) = &ai.doctrine
                && registry.doctrine(doctrine).is_none()
            {
                report.errors.push(format!(
                    "map `{}`: side {i} references missing doctrine `{doctrine}`",
                    self.id
                ));
            }
            // Planners are Rust, not data, so an unknown name is a typo. It
            // degrades to the utility planner rather than failing the load.
            if !crate::ai::BUILTIN_PLANNERS.contains(&ai.planner.as_str()) {
                report.warnings.push(format!(
                    "map `{}`: side {i} asks for unknown planner `{}`; falling back to `utility`",
                    self.id, ai.planner
                ));
            }
        }
        /// The four things every placement check needs, bundled so that only
        /// the placement itself varies from call to call.
        struct PlacementCheck<'a> {
            file: &'a MapFile,
            map: &'a HexMap,
            registry: &'a DataRegistry,
            report: &'a mut ValidationReport,
        }

        impl PlacementCheck<'_> {
            fn check(&mut self, at: [i32; 2], vehicle: Option<&str>, crew: &[String], side: u8) {
                let (file, registry) = (self.file, self.registry);
                let report = &mut *self.report;
                let hex = crate::offset_to_hex(at[0], at[1]);
                if !self.map.contains(hex) {
                    report.errors.push(format!(
                        "map `{}`: placement at [{}, {}] is outside the map",
                        file.id, at[0], at[1]
                    ));
                }
                if let Some(vehicle) = vehicle
                    && registry.vehicle(vehicle).is_none()
                {
                    report.errors.push(format!(
                        "map `{}`: placement references missing vehicle `{}`",
                        file.id, vehicle
                    ));
                }
                for c in crew {
                    if registry.character(c).is_none() {
                        report.errors.push(format!(
                            "map `{}`: placement references missing character `{}`",
                            file.id, c
                        ));
                    }
                }
                if side as usize >= file.sides.len() {
                    report.errors.push(format!(
                        "map `{}`: placement references side {} but only {} sides are declared",
                        file.id,
                        side,
                        file.sides.len()
                    ));
                }
            }
        }

        let mut check = PlacementCheck {
            file: self,
            map: &map,
            registry,
            report,
        };
        for u in &self.units {
            check.check(u.at, Some(&u.vehicle), &u.crew, u.side);
        }
        for a in &self.armies {
            check.check(a.at, None, &[], a.side);
            for u in &a.units {
                // KNOWN BUG, preserved deliberately: this passes the army's
                // own hex rather than `u.at`, so a unit's coordinates inside
                // an `ArmyPlacement` are neither validated nor used. Fixing it
                // is a behaviour change with a design question attached —
                // either honour the field or drop it — and it is tracked as
                // such in CLAUDE.md. `frontier.json` currently carries 14 `at`
                // fields that mean nothing because of this.
                check.check(a.at, Some(&u.vehicle), &u.crew, u.side);
            }
        }
    }
}

/// Procedural map generation hook. Implementations produce a [`MapFile`]
/// (not a [`HexMap`]) so generated maps go through the same validation and
/// can be dumped to JSON for inspection or hand-editing.
pub trait MapGenerator {
    fn generate(
        &mut self,
        registry: &crate::data::DataRegistry,
        seed: u64,
    ) -> Result<MapFile, MapError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_file() -> MapFile {
        serde_json::from_str(
            r##"{
                "id": "test",
                "palette": { "g": "grass", "f": "forest" },
                "rows":      ["ggf", "g f", "fgg"],
                "elevation": ["012", "0 0", "000"]
            }"##,
        )
        .unwrap()
    }

    #[test]
    fn parses_rows_and_elevation() {
        let map = HexMap::from_map_file(&sample_file()).unwrap();
        // One tile is a space, so 8 of 9 remain.
        assert_eq!(map.len(), 8);
        let top_right = map.get(crate::offset_to_hex(2, 0)).unwrap();
        assert_eq!(top_right.terrain, "forest");
        assert_eq!(top_right.elevation, 2);
        assert!(!map.contains(crate::offset_to_hex(1, 1)));
    }

    #[test]
    fn offset_roundtrip() {
        for col in 0..6 {
            for row in 0..6 {
                let hex = crate::offset_to_hex(col, row);
                assert_eq!(crate::hex_to_offset(hex), [col, row]);
            }
        }
    }

    #[test]
    fn unknown_glyph_is_an_error() {
        let mut file = sample_file();
        file.rows[0] = "gXf".into();
        assert!(matches!(
            HexMap::from_map_file(&file),
            Err(MapError::UnknownGlyph { glyph: 'X', .. })
        ));
    }

    #[test]
    fn adjacent_offset_cells_are_hex_neighbors() {
        let a = crate::offset_to_hex(2, 2);
        for (dc, dr) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let b = crate::offset_to_hex(2 + dc, 2 + dr);
            assert_eq!(a.distance_to(b), 1, "({dc},{dr}) should be adjacent");
        }
    }
}
