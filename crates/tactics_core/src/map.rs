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
    /// Retired 2026-09-23, and kept only to be warned about: a campaign has
    /// no treasury. Funds were paid by terrain and by campaign scripts and
    /// spent by nothing, so a number here never changed what anybody could
    /// do.
    #[serde(default, rename = "funds", skip_serializing)]
    pub retired_funds: Option<i32>,
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

/// A vehicle travelling with an army on a campaign map.
///
/// Deliberately **not** a [`UnitPlacement`]. A placement is scenario data: it
/// says where on a battlefield a vehicle starts, which way it faces and whose
/// formation it belongs to, and none of those are questions about a vehicle
/// inside an army. An army stands on one overworld hex, its vehicles stand
/// with it, and where each of them deploys is worked out from the battle map
/// when a battle is actually fought.
///
/// It *was* a `UnitPlacement`, and every field the two do not share was dead:
/// `frontier` carried eighteen `at` fields that named hexes the army was not
/// on — some of them not even the same hex twice — and eighteen `side` fields
/// that could have disagreed with the army's and would have been ignored if
/// they had. The validator read them too, and passed the army's hex to the
/// check instead of the unit's, which is how nobody noticed for a year. The
/// two retired fields below survive only so that a map still declaring them
/// is told, the same way `planner.mission_weight` does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmyUnitPlacement {
    pub vehicle: String,
    /// Who crews her, by character id. An empty list is an anonymous crew.
    #[serde(default)]
    pub crew: Vec<String>,
    /// Display name override; defaults to the first crew member's name.
    #[serde(default)]
    pub name: Option<String>,
    /// Retired 2026-09-11, and kept only to be warned about: a vehicle
    /// travelling with an army is at the army's hex.
    #[serde(default, skip_serializing)]
    pub at: Option<[i32; 2]>,
    /// Retired 2026-09-11, and kept only to be warned about: a vehicle
    /// travelling with an army is on the army's side. Nothing ever read this,
    /// so a map that set it to the wrong side was fielding it for the right
    /// one regardless.
    #[serde(default, skip_serializing)]
    pub side: Option<u8>,
}

impl ArmyUnitPlacement {
    /// The retired fields this placement still declares, by name.
    pub fn retired_fields(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.at.is_some() {
            out.push("at");
        }
        if self.side.is_some() {
            out.push("side");
        }
        out
    }
}

/// A unit placed by a battle map (scenario-style).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitPlacement {
    /// `[column, row]` offset coordinates into the rows grid.
    pub at: [i32; 2],
    /// Spawn already riding in the transport placed at these offset
    /// coordinates. The scenario says who starts mounted — the AI plans no
    /// taxi runs of its own yet — and a reference nothing stands on, or
    /// something without lift, degrades to spawning on foot at `at` rather
    /// than to an error: a map should fight even when its author moved the
    /// halftrack and forgot the riders.
    #[serde(default)]
    pub aboard_at: Option<[i32; 2]>,
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
    /// Whether the cadet in this vehicle commands the formation. At most one
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

/// A scenario's stake in a formation: what it costs its side to lose one.
///
/// Decapitation is deliberately map data rather than an engine rule. Losing a
/// commander always degrades a formation — that is succession and the morale
/// hit, and it happens on every map — but whether a battle is *over* because
/// of it is a question about what this battle was for, and only the scenario
/// knows. A raid on a headquarters ends when the headquarters is gone; the
/// same platoon losing the same cadet in a meeting engagement fights on with a
/// new commander. A map that declares none of these behaves exactly as it did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LossCondition {
    /// The side that *loses* if this comes true. Named rather than inferred
    /// from the formation so the declaration reads as what it is — a stake one
    /// army has placed on one of its formations.
    pub side: u8,
    /// The formation whose fate decides it, by [`FormationDef::id`].
    pub formation: String,
    pub when: LossTrigger,
}

