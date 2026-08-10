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
    /// The [`FormationDef`] this vehicle belongs to, by id. Absent means the
    /// side's flat pool — which is every unit on every map written before
    /// formations existed, and is exactly today's game.
    #[serde(default)]
    pub formation: Option<String>,
    /// Whether the girl in this vehicle commands the formation. At most one
    /// placement per formation may say so; a formation whose author names
    /// nobody is led by its first-declared member, which is the authorable
    /// rule (declaration order is a chain of seniority a map writer controls)
    /// rather than a hidden one.
    #[serde(default)]
    pub leads: bool,
}

/// Ground a battle is fought *for*, as written in a map file.
///
/// Before this existed the only way to win was to destroy the enemy, which
/// made holding the best cover on the map an unpunishable strategy — and the
/// AI, played well, duly discovered that and stopped advancing. An objective
/// is the thing that makes ground cost something: it is worth points to
/// whoever stands on it, so a side that refuses to leave its start line loses
/// on points to one that walked to the bridge.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectiveSpec {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// The hexes that make up this objective, in `[column, row]` offset
    /// coordinates like every other placement in a map file. Several hexes
    /// because the things worth fighting for — a bridge, a crossroads, a
    /// village — are rarely one tile wide.
    pub at: Vec<[i32; 2]>,
    /// Points collected for it: per round for ground held, per vehicle for
    /// ground driven off.
    #[serde(default = "default_objective_value")]
    pub value: u32,
    #[serde(default)]
    pub kind: ObjectiveKind,
    /// The side this objective belongs to, if only one may use it. Absent
    /// means anybody's — which is what contested ground is, and is why `hold`
    /// leaves it alone. An `exit` almost always names a side: a lane off the
    /// map that either army may use is a lane both armies will use on turn
    /// one.
    #[serde(default)]
    pub side: Option<u8>,
}

fn default_objective_value() -> u32 {
    1
}

/// What a side is supposed to do with an objective.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectiveKind {
    /// Ground worth standing on. Pays its value every round to whoever holds
    /// it. The default, so a map that says nothing means what every map that
    /// predates exits meant.
    #[default]
    Hold,
    /// Ground worth *leaving* by. A vehicle that reaches it drives off the
    /// map — out of the battle, but home rather than burning — and pays its
    /// value once. This is what a withdrawal, a breakthrough and a raid that
    /// means to get away again are all made of.
    Exit,
}

/// An objective with its hexes resolved, as a battle uses it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Objective {
    pub id: String,
    pub name: String,
    pub hexes: Vec<Hex>,
    pub value: u32,
    pub kind: ObjectiveKind,
    pub side: Option<u8>,
}

impl Objective {
    /// Whether `side` is allowed to score this one.
    pub fn open_to(&self, side: u8) -> bool {
        self.side.is_none_or(|s| s == side)
    }
}

impl Objective {
    /// Where to draw the marker, and what the AI steers at: the first hex,
    /// which is the one the map author wrote down first. Deliberately not a
    /// centroid — a centroid of an L-shaped objective can land off it.
    pub fn anchor(&self) -> Hex {
        self.hexes.first().copied().unwrap_or(Hex::ZERO)
    }

    pub fn contains(&self, hex: Hex) -> bool {
        self.hexes.contains(&hex)
    }
}

/// A formation a map declares: a group of units with somebody in charge.
///
/// Formations exist because an order has to be issued by *someone*, to
/// *someone*. Today a side is a flat pool that an all-seeing planner drives
/// unit by unit; a formation is the unit of command that pool is missing —
/// the thing a mission is given to, that a leader can be lost from, and that
/// can be out of contact while the rest of the side is not.
///
/// A map that declares none is one flat pool per side and behaves exactly as
/// it did, which is the additivity rule this project holds every harsh system
/// to. Declaring them is therefore always opt-in, and the checks in
/// [`MapFile::validate_into`] are strict precisely because they are: a
/// formation that references nothing, or that nobody is in, is dead data
/// which will mislead whoever edits the map next.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormationDef {
    pub id: String,
    /// Display name. Empty falls back to the id — see [`Self::display_name`].
    #[serde(default)]
    pub name: String,
    /// Index into the map's `sides` list. Every member placement must agree
    /// with it: a formation spanning two armies is not a chain of command,
    /// it is a typo.
    pub side: u8,
    /// A doctrine of this formation's own, overriding the side's. Two
    /// platoons of one academy may fight differently — a recon screen is not
    /// supposed to behave like the tanks it screens for. Absent means the
    /// side's doctrine, which is what every side has today.
    ///
    /// Unread until missions exist; declared now so the map format does not
    /// have to change again to acquire it.
    #[serde(default)]
    pub doctrine: Option<String>,
}

