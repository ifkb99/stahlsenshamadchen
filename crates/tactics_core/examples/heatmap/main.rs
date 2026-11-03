//! Where a map's battles actually happen, and whether they are the battles
//! it was drawn for.
//!
//! ```text
//! heatmap --map slice_hollerbach --games 16 [--swap] [--map-ai] [--vehicle light_tank]
//! heatmap --seat <dir>...                  # hand games from examples/command
//! heatmap ... --json a.json                # keep the counts
//! heatmap --diff a.json b.json             # what changed between two runs
//! heatmap --sight --map slice_hollerbach [--range 8] [--from 13,10 ...]
//! heatmap ... --timeline                   # every unit, round by round, as text
//! ```
//! (`cargo run --release -p tactics_core --example heatmap -- <args>`.)
//!
//! A map is drawn with a battle in mind: this lane is the covered approach,
//! that knoll the base of fire. Every other instrument here reports who won
//! and what the shot did; none says whether the battle fought is the one
//! the map was drawn for. This one counts, per hex and per side:
//!
//! - **moved**: hexes entered (`UnitMoved` paths): the routes.
//! - **stood**: unit-rounds standing there at a round's end.
//! - **fired from**: `ShotFired.from`: the firing positions.
//! - **hurt**: where a side's units were hit, with a cross per unit lost.
//! - **seen**: where the enemy spotted a side's units, with each battle's
//!   first contact dashed from the nearest spotter.
//! - **pinned**: `PinnedDown`.
//! - **lines of fire**: from-to pairs, thicker for more shots.
//!
//! and prints the busiest hexes, shots and spots by range, and **orders
//! kept**: the share of crews whose plan kept the ground their orders named
//! (order against intent), and whose round ended where they planned (intent
//! against outcome). `--timeline` prints each battle's units round by round.
//!
//! `--diff` draws B minus A from two `--json` files: the before-and-after
//! of a map edit or a rule change. `--sight` reads only the ground: how much
//! each hex overlooks, and what chosen hexes see, so a map's sight lines can
//! be checked before anybody fights on it.
//!
//! AI battles use the `playthrough` example's seats (utility planners,
//! `massed_armor` against `elastic_defense`; `--swap` exchanges the
//! doctrines, `--map-ai` uses the map's own `ai` blocks). The picture is an
//! SVG (`--out`, default `target/heatmap.svg`).

mod draw;
mod log;
mod tally;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use tactics_core::ai::{AiConfig, AiDriver, make_battle_planner};
use tactics_core::battle::BattleState;
use tactics_core::data::DataRegistry;
use tactics_core::save;
use tactics_core::{hex_to_offset, offset_to_hex};

