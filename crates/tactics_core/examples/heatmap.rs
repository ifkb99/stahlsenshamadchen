//! Where a map's battles actually happen, drawn over the ground.
//!
//! ```text
//! cargo run --release -p tactics_core --example heatmap -- --map slice_hollerbach --games 16
//! cargo run --release -p tactics_core --example heatmap -- --map slice_hollerbach --games 16 --swap
//! cargo run --release -p tactics_core --example heatmap -- --seat game1 game2
//! ```
//!
//! A map is drawn with a battle in mind: this lane is the covered approach,
//! that knoll the base of fire, the brook the line the defence holds. Every
//! other instrument here reports who won and what the shot did, and none of
//! them can say whether the battle that was fought is the one the map was
//! drawn for. This one counts, per hex and per side, what happened there,
//! and draws each count as a heat over the terrain:
//!
//! - **moved**: hexes entered (`UnitMoved` paths): the routes.
//! - **stood**: unit-rounds standing there at a round's end: where they
//!   stopped.
//! - **fired from**: `ShotFired.from`: the firing positions.
//! - **hurt**: where a side's units were hit (`ShotHit`, placed at the
//!   attacker's aim point that round), with a cross per unit lost there.
//! - **seen**: where the enemy spotted a side's units (`UnitSpotted`):
//!   exposure.
//! - **pinned**: `PinnedDown`: where the fire stopped them.
//! - **lines of fire**: from-to pairs, thicker for more shots.
//!
//! Values are per battle. Both sides in a row share one scale, so the two
//! columns compare. AI against AI by default, with the same seats as the
//! `playthrough` example (utility planners, `massed_armor` against
//! `elastic_defense`; `--swap` exchanges the doctrines, `--map-ai` uses the
//! map's own `ai` blocks). `--seat <dir>...` reads hand-fought battles from
//! the `rounds.jsonl` that `examples/command` writes instead.
//!
//! The picture is an SVG (`--out`, default `target/heatmap.svg`); the
//! busiest hexes are also printed, so the numbers survive without a viewer.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use serde::Deserialize;
use tactics_core::ai::{AiConfig, AiDriver, make_battle_planner};
use tactics_core::battle::{BattleState, Event, UnitId};
use tactics_core::data::DataRegistry;
use tactics_core::save;
use tactics_core::{hex_to_offset, offset_to_hex};

/// One resolved round: its events, and every unit's side and place after it.
/// The same shape `examples/command` writes to `rounds.jsonl`.
#[derive(Debug, Deserialize)]
struct Round {
    events: Vec<Event>,
    units: Vec<Placed>,
}

#[derive(Debug, Deserialize)]
struct Placed {
    id: u32,
    side: u8,
    at: [i32; 2],
    standing: bool,
    /// Chassis id; absent in logs written before it was recorded.
    #[serde(default)]
    vehicle: String,
}

const LAYERS: [&str; 6] = ["moved", "stood", "fired from", "hurt", "seen", "pinned"];
const MOVED: usize = 0;
const STOOD: usize = 1;
const FIRED: usize = 2;
const HURT: usize = 3;
const SEEN: usize = 4;
const PINNED: usize = 5;

type Cell = [i32; 2];

#[derive(Default)]
struct Heat {
    layers: [[BTreeMap<Cell, f64>; 2]; LAYERS.len()],
    lines: [BTreeMap<(Cell, Cell), f64>; 2],
    lost: [BTreeMap<Cell, f64>; 2],
    /// Shots by range in hexes, per side; the last bucket is "this far or
    /// further".
    ranges: [[u32; 9]; 2],
    battles: u32,
    wins: [u32; 2],
    rounds: u32,
    /// Count only units of this chassis (`--vehicle`); the ending still
    /// counts.
    only: Option<String>,
}

impl Heat {
    fn add(&mut self, layer: usize, side: u8, at: Cell) {
        *self.layers[layer][side as usize].entry(at).or_default() += 1.0;
    }

