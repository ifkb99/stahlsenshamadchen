//! Making a world, bottom up (WORLD.md W1.2–W1.4).
//!
//! What is pinned: that a world is a pure function of its seed and rules —
//! the same tiles in any order, on any machine, chunk by chunk or all at
//! once, with no seam where two chunks meet; that its skeleton hangs
//! together (rivers run to the edge, every town is on the road network);
//! that it comes out with the shares its mod asked for; and that a campaign
//! hex is named from the tiles inside it and nothing else.

use hexx::Hex;
use std::collections::{HashMap, HashSet};
use tactics_core::data::{DataRegistry, SkeletonFeature, WorldGen};
use tactics_core::world::{Presence, World, chunk_of};
use tactics_core::worldgen::GeneratedWorld;

mod common;
use common::registry;

fn world(reg: &DataRegistry, seed: u64) -> GeneratedWorld {
    GeneratedWorld::new(reg, seed).expect("the base mod declares a world")
}

/// A smaller world than the base mod ships, so a test that walks every tile
/// stays quick; the rules are otherwise the mod's own.
fn small(reg: &DataRegistry, seed: u64) -> GeneratedWorld {
    let rules = WorldGen {
        radius: 3,
        ..reg.worldgen.clone().expect("the base mod declares a world")
    };
    GeneratedWorld::with_rules(rules, reg.scale.battle_map_radius(), seed).expect("rules are good")
}

#[test]
fn the_same_seed_makes_the_same_world() {
    let reg = registry();
    let (a, b) = (world(&reg, 5), world(&reg, 5));
    assert_eq!(a.skeleton, b.skeleton);
    for chunk in a.chunks().step_by(9) {
        assert_eq!(a.chunk_tiles(chunk), b.chunk_tiles(chunk), "{chunk:?}");
    }
    let c = world(&reg, 6);
    assert_ne!(a.skeleton, c.skeleton, "and another seed makes another");
}

#[test]
fn a_chunk_is_the_same_ground_alone_as_among_its_neighbours() {
    // The tile function has no chunk boundary in it, so there is nothing
    // for a seam to be made of: every tile of a chunk is the tile the world
    // says is at that hex, and loading chunks in any order into a `World`
    // gives the same ground as loading them in another.
    let reg = registry();
    let made = small(&reg, 3);
    let chunks: Vec<Hex> = made.chunks().collect();
    for &chunk in &chunks {
        for (hex, terrain, level) in made.chunk_tiles(chunk) {
            assert_eq!(made.tile(hex), Some((terrain, level)));
        }
    }
    let r = reg.scale.battle_map_radius();
    let mut forward = World::chunked(r);
    let mut backward = World::chunked(r);
    for &c in &chunks {
        forward.load_chunk(&reg, c, made.chunk_tiles(c));
    }
    for &c in chunks.iter().rev() {
        backward.load_chunk(&reg, c, made.chunk_tiles(c));
    }
    assert_eq!(forward, backward);
    assert_eq!(forward.len(), chunks.len() * 1261);
}

#[test]
fn the_world_ends_where_its_radius_says() {
    let reg = registry();
    let made = small(&reg, 3);
    let past = Hex::new(4, 0);
    assert!(made.chunk_tiles(past).is_empty());
    assert!(made.summary(past).is_none());
    let mut w = World::chunked(reg.scale.battle_map_radius());
    w.load_chunk(&reg, past, made.chunk_tiles(past));
    let hex = tactics_core::world::chunk_centre(past, reg.scale.battle_map_radius());
    assert_eq!(
        w.presence(hex),
        Presence::Outside,
        "past the edge is outside, not unloaded"
    );
}

#[test]
fn rivers_run_unbroken_to_the_edge_of_the_world_or_into_another_river() {
    let reg = registry();
    for seed in [1, 2, 3] {
        let made = world(&reg, seed);
        assert!(!made.skeleton.rivers.is_empty(), "seed {seed} has no river");
        let mut laid: HashSet<Hex> = HashSet::new();
        for river in &made.skeleton.rivers {
            for pair in river.windows(2) {
                assert_eq!(pair[0].unsigned_distance_to(pair[1]), 1, "a river jumps");
            }
            let mouth = *river.last().unwrap();
            let on_rim = mouth.all_neighbors().iter().any(|n| !made.contains(*n));
            assert!(
                on_rim || laid.contains(&mouth),
                "seed {seed}: a river ends at {mouth:?}, neither the edge nor another river"
            );
            laid.extend(river.iter().copied());
        }
    }
}

