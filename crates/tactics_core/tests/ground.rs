//! Reading ground (`tactics_core::ground`): the terrain half of the planning
//! design in `PLANNING.md`.
//!
//! Held against the ridge arena because the arena's features are *rules*
//! (`Arena::band`) rather than drawings, so a test can name the crest, the
//! spurs and the two kinds of wood exactly and ask whether the reader found
//! them. Contents: what the ridge reads as; that a plain has no high ground;
//! that a reading is its own mirror image; dead ground and covered routes; and
//! the multi-tile contract — lazy regions, and a reading that is the same
//! across a seam between two maps as it would be on one map that size.

use tactics_core::Hex;
use tactics_core::battle::{BattleState, SideState};
use tactics_core::data::{DataRegistry, MovementClass};
use tactics_core::ground::{
    Area, FeatureKind, Ground, Patch, REGION_RADIUS, TerrainReader, covered_route, dead_ground,
    firing_positions, region_of,
};
use tactics_core::harness::arena::{Arena, RIDGE_ARENA};
use tactics_core::map::Battlefield;

mod common;
use common::registry;

fn ridge(reg: &DataRegistry) -> (&'static Arena, Patch) {
    let arena = &RIDGE_ARENA;
    let map = arena.map().expect("the ridge arena builds");
    (arena, Patch::of(reg, &map.terrain))
}

fn sorted(mut hexes: Vec<Hex>) -> Vec<Hex> {
    hexes.sort_unstable_by_key(|h| (h.x, h.y));
    hexes
}

// --- what the ridge reads as -----------------------------------------------

/// The ridge arena's high ground is its crest and its two spurs, and the
/// reader finds exactly those hexes — the crest first, because it sees most.
///
/// The first draft found vantages by how much *more* a hex sees than its
/// neighbours, and it was noise: standing in a wood blinds a tile, so every
/// hex beside a wood read as prominent and `battle_plains`, which has no
/// hills, came out with seventeen of them. High ground is relief; how much it
/// sees is how it is ranked.
#[test]
fn the_ridge_reads_as_its_crest_and_its_two_spurs() {
    let reg = registry();
    let (arena, patch) = ridge(&reg);
    let mut reader = TerrainReader::new(&reg);
    let features = reader.vantages(&reg, &patch, &Area::of(arena.tiles()));
    assert!(features.iter().all(|f| f.kind == FeatureKind::Vantage));

    let crest = sorted(arena.band(0..=4, 0..=1));
    assert_eq!(
        features.first().map(|f| f.hexes.clone()),
        Some(crest),
        "the crest is the best vantage on the field, all thirteen hexes of it"
    );
    for side in [1, -1] {
        let spur = sorted(
            arena
                .band(0..=10, 7..=8)
                .into_iter()
                .filter(|h| arena.axis_key(*h).1.signum() == side)
                .collect(),
        );
        assert!(
            features.iter().any(|f| f.hexes == spur),
            "the spur on the {side} flank is one feature: {:?}",
            features
                .iter()
                .map(|f| (arena.axis_key(f.anchor), f.hexes.len()))
                .collect::<Vec<_>>()
        );
    }
}

/// Ground with no relief has no vantage — not one per copse, and not one at
/// the edge of the map, where a hex has less world around it to see.
#[test]
fn a_plain_has_no_high_ground() {
    let reg = registry();
    let rows: Vec<String> = (0..12)
        .map(|r| {
            if r % 4 == 0 {
                "ggfgggggfggg"
            } else {
                "gggggggggggg"
            }
            .to_string()
        })
        .collect();
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "plain",
        "palette": { "g": "grass", "f": "forest" },
        "rows": rows,
    }))
    .unwrap();
    let map = Battlefield::from_map_file(&file).unwrap();
    let patch = Patch::of(&reg, &map.terrain);
    let mut reader = TerrainReader::new(&reg);
    let area = Area::of(map.terrain.iter().map(|(h, _)| h));
    assert_eq!(reader.vantages(&reg, &patch, &area), Vec::new());
}