impl FormationDef {
    /// What to call this formation on screen. The fallback lives here rather
    /// than in each caller so "no name means the id" is one rule in one
    /// place, the same way [`Objective`] resolves its own name at parse time.
    pub fn display_name(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
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
    /// Ground worth fighting for. A map that names none is fought to the
    /// death, exactly as every map was before objectives existed.
    #[serde(default)]
    pub objectives: Vec<ObjectiveSpec>,
    /// Points that win the battle outright. Absent means objectives only
    /// decide a battle that would otherwise be a draw, which is the gentler
    /// rule and the one a map gets by saying nothing.
    #[serde(default)]
    pub victory_score: Option<u32>,
    /// Who answers to whom. A map that declares none is one flat pool per
    /// side, which is every map that predates the chain of command.
    #[serde(default)]
    pub formations: Vec<FormationDef>,
}

/// One tile of a parsed map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tile {
    /// Terrain definition id.
    pub terrain: String,
    /// Elevation level, 0-9.
    pub elevation: i32,
}

/// Serialising a map keyed by [`Hex`].
///
/// JSON object keys must be strings and a `Hex` is a struct, so these are
/// written as a list of pairs instead. The list is sorted by coordinate rather
/// than left in hash order: identical game states should produce identical
/// save files, both so a diff between two saves means something and because
/// this project treats "the same inputs give the same bytes" as a property
/// worth keeping everywhere it is cheap.
pub(crate) mod hex_keyed {
    use hexx::Hex;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::HashMap;

