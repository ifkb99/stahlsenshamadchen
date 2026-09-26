//! The world: ground with one owner, held whole or a chunk at a time.
//!
//! WORLD.md W0.3–W0.4 and W1.1. What is pinned here is the geometry a streamed
//! world rests on — which campaign hex a tile lies under, and that the answer
//! is exact — and the three things a world can say about a hex, because the
//! difference between *outside* and *unloaded* is the difference between a
//! sight line over the edge of the map and a sight line through a ridge
//! nobody paged in.

use hexx::Hex;
use tactics_core::battle::BattleState;
use tactics_core::world::{Presence, World, chunk_centre, chunk_hexes, chunk_of};

mod common;
use common::registry;

/// The radius the base mod's scale derives: one campaign hex, 4 km of 100 m
/// tiles.
const R: u32 = 20;

#[test]
fn a_chunk_is_the_chunk_hexx_would_name() {
    // `chunk_of` is integer arithmetic standing in for `to_lower_res`, which
    // divides in f32. They must agree everywhere a world will ever reach, at
    // every radius the game uses: the terrain reader's regions, a campaign
    // hex, and the degenerate one.
    for radius in [0, 1, 4, R] {
        for hex in Hex::ZERO.range(300) {
            assert_eq!(
                chunk_of(hex, radius),
                hex.to_lower_res(radius),
                "{hex:?} at radius {radius}"
            );
        }
    }
}

#[test]
fn chunks_tile_the_plane_with_no_gap_and_no_overlap() {
    // Every tile of a chunk says it belongs to that chunk, a chunk has the
    // 1261 tiles a battle map has, and every tile near the origin is in the
    // chunk it names — so loading chunks can never leave a hole or load a
    // tile twice.
    let chunks: Vec<Hex> = Hex::ZERO.range(3).collect();
    for &chunk in &chunks {
        let tiles: Vec<Hex> = chunk_hexes(chunk, R).collect();
        assert_eq!(tiles.len(), 1261);
        assert_eq!(tiles[0], chunk_centre(chunk, R), "the centre comes first");
        for hex in tiles {
            assert_eq!(chunk_of(hex, R), chunk, "{hex:?} strays out of {chunk:?}");
        }
    }
    for hex in Hex::ZERO.range(60) {
        let chunk = chunk_of(hex, R);
        assert!(
            chunk_hexes(chunk, R).any(|h| h == hex),
            "{hex:?} is in no chunk"
        );
    }
}

#[test]
fn a_world_held_whole_says_outside_past_its_edge_and_never_unloaded() {
    // Every battle today stands on a whole world. A hex off the map is
    // outside — a sight line passing over it is passing over the edge of the
    // known world, and open sky is the right answer there.
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 1).expect("battle");
    let world = &state.world;
    let (inside, _) = world.iter().next().expect("the map has tiles");
    assert!(matches!(world.presence(inside), Presence::Known(_)));
    let far = Hex::new(10_000, -10_000);
    assert_eq!(world.presence(far), Presence::Outside);
    assert!(world.resident_chunks().is_none());
}

fn grass(chunk: Hex) -> Vec<(Hex, &'static str, i32)> {
    chunk_hexes(chunk, R).map(|h| (h, "grass", 0)).collect()
}

#[test]
fn a_chunk_not_loaded_is_unloaded_and_a_chunk_loaded_empty_is_outside() {
    let reg = registry();
    let mut world = World::chunked(R);
    let home = Hex::ZERO;
    let next = Hex::new(1, 0);
    world.load_chunk(&reg, home, grass(home));

    let here = chunk_centre(home, R);
    let there = chunk_centre(next, R);
    assert!(matches!(world.presence(here), Presence::Known(_)));
    assert_eq!(
        world.presence(there),
        Presence::Unloaded,
        "ground nobody has paged in is not the edge of the world"
    );

    // A chunk past the edge of the world is loaded with nothing in it, and
    // that is how the world learns the difference.
    world.load_chunk(&reg, next, Vec::new());
    assert_eq!(world.presence(there), Presence::Outside);
    assert_eq!(
        world.resident_chunks().unwrap().collect::<Vec<_>>(),
        [home, next]
    );
}

#[test]
fn unloading_a_chunk_forgets_its_ground_and_both_grids_forget_it_too() {
    let reg = registry();
    let mut world = World::chunked(R);
    world.load_chunk(&reg, Hex::ZERO, grass(Hex::ZERO));
    world.load_chunk(&reg, Hex::new(1, 0), grass(Hex::new(1, 0)));
    assert_eq!(world.len(), 2 * 1261);
    assert_eq!(world.sight().len(), 2 * 1261);

    world.unload_chunk(Hex::new(1, 0));
    assert_eq!(world.len(), 1261);
    assert_eq!(world.sight().len(), 1261, "the grids go with the tiles");
    assert!(world.moves().len() == 1261);
    assert_eq!(
        world.presence(chunk_centre(Hex::new(1, 0), R)),
        Presence::Unloaded
    );
}