    /// Fold one battle's rounds in.
    fn battle(&mut self, rounds: &[Round]) {
        self.battles += 1;
        self.rounds += rounds.len() as u32;
        let mut side_of: BTreeMap<u32, u8> = BTreeMap::new();
        for round in rounds {
            for u in &round.units {
                if self.only.as_ref().is_none_or(|v| *v == u.vehicle) {
                    side_of.insert(u.id, u.side);
                }
            }
        }
        // A filtered-out unit has no side, so every event about it drops.
        let side = |id: &UnitId| side_of.get(&id.0).copied();
        for round in rounds {
            // A hit names its target, not where she stood; the attacker's
            // aim point this round is where it landed.
            let mut aim: BTreeMap<u32, Cell> = BTreeMap::new();
            for ev in &round.events {
                match ev {
                    Event::UnitMoved { unit, path } => {
                        if let Some(s) = side(unit) {
                            for h in path.iter().skip(1) {
                                self.add(MOVED, s, hex_to_offset(*h));
                            }
                        }
                    }
                    Event::ShotFired {
                        attacker, from, at, ..
                    } => {
                        if let Some(s) = side(attacker) {
                            let (f, t) = (hex_to_offset(*from), hex_to_offset(*at));
                            self.add(FIRED, s, f);
                            let d = from.unsigned_distance_to(*at) as usize;
                            self.ranges[s as usize][d.min(8)] += 1;
                            *self.lines[s as usize].entry((f, t)).or_default() += 1.0;
                            aim.insert(attacker.0, t);
                        }
                    }
                    Event::ShotHit {
                        attacker, target, ..
                    } => {
                        if let (Some(s), Some(at)) = (side(target), aim.get(&attacker.0)) {
                            self.add(HURT, s, *at);
                        }
                    }
                    Event::UnitDestroyed { unit, at } => {
                        if let Some(s) = side(unit) {
                            *self.lost[s as usize].entry(hex_to_offset(*at)).or_default() += 1.0;
                        }
                    }
                    Event::UnitSpotted { unit, at, .. } => {
                        if let Some(s) = side(unit) {
                            self.add(SEEN, s, hex_to_offset(*at));
                        }
                    }
                    // The ending's own word, not the score: an elimination
                    // can be won from behind on points.
                    Event::BattleEnded {
                        winner: Some(w), ..
                    } => self.wins[*w as usize] += 1,
                    Event::PinnedDown { unit, at } => {
                        if let Some(s) = side(unit) {
                            self.add(PINNED, s, hex_to_offset(*at));
                        }
                    }
                    _ => {}
                }
            }
            for u in round
                .units
                .iter()
                .filter(|u| u.standing && side_of.contains_key(&u.id))
            {
                self.add(STOOD, u.side, u.at);
            }
        }
    }
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (registry, _) = DataRegistry::load_dir(&root).expect("mods load");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| -> Option<String> {
        let i = args.iter().position(|a| a == name)?;
        args.get(i + 1).cloned()
    };
    let out = PathBuf::from(flag("--out").unwrap_or_else(|| "target/heatmap.svg".into()));

