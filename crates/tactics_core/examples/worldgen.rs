//! What a generated world looks like, without starting the game.
//!
//! ```sh
//! cargo run --release -p tactics_core --example worldgen [seed]
//! cargo run --release -p tactics_core --example worldgen -- [seed] --chunk q,r
//! cargo run --release -p tactics_core --example worldgen -- [seed] --relief
//! cargo run --release -p tactics_core --example worldgen -- [seed] --world woodland=heavy,size=small
//! cargo run --release -p tactics_core --example worldgen -- [seed] --settings
//! cargo run --release -p tactics_core --example worldgen -- [seed] --picture world.ppm
//! ```
//!
//! `--picture` writes the world a pixel a tile, drawn the way the game's
//! setup screen draws it (`worldgen::picture`), as a PPM any viewer reads.
//! `--world` makes the world to the choices the setup screen offers
//! (`world_settings` in `mod.json`). `--settings` makes one world per option
//! of every setting, the others at their defaults, and prints a row of
//! measurements for each — what a setting actually does, read off the
//! tiles. The measurements are the ones the W6 diagnosis was made with:
//! `clump` is how much more often neighbouring campaign hexes share a class
//! than chance would have them (0 is salt and pepper), `wood hi/lo` is the
//! wood share on the upper half of the land's levels against the lower, and
//! `wet@water` is the share of wet ground within 300 m of a river.
//!
//! Prints the campaign map the tiles summarise to (WORLD.md W1.4: a campaign
//! hex is `R` of its tiles), the terrain shares the world came out with
//! against the ones the mod asked for, the skeleton, and how long each part
//! took. `--chunk q,r` draws one campaign hex tile by tile; `--relief` draws
//! the campaign map by elevation instead of terrain.

use hexx::Hex;
use std::collections::{BTreeMap, HashMap};
use std::time::Instant;
use tactics_core::data::{DataRegistry, WorldGen, WorldSetup};
use tactics_core::harness::parallel::run_all;
use tactics_core::world::chunk_hexes;
use tactics_core::worldgen::GeneratedWorld;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seed: u64 = args.iter().find_map(|a| a.parse().ok()).unwrap_or(1);
    let chunk: Option<Hex> = args
        .iter()
        .position(|a| a == "--chunk")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| {
            let (q, r) = s.split_once(',')?;
            Some(Hex::new(q.trim().parse().ok()?, r.trim().parse().ok()?))
        });
    let relief = args.iter().any(|a| a == "--relief");
    let setup = args
        .iter()
        .position(|a| a == "--world")
        .and_then(|i| args.get(i + 1))
        .map(|s| WorldSetup::parse(s))
        .transpose()
        .unwrap_or_else(|e| {
            eprintln!("error: --world: {e}");
            std::process::exit(1);
        })
        .unwrap_or_default();

    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/mods");
    let (registry, _) = DataRegistry::load_dir(root.as_ref()).expect("mods load");
    let base = registry
        .worldgen
        .clone()
        .expect("the base mod declares a world");
    let radius = registry.scale.battle_map_radius();

    let picture_path = args
        .iter()
        .position(|a| a == "--picture")
        .and_then(|i| args.get(i + 1))
        .cloned();

    if args.iter().any(|a| a == "--settings") {
        settings_table(&registry, &base, radius, seed);
        return;
    }

    let rules = base
        .with_settings(&registry.world_settings, &setup.choices)
        .unwrap_or_else(|e| {
            eprintln!("error: --world: {e}");
            std::process::exit(1);
        });
    let seed = setup.seed.unwrap_or(seed);
    let start = Instant::now();
    let world = GeneratedWorld::with_rules(rules, radius, seed).expect("the rules make a world");
    let made = start.elapsed();

    if let Some(path) = picture_path {
        let pic = tactics_core::worldgen::picture(&registry, &world, &[]);
        // A binary PPM: no image library in the core crate, and every
        // viewer and converter reads it.
        let mut bytes = format!("P6\n{} {}\n255\n", pic.width, pic.height).into_bytes();
        for px in pic.rgba.chunks(4) {
            bytes.extend_from_slice(&px[..3]);
        }
        std::fs::write(&path, bytes).expect("the picture writes");
        println!("wrote {path} ({}x{})", pic.width, pic.height);
    }

    if let Some(chunk) = chunk {
        draw_chunk(&world, chunk);
        return;
    }

    let start = Instant::now();
    let summaries: Vec<(Hex, tactics_core::worldgen::Summary)> = world
        .chunks()
        .filter_map(|c| world.summary(c).map(|s| (c, s)))
        .collect();
    let summarised = start.elapsed();

    println!(
        "world seed {seed}: radius {} campaign hexes ({} of them), chunk radius {} tiles",
        world.rules.radius,
        summaries.len(),
        world.chunk_radius
    );
    println!();
    draw_campaign(&summaries, relief);
    println!();

    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut levels = [0u64; 10];
    let mut tiles = 0u64;
    for (c, _) in &summaries {
        for (_, terrain, level) in world.chunk_tiles(*c) {
            *counts.entry(terrain.to_string()).or_default() += 1;
            levels[level.clamp(0, 9) as usize] += 1;
            tiles += 1;
        }
    }
    println!("{tiles} tiles");
    for (terrain, n) in &counts {
        println!("  {terrain:<10} {:>5.1}%", 100.0 * *n as f64 / tiles as f64);
    }
    println!(
        "  levels     {}   (the mod asks for {:?})",
        levels
            .iter()
            .take(world.rules.relief.shares.len())
            .map(|n| format!("{:.0}", 100.0 * *n as f64 / tiles as f64))
            .collect::<Vec<_>>()
            .join(" "),
        world.rules.relief.shares
    );
    let sk = &world.skeleton;
    println!(
        "skeleton: {} watercourse(s), {} of them rivers {:?} tiles of river long, {} town(s) ({} with a factory), {} road(s) {:?} tiles long",
        sk.rivers.len(),
        sk.rivers.iter().filter(|c| !c.river().is_empty()).count(),
        sk.rivers
            .iter()
            .map(|c| c.river().len())
            .filter(|n| *n > 0)
            .collect::<Vec<_>>(),
        sk.towns.len(),
        sk.towns.iter().filter(|t| t.factory).count(),
        sk.roads.len(),
        sk.roads.iter().map(Vec::len).collect::<Vec<_>>(),
    );
    println!(
        "made in {:.1} ms; every tile generated and summarised in {:.1} ms",
        made.as_secs_f64() * 1e3,
        summarised.as_secs_f64() * 1e3
    );
    println!();
    println!("{}", Stats::HEADER);
    println!("{}", Stats::of(&world).row("this world"));
}

