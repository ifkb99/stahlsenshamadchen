//! The ground, and the two derived grids that answer questions about it, with
//! one owner.
//!
//! WORLD.md, W0.3. A battle used to hold three things — `map`, `sight` and
//! `moves` — that had to agree and were each built from the others by
//! whoever set the battle up. That was fine while a battle's ground was one
//! map loaded once; it is not fine for a world whose tiles arrive a chunk at a
//! time, where the three have to change together or a crew can drive across
//! ground she cannot see or see across ground that is not there. So there is
//! one type, and **one door** a tile comes in by, [`World::insert`], which
//! writes the tile and resolves it into both grids in the same call.
//!
//! This is what `ground::Patch` was — "the shape a streamed world will have",
//! used by the terrain reader's tests to hold it to working across a seam —
//! promoted to the thing a battle stands on. There is now one implementation
//! of [`Ground`] that owns ground, and a battle answers the four questions by
//! asking it.
//!
//! **Saving.** A world is saved as its tiles and nothing else: the file format
//! is a plain [`HexMap`], exactly what a battle's `map` was. The grids are
//! derived from the tiles and the terrain definitions, so a world comes off
//! disk *unbuilt* and [`crate::battle::SavedBattle::rehydrate`] rebuilds it
//! through [`World::rebuilt`] — which destructures this struct by name, so a
//! derived structure added here tomorrow does not compile until somebody has
//! said how it is rebuilt.

use crate::battle::{MoveGrid, SightGrid};
use crate::data::{DataRegistry, MovementClass};
use crate::ground::Ground;
use crate::map::{HexMap, Tile};
use hexx::Hex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeSet;

/// The chunk a hex belongs to, for chunks of `radius`: which campaign hex it
/// lies under (WORLD.md, W1.1).
///
/// The tiling is `hexx`'s hexagons-of-hexagons — the same one
/// [`crate::ground::region_of`] reads terrain in — so a chunk of radius 20 is
/// the 1261-tile hexagon a battle map already is, and a campaign hex is
/// exactly one of them. The chunk lattice is turned about 29° against the
/// tile grid (the centre of chunk `(1, 0)` is tile `(2r + 1, -r)`), so
/// "east" on the campaign map is not a tile direction: nothing that decides
/// anything may read a chunk coordinate ahead of a real key, the same rule
/// every other coordinate lives under.
///
/// Written in integers rather than through `Hex::to_lower_res`, which
/// divides in `f32`. That is exact at every size this game will use, but
/// determinism is load-bearing and `div_euclid` does not need an argument;
/// `a_chunk_is_the_chunk_hexx_would_name` holds the two to the same answer.
pub fn chunk_of(hex: Hex, radius: u32) -> Hex {
    let [x, y, z] = hex.to_cubic_array();
    let area = Hex::range_count(radius) as i32;
    let shift = 3 * radius as i32 + 2;
    let a = (y + shift * x).div_euclid(area);
    let b = (z + shift * y).div_euclid(area);
    let c = (x + shift * z).div_euclid(area);
    Hex::new((1 + a - b).div_euclid(3), (1 + b - c).div_euclid(3))
}

/// The tile at the centre of `chunk`.
pub fn chunk_centre(chunk: Hex, radius: u32) -> Hex {
    chunk.to_higher_res(radius)
}

/// Every tile in `chunk`, ring by ring out from its centre — a pure function
/// of coordinates, so the order is the same on every machine.
pub fn chunk_hexes(chunk: Hex, radius: u32) -> impl Iterator<Item = Hex> {
    chunk_centre(chunk, radius).spiral_range(0..=radius)
}

/// What a world can say about a hex.
///
/// Three answers where there used to be two (WORLD.md, W0.4). A lookup that
/// finds nothing meant "not part of the world" while the world was one map,
/// and a sight line rightly treats that as open sky — the ray is passing
/// over the edge of the known world. Once the world is a window onto
/// something larger, "nothing here" can also mean "not loaded", and open sky
/// is then a wrong answer: a ridge in an unloaded chunk would stop blocking,
/// and whether a crew can see a tank would depend on what was paged in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence<'a> {
    /// A tile is known here.
    Known(Tile<'a>),
    /// There is no ground here and never will be: past the edge of the
    /// world, or a gap a map declares.
    Outside,
    /// There is ground here, but its chunk is not resident. Nothing that
    /// decides anything may read this; the residency rule (W1.5) is what
    /// keeps it from being asked.
    Unloaded,
}