/// What has to happen to a formation for its side to have lost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossTrigger {
    /// The cadet who was in command when the battle opened is dead.
    ///
    /// The *founding* leader, not whoever holds the job now: succession
    /// replaces her within a tick, and a condition that tested the current
    /// leader would be satisfied by the successor's death instead, which is a
    /// different scenario. Dead, not merely off the board — a commander who
    /// drove out by an exit lane withdrew, and a withdrawal that ends the
    /// battle in the enemy's favour would make every exit a trap.
    LeaderLost,
    /// The formation no longer exists: every member is dead or gone, and at
    /// least one of them died. The second half is what keeps a clean
    /// withdrawal from being a decapitation — a formation that drove off the
    /// map entire did what it was told, and the map has to be able to say so.
    Wiped,
}

/// Which way across a generated world, for a campaign file to say where a
/// side begins. Directions on the plane, not tile directions: the words a
/// scenario author uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Toward {
    West,
    East,
    North,
    South,
    NorthWest,
    NorthEast,
    SouthWest,
    SouthEast,
}

impl Toward {
    /// The direction as a vector on the plane, `y` growing southward as rows
    /// do.
    pub fn vector(self) -> (f64, f64) {
        let d = std::f64::consts::FRAC_1_SQRT_2;
        match self {
            Self::West => (-1.0, 0.0),
            Self::East => (1.0, 0.0),
            Self::North => (0.0, -1.0),
            Self::South => (0.0, 1.0),
            Self::NorthWest => (-d, -d),
            Self::NorthEast => (d, -d),
            Self::SouthWest => (-d, d),
            Self::SouthEast => (d, d),
        }
    }
}

/// A feature of a generated world an army can be placed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaceFeature {
    Town,
    Factory,
}

/// Where an army begins on a generated world, named by a feature rather
/// than a coordinate (the designer's ruling, 2026-09-26: a scenario names
/// features and the generator resolves them). `rank` 0 is the one of that
/// feature furthest `toward`; 1 the next; and so on. An army whose place
/// another army already holds stands on the nearest free campaign hex.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Place {
    pub feature: PlaceFeature,
    pub toward: Toward,
    #[serde(default)]
    pub rank: u32,
}

/// The world a campaign map is generated rather than drawn from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldSpec {
    /// The world's seed: the same world every time the campaign is started.
    /// `None` is a new world from the campaign's own seed each time.
    #[serde(default)]
    pub seed: Option<u64>,
    /// Rules for this campaign's world, replacing the mod's `worldgen` block
    /// wholesale. `None` is the mod's.
    #[serde(default)]
    pub rules: Option<crate::data::WorldGen>,
}

/// An army placed by an overworld map.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmyPlacement {
    /// Where it stands on a drawn campaign map, in offset coordinates. Unread
    /// on a generated one, which says [`Self::place`] instead.
    #[serde(default)]
    pub at: [i32; 2],
    /// Where it stands on a generated campaign map.
    #[serde(default)]
    pub place: Option<Place>,
    pub side: u8,
    pub name: String,
    /// Units travelling with this army, spawned into battles it fights.
    pub units: Vec<ArmyUnitPlacement>,
    /// Overworld movement points per turn.
    #[serde(default = "default_army_movement")]
    pub movement: u32,
    /// This army carries the side's headquarters.
    ///
    /// Two things read it. The signals net roots here — every other army is
    /// in contact by being within radio reach of this one, or of somebody
    /// who is — and under [`CampaignVictory::decapitation`] losing it loses
    /// the campaign. One flag rather than two because they are one fact: the
    /// army the orders come from is the army whose loss leaves nobody to
    /// give them.
    ///
    /// A side that flags nobody gets its first-declared army, which is what
    /// the net rooted at before the flag existed, so a map written earlier
    /// behaves exactly as it did. Two flags on one side is a warning and the
    /// first wins.
    #[serde(default)]
    pub headquarters: bool,
}