/// One world per option of every setting, the rest at their defaults.
fn settings_table(registry: &DataRegistry, base: &WorldGen, radius: u32, seed: u64) {
    let mut jobs: Vec<(String, BTreeMap<String, String>)> =
        vec![("defaults".into(), BTreeMap::new())];
    for setting in &registry.world_settings {
        for option in &setting.options {
            if option.id == setting.default {
                continue;
            }
            jobs.push((
                format!("{}={}", setting.id, option.id),
                BTreeMap::from([(setting.id.clone(), option.id.clone())]),
            ));
        }
    }
    let rows = run_all(&jobs, |(label, choices)| {
        let rules = base
            .with_settings(&registry.world_settings, choices)
            .expect("validate-mods passed");
        let world = GeneratedWorld::with_rules(rules, radius, seed).expect("a world");
        Stats::of(&world).row(label)
    });
    println!("world seed {seed}, one setting changed at a time");
    println!("{}", Stats::HEADER);
    for row in rows {
        println!("{row}");
    }
}

/// What a world came out as, in the measurements the W6 diagnosis used.
struct Stats {
    hexes: usize,
    shares: BTreeMap<String, f64>,
    mean_level: f64,
    rivers: usize,
    river_tiles: usize,
    towns: usize,
    classes: BTreeMap<char, usize>,
    clump: f64,
    height_corr: f64,
    wood_hi: f64,
    wood_lo: f64,
    wet_at_water: f64,
}

impl Stats {
    const HEADER: &str = "  world                  hexes  wood hedge  wet water town  level  rivers  towns   M   W   =   clump  h-corr  wood hi/lo  wet@water";