    let mut heat = Heat {
        only: flag("--vehicle"),
        ..Heat::default()
    };
    let ground: BattleState;
    let title: String;
    if let Some(i) = args.iter().position(|a| a == "--seat") {
        let dirs: Vec<PathBuf> = args[i + 1..]
            .iter()
            .take_while(|a| !a.starts_with("--"))
            .map(PathBuf::from)
            .collect();
        let mut last = None;
        for dir in &dirs {
            let (game, _) = save::read(&registry, dir.join("battle.json")).expect("save reads");
            let state = game.battle.expect("the save holds a battle");
            let text = std::fs::read_to_string(dir.join("rounds.jsonl")).expect("rounds.jsonl");
            let rounds: Vec<Round> = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| serde_json::from_str(l).expect("a round parses"))
                .collect();
            heat.battle(&rounds);
            last = Some(state);
        }
        ground = last.expect("at least one --seat directory");
        title = format!("{} hand-fought battle(s)", dirs.len());
    } else {
        let map = flag("--map").unwrap_or_else(|| "slice_hollerbach".into());
        let games: u64 = flag("--games").and_then(|s| s.parse().ok()).unwrap_or(16);
        let first: u64 = flag("--seed").and_then(|s| s.parse().ok()).unwrap_or(1);
        let planner = flag("--planner").unwrap_or_else(|| "utility".into());
        let swap = args.iter().any(|a| a == "--swap");
        let map_ai = args.iter().any(|a| a == "--map-ai");
        let doctrines = if swap {
            ["elastic_defense", "massed_armor"]
        } else {
            ["massed_armor", "elastic_defense"]
        };
        for seed in first..first + games {
            let mut state = BattleState::from_map(&registry, &map, seed).expect("battle");
            let mut ai = AiDriver::new();
            for side in 0..2u8 {
                let shipped = AiConfig {
                    planner: planner.clone(),
                    difficulty: 4,
                    doctrine: Some(doctrines[side as usize].into()),
                };
                let config = match &state.sides[side as usize].ai {
                    Some(c) if map_ai => c.clone(),
                    _ => shipped,
                };
                ai.insert(
                    side,
                    make_battle_planner(&config, seed + u64::from(side), &registry),
                );
            }
            let mut rounds = Vec::new();
            while !state.is_over() && rounds.len() < 200 {
                ai.plan_round_with(&registry, &mut state, |_| {});
                let events = state.resolve_round(&registry);
                let units = state
                    .units
                    .iter()
                    .map(|u| Placed {
                        id: u.id.0,
                        side: u.side,
                        at: hex_to_offset(u.pos),
                        standing: u.fate.alive() && u.aboard.is_none(),
                        vehicle: u.vehicle.clone(),
                    })
                    .collect();
                rounds.push(Round { events, units });
            }
            heat.battle(&rounds);
        }
        ground = BattleState::from_map(&registry, &map, first).expect("battle");
        title = format!(
            "{map}, {games} AI battles ({planner}; {} / {}{})",
            doctrines[0],
            doctrines[1],
            if map_ai {
                "; map's own AI where given"
            } else {
                ""
            }
        );
    }

    let title = match &heat.only {
        Some(v) => format!("{title}; {v} only"),
        None => title,
    };
    let names = [ground.sides[0].name.clone(), ground.sides[1].name.clone()];
    println!("{title}");
    println!(
        "{} wins {}, {} wins {}, of {}; {:.1} rounds a battle",
        names[0],
        heat.wins[0],
        names[1],
        heat.wins[1],
        heat.battles,
        f64::from(heat.rounds) / f64::from(heat.battles.max(1))
    );
    report(&heat, &names);
    let svg = draw(&ground, &heat, &names, &title);
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(&out, svg).expect("svg writes");
    println!("drawn to {}", out.display());
}

/// The busiest hexes of every layer, per battle, as text.
fn report(heat: &Heat, names: &[String; 2]) {
    let per = f64::from(heat.battles.max(1));
    let top = |m: &BTreeMap<Cell, f64>| -> String {
        let mut v: Vec<_> = m.iter().collect();
        v.sort_by(|a, b| b.1.total_cmp(a.1).then(a.0.cmp(b.0)));
        let total: f64 = m.values().sum();
        let list: Vec<String> = v
            .iter()
            .take(6)
            .map(|(c, n)| format!("({},{}) {:.1}", c[0], c[1], *n / per))
            .collect();
        format!("total {:>6.1}  {}", total / per, list.join("  "))
    };
    for (l, name) in LAYERS.iter().enumerate() {
        for (side, m) in names.iter().zip(&heat.layers[l]) {
            println!("{name:>10} {side:<16} {}", top(m));
        }
    }
    for (side, m) in names.iter().zip(&heat.lost) {
        println!("{:>10} {side:<16} {}", "lost", top(m));
    }
    println!("shots by range (hexes; % of the side's shots):");
    println!("{:>27}   0    1    2    3    4    5    6    7   8+", "");
    for (side, ranges) in names.iter().zip(&heat.ranges) {
        let total: u32 = ranges.iter().sum();
        let row: Vec<String> = ranges
            .iter()
            .map(|n| format!("{:>4.0}", 100.0 * f64::from(*n) / f64::from(total.max(1))))
            .collect();
        println!("{side:>16} ({total:>5} shots) {}", row.join(" "));
    }
}