/// What ends a campaign, declared by the overworld map.
///
/// A map that declares none is fought until one side has no armies left,
/// which is every campaign that predates the block, so both halves are
/// additive: an empty `hold` list never fires and `decapitation: false` is
/// the rule's absence. The two halves are independent — a raid campaign
/// might be decapitation alone, a war of position the ground alone — and a
/// map that wants both writes both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CampaignVictory {
    /// Terrain ids whose every tile one side must hold at dawn to win.
    ///
    /// Checked when the day turns over rather than the moment the last tile
    /// is captured, so the side that just lost it always gets a turn to
    /// take it back: "hold" means having kept it, not having reached it.
    /// The terrains named must be `capturable` and must actually be on the
    /// map, both of which `validate-mods` checks, because a hold rule over
    /// ground that does not exist is one nobody can ever satisfy and nobody
    /// would be told why.
    #[serde(default)]
    pub hold: Vec<String>,
    /// How many dawns in a row the ground must be held before it counts.
    /// One — the default, and the rule's plainest reading — ends the
    /// campaign the first morning one side owns every tile; `frontier`
    /// says three, because with two factories on a nine-row map the AI can
    /// take both in four days while a player looks the other way, and a
    /// campaign that ends before its first battle is not a campaign. A
    /// broken streak starts again from nothing.
    #[serde(default = "default_hold_days")]
    pub hold_days: u32,
    /// Losing the army flagged [`ArmyPlacement::headquarters`] loses the
    /// campaign. A side that flagged none cannot be decapitated, and
    /// `validate-mods` says so.
    #[serde(default)]
    pub decapitation: bool,
}

fn default_hold_days() -> u32 {
    1
}

impl Default for CampaignVictory {
    fn default() -> Self {
        Self {
            hold: Vec::new(),
            hold_days: default_hold_days(),
            decapitation: false,
        }
    }
}

impl CampaignVictory {
    /// Whether the map declared any rule at all beyond elimination.
    pub fn is_empty(&self) -> bool {
        self.hold.is_empty() && !self.decapitation
    }
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
    /// Formations whose loss ends the battle. A map that declares none is
    /// fought until somebody is eliminated or the points decide it, exactly
    /// as every map was before commanders could be lost.
    #[serde(default)]
    pub loss_conditions: Vec<LossCondition>,
    /// What ends the campaign fought on this map, for an overworld map. The
    /// campaign's counterpart of `loss_conditions`, and absent for the same
    /// reason those may be: a map that says nothing is fought to elimination,
    /// exactly as every campaign was before there was anything else to say.
    #[serde(default)]
    pub victory: CampaignVictory,
    /// For a campaign map: the world it is generated from, in which case
    /// `rows` and `palette` are empty and every army says `place` rather
    /// than `at` (WORLD.md W2).
    #[serde(default)]
    pub world: Option<WorldSpec>,
}

/// One tile of a map, as anybody reading it sees it: which terrain, and how
/// high.
///
/// A view rather than what the map stores, and `Copy`: the terrain id
/// borrows the map's palette. A map keeps an index into that palette per tile
/// instead of a `String` (WORLD.md, W0.2), because a world is about 160,000
/// tiles and a heap-allocated name on every one of them was most of what the
/// ground cost to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tile<'a> {
    /// Terrain definition id.
    pub terrain: &'a str,
    /// Elevation level, 0-9.
    pub elevation: i32,
}

/// What a map stores for one tile: its terrain as an index into the map's
/// own palette, and its level. Eight bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Cell {
    terrain: u16,
    elevation: i32,
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

/// The ground of a map: tiles, and nothing that is about a fight on them.
///
/// It used to carry the scenario too — objectives, formations, loss
/// conditions and a victory score — because both battle-setup paths already
/// held a `HexMap` and anything riding on it reached both for free. That
/// stopped being true the moment the ground became one world
/// ([WORLD.md](../../../../WORLD.md), W0.1): terrain belongs to the world and
/// is shared by every engagement fought on it, while what a fight is *about*
/// belongs to that fight. The scenario is [`Scenario`] now, and the two
/// travel together only as a [`Battlefield`], at setup.
///
/// **Terrain ids are interned per map, not per registry.** Each map carries
/// its own palette — the ids it uses, in the order it first met them — and a
/// tile holds an index into it. Interning against the registry instead was
/// the obvious design and has the blocker STRUCTURE.md item 8 names: a map is
/// serialised in every save, and serde cannot turn a registry index back into
/// a name without a registry in hand. A palette of the map's own keeps the
/// file self-describing — the names are in it, once each — and needs nothing
/// at load. It is also what folding maps together wants: a world interns
/// into its palette the same way one map does, through [`HexMap::insert`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(try_from = "RawHexMap")]
pub struct HexMap {
    /// Terrain ids this map uses, each once, in the order it met them.
    palette: Vec<String>,
    #[serde(with = "hex_keyed")]
    tiles: HashMap<Hex, Cell>,
}