    fn of(world: &GeneratedWorld) -> Self {
        let chunks: Vec<Hex> = world.chunks().collect();
        let mut tiles: HashMap<Hex, (String, i32)> = HashMap::new();
        for c in &chunks {
            for (h, t, e) in world.chunk_tiles(*c) {
                tiles.insert(h, (t.to_string(), e));
            }
        }
        let n = tiles.len() as f64;
        let mut shares: BTreeMap<String, f64> = BTreeMap::new();
        let mut level_sum = 0i64;
        let mut by_level: BTreeMap<i32, (u64, u64)> = BTreeMap::new();
        let palette = &world.rules.terrain;
        for (t, e) in tiles.values() {
            *shares.entry(t.clone()).or_default() += 100.0 / n;
            level_sum += *e as i64;
            let slot = by_level.entry(*e).or_default();
            slot.1 += 1;
            if *t == palette.wood {
                slot.0 += 1;
            }
        }
        // The median level splits the land into an upper and a lower half.
        let mut seen = 0u64;
        let mut median = 0;
        for (level, (_, count)) in &by_level {
            seen += count;
            if seen as f64 >= n / 2.0 {
                median = *level;
                break;
            }
        }
        let share_of = |f: &dyn Fn(i32) -> bool| {
            let (w, c) = by_level
                .iter()
                .filter(|(l, _)| f(**l))
                .fold((0u64, 0u64), |(w, c), (_, (a, b))| (w + a, c + b));
            if c == 0 {
                0.0
            } else {
                100.0 * w as f64 / c as f64
            }
        };
        let wood_hi = share_of(&|l| l > median);
        let wood_lo = share_of(&|l| l <= median);
        let water: Vec<Hex> = tiles
            .iter()
            .filter(|(_, (t, _))| *t == palette.water)
            .map(|(h, _)| *h)
            .collect();
        let near: std::collections::HashSet<Hex> = water.iter().flat_map(|h| h.range(3)).collect();
        let wet: Vec<&Hex> = tiles
            .iter()
            .filter(|(_, (t, _))| *t == palette.wet)
            .map(|(h, _)| h)
            .collect();
        let wet_at_water = if wet.is_empty() {
            0.0
        } else {
            100.0 * wet.iter().filter(|h| near.contains(h)).count() as f64 / wet.len() as f64
        };
        let summaries: HashMap<Hex, char> = chunks
            .iter()
            .filter_map(|c| world.summary(*c).map(|s| (*c, campaign_glyph(&s.terrain))))
            .collect();
        let mut classes: BTreeMap<char, usize> = BTreeMap::new();
        for g in summaries.values() {
            *classes.entry(*g).or_default() += 1;
        }
        // Neighbouring campaign hexes sharing a class, less what chance
        // gives: for class shares p, a random map shares sum(p^2) of the
        // time. Zero is salt and pepper; real country is well above it.
        let (mut same, mut pairs) = (0u64, 0u64);
        for (h, g) in &summaries {
            for nb in h.all_neighbors() {
                if let Some(o) = summaries.get(&nb) {
                    pairs += 1;
                    same += (o == g) as u64;
                }
            }
        }
        // Neighbouring campaign hexes' mean heights, correlated: whether the
        // land has regions larger than a hex (W6.4). Near zero is none.
        let mut height: HashMap<Hex, (i64, i64)> = HashMap::new();
        for (h, (_, e)) in &tiles {
            let slot = height
                .entry(tactics_core::world::chunk_of(*h, world.chunk_radius))
                .or_default();
            slot.0 += *e as i64;
            slot.1 += 1;
        }
        let mean: HashMap<Hex, f64> = height
            .iter()
            .map(|(c, (s, n))| (*c, *s as f64 / *n as f64))
            .collect();
        let hpairs: Vec<(f64, f64)> = mean
            .iter()
            .flat_map(|(c, a)| {
                c.all_neighbors()
                    .into_iter()
                    .filter_map(|n| mean.get(&n).map(|b| (*a, *b)))
                    .collect::<Vec<_>>()
            })
            .collect();
        let np = hpairs.len().max(1) as f64;
        let ma = hpairs.iter().map(|p| p.0).sum::<f64>() / np;
        let mb = hpairs.iter().map(|p| p.1).sum::<f64>() / np;
        let cov: f64 = hpairs.iter().map(|(a, b)| (a - ma) * (b - mb)).sum();
        let va: f64 = hpairs.iter().map(|(a, _)| (a - ma).powi(2)).sum();
        let vb: f64 = hpairs.iter().map(|(_, b)| (b - mb).powi(2)).sum();
        let height_corr = if va * vb > 0.0 {
            cov / (va * vb).sqrt()
        } else {
            0.0
        };
        let total = summaries.len() as f64;
        let chance: f64 = classes.values().map(|c| (*c as f64 / total).powi(2)).sum();
        let clump = if pairs == 0 {
            0.0
        } else {
            same as f64 / pairs as f64 - chance
        };
        Self {
            hexes: summaries.len(),
            shares,
            mean_level: level_sum as f64 / n,
            rivers: world
                .skeleton
                .rivers
                .iter()
                .filter(|c| !c.river().is_empty())
                .count(),
            river_tiles: world.skeleton.rivers.iter().map(|c| c.river().len()).sum(),
            towns: world.skeleton.towns.len(),
            classes,
            clump,
            height_corr,
            wood_hi,
            wood_lo,
            wet_at_water,
        }
    }

