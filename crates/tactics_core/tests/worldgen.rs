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
        for course in &made.skeleton.rivers {
            let river = &course.tiles;
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
fn a_larger_map_has_more_towns_at_the_same_density() {
    // Towns are a density, so size and settlement compose: a large map is
    // more country, not the same villages spread thinner. (Rivers are a
    // drainage network since W6.5, and more land drains into more of them
    // without being told.)
    let reg = registry();
    let size = |option| rules_for(&reg, &[("size", option)]);
    let (small, standard, large) = (size("small"), size("standard"), size("large"));
    assert!(small.hexes() < standard.hexes() && standard.hexes() < large.hexes());
    let towns = |r: &WorldGen| r.towns.count(r.hexes());
    assert!(towns(&small) < towns(&standard) && towns(&standard) < towns(&large));
    // And the shipped standard world is the one the mod described by count
    // before the counts were densities.
    assert_eq!(towns(&standard), 14);
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

// --- relief with a shape (W6.4) ---------------------------------------------

/// How alike neighbouring campaign hexes stand: the correlation of their
/// mean heights across every neighbouring pair. Near zero is a campaign map
/// of salt and pepper; real country, where a hex on an upland has uplands
/// round it, is well above.
fn neighbour_height_correlation(world: &GeneratedWorld) -> f64 {
    let mean: HashMap<Hex, f64> = world
        .chunks()
        .map(|c| {
            let tiles = world.chunk_tiles(c);
            let sum: i64 = tiles.iter().map(|(_, _, l)| *l as i64).sum();
            (c, sum as f64 / tiles.len().max(1) as f64)
        })
        .collect();
    let pairs: Vec<(f64, f64)> = mean
        .iter()
        .flat_map(|(c, a)| {
            c.all_neighbors()
                .into_iter()
                .filter_map(|n| mean.get(&n).map(|b| (*a, *b)))
                .collect::<Vec<_>>()
        })
        .collect();
    let n = pairs.len() as f64;
    let (ma, mb) = (
        pairs.iter().map(|p| p.0).sum::<f64>() / n,
        pairs.iter().map(|p| p.1).sum::<f64>() / n,
    );
    let cov: f64 = pairs.iter().map(|(a, b)| (a - ma) * (b - mb)).sum();
    let va: f64 = pairs.iter().map(|(a, _)| (a - ma).powi(2)).sum();
    let vb: f64 = pairs.iter().map(|(_, b)| (b - mb).powi(2)).sum();
    cov / (va * vb).sqrt()
}

#[test]
fn the_land_has_regions_larger_than_a_campaign_hex() {
    // The diagnosis W6.4 answers: nothing in the relief was larger than a
    // campaign hex, so neighbouring hexes' heights were nearly independent
    // and the campaign map was salt and pepper. The landform and the ridges
    // are what make a hex on an upland have uplands round it.
    let reg = registry();
    let base = reg.worldgen.clone().unwrap();
    let with = |relief: tactics_core::data::Relief| WorldGen {
        radius: 5,
        relief,
        ..base.clone()
    };
    let hills_only = tactics_core::data::Relief {
        landform_percent: 0,
        ridge_percent: 0,
        ..base.relief.clone()
    };
    let radius = reg.scale.battle_map_radius();
    for seed in [1, 2] {
        let shaped = GeneratedWorld::with_rules(with(base.relief.clone()), radius, seed).unwrap();
        let flat = GeneratedWorld::with_rules(with(hills_only.clone()), radius, seed).unwrap();
        let (s, f) = (
            neighbour_height_correlation(&shaped),
            neighbour_height_correlation(&flat),
        );
        assert!(
            s > 0.6 && s > f + 0.25,
            "seed {seed}: neighbouring campaign hexes correlate {s:.2} with the landform \
             and ridges, {f:.2} with the hills alone"
        );
    }
}

#[test]
fn a_world_made_by_another_generator_is_refused_rather_than_regenerated() {
    // A world saves as how to make it, so a save from a build whose
    // generator made different ground would load with different ground
    // under the same armies and no error. The version travels with it.
    let reg = registry();
    let world = small(&reg, 2);
    let mut json = serde_json::to_value(&world).unwrap();
    assert_eq!(
        json["generator"],
        serde_json::json!(tactics_core::worldgen::GENERATOR_VERSION)
    );
    let back: GeneratedWorld = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(back.skeleton, world.skeleton);
    json["generator"] = serde_json::json!(tactics_core::worldgen::GENERATOR_VERSION - 1);
    let refused = serde_json::from_value::<GeneratedWorld>(json.clone());
    assert!(refused.is_err());
    // A save from before the version was written down is generator 1.
    json.as_object_mut().unwrap().remove("generator");
    assert!(serde_json::from_value::<GeneratedWorld>(json).is_err());
}

// --- water that drains (W6.5) -------------------------------------------------

/// The mod's world at a radius a test can afford to walk tile by tile.
fn sized(reg: &DataRegistry, seed: u64, radius: u32) -> GeneratedWorld {
    let rules = WorldGen {
        radius,
        ..reg.worldgen.clone().expect("the base mod declares a world")
    };
    GeneratedWorld::with_rules(rules, reg.scale.battle_map_radius(), seed).expect("rules are good")
}

#[test]
fn water_gathers_into_a_network_that_runs_downhill_and_widens() {
    // The diagnosis: three rivers, each one path from a high point to the
    // rim, none in the interior, none joining. Drained, the land has
    // streams that join into rivers, rivers that join each other, and
    // water that never climbs and never narrows on its way down.
    let reg = registry();
    let (mut joined_rivers, mut broad) = (0, 0);
    for seed in [1, 2] {
        let made = sized(&reg, seed, 6);
        let courses = &made.skeleton.rivers;
        assert!(
            courses.len() >= 10,
            "seed {seed}: {} watercourses",
            courses.len()
        );
        for course in courses {
            // Stream, then river, then broad: a course only grows.
            assert!(course.river_from <= course.broad_from);
            let mut last = i32::MAX;
            for hex in course.own() {
                let (_, level) = made.tile(*hex).unwrap();
                // A town or a road built over the water keeps the ground's
                // level; only the water's own tiles are held to it.
                if made.tile(*hex).unwrap().0 == reg.worldgen.as_ref().unwrap().terrain.water
                    || made.tile(*hex).unwrap().0 == reg.worldgen.as_ref().unwrap().terrain.stream
                {
                    assert!(level <= last, "seed {seed}: water climbs at {hex:?}");
                    last = level;
                }
            }
            if course.joins && !course.river().is_empty() {
                joined_rivers += 1;
            }
            if course.broad_from < course.own().len() {
                broad += 1;
            }
        }
    }
    assert!(joined_rivers > 0, "no river is a tributary of another");
    assert!(broad > 0, "no river grows broad");
}

#[test]
fn a_stream_can_be_forded_and_a_river_cannot() {
    // Why there are two kinds of running water: a network of impassable
    // water would cut the land into pieces joined only at bridges. Small
    // water is crossed at a wade, slowly; a river wants a bridge.
    let reg = registry();
    let palette = &reg.worldgen.as_ref().unwrap().terrain;
    let stream = reg
        .terrain(&palette.stream)
        .expect("the stream is declared");
    let water = reg.terrain(&palette.water).expect("the river is declared");
    use tactics_core::data::MovementClass;
    let (grass, wade) = (
        reg.terrain(&palette.open)
            .unwrap()
            .cost_for(MovementClass::Tracked)
            .unwrap(),
        stream
            .cost_for(MovementClass::Tracked)
            .expect("a tank can ford a stream"),
    );
    assert!(wade > grass, "fording is slower than open ground");
    assert!(stream.cost_for(MovementClass::Foot).is_some());
    assert!(water.cost_for(MovementClass::Tracked).is_none());
    assert!(water.cost_for(MovementClass::Foot).is_none());
    // And the world has both.
    let made = sized(&reg, 1, 6);
    let mut seen = (false, false);
    for chunk in made.chunks() {
        for (_, t, _) in made.chunk_tiles(chunk) {
            seen.0 |= t == palette.stream;
            seen.1 |= t == palette.water;
        }
    }
    assert_eq!(seen, (true, true));
}

#[test]
fn wet_meadow_lies_along_the_rivers() {
    // Before W6.5 not one wet tile in 17,732 lay within 300 m of water: the
    // wet ground was noise on the lowest level, and the rivers were laid
    // across it without touching it. A floodplain is the wettest ground
    // there is.
    let reg = registry();
    let made = sized(&reg, 1, 6);
    let palette = &made.rules.terrain;
    let mut tiles: HashMap<Hex, String> = HashMap::new();
    for chunk in made.chunks() {
        for (h, t, _) in made.chunk_tiles(chunk) {
            tiles.insert(h, t.to_string());
        }
    }
    let near: HashSet<Hex> = tiles
        .iter()
        .filter(|(_, t)| **t == palette.water)
        .flat_map(|(h, _)| h.range(made.rules.rivers.floodplain))
        .collect();
    let share = |by_river: bool| {
        let land: Vec<&String> = tiles
            .iter()
            .filter(|(h, t)| **t != palette.water && near.contains(*h) == by_river)
            .map(|(_, t)| t)
            .collect();
        land.iter().filter(|t| ***t == palette.wet).count() as f64 / land.len().max(1) as f64
    };
    let (by, away) = (share(true), share(false));
    assert!(
        by > 3.0 * away,
        "wet ground is {:.1}% of the floodplain and {:.1}% of the rest",
        100.0 * by,
        100.0 * away
    );
}

// --- cover that reads the ground (W6.6) ---------------------------------------

/// Every tile of a world, by hex.
fn all_tiles(made: &GeneratedWorld) -> HashMap<Hex, (String, i32)> {
    let mut tiles = HashMap::new();
    for chunk in made.chunks() {
        for (h, t, l) in made.chunk_tiles(chunk) {
            tiles.insert(h, (t.to_string(), l));
        }
    }
    tiles
}

/// Wood's share of the upper and lower halves of the land by level, and of
/// steep ground (a neighbour at another level) and flat.
fn wood_by_ground(made: &GeneratedWorld) -> ((f64, f64), (f64, f64)) {
    let tiles = all_tiles(made);
    let wood = &made.rules.terrain.wood;
    let mut levels: Vec<i32> = tiles.values().map(|(_, l)| *l).collect();
    levels.sort_unstable();
    let median = levels[levels.len() / 2];
    let mut counts = [[0u64; 2]; 4];
    for (h, (t, l)) in &tiles {
        let is_wood = (t == wood) as u64;
        let steep = h
            .all_neighbors()
            .iter()
            .any(|n| tiles.get(n).is_some_and(|(_, m)| m != l));
        for (slot, yes) in [(0, *l > median), (1, *l <= median), (2, steep), (3, !steep)] {
            if yes {
                counts[slot][0] += is_wood;
                counts[slot][1] += 1;
            }
        }
    }
    let share = |i: usize| counts[i][0] as f64 / counts[i][1].max(1) as f64;
    ((share(0), share(1)), (share(2), share(3)))
}

#[test]
fn woods_stand_on_high_and_steep_ground() {
    // The diagnosis: forest was 27–31% at every level and the same on slopes
    // as on the flat, because it was a noise laid over the relief without
    // reading it. Farmers clear the flat low land and leave the hills and
    // the valley sides to the trees. The control is the same world with the
    // lean switched off.
    let reg = registry();
    let base = reg.worldgen.clone().unwrap();
    let leaned = GeneratedWorld::with_rules(
        WorldGen {
            radius: 4,
            ..base.clone()
        },
        reg.scale.battle_map_radius(),
        1,
    )
    .unwrap();
    let level = GeneratedWorld::with_rules(
        WorldGen {
            radius: 4,
            cover: tactics_core::data::Cover {
                wood_on_high: 0,
                wood_on_slope: 0,
                ..base.cover.clone()
            },
            ..base.clone()
        },
        reg.scale.battle_map_radius(),
        1,
    )
    .unwrap();
    let ((hi, lo), (steep, flat)) = wood_by_ground(&leaned);
    assert!(
        hi > 2.0 * lo && steep > 1.5 * flat,
        "woods: {:.0}% high, {:.0}% low; {:.0}% steep, {:.0}% flat",
        100.0 * hi,
        100.0 * lo,
        100.0 * steep,
        100.0 * flat
    );
    let ((hi, lo), _) = wood_by_ground(&level);
    assert!(
        (hi - lo).abs() < 0.1,
        "without the lean the woods do not read the ground: {hi:.2} against {lo:.2}"
    );
}

#[test]
fn hedges_bound_fields_rather_than_fringe_woods() {
    // The old hedge was the band of cover noise just short of wood, so every
    // wood wore a hedge halo and a hedge was almost always beside a wood.
    // A hedge is the boundary between two fields, and fields are farmland.
    let reg = registry();
    let made = sized(&reg, 1, 4);
    let tiles = all_tiles(&made);
    let palette = &made.rules.terrain;
    let hedges: Vec<&Hex> = tiles
        .iter()
        .filter(|(_, (t, _))| *t == palette.hedge)
        .map(|(h, _)| h)
        .collect();
    assert!(hedges.len() > 1000, "only {} hedge tiles", hedges.len());
    let by_wood = hedges
        .iter()
        .filter(|h| {
            h.all_neighbors()
                .iter()
                .any(|n| tiles.get(n).is_some_and(|(t, _)| *t == palette.wood))
        })
        .count() as f64
        / hedges.len() as f64;
    assert!(
        by_wood < 0.35,
        "{:.0}% of hedges touch a wood: they fringe the woods rather than bound fields",
        100.0 * by_wood
    );
}

// --- settlements in a hierarchy (W6.7) ------------------------------------------

#[test]
fn settlements_come_in_sizes_and_the_factories_are_in_the_cities() {
    // Fourteen identical towns of 700 m was the diagnosis. A country has a
    // few cities, more towns, and villages everywhere between them; the
    // factories — what the war is about — are in the cities.
    let reg = registry();
    let made = sized(&reg, 1, 6);
    let t = &made.rules.towns;
    assert!(
        t.cities > 0 && t.city_radius > t.radius,
        "the mod ships cities"
    );
    let towns = &made.skeleton.towns;
    let cities: Vec<_> = towns.iter().filter(|x| x.radius == t.city_radius).collect();
    assert_eq!(cities.len() as u32, t.cities.min(towns.len() as u32));
    assert!(
        towns
            .iter()
            .filter(|x| x.factory)
            .all(|x| x.radius == t.city_radius),
        "a factory in a town, not a city"
    );
    let villages = &made.skeleton.villages;
    assert_eq!(
        villages.len() as u32,
        made.rules.villages.count(made.rules.hexes()),
        "every village the density asks for found a site"
    );
    assert!(villages.len() > 5 * towns.len());
    for (i, a) in villages.iter().enumerate() {
        for b in &villages[i + 1..] {
            assert!(a.unsigned_distance_to(*b) >= made.rules.villages.spacing);
        }
        for town in towns {
            assert!(a.unsigned_distance_to(town.centre) > town.radius);
        }
    }
}

#[test]
fn villages_settle_low_ground_by_water_and_towns_the_rivers() {
    let reg = registry();
    let made = sized(&reg, 1, 6);
    let tiles = all_tiles(&made);
    let palette = &made.rules.terrain;
    let mut levels: Vec<i32> = tiles.values().map(|(_, l)| *l).collect();
    levels.sort_unstable();
    let median = levels[levels.len() / 2];
    let level_at = |h: &Hex| tiles.get(h).map(|(_, l)| *l).unwrap_or(0);
    let water_near = |h: &Hex, r: u32, terrains: &[&String]| {
        h.range(r)
            .any(|n| tiles.get(&n).is_some_and(|(t, _)| terrains.contains(&t)))
    };
    let villages = &made.skeleton.villages;
    let low =
        villages.iter().filter(|v| level_at(v) <= median).count() as f64 / villages.len() as f64;
    let wet = villages
        .iter()
        .filter(|v| water_near(v, 3, &[&palette.water, &palette.stream]))
        .count() as f64
        / villages.len() as f64;
    // Against how much of the land lies that near running water at all, so
    // the claim is that villages seek it, not that water is common.
    let land = tiles
        .iter()
        .filter(|(_, (t, _))| *t != palette.water && *t != palette.stream)
        .map(|(h, _)| *h)
        .collect::<Vec<_>>();
    let base = land
        .iter()
        .step_by(7)
        .filter(|h| water_near(h, 3, &[&palette.water, &palette.stream]))
        .count() as f64
        / land.iter().step_by(7).count() as f64;
    assert!(
        low > 0.75,
        "{:.0}% of villages on the lower half",
        100.0 * low
    );
    assert!(
        wet > 5.0 * base,
        "{:.0}% of villages by running water, against {:.0}% of the land",
        100.0 * wet,
        100.0 * base
    );
    // A town wants a river, not merely a stream: in reach of the river trade
    // and at its bridge.
    let by_river = made
        .skeleton
        .towns
        .iter()
        .filter(|t| water_near(&t.centre, t.radius + 4, &[&palette.water]))
        .count() as f64
        / made.skeleton.towns.len() as f64;
    assert!(
        by_river > 0.5,
        "{:.0}% of towns by a river",
        100.0 * by_river
    );
}