/// A map as it comes off disk, before its indices have been checked against
/// its palette.
#[derive(Deserialize)]
struct RawHexMap {
    palette: Vec<String>,
    #[serde(with = "hex_keyed")]
    tiles: HashMap<Hex, Cell>,
}

impl TryFrom<RawHexMap> for HexMap {
    type Error = String;

    /// A tile pointing past the end of its palette would panic the first
    /// time anybody asked what it was, which could be many rounds into a
    /// loaded battle; refusing the file names the problem where it is.
    fn try_from(raw: RawHexMap) -> Result<Self, Self::Error> {
        let len = raw.palette.len();
        if let Some((hex, cell)) = raw
            .tiles
            .iter()
            .find(|(_, cell)| cell.terrain as usize >= len)
        {
            return Err(format!(
                "tile {hex:?} names terrain {} of a palette of {len}",
                cell.terrain
            ));
        }
        Ok(Self {
            palette: raw.palette,
            tiles: raw.tiles,
        })
    }
}

/// Two maps are equal when every hex holds the same terrain at the same
/// level. Palette order is how a map happened to be built, not what it is,
/// so it takes no part.
impl PartialEq for HexMap {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().all(|(hex, tile)| other.get(hex) == Some(tile))
    }
}

/// What a fight on some ground is about: the ground worth holding, who
/// answers to whom, and what ends it short of elimination.
///
/// Immutable for the length of a battle, like the terrain, which is why a
/// battle holds it behind an `Arc` — search planners clone the whole state
/// constantly. It is split from [`HexMap`] because the two have different
/// owners once the ground is one world: the world owns the tiles; an
/// engagement, or the campaign that stages it, owns this.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    /// Ground worth fighting for, in the order the map file declared it.
    ///
    /// Declaration order is load bearing: it indexes the battle's control
    /// and is walked when scoring, so it must not become a hash order.
    #[serde(default)]
    objectives: Vec<Objective>,
    #[serde(default)]
    victory_score: Option<u32>,
    /// Formations declared by the map file, in declaration order.
    ///
    /// Declaration order is load bearing: it is the order
    /// [`crate::battle::CommandState`] walks, and by the seniority rule it is
    /// also who takes over when a leader is lost. It must never become a hash
    /// order.
    #[serde(default)]
    formations: Vec<FormationDef>,
    /// Stakes placed on those formations, in declaration order — a fact
    /// about the scenario, fixed before the first shot.
    #[serde(default)]
    loss_conditions: Vec<LossCondition>,
}

/// Ground and a scenario on it, as a battle is set up from them.
///
/// The pair a map file describes, and the one thing both setup paths —
/// [`crate::battle::BattleState::from_map`] and `from_placements` — are
/// handed. It exists only until the battle is built; the battle keeps the two
/// halves apart.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Battlefield {
    pub terrain: HexMap,
    pub scenario: Scenario,
}

impl Battlefield {
    /// Both halves of one map file.
    pub fn from_map_file(file: &MapFile) -> Result<Self, MapError> {
        Ok(Self {
            terrain: HexMap::from_map_file(file)?,
            scenario: Scenario::from_map_file(file),
        })
    }
}

impl Scenario {
    /// A scenario that says who answers to whom and nothing else: no ground
    /// worth fighting for, no stakes beyond elimination. What an engagement
    /// on the world starts from (WORLD.md W3.2).
    pub fn with_formations(formations: Vec<FormationDef>) -> Self {
        Self {
            formations,
            ..Self::default()
        }
    }

    /// This scenario with `objective` added after the ones it has.
    pub fn with_objective(mut self, objective: Objective) -> Self {
        self.objectives.push(objective);
        self
    }