/// A reading is its own mirror image on mirrored ground: every hex reads the
/// same as its reflection under both of the arena's reflections, and the
/// features map onto one another. Every key the reader sorts by is a count,
/// a share or an elevation, with a coordinate only ever last — the invariant
/// that has shipped broken three times in this engine.
#[test]
fn a_reading_is_its_own_mirror_image() {
    let reg = registry();
    let (arena, patch) = ridge(&reg);
    let mut reader = TerrainReader::new(&reg);
    for hex in arena.tiles() {
        let here = reader.tile(&reg, &patch, hex);
        for (name, reflect) in [("mirror", arena.mirror(hex)), ("flip", arena.flip(hex))] {
            assert_eq!(
                reader.tile(&reg, &patch, reflect),
                here,
                "{hex:?} and its {name} read differently"
            );
        }
    }
    let features = reader.vantages(&reg, &patch, &Area::of(arena.tiles()));
    let sets: Vec<Vec<Hex>> = features.iter().map(|f| f.hexes.clone()).collect();
    for f in &features {
        for reflected in [
            sorted(f.hexes.iter().map(|h| arena.mirror(*h)).collect()),
            sorted(f.hexes.iter().map(|h| arena.flip(*h)).collect()),
        ] {
            assert!(
                sets.contains(&reflected),
                "a feature anchored at {:?} has no reflection among the features",
                arena.axis_key(f.anchor)
            );
        }
    }
}

// --- dead ground and covered routes ----------------------------------------

/// Dead ground is relative to who is looking. From the crest, the reverse-slope
/// woods are dead ground — every hex of them, which is what the arena was built
/// for — while the near woods are only partly hidden: the crest sees into
/// their edge and not through them.
#[test]
fn from_the_crest_the_reverse_slope_woods_are_dead_ground() {
    let reg = registry();
    let (arena, patch) = ridge(&reg);
    let crest: Vec<(Hex, u32)> = arena
        .band(0..=4, 0..=1)
        .into_iter()
        .map(|h| (h, 12))
        .collect();
    let reverse = Area::of(arena.band(0..=10, 10..=11));
    assert_eq!(
        dead_ground(&patch, &crest, &reverse),
        reverse.hexes().to_vec(),
        "nothing on the crest sees into the reverse-slope woods"
    );
    let near = Area::of(arena.band(6..=12, 4..=5));
    let hidden = dead_ground(&patch, &crest, &near).len();
    assert!(
        hidden > 0 && hidden < near.len(),
        "the crest sees the edge of the near woods and not through them: {hidden} of {} hidden",
        near.len()
    );
}

/// A covered route trades distance for dead ground: priced for exposure, the
/// way round from the western flank to the far side takes longer and is seen
/// far less by the crest. At a price of zero it is the plain shortest route.
#[test]
fn a_covered_route_trades_distance_for_dead_ground() {
    let reg = registry();
    let (arena, patch) = ridge(&reg);
    let crest: Vec<(Hex, u32)> = arena
        .band(0..=4, 0..=1)
        .into_iter()
        .map(|h| (h, 12))
        .collect();
    let (from, to) = (arena.rel(-9, 5), arena.mirror(arena.rel(-9, 3)));
    let area = Area::of(arena.tiles());
    let route = |price| {
        covered_route(
            &patch,
            MovementClass::Tracked,
            1,
            from,
            to,
            &crest,
            &area,
            price,
        )
        .expect("the far side is reachable")
    };
    let (plain, covered) = (route(0), route(5));
    assert_eq!(plain.path.first(), Some(&from));
    assert_eq!(covered.path.last(), Some(&to));
    assert!(
        covered.exposed < plain.exposed && covered.cost > plain.cost,
        "covered ({} mp, {} seen) should be longer and quieter than plain ({} mp, {} seen)",
        covered.cost,
        covered.exposed,
        plain.cost,
        plain.exposed
    );
}

/// Firing positions onto a piece of ground are ranked by how much of it they
/// see: onto the crest, the best position sees every hex of it.
#[test]
fn the_best_firing_position_onto_the_crest_sees_all_of_it() {
    let reg = registry();
    let (arena, patch) = ridge(&reg);
    let crest = Area::of(arena.band(0..=4, 0..=1));
    let positions = firing_positions(&reg, &patch, &Area::of(arena.tiles()), &crest, 12);
    assert_eq!(
        positions.first().map(|p| p.1),
        Some(crest.len() as u32),
        "somewhere on the field sees the whole crest"
    );
    assert!(
        positions.windows(2).all(|w| w[0].1 >= w[1].1),
        "and they come most-seeing first"
    );
}

// --- more than one map ------------------------------------------------------