#[test]
#[should_panic(expected = "tile")]
fn ground_arriving_under_the_wrong_campaign_hex_is_refused() {
    let reg = registry();
    let mut world = World::chunked(R);
    world.load_chunk(&reg, Hex::ZERO, grass(Hex::new(1, 0)));
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "not loaded")]
fn a_sight_line_through_ground_nobody_loaded_is_a_loud_failure() {
    // The grid would read the unloaded hexes as open sky. The residency rule
    // keeps the question from being asked; this is what happens if it ever
    // is, in every debug build and every test.
    let reg = registry();
    let mut world = World::chunked(R);
    world.load_chunk(&reg, Hex::ZERO, grass(Hex::ZERO));
    let here = chunk_centre(Hex::ZERO, R);
    let far = chunk_centre(Hex::new(2, 0), R);
    let _ = world.sight_clear(here, far);
}

#[test]
fn a_world_held_a_chunk_at_a_time_is_not_saved_as_if_it_were_whole() {
    // Saved as tiles, it would come back whole with every chunk it had not
    // loaded suddenly outside the world.
    let reg = registry();
    let mut world = World::chunked(R);
    world.load_chunk(&reg, Hex::ZERO, grass(Hex::ZERO));
    assert!(serde_json::to_string(&world).is_err());
}

#[test]
fn a_force_whose_names_do_not_rise_is_refused() {
    // A battle keeps its units in id order and walks them in that order
    // wherever determinism is at stake. Names that do not rise with the
    // placements would make id order a different order from placement
    // order, so the setup refuses them rather than quietly re-sorting.
    use tactics_core::battle::UnitId;
    let reg = registry();
    let count = reg.map("river_crossing").unwrap().units.len();
    let mut names: Vec<UnitId> = (0..count as u32).map(|i| UnitId(10 * i)).collect();
    names.swap(0, 1);
    let err = BattleState::from_map_numbered(&reg, "river_crossing", 1, Some(&names))
        .expect_err("names out of order");
    assert!(err.to_string().contains("rise"), "{err}");
    let short = &names[..count - 1];
    assert!(BattleState::from_map_numbered(&reg, "river_crossing", 1, Some(short)).is_err());
}

// --- dice per engagement (W0.6) ---

use tactics_core::field::Clash;
use tactics_core::overworld::OverworldState;
use tactics_core::save::SaveGame;
use tactics_core::world::{EngagementKey, engagement_seed};

fn key(when: u64) -> EngagementKey {
    EngagementKey {
        when,
        at: Hex::new(3, -2),
        attacker: 0,
        defender: 1,
    }
}

#[test]
fn an_engagement_draws_the_same_dice_on_every_machine_and_every_compiler() {
    // Pinned to the bit. `engagement_seed` is written out rather than going
    // through `std::hash` so that it cannot move under a toolchain upgrade;
    // this is the check that it did not move under an edit either. A change
    // here changes every campaign battle ever fought from a seed.
    assert_eq!(engagement_seed(7, key(2)), engagement_seed(7, key(2)));
    assert_eq!(engagement_seed(7, key(2)), 0x0e78_61b4_7a4c_71c9);
}

#[test]
fn every_part_of_an_engagements_name_changes_its_dice() {
    let base = engagement_seed(7, key(2));
    let mut moved = key(2);
    moved.at = Hex::new(3, -1);
    let mut swapped = key(2);
    (swapped.attacker, swapped.defender) = (1, 0);
    for (what, other) in [
        ("the world", engagement_seed(8, key(2))),
        ("the day", engagement_seed(7, key(3))),
        ("the place", engagement_seed(7, moved)),
        ("who attacked", engagement_seed(7, swapped)),
    ] {
        assert_ne!(base, other, "{what} made no difference");
    }
}

#[test]
fn a_campaign_loaded_from_a_save_fights_the_battle_it_would_have_fought() {
    // Before W0.6 the game seeded a campaign's battles from the wall clock
    // and the harness from how many battles had come before, so neither a
    // save nor a second campaign in the same process met the same fight.
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 11).expect("campaign");
    let (a, b) = (state.armies[0].id, state.armies[1].id);
    let first = Clash::muster(&state, a, b, &[], &[], "river_crossing".into());

    let text = SaveGame::<BattleState>::new(&reg, Some(state.clone()), None)
        .to_json()
        .expect("saves");
    let loaded = SaveGame::from_json(&reg, &text)
        .expect("loads")
        .0
        .overworld
        .expect("the campaign came back");
    let again = Clash::muster(&loaded, a, b, &[], &[], "river_crossing".into());
    assert_eq!(first.seed, again.seed);

    let at = state.army(b).unwrap().pos;
    assert_eq!(
        first.seed,
        engagement_seed(
            11,
            EngagementKey {
                when: state.turn as u64,
                at,
                attacker: a.0,
                defender: b.0,
            }
        ),
        "the fight's dice are its world's seed and its own name, nothing else"
    );
}