    fn row(&self, label: &str) -> String {
        let share = |t: &str| self.shares.get(t).copied().unwrap_or(0.0);
        let class = |g: char| self.classes.get(&g).copied().unwrap_or(0);
        format!(
            "  {label:<22} {:>5} {:>5.1} {:>5.1} {:>4.1} {:>5.2} {:>4.1} {:>6.2}  {:>2} {:>5}  {:>5}  {:>3} {:>3} {:>3}  {:>6.3}  {:>6.2}  {:>4.0}/{:<4.0}  {:>8.0}%",
            self.hexes,
            share("forest"),
            share("hedgerow"),
            share("mud"),
            share("water"),
            share("town"),
            self.mean_level,
            self.rivers,
            self.river_tiles,
            self.towns,
            class('M'),
            class('W'),
            class('='),
            self.clump,
            self.height_corr,
            self.wood_hi,
            self.wood_lo,
            self.wet_at_water,
        )
    }
}

/// One glyph per campaign terrain.
fn campaign_glyph(terrain: &str) -> char {
    match terrain {
        "factory" => 'F',
        "city" => 'C',
        "mountains" => 'M',
        "deep_forest" => 'W',
        "highway" => '=',
        "plains" => '.',
        _ => '?',
    }
}

/// The campaign map, a row per `r`, shifted so the hexagon reads as one.
fn draw_campaign(summaries: &[(Hex, tactics_core::worldgen::Summary)], relief: bool) {
    let by: BTreeMap<(i32, i32), &tactics_core::worldgen::Summary> =
        summaries.iter().map(|(h, s)| ((h.y, h.x), s)).collect();
    let (min_r, max_r) = (
        summaries.iter().map(|(h, _)| h.y).min().unwrap_or(0),
        summaries.iter().map(|(h, _)| h.y).max().unwrap_or(0),
    );
    let min_q = summaries.iter().map(|(h, _)| h.x).min().unwrap_or(0);
    let max_q = summaries.iter().map(|(h, _)| h.x).max().unwrap_or(0);
    for r in min_r..=max_r {
        let mut line = " ".repeat((r - min_r) as usize);
        for q in min_q..=max_q {
            match by.get(&(r, q)) {
                Some(s) if relief => line.push_str(&format!("{} ", s.elevation)),
                Some(s) => {
                    line.push(campaign_glyph(&s.terrain));
                    line.push(' ');
                }
                None => line.push_str("  "),
            }
        }
        println!("  {}", line.trim_end());
    }
    if !relief {
        println!("  F factory  C city  M mountains  W deep forest  = highway  . plains");
    }
}

/// One campaign hex, tile by tile.
fn draw_chunk(world: &GeneratedWorld, chunk: Hex) {
    let tiles: BTreeMap<(i32, i32), (String, i32)> = world
        .chunk_tiles(chunk)
        .into_iter()
        .map(|(h, t, e)| ((h.y, h.x), (t.to_string(), e)))
        .collect();
    if tiles.is_empty() {
        println!("chunk {chunk:?} is past the edge of the world");
        return;
    }
    println!(
        "chunk ({}, {}): {}",
        chunk.x,
        chunk.y,
        world.summary(chunk).map_or("nothing".into(), |s| format!(
            "{} at {}",
            s.terrain, s.elevation
        ))
    );
    let hexes: Vec<Hex> = chunk_hexes(chunk, world.chunk_radius).collect();
    let (min_r, max_r) = (
        hexes.iter().map(|h| h.y).min().unwrap(),
        hexes.iter().map(|h| h.y).max().unwrap(),
    );
    let (min_q, max_q) = (
        hexes.iter().map(|h| h.x).min().unwrap(),
        hexes.iter().map(|h| h.x).max().unwrap(),
    );
    for r in min_r..=max_r {
        let mut line = " ".repeat((r - min_r) as usize);
        for q in min_q..=max_q {
            let glyph = match tiles.get(&(r, q)) {
                Some((t, e)) => match t.as_str() {
                    "grass" => char::from_digit(*e as u32, 10).unwrap_or('.'),
                    "forest" => 'f',
                    "hedgerow" => 'h',
                    "mud" => 'm',
                    "water" => '~',
                    "road" => '#',
                    "town" => 'T',
                    _ => '?',
                },
                None => ' ',
            };
            line.push(glyph);
            line.push(' ');
        }
        println!("  {}", line.trim_end());
    }
    println!("  digit: open ground at that level  f wood  h hedge  m mud  ~ water  # road  T town");
}
