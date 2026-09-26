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

/// Tiles, and what they mean to a crew's eyes and to her tracks.
#[derive(Debug, Clone, Default)]
pub struct World {
    tiles: HexMap,
    sight: SightGrid,
    moves: MoveGrid,
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
        } = self;
        Self::build(registry, tiles)
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
        self.sight.clear(from, to)
    }
    fn step_cost(&self, class: MovementClass, max_climb: i32, from: Hex, to: Hex) -> Option<u32> {
        self.moves.cost(class, max_climb, from, to)
    }
}

/// Saved as its tiles: the same bytes a battle's `map` always was.
impl Serialize for World {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
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