    pub fn serialize<S, V>(map: &HashMap<Hex, V>, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
        V: Serialize,
    {
        let mut pairs: Vec<(&Hex, &V)> = map.iter().collect();
        pairs.sort_by_key(|(h, _)| (h.x, h.y));
        pairs.serialize(s)
    }

    pub fn deserialize<'de, D, V>(d: D) -> Result<HashMap<Hex, V>, D::Error>
    where
        D: Deserializer<'de>,
        V: Deserialize<'de>,
    {
        Ok(Vec::<(Hex, V)>::deserialize(d)?.into_iter().collect())
    }
}

/// A parsed, playable hex map.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HexMap {
    #[serde(with = "hex_keyed")]
    tiles: HashMap<Hex, Tile>,
    /// Ground worth fighting for, in the order the map file declared it.
    ///
    /// These live on the map rather than on the battle because they are
    /// immutable terrain-like facts: which hexes are the bridge does not
    /// change during a fight, only who is standing on them. That split also
    /// means both battle-setup paths — a scenario map and a field battle the
    /// overworld assembles from placements — pick objectives up for free,
    /// since both already carry a `HexMap`.
    ///
    /// `#[serde(default)]` so saves written before objectives existed still
    /// load, as maps without them.
    #[serde(default)]
    objectives: Vec<Objective>,
    #[serde(default)]
    victory_score: Option<u32>,
    /// Formations declared by the map file, in declaration order.
    ///
    /// These ride here for the same reason objectives do, and it is worth
    /// stating because a formation is not a hex and this struct is mostly
    /// hexes. `HexMap` is what *both* battle-setup paths already carry — a
    /// scenario map and a field battle the overworld assembles from
    /// placements — so anything that has to reach both without growing an
    /// argument on every constructor travels here. `victory_score` set the
    /// precedent; this follows it.
    ///
    /// Declaration order is load bearing: it is the order
    /// [`crate::battle::CommandState`] walks, and by the seniority rule it is
    /// also who takes over when a leader is lost. It must never become a hash
    /// order.
    #[serde(default)]
    formations: Vec<FormationDef>,
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
        let objectives = file
            .objectives
            .iter()
            .map(|spec| Objective {
                id: spec.id.clone(),
                name: if spec.name.is_empty() {
                    spec.id.clone()
                } else {
                    spec.name.clone()
                },
                hexes: spec
                    .at
                    .iter()
                    .map(|at| crate::offset_to_hex(at[0], at[1]))
                    .collect(),
                value: spec.value,
                kind: spec.kind,
                side: spec.side,
            })
            .collect();
        Ok(Self {
            tiles,
            objectives,
            victory_score: file.victory_score,
            formations: file.formations.clone(),
        })
    }

    /// Ground worth fighting for, in map-file order. That order is load
    /// bearing: it indexes the battle's control and is walked when scoring,
    /// so it must not become a hash order.
    pub fn objectives(&self) -> &[Objective] {
        &self.objectives
    }

    /// Points that end the battle outright, if this map sets any.
    pub fn victory_score(&self) -> Option<u32> {
        self.victory_score
    }

    /// Formations this map declares, in declaration order — which is the
    /// order a battle resolves them in and the seniority they succeed in.
    pub fn formations(&self) -> &[FormationDef] {
        &self.formations
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

    /// Check that the chain of command a map writes down joins up.
    ///
    /// Every referential mistake here is an error rather than a warning, and
    /// deliberately so: a formation is data about *who obeys whom*, so a typo
    /// does not merely look wrong, it silently leaves a vehicle outside the
    /// chain — indistinguishable, once missions exist, from a crew that was
    /// ordered to sit still. The one exception is an unknown doctrine, which
    /// degrades to the side's exactly the way an unknown doctrine on a side
    /// degrades to the balanced default (see [`crate::ai::resolve_doctrine`]).
    ///
    /// Only the map's own `units` are considered. A unit inside an
    /// [`ArmyPlacement`] belongs to an army rather than to this map's
    /// scenario, and armies grow formations of their own when the campaign
    /// half of the chain of command lands.
    fn validate_formations(&self, registry: &DataRegistry, report: &mut ValidationReport) {
        let mut seen: Vec<&str> = Vec::new();
        for formation in &self.formations {
            if seen.contains(&formation.id.as_str()) {
                report.errors.push(format!(
                    "map `{}`: two formations share the id `{}`",
                    self.id, formation.id
                ));
            }
            seen.push(&formation.id);

            if let Some(doctrine) = &formation.doctrine
                && registry.doctrine(doctrine).is_none()
            {
                report.warnings.push(format!(
                    "map `{}`: formation `{}` asks for unknown doctrine `{doctrine}`; falling \
                     back to its side's",
                    self.id, formation.id
                ));
            }

            let members: Vec<&UnitPlacement> = self
                .units
                .iter()
                .filter(|u| u.formation.as_deref() == Some(formation.id.as_str()))
                .collect();
            if members.is_empty() {
                report.errors.push(format!(
                    "map `{}`: formation `{}` has no members, so nobody can ever be ordered \
                     through it",
                    self.id, formation.id
                ));
            }
            let leaders = members.iter().filter(|u| u.leads).count();
            if leaders > 1 {
                report.errors.push(format!(
                    "map `{}`: formation `{}` has {leaders} placements marked `leads`, and only \
                     one girl can be in command",
                    self.id, formation.id
                ));
            }
            for member in members.iter().filter(|u| u.side != formation.side) {
                report.errors.push(format!(
                    "map `{}`: a side {} unit at [{}, {}] is in formation `{}`, which belongs to \
                     side {}",
                    self.id, member.side, member.at[0], member.at[1], formation.id, formation.side
                ));
            }
        }

        for unit in &self.units {
            match &unit.formation {
                Some(id) if !seen.contains(&id.as_str()) => report.errors.push(format!(
                    "map `{}`: unit at [{}, {}] is in formation `{id}`, which the map does not \
                     declare",
                    self.id, unit.at[0], unit.at[1]
                )),
                // Leading nothing is not a rank. Left as an error rather than
                // ignored because the author plainly meant this crew to be in
                // charge of something and naming what was forgotten.
                None if unit.leads => report.errors.push(format!(
                    "map `{}`: unit at [{}, {}] is marked `leads` but is in no formation",
                    self.id, unit.at[0], unit.at[1]
                )),
                _ => {}
            }
        }
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
        // Objectives decide battles, so a typo in one silently changes the
        // result rather than merely looking wrong. An objective off the map,
        // or one with no hexes at all, can never be held by anybody.
        let mut seen_objectives: Vec<&str> = Vec::new();
        for objective in &self.objectives {
            if seen_objectives.contains(&objective.id.as_str()) {
                report.errors.push(format!(
                    "map `{}`: two objectives share the id `{}`",
                    self.id, objective.id
                ));
            }
            seen_objectives.push(&objective.id);
            if objective.at.is_empty() {
                report.errors.push(format!(
                    "map `{}`: objective `{}` names no hexes, so nobody can ever hold it",
                    self.id, objective.id
                ));
            }
            for at in &objective.at {
                if !map.contains(crate::offset_to_hex(at[0], at[1])) {
                    report.errors.push(format!(
                        "map `{}`: objective `{}` includes [{}, {}], which is outside the map",
                        self.id, objective.id, at[0], at[1]
                    ));
                }
            }
            if let Some(side) = objective.side
                && side as usize >= self.sides.len()
            {
                report.errors.push(format!(
                    "map `{}`: objective `{}` belongs to side {side} but only {} sides are \
                     declared",
                    self.id,
                    objective.id,
                    self.sides.len()
                ));
            }
            // An exit anybody may drive off is a lane both armies leave by on
            // the first round, which is not a battle. Warned rather than
            // refused: a scenario about two forces disengaging from each
            // other is a real thing to want.
            if objective.kind == ObjectiveKind::Exit && objective.side.is_none() {
                report.warnings.push(format!(
                    "map `{}`: exit `{}` names no side, so every side may leave by it",
                    self.id, objective.id
                ));
            }
            // Standing on an exit means taking it, so a unit placed on one
            // drives off the map on the first tick without ever being played.
            if objective.kind == ObjectiveKind::Exit {
                for placement in &self.units {
                    if objective.side.is_none_or(|s| s == placement.side)
                        && objective
                            .at
                            .iter()
                            .any(|at| at[0] == placement.at[0] && at[1] == placement.at[1])
                    {
                        report.errors.push(format!(
                            "map `{}`: a side {} unit is placed on its own exit `{}` at [{}, {}], \
                             so it would leave the battle on the first tick",
                            self.id, placement.side, objective.id, placement.at[0], placement.at[1]
                        ));
                    }
                }
            }
        }
        self.validate_formations(registry, report);
        if self.victory_score.is_some() && self.objectives.is_empty() {
            report.warnings.push(format!(
                "map `{}`: sets `victory_score` but declares no objectives, so no side can \
                 ever score",
                self.id
            ));
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

    /// A formation with no `name` is displayed as its id, so a map author who
    /// only wanted a handle gets a readable one rather than a blank label.
    #[test]
    fn a_formation_without_a_name_is_called_by_its_id() {
        let file: MapFile = serde_json::from_str(
            r##"{
                "id": "test",
                "palette": { "g": "grass" },
                "rows": ["gg"],
                "formations": [
                    { "id": "1st_platoon", "side": 0 },
                    { "id": "2nd_platoon", "name": "2nd Platoon", "side": 0 }
                ]
            }"##,
        )
        .unwrap();
        let map = HexMap::from_map_file(&file).unwrap();
        let names: Vec<&str> = map.formations().iter().map(|f| f.display_name()).collect();
        assert_eq!(names, ["1st_platoon", "2nd Platoon"]);
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
