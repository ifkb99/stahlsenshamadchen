//! Making a world, bottom up (WORLD.md W1.2–W1.4).
//!
//! What is pinned: that a world is a pure function of its seed and rules —
//! the same tiles in any order, on any machine, chunk by chunk or all at
//! once, with no seam where two chunks meet; that its skeleton hangs
//! together (rivers run to the edge, every town is on the road network);
//! that it comes out with the shares its mod asked for; and that a campaign
//! hex is named from the tiles inside it and nothing else.

use hexx::Hex;
use std::collections::{BTreeMap, HashMap, HashSet};
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
        assert_eq!(
            towns.len() as u32,
            made.rules.towns.count(made.rules.hexes()),
            "seed {seed}"
        );
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

// --- a world made to order (W6.1) ---------------------------------------

/// `setting=option` pairs as a choice.
fn choose(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(s, o)| (s.to_string(), o.to_string()))
        .collect()
}

/// The mod's rules with these choices made, and then shrunk to a world
/// small enough to generate in a test — except where the choice is the
/// size, which is what the test is about.
fn rules_for(reg: &DataRegistry, pairs: &[(&str, &str)]) -> WorldGen {
    reg.worldgen
        .as_ref()
        .expect("the base mod declares a world")
        .with_settings(&reg.world_settings, &choose(pairs))
        .expect("a choice the mod offers")
}

fn made(reg: &DataRegistry, pairs: &[(&str, &str)]) -> GeneratedWorld {
    let rules = WorldGen {
        radius: 4,
        ..rules_for(reg, pairs)
    };
    GeneratedWorld::with_rules(rules, reg.scale.battle_map_radius(), 3).expect("rules are good")
}

/// Share of a world's tiles that are `terrain`, and its mean level.
fn measure(world: &GeneratedWorld, terrain: &str) -> (f64, f64) {
    let (mut n, mut hits, mut levels) = (0u64, 0u64, 0i64);
    for chunk in world.chunks() {
        for (_, t, level) in world.chunk_tiles(chunk) {
            n += 1;
            hits += (t == terrain) as u64;
            levels += level as i64;
        }
    }
    (hits as f64 / n as f64, levels as f64 / n as f64)
}

#[test]
fn a_world_with_every_setting_at_its_default_is_the_mods_world() {
    // The additivity rule for the setup screen: saying nothing is the world
    // the mod describes, field for field, so a campaign that never showed a
    // player the screen is the campaign it always was.
    let reg = registry();
    assert!(
        !reg.world_settings.is_empty(),
        "the base mod offers settings"
    );
    let base = reg.worldgen.clone().unwrap();
    assert_eq!(
        base.with_settings(&reg.world_settings, &choose(&[])),
        Ok(base.clone())
    );
    // Naming every default is the same as naming none.
    let defaults: Vec<(&str, &str)> = reg
        .world_settings
        .iter()
        .map(|s| (s.id.as_str(), s.default.as_str()))
        .collect();
    assert_eq!(rules_for(&reg, &defaults), base);
}

#[test]
fn a_larger_map_has_more_towns_and_rivers_at_the_same_density() {
    // Towns and rivers are densities, so size and settlement compose: a
    // large map is more country, not the same villages spread thinner.
    let reg = registry();
    let size = |option| rules_for(&reg, &[("size", option)]);
    let (small, standard, large) = (size("small"), size("standard"), size("large"));
    assert!(small.hexes() < standard.hexes() && standard.hexes() < large.hexes());
    let towns = |r: &WorldGen| r.towns.count(r.hexes());
    let rivers = |r: &WorldGen| r.rivers.count(r.hexes());
    assert!(towns(&small) < towns(&standard) && towns(&standard) < towns(&large));
    assert!(rivers(&small) <= rivers(&standard) && rivers(&standard) < rivers(&large));
    // And the shipped standard world is the one the mod described by count
    // before the counts were densities.
    assert_eq!((towns(&standard), rivers(&standard)), (14, 3));
}

#[test]
fn each_setting_moves_the_world_the_way_it_says() {
    let reg = registry();
    let wood = &reg.worldgen.as_ref().unwrap().terrain.wood;
    let hedge = &reg.worldgen.as_ref().unwrap().terrain.hedge;
    let wet = &reg.worldgen.as_ref().unwrap().terrain.wet;

    let woods: Vec<f64> = ["sparse", "normal", "heavy"]
        .iter()
        .map(|o| measure(&made(&reg, &[("woodland", o)]), wood).0)
        .collect();
    assert!(woods[0] < woods[1] && woods[1] < woods[2], "{woods:?}");

    let hedges: Vec<f64> = ["open", "mixed", "bocage"]
        .iter()
        .map(|o| measure(&made(&reg, &[("fields", o)]), hedge).0)
        .collect();
    assert!(hedges[0] < hedges[1] && hedges[1] < hedges[2], "{hedges:?}");

    let wets: Vec<f64> = ["dry", "normal", "wet"]
        .iter()
        .map(|o| measure(&made(&reg, &[("water", o)]), wet).0)
        .collect();
    assert!(wets[0] < wets[1] && wets[1] < wets[2], "{wets:?}");

    let heights: Vec<f64> = ["flat", "rolling", "hilly", "mountainous"]
        .iter()
        .map(|o| measure(&made(&reg, &[("relief", o)]), wood).1)
        .collect();
    assert!(
        heights.windows(2).all(|w| w[0] < w[1]),
        "mean level by relief: {heights:?}"
    );

    let towns: Vec<usize> = ["sparse", "normal", "dense"]
        .iter()
        .map(|o| made(&reg, &[("settlement", o)]).skeleton.towns.len())
        .collect();
    assert!(towns[0] < towns[1] && towns[1] < towns[2], "{towns:?}");
}

#[test]
fn a_choice_nobody_offers_is_refused() {
    let reg = registry();
    let base = reg.worldgen.clone().unwrap();
    assert!(matches!(
        base.with_settings(&reg.world_settings, &choose(&[("gravity", "low")])),
        Err(tactics_core::data::SettingError::UnknownSetting(_))
    ));
    assert!(matches!(
        base.with_settings(&reg.world_settings, &choose(&[("size", "enormous")])),
        Err(tactics_core::data::SettingError::UnknownOption { .. })
    ));
}

#[test]
fn an_option_that_names_no_field_fails_validation() {
    // A setting is data, so a typo in one is a content error, reported
    // against the option that wrote it — not a world quietly made without it.
    let mut reg = registry();
    assert!(reg.validate().is_ok(), "{:?}", reg.validate().errors);
    reg.world_settings[0].options[0]
        .set
        .insert("cover.wood_procent".into(), serde_json::json!(40));
    let report = reg.validate();
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("wood_procent") && e.contains(&reg.world_settings[0].id)),
        "{:?}",
        report.errors
    );
}

#[test]
fn a_fault_between_two_settings_fails_validation() {
    // Neither option alone is wrong; together they leave fewer towns than
    // the campaign has factories, which only the combination can show.
    let mut reg = registry();
    let sparse = reg
        .world_settings
        .iter_mut()
        .find(|s| s.id == "settlement")
        .unwrap()
        .options
        .iter_mut()
        .find(|o| o.id == "sparse")
        .unwrap();
    sparse
        .set
        .insert("towns.every".into(), serde_json::json!(120));
    let report = reg.validate();
    assert!(
        report.errors.iter().any(|e| e.contains("size=small")
            && e.contains("settlement=sparse")
            && e.contains("factories")),
        "{:?}",
        report.errors
    );
}