// ---- drawing ----------------------------------------------------------------

const R: f64 = 10.0;
const SIDE_COLOUR: [&str; 2] = ["#1f5fbf", "#c0392b"];

fn centre(c: i32, r: i32) -> (f64, f64) {
    let w = 3f64.sqrt() * R;
    (
        w * (f64::from(c) + 0.5 * f64::from(r & 1)) + w / 2.0,
        1.5 * R * f64::from(r) + R,
    )
}

fn hexagon(c: i32, r: i32) -> String {
    let (x, y) = centre(c, r);
    let pts: Vec<String> = (0..6)
        .map(|i| {
            let a = std::f64::consts::PI / 180.0 * (60.0 * f64::from(i) - 30.0);
            format!("{:.1},{:.1}", x + R * a.cos(), y + R * a.sin())
        })
        .collect();
    pts.join(" ")
}

fn terrain_colour(t: &str) -> &'static str {
    match t {
        "plains" => "#ece6cf",
        "grass" => "#dbe6c0",
        "road" => "#c9bca0",
        "town" => "#a9a2a0",
        "forest" => "#9fbf8f",
        "deep_forest" => "#86a878",
        "hedgerow" => "#c4d4a4",
        "stream" | "water" => "#a9cbe3",
        "mud" => "#cbbfa6",
        _ => "#ffffff",
    }
}

