//! A battlefield with nothing to argue about but skill.
//!
//! The skill-gap table needs ground where the only difference between the two
//! ends is who is commanding, so every feature is declared once and reflected
//! through the centre. That was not always true: it used to be written as rows
//! of ASCII with forest at fixed columns — symmetric *as text*, and sheared
//! into asymmetry by the odd-r offset conversion, which gave side A nine
//! forest hexes near its deployment against side B's four and cost about four
//! points of measured win rate.
//!
//! [`assert_arena_is_mirrored`] is why that cannot come back, and it is called
//! by [`arena_map`] itself rather than being left for a caller to remember.

use crate::Hex;
use crate::map::{HexMap, MapFile};

/// Radius of the arena, in hexes.
pub const ARENA_RADIUS: u32 = 10;

/// Where the arena's centre lands in map-file offset coordinates. The map is
/// built around it, so it is the reflection pivot as well as the middle.
pub const ARENA_CENTRE: [i32; 2] = [15, 10];

pub fn arena_centre() -> Hex {
    crate::offset_to_hex(ARENA_CENTRE[0], ARENA_CENTRE[1])
}

/// The point reflection that swaps the arena's two ends.
///
/// One function, used by the terrain, the objectives, the deployment and the
/// test, so there is no second opinion about what "the other side" means.
pub fn arena_mirror(h: Hex) -> Hex {
    arena_centre() * 2 - h
}

/// Declare something on one side and get it on both.
pub fn mirrored(seeds: impl IntoIterator<Item = Hex>) -> Vec<Hex> {
    let mut out = Vec::new();
    for h in seeds {
        out.push(h);
        out.push(arena_mirror(h));
    }
    out.sort_by_key(|h| (h.x, h.y));
    out.dedup();
    out
}

/// The timber: one belt three hexes short of the middle, and its mirror.
///
/// A line of constant axial x rather than a column of the text grid, which is
/// the specific mistake being corrected — a text column is a diagonal in hex
/// space and its two copies are not the same shape.
pub fn arena_woods() -> Vec<Hex> {
    let c = arena_centre();
    mirrored((-3..=3).map(move |k| c + Hex::new(-4, k)))
}

/// Where each side forms up: four hexes seven out from the middle, and their
/// mirrors. Side 0 takes the first of each pair.
pub fn arena_deployment() -> Vec<(Hex, Hex)> {
    let c = arena_centre();
    [-3i32, -1, 1, 3]
        .into_iter()
        .map(move |k| {
            let west = c + Hex::new(-7, k);
            (west, arena_mirror(west))
        })
        .collect()
}

/// The ground worth holding: the middle hex and the two either side of it
/// along one axis, which is a set the reflection maps onto itself.
pub fn arena_objective() -> Vec<Hex> {
    let c = arena_centre();
    vec![c + Hex::new(0, -1), c, c + Hex::new(0, 1)]
}

pub fn arena_map() -> Option<HexMap> {
    let centre = arena_centre();
    let woods: std::collections::HashSet<Hex> = arena_woods().into_iter().collect();

    // A bounding box on the offset grid wide enough for the hexagon. The
    // shear costs half a column per row, so the columns have to stretch by
    // the radius as well as by itself.
    let radius = ARENA_RADIUS as i32;
    let rows: Vec<String> = (0..=ARENA_CENTRE[1] + radius)
        .map(|row| {
            (0..=ARENA_CENTRE[0] + radius + radius / 2 + 1)
                .map(|col| {
                    let hex = crate::offset_to_hex(col, row);
                    // A space is "no tile here", which is how a sparse
                    // `HexMap` spells the outside of a shape.
                    if hex.distance_to(centre) > radius {
                        ' '
                    } else if woods.contains(&hex) {
                        'f'
                    } else {
                        'g'
                    }
                })
                .collect()
        })
        .collect();

    let objective: Vec<[i32; 2]> = arena_objective()
        .into_iter()
        .map(crate::hex_to_offset)
        .collect();
    let file: MapFile = serde_json::from_value(serde_json::json!({
        "id": "skill_arena",
        "kind": "battle",
        "shape": "free",
        "palette": { "g": "grass", "f": "forest" },
        "rows": rows,
        "objectives": [
            {
                "id": "center",
                "name": "The Crossroads",
                "at": objective,
                "value": 2
            }
        ],
        "victory_score": 30,
    }))
    .ok()?;
    let map = HexMap::from_map_file(&file).ok()?;
    assert_arena_is_mirrored(&map);
    Some(map)
}

/// Refuse to hand back an arena that is not the mirror it claims to be.
///
/// A runtime check rather than a test, because the thing being defended is a
/// *claim in a table heading*: `skill gap: ... on a mirrored arena`. The first
/// version of this arena was asymmetric for months — nine forest hexes within
/// six of one deployment against four of the other — and no test failed,
/// because there was no test, because the symmetry was believed rather than
/// asserted. It costs about a microsecond per run of a table that fights
/// hundreds of battles, and every number the skill-gap and brains tables print
/// depends on it being true.
pub fn assert_arena_is_mirrored(map: &HexMap) {
    let tiles: std::collections::HashMap<Hex, &str> =
        map.iter().map(|(h, t)| (h, t.terrain.as_str())).collect();
    for (hex, terrain) in &tiles {
        let mirror = arena_mirror(*hex);
        match tiles.get(&mirror) {
            None => panic!("the arena has {hex:?} but not its mirror {mirror:?}"),
            Some(other) => assert_eq!(
                terrain, other,
                "the arena is '{terrain}' at {hex:?} and '{other}' at its mirror {mirror:?}"
            ),
        }
    }
    for objective in map.objectives() {
        for hex in &objective.hexes {
            assert!(
                objective.hexes.contains(&arena_mirror(*hex)),
                "objective `{}` holds {hex:?} but not its mirror",
                objective.id
            );
        }
    }
    for (west, east) in arena_deployment() {
        assert_eq!(
            arena_mirror(west),
            east,
            "the deployment pair {west:?}/{east:?} are not mirrors"
        );
        assert!(
            tiles.contains_key(&west) && tiles.contains_key(&east),
            "the deployment pair {west:?}/{east:?} is not on the map"
        );
    }
}