#[test]
fn every_town_is_on_the_road_network() {
    let reg = registry();
    for seed in [1, 2, 3] {
        let made = world(&reg, seed);
        let towns = &made.skeleton.towns;
        assert_eq!(towns.len() as u32, made.rules.towns.count, "seed {seed}");
        // Union the towns each road joins; one component means every town
        // can be driven to from every other.
        let mut parent: Vec<usize> = (0..towns.len()).collect();
        fn root(p: &mut Vec<usize>, i: usize) -> usize {
            if p[i] != i {
                let r = root(p, p[i]);
                p[i] = r;
            }
            p[i]
        }
        let town_at: HashMap<Hex, usize> = towns
            .iter()
            .enumerate()
            .map(|(i, t)| (t.centre, i))
            .collect();
        for road in &made.skeleton.roads {
            for pair in road.windows(2) {
                assert_eq!(pair[0].unsigned_distance_to(pair[1]), 1, "a road jumps");
            }
            let (a, b) = (
                town_at[road.first().unwrap()],
                town_at[road.last().unwrap()],
            );
            let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
            parent[ra] = rb;
        }
        let r0 = root(&mut parent, 0);
        assert!(
            (0..towns.len()).all(|i| root(&mut parent, i) == r0),
            "seed {seed}: a town is cut off"
        );
        assert_eq!(
            towns.iter().filter(|t| t.factory).count() as u32,
            made.rules.towns.factories
        );
    }
}

#[test]
fn a_world_comes_out_with_the_shares_its_mod_asked_for() {
    // The thresholds are quantiles of the world's own noise, so a mod that
    // asks for 28% woodland gets 28% woodland — not whatever fraction of a
    // bell curve lies above a guessed constant.
    let reg = registry();
    let made = world(&reg, 1);
    let rules = made.rules.clone();
    let mut levels = vec![0u64; rules.relief.shares.len()];
    let mut wood = 0u64;
    let mut n = 0u64;
    for chunk in made.chunks() {
        for (_, terrain, level) in made.chunk_tiles(chunk) {
            n += 1;
            if let Some(slot) = levels.get_mut(level as usize) {
                *slot += 1;
            }
            wood += (terrain == rules.terrain.wood) as u64;
        }
    }
    for (level, (&got, &asked)) in levels.iter().zip(&rules.relief.shares).enumerate() {
        let got = 100.0 * got as f64 / n as f64;
        assert!(
            (got - asked as f64).abs() <= 1.5,
            "level {level}: asked {asked}%, got {got:.1}%"
        );
    }
    let got = 100.0 * wood as f64 / n as f64;
    assert!(
        (got - rules.cover.wood_percent as f64).abs() <= 2.0,
        "wood: asked {}%, got {got:.1}%",
        rules.cover.wood_percent
    );
}

#[test]
fn every_terrain_a_world_is_made_of_is_one_the_mods_declare() {
    let reg = registry();
    let made = small(&reg, 4);
    let mut seen: HashSet<String> = HashSet::new();
    for chunk in made.chunks() {
        for (_, terrain, _) in made.chunk_tiles(chunk) {
            seen.insert(terrain.to_string());
        }
        seen.insert(made.summary(chunk).unwrap().terrain);
    }
    for terrain in &seen {
        assert!(
            reg.terrain(terrain).is_some(),
            "`{terrain}` is not declared"
        );
    }
}

#[test]
fn a_campaign_hex_is_named_for_what_its_tiles_hold() {
    // R, in WORLD.md's terms: the summary reads the tiles and the skeleton
    // and the mod's rules, first rule that holds. Every factory town's hex
    // is a factory; every other town's hex is a city; and the summary's
    // elevation is its land's mean level.
    let reg = registry();
    let made = world(&reg, 1);
    let r = made.chunk_radius;
    for town in &made.skeleton.towns {
        let chunk = chunk_of(town.centre, r);
        let named = made.summary(chunk).unwrap().terrain;
        let expect = if made.holds(chunk, SkeletonFeature::Factory) {
            "factory"
        } else {
            "city"
        };
        assert_eq!(named, expect, "the town at {:?}", town.centre);
    }
    for chunk in made.chunks() {
        let tiles = made.chunk_tiles(chunk);
        let land: Vec<i32> = tiles
            .iter()
            .filter(|(_, t, _)| *t != made.rules.terrain.water)
            .map(|(_, _, e)| *e)
            .collect();
        let tenths = land.iter().map(|e| *e as i64).sum::<i64>() * 10 / land.len() as i64;
        assert_eq!(
            made.summary(chunk).unwrap().elevation as i64,
            (tenths + 5) / 10,
            "{chunk:?}"
        );
    }
}

#[test]
fn a_mod_that_declares_no_world_cannot_make_one() {
    let mut reg = registry();
    reg.worldgen = None;
    assert!(GeneratedWorld::new(&reg, 1).is_err());
}