    /// The scenario a map file declares. Infallible: everything that can be
    /// wrong with it is a validation error with a better message, reported by
    /// [`MapFile::validate_into`].
    pub fn from_map_file(file: &MapFile) -> Self {
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
        Self {
            objectives,
            victory_score: file.victory_score,
            formations: file.formations.clone(),
            loss_conditions: file.loss_conditions.clone(),
        }
    }

    /// Ground worth fighting for, in map-file order. That order is load
    /// bearing: it indexes the battle's control and is walked when scoring,
    /// so it must not become a hash order.
    pub fn objectives(&self) -> &[Objective] {
        &self.objectives
    }

    /// Points that end the battle outright, if this scenario sets any.
    pub fn victory_score(&self) -> Option<u32> {
        self.victory_score
    }

    /// Formations this scenario declares, in declaration order — which is
    /// the order a battle resolves them in and the seniority they succeed in.
    pub fn formations(&self) -> &[FormationDef] {
        &self.formations
    }

    /// What ends this battle short of elimination or points: the formations
    /// whose loss a side cannot survive. Empty for every map that says
    /// nothing, which is how a scenario opts out by omission.
    pub fn loss_conditions(&self) -> &[LossCondition] {
        &self.loss_conditions
    }
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
        let mut map = Self::default();
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
                map.insert(
                    crate::offset_to_hex(col as i32, row as i32),
                    terrain,
                    elevation,
                );
            }
        }
        Ok(map)
    }

    /// Put a tile at `hex`, replacing whatever was known there. The terrain
    /// id is interned into this map's palette on first sight.
    ///
    /// # Panics
    ///
    /// If one map is asked to hold more than 65,536 distinct terrains, which
    /// is a mod with a bug rather than a mod.
    pub fn insert(&mut self, hex: Hex, terrain: &str, elevation: i32) {
        let index = match self.palette.iter().position(|t| t == terrain) {
            Some(i) => i,
            None => {
                self.palette.push(terrain.to_string());
                self.palette.len() - 1
            }
        };
        let terrain = u16::try_from(index).expect("more distinct terrains than one map can index");
        self.tiles.insert(hex, Cell { terrain, elevation });
    }

    /// Forget the tile at `hex`, as an unloaded chunk forgets its ground. The
    /// palette keeps the name: it is a handful of strings, and a chunk that
    /// comes back will want it again.
    pub fn remove(&mut self, hex: Hex) {
        self.tiles.remove(&hex);
    }

    fn view(&self, cell: &Cell) -> Tile<'_> {
        Tile {
            terrain: &self.palette[cell.terrain as usize],
            elevation: cell.elevation,
        }
    }

    pub fn get(&self, hex: Hex) -> Option<Tile<'_>> {
        self.tiles.get(&hex).map(|c| self.view(c))
    }

    pub fn contains(&self, hex: Hex) -> bool {
        self.tiles.contains_key(&hex)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Hex, Tile<'_>)> {
        self.tiles.iter().map(|(h, c)| (*h, self.view(c)))
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
                     one cadet can be in command",
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

        // A loss condition decides the battle, so a typo in one does not look
        // wrong, it quietly makes a scenario unwinnable or unlosable. Both
        // checks are errors for that reason: a condition naming a formation
        // that does not exist can never fire, and one naming another side's
        // formation is a stake placed on somebody else's cadets.
        for condition in &self.loss_conditions {
            match self.formations.iter().find(|f| f.id == condition.formation) {
                None => report.errors.push(format!(
                    "map `{}`: side {} would lose with formation `{}`, which the map does not \
                     declare",
                    self.id, condition.side, condition.formation
                )),
                Some(formation) if formation.side != condition.side => report.errors.push(format!(
                    "map `{}`: side {} would lose with formation `{}`, but that formation \
                         belongs to side {}",
                    self.id, condition.side, condition.formation, formation.side
                )),
                Some(_) => {}
            }
        }
    }

    /// Validate this map against loaded terrain/vehicle/character defs.
    pub fn validate_into(&self, registry: &DataRegistry, report: &mut ValidationReport) {
        if self.world.is_some() {
            self.validate_generated(registry, report);
            return;
        }
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
        // A map that declares no elevation at all is flat, which is how most
        // maps say it and must stay silent. A map that declares one has made
        // a promise, and the promise is per *tile*: `from_map_file` reads a
        // character per glyph and defaults anything it runs off the end of to
        // zero, so a grid that stops short does not fail — it silently
        // flattens whatever it did not reach, which is the one shape of this
        // mistake an author cannot see. Checking tiles rather than string
        // lengths is what makes this exact: a row of `rows` may be padded
        // with spaces where there is no tile, and an elevation row that stops
        // before them has promised nothing it did not keep.
        if !self.elevation.is_empty() {
            let mut missing = Vec::new();
            for (row, line) in self.rows.iter().enumerate() {
                let elevations: Vec<char> = self
                    .elevation
                    .get(row)
                    .map(|e| e.chars().collect())
                    .unwrap_or_default();
                for (col, glyph) in line.chars().enumerate() {
                    if glyph == ' ' {
                        continue;
                    }
                    if elevations.get(col).is_none() {
                        missing.push((row, col));
                    }
                }
            }
            if let Some((row, col)) = missing.first() {
                report.errors.push(format!(
                    "map `{}`: the elevation grid stops short of {} tile(s), the first at \
                     [{col}, {row}]; a map that declares elevation must give every tile a \
                     level, and one that is flat should declare none",
                    self.id,
                    missing.len(),
                ));
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
                // The army's hex and the army's side, which is not a
                // shortcut: a vehicle travelling with an army is where the
                // army is and fights for whom the army fights. This used to
                // read `u.side` off a field of its own, beside a `u.at` that
                // named some other hex entirely and that nothing checked or
                // used; both are retired and warned about below.
                check.check(a.at, Some(&u.vehicle), &u.crew, a.side);
            }
        }
        for side in &self.sides {
            if let Some(was) = side.retired_funds {
                report.warnings.push(format!(
                    "map `{}`: side `{}` declares funds {was}, which is no longer read; a \
                     campaign has no treasury",
                    self.id, side.name
                ));
            }
        }
        for army in &self.armies {
            for u in &army.units {
                let retired = u.retired_fields();
                if retired.is_empty() {
                    continue;
                }
                report.warnings.push(format!(
                    "map `{}`: army `{}`'s `{}` declares `{}`; a vehicle travelling with an \
                     army is at the army's hex and on its side, and these are ignored",
                    self.id,
                    army.name,
                    u.vehicle,
                    retired.join("`/`"),
                ));
            }
        }

        self.validate_victory(&map, registry, report);

        // One cadet, one seat. A campaign map is written by hand and it is
        // very easy to spread ten characters over eighteen vehicles without
        // noticing; the campaign drops the second mention and crews that
        // vehicle anonymously, which is the right behaviour and a silent one.
        // Said out loud here so the author finds out at `validate-mods` time
        // rather than by reading a casualty list with the same name on it
        // twice.
        //
        // Sorted before reporting: the warning text must not depend on hash
        // order, or two runs of validation disagree about a file that has not
        // changed.
        let mut seen: std::collections::HashMap<(u8, &str), usize> =
            std::collections::HashMap::new();
        for army in &self.armies {
            for unit in &army.units {
                for cadet in &unit.crew {
                    *seen.entry((army.side, cadet.as_str())).or_default() += 1;
                }
            }
        }
        let mut repeated: Vec<((u8, &str), usize)> =
            seen.into_iter().filter(|(_, n)| *n > 1).collect();
        repeated.sort();
        for ((side, cadet), times) in repeated {
            report.warnings.push(format!(
                "map `{}` names `{cadet}` in {times} of side {side}'s crews; \
                 she can only be in one, so the rest deploy with an anonymous crew",
                self.id
            ));
        }
    }
}