fn draw(ground: &BattleState, heat: &Heat, names: &[String; 2], title: &str) -> String {
    let (mut w, mut h) = (0, 0);
    for (hex, _) in ground.world.iter() {
        let [c, r] = hex_to_offset(hex);
        w = w.max(c + 1);
        h = h.max(r + 1);
    }
    let pw = 3f64.sqrt() * R * (f64::from(w) + 0.5) + 4.0;
    let ph = 1.5 * R * f64::from(h) + R + 4.0;
    let head = 26.0;
    let gap = 12.0;
    let rows = LAYERS.len() + 1;
    let total_w = 2.0 * pw + 3.0 * gap;
    let total_h = 56.0 + rows as f64 * (ph + head + gap);
    let per = f64::from(heat.battles.max(1));

    // The ground, drawn once and reused by every panel.
    let mut base = String::new();
    let objective: std::collections::BTreeSet<Cell> = ground
        .objectives()
        .flat_map(|(o, _)| o.hexes.iter().map(|h| hex_to_offset(*h)))
        .collect();
    for (hex, tile) in ground.world.iter() {
        let [c, r] = hex_to_offset(hex);
        // Height as shade: each level darkens the ground a step, so the
        // plateau, the slope and the valley read without a legend.
        writeln!(
            base,
            r##"<polygon points="{p}" fill="{}" stroke="#ffffff" stroke-width="0.3"/><polygon points="{p}" fill="#3a2a10" fill-opacity="{:.2}"/>"##,
            terrain_colour(tile.terrain),
            0.07 * f64::from(tile.elevation),
            p = hexagon(c, r),
        )
        .ok();
    }
    let mut marks = String::new();
    for &[c, r] in &objective {
        writeln!(
            marks,
            r##"<polygon points="{}" fill="none" stroke="#222" stroke-width="1.6"/>"##,
            hexagon(c, r)
        )
        .ok();
    }
    let _ = offset_to_hex;

    let mut svg = String::new();
    writeln!(
        svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{total_w:.0}" height="{total_h:.0}" font-family="sans-serif" font-size="12">"##
    )
    .ok();
    writeln!(
        svg,
        r##"<rect width="100%" height="100%" fill="#fbfaf7"/>"##
    )
    .ok();
    writeln!(
        svg,
        r##"<text x="{gap}" y="18" font-size="15" font-weight="bold">{title}</text><text x="{gap}" y="34" fill="#555">{} won {}, {} won {}, of {}.</text><text x="{gap}" y="50" fill="#555">Values per battle; both sides of a row share one scale. Black outlines: objectives. Darker ground is higher.</text>"##,
        names[0], heat.wins[0], names[1], heat.wins[1], heat.battles
    )
    .ok();

    let panel = |svg: &mut String, row: usize, col: usize, label: &str, body: &str| {
        let x = gap + col as f64 * (pw + gap);
        let y = 56.0 + row as f64 * (ph + head + gap);
        writeln!(
            svg,
            r##"<g transform="translate({x:.1},{y:.1})"><text x="0" y="16" font-weight="bold" fill="{}">{label}</text><g transform="translate(0,{head})">{base}{body}{marks}</g></g>"##,
            SIDE_COLOUR[col]
        )
        .ok();
    };

    for (l, layer) in LAYERS.iter().enumerate() {
        let max = heat.layers[l]
            .iter()
            .flat_map(|m| m.values())
            .fold(0.0f64, |a, b| a.max(*b));
        for s in 0..2 {
            let mut body = String::new();
            for (&[c, r], &n) in &heat.layers[l][s] {
                // Under 2% of the row's busiest hex is a stray, not a pattern.
                if n < 0.02 * max {
                    continue;
                }
                let a = 0.08 + 0.8 * (n / max).powf(0.7);
                writeln!(
                    body,
                    r##"<polygon points="{}" fill="{}" fill-opacity="{a:.2}"/>"##,
                    hexagon(c, r),
                    SIDE_COLOUR[s]
                )
                .ok();
            }
            if l == HURT {
                for (&[c, r], &n) in &heat.lost[s] {
                    let (x, y) = centre(c, r);
                    let k = 2.5 + 2.5 * (n / per).sqrt().min(2.0);
                    writeln!(
                        body,
                        r##"<path d="M{:.1},{:.1}L{:.1},{:.1}M{:.1},{:.1}L{:.1},{:.1}" stroke="#000" stroke-width="1.8"/>"##,
                        x - k, y - k, x + k, y + k, x - k, y + k, x + k, y - k
                    )
                    .ok();
                }
            }
            let label = format!(
                "{} — {layer}{} (row max {:.1})",
                names[s],
                if l == HURT { ", × = lost" } else { "" },
                max / per
            );
            panel(&mut svg, l, s, &label, &body);
        }
    }

    let lmax = heat
        .lines
        .iter()
        .flat_map(|m| m.values())
        .fold(0.0f64, |a, b| a.max(*b));
    for s in 0..2 {
        let mut body = String::new();
        let mut v: Vec<_> = heat.lines[s].iter().collect();
        v.sort_by(|a, b| a.1.total_cmp(b.1).then(a.0.cmp(b.0)));
        for (&(f, t), &n) in v.iter().rev().take(150).rev().copied() {
            let (x1, y1) = centre(f[0], f[1]);
            let (x2, y2) = centre(t[0], t[1]);
            let k = (n / lmax).sqrt();
            writeln!(
                body,
                r##"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="{}" stroke-opacity="{:.2}" stroke-width="{:.1}"/><circle cx="{x2:.1}" cy="{y2:.1}" r="{:.1}" fill="{}" fill-opacity="0.6"/>"##,
                SIDE_COLOUR[s],
                0.25 + 0.6 * k,
                0.6 + 3.5 * k,
                0.8 + 1.5 * k,
                SIDE_COLOUR[s]
            )
            .ok();
        }
        let label = format!(
            "{} — lines of fire, dot at the target (busiest 150 pairs, max {:.1})",
            names[s],
            lmax / per
        );
        panel(&mut svg, LAYERS.len(), s, &label, &body);
    }
    svg.push_str("</svg>\n");
    svg
}