/// Asking about one hex reads one region and no more; a region is a fixed
/// tiling of the whole plane, so it is defined for coordinates no map has yet.
#[test]
fn a_reading_is_lazy_and_a_region_is_a_tiling_of_the_plane() {
    let reg = registry();
    let (arena, patch) = ridge(&reg);
    let mut reader = TerrainReader::new(&reg);
    assert_eq!(reader.regions_read(), 0);
    reader.tile(&reg, &patch, arena.centre_hex());
    assert_eq!(reader.regions_read(), 1, "one hex asked, one region read");
    reader.tile(&reg, &patch, arena.centre_hex());
    assert_eq!(reader.regions_read(), 1, "and asked again, read once");

    let far = Hex::new(100_000, -40_000);
    assert!(
        far.distance_to(region_of(far).to_higher_res(REGION_RADIUS)) <= 2 * REGION_RADIUS as i32,
        "a hex nowhere near any map still has a region"
    );
    assert_eq!(
        reader.tile(&reg, &patch, far),
        None,
        "and no reading, not a guess"
    );
}

/// Two maps folded in side by side read as one piece of ground: a hex near
/// the seam counts the tiles across it, and a reader that had already read
/// that hex before the second map arrived gives the same answer as a fresh
/// one once it has forgotten what the new tiles could reach.
///
/// This is the contract a world streamed a tile at a time will need, held
/// before there is one.
#[test]
fn two_maps_side_by_side_read_as_one_piece_of_ground() {
    let reg = registry();
    let arena = &RIDGE_ARENA;
    let map = arena.map().unwrap();
    let r = arena.radius as i32;
    let offset = Hex::new(2 * r + 1, -r);

    let mut world = Patch::of(&reg, &map.terrain);
    let single = world.len();
    // A seam: the second map touches the first without overlapping it.
    let seam: Vec<Hex> = map
        .terrain
        .iter()
        .map(|(h, _)| h)
        .filter(|h| {
            h.all_neighbors()
                .iter()
                .any(|n| map.terrain.contains(*n - offset))
        })
        .collect();
    assert!(!seam.is_empty(), "the stage needs the maps to touch");

    let mut reader = TerrainReader::new(&reg);
    let edge = *seam.iter().min_by_key(|h| (h.x, h.y)).unwrap();
    let before = reader.tile(&reg, &world, edge).unwrap();

    world.add(&reg, &map.terrain, offset);
    assert_eq!(world.len(), 2 * single, "the maps do not overlap");
    let fresh = TerrainReader::new(&reg).tile(&reg, &world, edge).unwrap();
    assert!(
        fresh.in_range > before.in_range,
        "a hex at the seam counts the ground across it: {} then {}",
        before.in_range,
        fresh.in_range
    );
    assert_eq!(
        reader.tile(&reg, &world, edge),
        Some(before),
        "a reader that has not been told keeps its old answer, which is why it must be told"
    );
    reader.forget_around(edge);
    assert_eq!(
        reader.tile(&reg, &world, edge),
        Some(fresh),
        "told, it reads the joined ground exactly as a fresh reader does"
    );

    let both = Area::of(arena.tiles().into_iter().flat_map(|h| [h, h + offset]));
    let crests = TerrainReader::new(&reg)
        .vantages(&reg, &world, &both)
        .into_iter()
        .filter(|f| f.hexes.len() == 13)
        .count();
    assert_eq!(crests, 2, "and it finds a crest on each map");
}

/// A battle is ground too, and reads exactly as a patch of the same map does:
/// the reader cannot tell which one it was handed.
#[test]
fn a_battle_and_a_patch_of_its_map_read_alike() {
    let reg = registry();
    let (arena, patch) = ridge(&reg);
    let map = arena.map().unwrap();
    let state = BattleState::from_placements(
        &reg,
        map,
        ["West", "East"]
            .map(|name| SideState {
                name: name.into(),
                ai: None,
            })
            .to_vec(),
        &[],
        &[],
        std::sync::Arc::new(tactics_core::roster::Roster::new()),
        1,
    )
    .expect("an empty battle on the ridge");
    let (mut a, mut b) = (TerrainReader::new(&reg), TerrainReader::new(&reg));
    for hex in arena.tiles() {
        assert_eq!(state.terrain_id(hex), patch.terrain_id(hex));
        assert_eq!(a.tile(&reg, &state, hex), b.tile(&reg, &patch, hex));
    }
}