impl MapFile {
    /// A campaign generated rather than drawn: there are no tiles to check
    /// until the world is made, so what is checked is that it can be made,
    /// that every army says where it stands, and that the ending is one
    /// the generated ground can satisfy.
    fn validate_generated(&self, registry: &DataRegistry, report: &mut ValidationReport) {
        if self.kind != MapKind::Overworld {
            report.errors.push(format!(
                "map `{}`: only a campaign map can be generated",
                self.id
            ));
            return;
        }
        let rules = self
            .world
            .as_ref()
            .and_then(|w| w.rules.as_ref())
            .or(registry.worldgen.as_ref());
        let Some(rules) = rules else {
            report.errors.push(format!(
                "map `{}`: generated, and no mod declares a `worldgen` block",
                self.id
            ));
            return;
        };
        for (i, army) in self.armies.iter().enumerate() {
            if army.place.is_none() {
                report.errors.push(format!(
                    "map `{}`: army {i} (`{}`) says no `place`; a generated map has no \
                     coordinates to put it at",
                    self.id, army.name
                ));
            }
            if let Some(place) = army.place
                && place.feature == PlaceFeature::Factory
                && place.rank >= rules.towns.factories
            {
                report.errors.push(format!(
                    "map `{}`: army `{}` stands by factory {}, and the world has {}",
                    self.id, army.name, place.rank, rules.towns.factories
                ));
            }
            if let Some(place) = army.place
                && place.feature == PlaceFeature::Town
                && place.rank >= rules.towns.count
            {
                report.errors.push(format!(
                    "map `{}`: army `{}` stands by town {}, and the world has {}",
                    self.id, army.name, place.rank, rules.towns.count
                ));
            }
            if army.side as usize >= self.sides.len() {
                report.errors.push(format!(
                    "map `{}`: army `{}` is on side {}, and the map declares {}",
                    self.id,
                    army.name,
                    army.side,
                    self.sides.len()
                ));
            }
            for unit in &army.units {
                if registry.vehicle(&unit.vehicle).is_none() {
                    report.errors.push(format!(
                        "map `{}`: army `{}` fields `{}`, which no mod ships",
                        self.id, army.name, unit.vehicle
                    ));
                }
                for cadet in &unit.crew {
                    if registry.character(cadet).is_none() {
                        report.errors.push(format!(
                            "map `{}`: army `{}` is crewed by `{cadet}`, whom no mod declares",
                            self.id, army.name
                        ));
                    }
                }
            }
        }
        let named: Vec<&str> = rules.summary.iter().map(|r| r.terrain.as_str()).collect();
        for terrain in &self.victory.hold {
            if !named.contains(&terrain.as_str()) {
                report.errors.push(format!(
                    "map `{}`: `victory.hold` names `{terrain}`, which the world's summary \
                     never calls a campaign hex",
                    self.id
                ));
            }
        }
    }