use log::Round;
use tally::{BUCKETS, Cell, Heat, LAYERS, Tally};

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (registry, _) = DataRegistry::load_dir(&root).expect("mods load");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| -> Option<String> {
        let i = args.iter().position(|a| a == name)?;
        args.get(i + 1).cloned()
    };
    let many = |name: &str| -> Vec<String> {
        match args.iter().position(|a| a == name) {
            Some(i) => args[i + 1..]
                .iter()
                .take_while(|a| !a.starts_with("--"))
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    };
    let has = |name: &str| args.iter().any(|a| a == name);
    let out = PathBuf::from(flag("--out").unwrap_or_else(|| "target/heatmap.svg".into()));
    let map = flag("--map").unwrap_or_else(|| "slice_hollerbach".into());

    let svg = if has("--diff") {
        let files = many("--diff");
        assert!(files.len() == 2, "--diff takes two json files");
        let read = |p: &str| -> Heat {
            serde_json::from_str(&std::fs::read_to_string(p).expect("json reads"))
                .expect("json parses")
        };
        let (a, b) = (read(&files[0]), read(&files[1]));
        let ground = BattleState::from_map(&registry, &b.map, 1).expect("battle");
        compare(&a, &b);
        draw::diff(&ground, &a, &b)
    } else if has("--sight") {
        let range: u32 = flag("--range").and_then(|s| s.parse().ok()).unwrap_or(8);
        let ground = BattleState::from_map(&registry, &map, 1).expect("battle");
        let froms: Vec<Cell> = many("--from")
            .iter()
            .filter_map(|s| {
                let (c, r) = s.split_once(',')?;
                Some([c.parse().ok()?, r.parse().ok()?])
            })
            .collect();
        sight(&ground, &map, range, &froms)
    } else {
        let mut tally = Tally::new(flag("--vehicle"));
        let rungs: Vec<(String, String)> = registry
            .morale
            .rungs
            .iter()
            .map(|r| (r.id.clone(), r.name.clone()))
            .collect();
        let timeline = has("--timeline");
        let ground: BattleState;
        let title: String;
        let seats = many("--seat");
        if !seats.is_empty() {
            let mut last = None;
            for dir in seats.iter().map(PathBuf::from) {
                let (game, _) = save::read(&registry, dir.join("battle.json")).expect("save reads");
                let state = game.battle.expect("the save holds a battle");
                let text = std::fs::read_to_string(dir.join("rounds.jsonl")).expect("rounds.jsonl");
                let rounds: Vec<Round> = text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(|l| serde_json::from_str(l).expect("a round parses"))
                    .collect();
                let lanes = tally.battle(&rounds, &rungs);
                if timeline {
                    println!("== {}", dir.display());
                    tally::print_timeline(&lanes, &side_names(&state));
                }
                last = Some(state);
            }
            ground = last.expect("at least one --seat directory");
            title = format!("{map}: {} hand-fought battle(s)", seats.len());
        } else {
            let games: u64 = flag("--games").and_then(|s| s.parse().ok()).unwrap_or(16);
            let first: u64 = flag("--seed").and_then(|s| s.parse().ok()).unwrap_or(1);
            let planner = flag("--planner").unwrap_or_else(|| "utility".into());
            let map_ai = has("--map-ai");
            let doctrines = if has("--swap") {
                ["elastic_defense", "massed_armor"]
            } else {
                ["massed_armor", "elastic_defense"]
            };
            for seed in first..first + games {
                let rounds = fight(&registry, &map, seed, &planner, doctrines, map_ai);
                let lanes = tally.battle(&rounds, &rungs);
                if timeline {
                    println!("== seed {seed}");
                    let state = BattleState::from_map(&registry, &map, seed).expect("battle");
                    tally::print_timeline(&lanes, &side_names(&state));
                }
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
        let mut heat = tally.finish();
        heat.map = map.clone();
        heat.names = side_names(&ground);
        heat.title = match &heat.only {
            Some(v) => format!("{title}; {v} only"),
            None => title,
        };
        report(&heat);
        if let Some(path) = flag("--json") {
            std::fs::write(&path, serde_json::to_string(&heat).expect("json"))
                .expect("json writes");
            println!("counts kept in {path}");
        }
        draw::heat(&ground, &heat)
    };
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(&out, svg).expect("svg writes");
    println!("drawn to {}", out.display());
}

fn side_names(state: &BattleState) -> [String; 2] {
    [state.sides[0].name.clone(), state.sides[1].name.clone()]
}

/// One AI battle, logged round by round as the seat logs a hand game.
fn fight(
    registry: &DataRegistry,
    map: &str,
    seed: u64,
    planner: &str,
    doctrines: [&str; 2],
    map_ai: bool,
) -> Vec<Round> {
    let mut state = BattleState::from_map(registry, map, seed).expect("battle");
    let mut ai = AiDriver::new();
    for side in 0..2u8 {
        let shipped = AiConfig {
            planner: planner.into(),
            difficulty: 4,
            doctrine: Some(doctrines[side as usize].into()),
        };
        let config = match &state.sides[side as usize].ai {
            Some(c) if map_ai => c.clone(),
            _ => shipped,
        };
        ai.insert(
            side,
            make_battle_planner(&config, seed + u64::from(side), registry),
        );
    }
    let mut rounds = vec![Round {
        round: 0,
        events: Vec::new(),
        units: log::placed(&state),
        intentions: Vec::new(),
    }];
    while !state.is_over() && rounds.len() <= 200 {
        let mut planning = Vec::new();
        ai.plan_round_with(registry, &mut state, |d| {
            planning.extend(d.events.iter().cloned())
        });
        let intentions = log::intentions(&state, &planning);
        let round = state.round;
        let events = state.resolve_round(registry);
        rounds.push(Round {
            round,
            events,
            units: log::placed(&state),
            intentions,
        });
    }
    rounds
}

fn pct(n: u32, of: u32) -> String {
    if of == 0 {
        "  -".into()
    } else {
        format!("{:>3.0}%", 100.0 * f64::from(n) / f64::from(of))
    }
}

fn range_rows(label: &str, names: &[String; 2], rows: &[[u32; BUCKETS]; 2]) {
    println!("{label} (hexes; % of the side's):");
    print!("{:>29}", "");
    for d in 0..BUCKETS {
        print!(
            "{:>4}",
            if d + 1 == BUCKETS {
                format!("{d}+")
            } else {
                d.to_string()
            }
        );
    }
    println!();
    for (side, row) in names.iter().zip(rows) {
        let total: u32 = row.iter().sum();
        print!("{side:>16} ({total:>6}) ");
        for n in row {
            print!("{:>4.0}", 100.0 * f64::from(*n) / f64::from(total.max(1)));
        }
        println!();
    }
}

/// The text half: who won, the busiest hexes, ranges, contacts, orders.
fn report(heat: &Heat) {
    let per = f64::from(heat.battles.max(1));
    let names = &heat.names;
    println!("{}", heat.title);
    println!(
        "{} wins {}, {} wins {}, of {}; {:.1} rounds a battle",
        names[0],
        heat.wins[0],
        names[1],
        heat.wins[1],
        heat.battles,
        f64::from(heat.rounds) / per
    );
    let top = |m: &Vec<(Cell, f64)>| -> String {
        let mut v: Vec<_> = m.iter().collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let total: f64 = m.iter().map(|x| x.1).sum();
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
    range_rows("shots by range", names, &heat.ranges);
    range_rows(
        "spots by range from the spotting side's nearest crew",
        names,
        &heat.spot_ranges,
    );
    for (s, side) in names.iter().enumerate() {
        let f = &heat.first[s];
        if f.is_empty() {
            continue;
        }
        let rounds: f64 = f.iter().map(|c| f64::from(c.round)).sum::<f64>() / f.len() as f64;
        let range: f64 = f
            .iter()
            .map(|c| f64::from(log::distance(c.spotter, c.spotted)))
            .sum::<f64>()
            / f.len() as f64;
        println!(
            "{side} first saw the enemy in round {rounds:.1} on average, at {range:.1} hexes ({} battles)",
            f.len()
        );
    }
    println!("orders kept:");
    for (s, side) in names.iter().enumerate() {
        println!(
            "{side:>16}: plan keeps the order {} of {}; round ends where planned {} of {}",
            pct(heat.kept[s], heat.orders[s]),
            heat.orders[s],
            pct(heat.arrived[s], heat.planned[s]),
            heat.planned[s]
        );
    }
}

/// The text half of a diff: wins, ranges, orders, side by side.
fn compare(a: &Heat, b: &Heat) {
    println!("A: {}\nB: {}", a.title, b.title);
    for (label, h) in [("A", a), ("B", b)] {
        let short: Vec<String> = (0..2)
            .map(|s| {
                let total: u32 = h.ranges[s].iter().sum();
                let close: u32 = h.ranges[s][..=2].iter().sum();
                format!(
                    "{} wins {}, shots within 2 hexes {}, orders kept {}",
                    h.names[s],
                    h.wins[s],
                    pct(close, total),
                    pct(h.kept[s], h.orders[s])
                )
            })
            .collect();
        println!("{label} ({} battles): {}", h.battles, short.join("; "));
    }
}

/// The ground's sight lines: what every hex overlooks within `range`, and
/// what each chosen hex sees. With no hex chosen, each side's opening
/// centre and each objective are shown.
fn sight(ground: &BattleState, map: &str, range: u32, froms: &[Cell]) -> String {
    let cells: Vec<Cell> = ground.world.iter().map(|(h, _)| hex_to_offset(h)).collect();
    let view = |from: Cell| -> BTreeSet<Cell> {
        let a = offset_to_hex(from[0], from[1]);
        cells
            .iter()
            .filter(|c| {
                let b = offset_to_hex(c[0], c[1]);
                a.unsigned_distance_to(b) <= range && ground.sees_through(a, b)
            })
            .copied()
            .collect()
    };
    let overlook: BTreeMap<Cell, f64> = cells.iter().map(|&c| (c, view(c).len() as f64)).collect();
    let mut views: Vec<(String, Cell, BTreeSet<Cell>)> = Vec::new();
    if froms.is_empty() {
        for side in 0..2u8 {
            let here: Vec<Cell> = ground
                .units
                .iter()
                .filter(|u| u.side == side)
                .map(|u| hex_to_offset(u.pos))
                .collect();
            if here.is_empty() {
                continue;
            }
            let n = here.len() as i32;
            let mid = [
                here.iter().map(|c| c[0]).sum::<i32>() / n,
                here.iter().map(|c| c[1]).sum::<i32>() / n,
            ];
            views.push((
                format!("{}'s start", ground.sides[side as usize].name),
                mid,
                view(mid),
            ));
        }
        for (o, _) in ground.objectives() {
            if let Some(h) = o.hexes.first() {
                let c = hex_to_offset(*h);
                views.push((o.name.clone(), c, view(c)));
            }
        }
    } else {
        for &c in froms {
            views.push(("hex".into(), c, view(c)));
        }
    }
    let mut ranked: Vec<_> = overlook.iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(a.1).then(a.0.cmp(b.0)));
    let best: Vec<String> = ranked
        .iter()
        .take(8)
        .map(|(c, n)| format!("({},{}) {n:.0}", c[0], c[1]))
        .collect();
    println!(
        "hexes that see the most within {range}: {}",
        best.join("  ")
    );
    for (label, from, seen) in &views {
        println!(
            "{label} ({},{}) sees {} hexes",
            from[0],
            from[1],
            seen.len()
        );
    }
    draw::sight(ground, map, range, &overlook, &views)
}