/// Which of the world's ground is held in memory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
enum Residency {
    /// All of it: a closed map, whose every tile was loaded with it. A hex
    /// with no tile is outside. Every battle is one of these today.
    #[default]
    Whole,
    /// A chunk at a time. A hex whose chunk is resident but holds no tile is
    /// outside; a hex whose chunk is not resident is unloaded.
    Chunks {
        radius: u32,
        /// Resident chunks as `(x, y)`, because `Hex` has no order and a set
        /// walked in hash order would put the compass in the load order.
        resident: BTreeSet<(i32, i32)>,
    },
}

/// Tiles, and what they mean to a crew's eyes and to her tracks.
#[derive(Debug, Clone, Default)]
pub struct World {
    tiles: HexMap,
    sight: SightGrid,
    moves: MoveGrid,
    residency: Residency,
}

impl World {
    /// A world made of one map, where it lies.
    pub fn build(registry: &DataRegistry, map: HexMap) -> Self {
        let mut world = Self::default();
        world.add(registry, &map, Hex::ZERO);
        world
    }

    /// Fold a map in, shifted by `offset`. A tile already known at a hex is
    /// replaced, as a streamed region replaces what was known of it.
    pub fn add(&mut self, registry: &DataRegistry, map: &HexMap, offset: Hex) {
        for (hex, tile) in map.iter() {
            self.insert(registry, hex + offset, tile.terrain, tile.elevation);
        }
    }

    /// Put one tile into the world and resolve it into both grids. The one
    /// door ground comes in by, so the tiles and what is derived from them
    /// cannot disagree.
    pub fn insert(&mut self, registry: &DataRegistry, hex: Hex, terrain: &str, elevation: i32) {
        let tile = Tile { terrain, elevation };
        self.sight.insert(registry, hex, tile);
        self.moves.insert(registry, hex, tile);
        self.tiles.insert(hex, terrain, elevation);
    }

    /// Rebuild everything derived from the tiles, as a world that came off
    /// disk needs. Destructures by name so that a cache added to [`World`]
    /// stops this compiling until it is rebuilt here too.
    pub fn rebuilt(self, registry: &DataRegistry) -> Self {
        let World {
            tiles,
            sight: _,
            moves: _,
            residency,
        } = self;
        Self {
            residency,
            ..Self::build(registry, tiles)
        }
    }

    /// An empty world held a chunk at a time, chunks of `radius`.
    pub fn chunked(radius: u32) -> Self {
        Self {
            residency: Residency::Chunks {
                radius,
                resident: BTreeSet::new(),
            },
            ..Self::default()
        }
    }

    /// Load one chunk's ground: every tile `tiles` yields that lies in
    /// `chunk`, and the chunk is resident afterwards whether or not it had any
    /// (a chunk past the edge of the world is resident and empty, which is how
    /// the world says *outside* rather than *unloaded*).
    ///
    /// # Panics
    ///
    /// On a world that is not chunked, or on a tile outside `chunk` — ground
    /// arriving under the wrong campaign hex is a generator bug, and folding
    /// it in would make residency lie.
    pub fn load_chunk<'a>(
        &mut self,
        registry: &DataRegistry,
        chunk: Hex,
        tiles: impl IntoIterator<Item = (Hex, &'a str, i32)>,
    ) {
        let Residency::Chunks { radius, .. } = self.residency else {
            panic!("load_chunk on a world that is not held a chunk at a time");
        };
        for (hex, terrain, elevation) in tiles {
            assert_eq!(
                chunk_of(hex, radius),
                chunk,
                "tile {hex:?} does not lie in chunk {chunk:?}"
            );
            self.insert(registry, hex, terrain, elevation);
        }
        if let Residency::Chunks { resident, .. } = &mut self.residency {
            resident.insert((chunk.x, chunk.y));
        }
    }

    /// Forget one chunk's ground; its hexes read as unloaded afterwards.
    pub fn unload_chunk(&mut self, chunk: Hex) {
        let Residency::Chunks { radius, resident } = &mut self.residency else {
            panic!("unload_chunk on a world that is not held a chunk at a time");
        };
        let radius = *radius;
        resident.remove(&(chunk.x, chunk.y));
        for hex in chunk_hexes(chunk, radius) {
            self.tiles.remove(hex);
            self.sight.remove(hex);
            self.moves.remove(hex);
        }
    }