    /// The campaign's ending, checked for the two ways it can be written so
    /// that it can never come true.
    ///
    /// Both are errors rather than warnings. A `hold` rule over ground the
    /// map does not have, or over ground nobody can capture, is a campaign
    /// that cannot be won that way and never says so; a `decapitation` rule
    /// on a side with no headquarters is a campaign that cannot be lost that
    /// way, which is the same silence from the other end. Neither has a
    /// reading under which the author got what she asked for.
    fn validate_victory(
        &self,
        map: &HexMap,
        registry: &DataRegistry,
        report: &mut ValidationReport,
    ) {
        let victory = &self.victory;
        if self.kind != MapKind::Overworld {
            if !victory.is_empty() {
                report.warnings.push(format!(
                    "map `{}`: declares `victory`, which only a campaign map reads",
                    self.id
                ));
            }
            return;
        }
        if !victory.hold.is_empty() && victory.hold_days == 0 {
            report.errors.push(format!(
                "map `{}`: `victory.hold_days` is 0; the ground has to be held for at least one night",
                self.id
            ));
        }
        for terrain in &victory.hold {
            match registry.terrain(terrain) {
                None => report.errors.push(format!(
                    "map `{}`: `victory.hold` names terrain `{terrain}`, which no mod ships",
                    self.id
                )),
                Some(t) if !t.capturable => report.errors.push(format!(
                    "map `{}`: `victory.hold` names `{terrain}`, which is not capturable, \
                     so no side could ever hold it",
                    self.id
                )),
                Some(_) if !map.iter().any(|(_, tile)| tile.terrain == terrain) => {
                    report.errors.push(format!(
                        "map `{}`: `victory.hold` names `{terrain}`, and there is none on the map",
                        self.id
                    ))
                }
                Some(_) => {}
            }
        }
        // Headquarters: at most one a side, and one on every side if losing
        // it is meant to matter.
        let mut sides: Vec<u8> = self.armies.iter().map(|a| a.side).collect();
        sides.sort_unstable();
        sides.dedup();
        for side in sides {
            let flagged = self
                .armies
                .iter()
                .filter(|a| a.side == side && a.headquarters)
                .count();
            if flagged > 1 {
                report.warnings.push(format!(
                    "map `{}`: side {side} flags {flagged} armies as headquarters; the first \
                     declared is the one the net roots at",
                    self.id
                ));
            }
            if victory.decapitation && flagged == 0 {
                report.errors.push(format!(
                    "map `{}`: `victory.decapitation` is set and side {side} flags no army as \
                     headquarters, so it can never be decapitated",
                    self.id
                ));
            }
        }
    }
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

