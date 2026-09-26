//! What a generated world looks like, without starting the game.
//!
//! ```sh
//! cargo run --release -p tactics_core --example worldgen [seed]
//! cargo run --release -p tactics_core --example worldgen -- [seed] --chunk q,r
//! cargo run --release -p tactics_core --example worldgen -- [seed] --relief
//! ```
//!
//! Prints the campaign map the tiles summarise to (WORLD.md W1.4: a campaign
//! hex is `R` of its tiles), the terrain shares the world came out with
//! against the ones the mod asked for, the skeleton, and how long each part
//! took. `--chunk q,r` draws one campaign hex tile by tile; `--relief` draws
//! the campaign map by elevation instead of terrain.

use hexx::Hex;
use std::collections::BTreeMap;
use std::time::Instant;
use tactics_core::data::DataRegistry;
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

    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/mods");
    let (registry, _) = DataRegistry::load_dir(root.as_ref()).expect("mods load");

    let start = Instant::now();
    let world = GeneratedWorld::new(&registry, seed).expect("the base mod declares a world");
    let made = start.elapsed();

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
        "skeleton: {} river(s) {:?} tiles long, {} town(s) ({} with a factory), {} road(s) {:?} tiles long",
        sk.rivers.len(),
        sk.rivers.iter().map(Vec::len).collect::<Vec<_>>(),
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