    /// The chunks held in memory, in coordinate order; `None` for a world
    /// that is held whole.
    pub fn resident_chunks(&self) -> Option<impl Iterator<Item = Hex> + '_> {
        match &self.residency {
            Residency::Whole => None,
            Residency::Chunks { resident, .. } => {
                Some(resident.iter().map(|&(x, y)| Hex::new(x, y)))
            }
        }
    }

    /// What the world can say about `hex`: see [`Presence`].
    pub fn presence(&self, hex: Hex) -> Presence<'_> {
        if let Some(tile) = self.tiles.get(hex) {
            return Presence::Known(tile);
        }
        match &self.residency {
            Residency::Whole => Presence::Outside,
            Residency::Chunks { radius, resident } => {
                let chunk = chunk_of(hex, *radius);
                if resident.contains(&(chunk.x, chunk.y)) {
                    Presence::Outside
                } else {
                    Presence::Unloaded
                }
            }
        }
    }

    /// Whether a crew at `from` could see a hull at `to`, geometry only.
    ///
    /// The one question the engine asks of the sight grid, asked through the
    /// world so the world can refuse to answer it wrongly. In a debug build
    /// of a chunked world, a line that crosses an unloaded hex panics: the
    /// grid would read that hex as open sky, and the residency rule exists so
    /// that never happens. A whole world pays nothing for the check.
    pub fn sight_clear(&self, from: Hex, to: Hex) -> bool {
        #[cfg(debug_assertions)]
        if matches!(self.residency, Residency::Chunks { .. }) {
            for hex in from.line_to(to) {
                assert!(
                    self.presence(hex) != Presence::Unloaded,
                    "sight line {from:?} -> {to:?} crosses {hex:?}, which is not loaded"
                );
            }
        }
        self.sight.clear(from, to)
    }

    /// Whether the derived grids hold anything — false for a world that has
    /// tiles and came off disk, and for the empty world.
    pub fn is_built(&self) -> bool {
        self.tiles.is_empty() || (!self.sight.is_empty() && !self.moves.is_empty())
    }

    /// The tiles themselves.
    pub fn tiles(&self) -> &HexMap {
        &self.tiles
    }

    /// Line of sight, resolved per tile.
    pub fn sight(&self) -> &SightGrid {
        &self.sight
    }

    /// Step costs, resolved per tile.
    pub fn moves(&self) -> &MoveGrid {
        &self.moves
    }

    /// The tile at `hex`, if the world has one there.
    pub fn get(&self, hex: Hex) -> Option<Tile<'_>> {
        self.tiles.get(hex)
    }

    /// Whether the world has a tile at `hex`.
    pub fn contains(&self, hex: Hex) -> bool {
        self.tiles.contains(hex)
    }

    /// Every tile, in no particular order: sort before letting the order
    /// reach anything a battle decides.
    pub fn iter(&self) -> impl Iterator<Item = (Hex, Tile<'_>)> {
        self.tiles.iter()
    }

    /// How many tiles are known.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// Whether nothing is known yet.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Centre of mass of the known tiles; see [`HexMap::center`].
    pub fn center(&self) -> Hex {
        self.tiles.center()
    }
}

impl Ground for World {
    fn terrain_id(&self, hex: Hex) -> Option<&str> {
        self.tiles.get(hex).map(|t| t.terrain)
    }
    fn elevation(&self, hex: Hex) -> Option<i32> {
        self.tiles.get(hex).map(|t| t.elevation)
    }
    fn sight_clear(&self, from: Hex, to: Hex) -> bool {
        World::sight_clear(self, from, to)
    }
    fn step_cost(&self, class: MovementClass, max_climb: i32, from: Hex, to: Hex) -> Option<u32> {
        self.moves.cost(class, max_climb, from, to)
    }
}

/// Saved as its tiles: the same bytes a battle's `map` always was.
///
/// Only a whole world can be saved this way. A chunked one would come back
/// as a whole one holding whatever happened to be resident — every hex it
/// had not loaded suddenly *outside* — so it is refused until the world's
/// own save format exists (W1.6: a seed, the skeleton and an edit overlay).
impl Serialize for World {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.residency != Residency::Whole {
            return Err(serde::ser::Error::custom(
                "a world held a chunk at a time is not saved as its tiles",
            ));
        }
        self.tiles.serialize(s)
    }
}

/// Loaded as tiles with nothing derived yet; [`World::rebuilt`] finishes it.
impl<'de> Deserialize<'de> for World {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self {
            tiles: HexMap::deserialize(d)?,
            ..Self::default()
        })
    }
}

/// Equal when the ground is: the grids are functions of it.
impl PartialEq for World {
    fn eq(&self, other: &Self) -> bool {
        self.tiles == other.tiles
    }
}