    /// What a world of tiles costs to hold is what one tile costs times
    /// about 160,000, and the name was the expensive part: a `String` is 24
    /// bytes before it has allocated anything. The palette index is two.
    #[test]
    fn a_tile_is_stored_in_eight_bytes_whatever_its_terrain_is_called() {
        assert_eq!(std::mem::size_of::<Cell>(), 8);
        let map = HexMap::from_map_file(&sample_file()).unwrap();
        assert_eq!(
            map.palette,
            ["grass", "forest"],
            "each terrain is named once, in the order the map met it"
        );
    }

    /// Folding ground in is how a world will be assembled, so a tile that
    /// replaces another must intern into the palette the map already has
    /// rather than growing a second entry for a name it knows.
    #[test]
    fn a_tile_put_where_another_was_replaces_it_and_reuses_the_name() {
        let mut map = HexMap::from_map_file(&sample_file()).unwrap();
        let hex = crate::offset_to_hex(0, 0);
        map.insert(hex, "forest", 3);
        assert_eq!(
            map.get(hex),
            Some(Tile {
                terrain: "forest",
                elevation: 3
            })
        );
        assert_eq!(map.palette.len(), 2);
        map.insert(hex, "water", 0);
        assert_eq!(map.get(hex).unwrap().terrain, "water");
        assert_eq!(map.palette.len(), 3);
        assert_eq!(map.len(), 8, "a replacement is not a new tile");
    }

    /// Palette order is an accident of construction. Two maps holding the
    /// same terrain at the same level on every hex are the same ground, and a
    /// save written from one must read back equal to it.
    #[test]
    fn two_maps_of_the_same_ground_are_equal_whatever_order_they_were_built_in() {
        let map = HexMap::from_map_file(&sample_file()).unwrap();
        let mut hexes: Vec<(Hex, Tile)> = map.iter().collect();
        hexes.sort_by_key(|(h, t)| (t.terrain, h.x, h.y));
        let mut rebuilt = HexMap::default();
        for (hex, tile) in &hexes {
            rebuilt.insert(*hex, tile.terrain, tile.elevation);
        }
        assert_ne!(
            map.palette, rebuilt.palette,
            "the test builds it the other way round"
        );
        assert_eq!(map, rebuilt);

        let text = serde_json::to_string(&rebuilt).unwrap();
        let back: HexMap = serde_json::from_str(&text).unwrap();
        assert_eq!(back, map);
        rebuilt.insert(hexes[0].0, "water", 0);
        assert_ne!(rebuilt, map);
    }

    /// A tile that points past the end of its palette would panic the first
    /// time anybody asked what it was — possibly rounds into a loaded
    /// battle. The file is refused instead, at the door.
    #[test]
    fn a_saved_map_whose_tile_names_no_terrain_is_refused() {
        let map = HexMap::from_map_file(&sample_file()).unwrap();
        let mut value = serde_json::to_value(&map).unwrap();
        value["palette"].as_array_mut().unwrap().pop();
        let err = serde_json::from_value::<HexMap>(value).unwrap_err();
        assert!(
            err.to_string().contains("palette of 1"),
            "the refusal should say what was wrong: {err}"
        );
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
        let scenario = Scenario::from_map_file(&file);
        let names: Vec<&str> = scenario
            .formations()
            .iter()
            .map(|f| f.display_name())
            .collect();
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
