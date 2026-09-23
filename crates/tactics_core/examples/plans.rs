//! Does a commander who knows a play beat one who does not, and does a
//! better commander play it better? PLANNING.md step 4's instrument.
//!
//! ```sh
//! cargo run --release -p tactics_core --example plans              # 48 battles a row
//! cargo run --release -p tactics_core --example plans -- 96        # more
//! cargo run --release -p tactics_core --example plans -- 96 1000   # at a seed offset
//! ```
//!
//! The ridge arena, because it is the ground where a covered route to a flank
//! exists and a wrong choice is punished. Four medium tanks a side in two
//! formations of two, under the `command` planner, ends alternating game by
//! game so neither column is a statement about deployment. What varies is
//! only what the commanders know and who they are:
//!
//! - **control**: nobody knows the play (a copy of bounding overwatch that
//!   teaches nothing) — the win column should be level.
//! - **A taught**: A's academy teaches fix and flank, B's does not.
//! - **both taught**: the play against itself.
//! - **A strong / A weak**: both taught; one side's commander is Irma Krieger
//!   (`command` 14, `will` 13) and the other's a cadet with no command
//!   training — planning strength is the commander's stats.
//!
//! A mirror match hides a symmetric rule (the "symmetric null"), which is why
//! every row but the control differs between the two sides.

use tactics_core::ai::{AiConfig, AiDriver, make_battle_planner};
use tactics_core::battle::{BattleState, Event, SideState};
use tactics_core::data::{ArmorFacing, DataRegistry};
use tactics_core::harness::arena::RIDGE_ARENA;
use tactics_core::harness::parallel::run_all;
use tactics_core::map::UnitPlacement;
use tactics_core::roster::Roster;

#[derive(Default, Clone, Copy)]
struct Tally {
    a: u32,
    b: u32,
    draws: u32,
    rounds: u32,
    a_plans: u32,
    a_goes: u32,
    b_plans: u32,
    hits: u32,
    flank_hits: u32,
    a_flank_hits: u32,
    a_hits: u32,
    by_score: u32,
}

/// (A's doctrine, A's commander, B's doctrine, B's commander).
type Row = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

const ROWS: &[Row] = &[
    ("control", "untaught", "anka", "untaught", "anka"),
    ("A taught", "taught", "anka", "untaught", "anka"),
    ("both taught", "taught", "anka", "taught", "anka"),
    ("A strong", "taught", "irma", "taught", "mina"),
    ("A weak", "taught", "mina", "taught", "irma"),
];

/// A side's hulls in pairs, leader first: each pair is one formation.
type Force = &'static [&'static str];

const MEDIUMS: Force = &["medium_tank"; 4];
/// Heavy tanks leading both formations: fronts the 75 cannot touch, which is
/// the case a flank exists for.
const HEAVIES: Force = &["heavy_tank", "medium_tank", "heavy_tank", "medium_tank"];
/// Three groups a side: a line to fix with, a group to go round, and one
/// left over — the arena's whole deployment, six pairs.
const MEDIUMS_3: Force = &["medium_tank"; 6];
const HEAVIES_3: Force = &[
    "heavy_tank",
    "medium_tank",
    "heavy_tank",
    "medium_tank",
    "heavy_tank",
    "medium_tank",
];

fn battle(reg: &DataRegistry, row: &Row, a_force: Force, b_force: Force, seed: u64) -> Tally {
    let arena = &RIDGE_ARENA;
    let flip = seed % 2 == 1;
    let (_, a_doc, a_cmd, b_doc, b_cmd) = *row;
    let (west, east) = if flip {
        ((b_doc, b_cmd), (a_doc, a_cmd))
    } else {
        ((a_doc, a_cmd), (b_doc, b_cmd))
    };
    let groups = a_force.len() / 2;
    let declared: Vec<(String, u8)> = (1..=groups)
        .flat_map(|g| [(format!("w{g}"), 0u8), (format!("e{g}"), 1u8)])
        .collect();
    let declared: Vec<(&str, u8)> = declared.iter().map(|(id, s)| (id.as_str(), *s)).collect();
    let map = arena
        .map_with_formations(&declared)
        .expect("the ridge builds");
    let pairs = arena.deployment();
    let mut placements = Vec::new();
    let (west_force, east_force) = if flip {
        (b_force, a_force)
    } else {
        (a_force, b_force)
    };
    for (side, (_, commander), force) in [(0u8, west, west_force), (1u8, east, east_force)] {
        // Formation one on one flank, two on the other (pairs 0 and 2 share a
        // flank, as do 1 and 3), and a third, if there is one, on pairs 4
        // and 5 behind them.
        let slots: &[(usize, usize, bool)] = &[
            (0, 1, true),
            (2, 1, false),
            (1, 2, true),
            (3, 2, false),
            (4, 3, true),
            (5, 3, false),
        ];
        for (slot, &(pair, formation, leads)) in slots.iter().enumerate().take(force.len()) {
            let (w, e) = pairs[pair];
            let at = if side == 0 { w } else { e };
            let crew = if slot == 0 {
                vec![commander.to_string()]
            } else {
                Vec::new()
            };
            placements.push(UnitPlacement {
                aboard_at: None,
                at: tactics_core::hex_to_offset(at),
                side,
                vehicle: force[slot].into(),
                crew,
                name: None,
                facing: None,
                formation: Some(format!("{}{formation}", if side == 0 { "w" } else { "e" })),
                leads,
            });
        }
    }
    let (roster, crews) = Roster::stamp_for(reg, &placements);
    let sides = ["West", "East"]
        .map(|name| SideState {
            name: name.into(),
            ai: None,
        })
        .to_vec();
    let mut state = BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
    .expect("the staged placements are content the base mod ships");
    let mut ai = AiDriver::new();
    for (side, (doctrine, _)) in [(0u8, west), (1u8, east)] {
        let cfg = AiConfig {
            planner: "command".into(),
            difficulty: 3,
            doctrine: Some(doctrine.into()),
        };
        ai.insert(
            side,
            make_battle_planner(&cfg, seed * 31 + side as u64, reg),
        );
    }
    let a_side = if flip { 1 } else { 0 };
    let mut t = Tally::default();
    let mut rounds = 0;
    let trace = std::env::var("PLANS_TRACE")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        == Some(seed);
    while !state.is_over() && rounds < 60 {
        ai.plan_round_with(reg, &mut state, |d| {
            let ours = d.side == a_side;
            if trace {
                for e in &d.events {
                    if matches!(
                        e,
                        Event::PlanAdopted { .. }
                            | Event::PlanGoing { .. }
                            | Event::PlanDone { .. }
                            | Event::PlanDropped { .. }
                            | Event::MissionAssigned { .. }
                    ) {
                        println!("  r{} side {}: {e:?}", rounds + 1, d.side);
                    }
                }
            }
            for e in &d.events {
                match e {
                    Event::PlanAdopted { .. } if ours => t.a_plans += 1,
                    Event::PlanAdopted { .. } => t.b_plans += 1,
                    Event::PlanGoing { .. } if ours => t.a_goes += 1,
                    _ => {}
                }
            }
        });
        if trace {
            for u in state.units.iter() {
                let f = state
                    .formation_of(u.id)
                    .map(|f| f.id.clone())
                    .unwrap_or_default();
                println!(
                    "    {f} {:?} {} at {:?} {}",
                    u.id,
                    u.vehicle,
                    arena.axis_key(u.pos),
                    if u.alive() { "" } else { "DEAD" }
                );
            }
            for p in state.plans(a_side) {
                println!(
                    "    A plan: {:?} fire {:?} assembly {:?} flank {:?} target {:?}",
                    p.phase,
                    arena.axis_key(p.fire_position),
                    arena.axis_key(p.assembly),
                    arena.axis_key(p.flank),
                    p.target
                );
            }
        }
        for e in state.resolve_round(reg) {
            if let Event::ShotHit {
                attacker, facing, ..
            }
            | Event::ShotBounced {
                attacker, facing, ..
            } = e
            {
                let flank = facing != ArmorFacing::Front;
                t.hits += 1;
                t.flank_hits += flank as u32;
                if state.units.get(attacker.index()).map(|u| u.side) == Some(a_side) {
                    t.a_hits += 1;
                    t.a_flank_hits += flank as u32;
                }
            }
        }
        rounds += 1;
    }
    t.rounds = rounds;
    t.by_score = matches!(
        state.over.map(|r| r.reason),
        Some(tactics_core::battle::EndReason::Objectives)
    ) as u32;
    match state.over.and_then(|r| r.winner) {
        Some(s) if s == a_side => t.a = 1,
        Some(_) => t.b = 1,
        None => t.draws = 1,
    }
    t
}

/// One battle on a shipped battlefield with both sides under the command
/// planner, each side's doctrine as given. Returns (winner, plans adopted by
/// each side, flank share of hits).
fn map_battle(
    reg: &DataRegistry,
    map: &str,
    doctrines: [&str; 2],
    seed: u64,
) -> (Option<u8>, [u32; 2], u32, u32) {
    let mut state = BattleState::from_map(reg, map, seed).expect("a shipped battlefield");
    let mut ai = AiDriver::new();
    for side in 0..2u8 {
        let cfg = AiConfig {
            planner: "command".into(),
            difficulty: 3,
            doctrine: Some(doctrines[side as usize].into()),
        };
        ai.insert(
            side,
            make_battle_planner(&cfg, seed * 17 + side as u64, reg),
        );
    }
    let (mut plans, mut hits, mut flank) = ([0u32; 2], 0, 0);
    let mut rounds = 0;
    while !state.is_over() && rounds < 60 {
        ai.plan_round_with(reg, &mut state, |d| {
            for e in &d.events {
                if matches!(e, Event::PlanAdopted { .. }) {
                    plans[d.side as usize] += 1;
                }
            }
        });
        for e in state.resolve_round(reg) {
            if let Event::ShotHit { facing, .. } | Event::ShotBounced { facing, .. } = e {
                hits += 1;
                flank += (facing != ArmorFacing::Front) as u32;
            }
        }
        rounds += 1;
    }
    (state.over.and_then(|r| r.winner), plans, hits, flank)
}

fn maps(reg: &DataRegistry, games: u64, offset: u64) {
    println!("shipped battlefields, both sides under the command planner, {games} battles a row\n");
    println!(
        "{:<16} {:<12} {:>5} {:>5} {:>5} {:>8} {:>8} {:>7}",
        "map", "row", "west", "east", "draw", "W plans", "E plans", "flank%"
    );
    let mut ids: Vec<&str> = reg
        .maps
        .values()
        .filter(|m| m.kind == tactics_core::map::MapKind::Battle)
        .map(|m| m.id.as_str())
        .collect();
    ids.sort_unstable();
    let (mut gain, mut n) = (0i32, 0);
    for map in ids {
        let mut control = [0u32; 2];
        for (row, docs) in [
            ("control", ["untaught", "untaught"]),
            ("west taught", ["taught", "untaught"]),
            ("east taught", ["untaught", "taught"]),
        ] {
            let seeds: Vec<u64> = (offset..offset + games).collect();
            let out = run_all(&seeds, |s| map_battle(reg, map, docs, *s));
            let mut w = [0u32; 3];
            let (mut plans, mut hits, mut flank) = ([0u32; 2], 0, 0);
            for (winner, p, h, f) in out {
                w[winner.map(|s| s as usize).unwrap_or(2)] += 1;
                plans[0] += p[0];
                plans[1] += p[1];
                hits += h;
                flank += f;
            }
            match row {
                "control" => control = [w[0], w[1]],
                "west taught" => {
                    gain += w[0] as i32 - control[0] as i32;
                    n += 1;
                }
                _ => {
                    gain += w[1] as i32 - control[1] as i32;
                    n += 1;
                }
            }
            println!(
                "{:<16} {:<12} {:>5} {:>5} {:>5} {:>8.2} {:>8.2} {:>6.0}%",
                map,
                row,
                w[0],
                w[1],
                w[2],
                plans[0] as f32 / games as f32,
                plans[1] as f32 / games as f32,
                100.0 * flank as f32 / hits.max(1) as f32
            );
        }
    }
    println!(
        "\n  knowing the play, summed over every map and both seatings: {gain:+} wins in {} battles",
        n as u64 * games
    );
}

fn main() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (mut reg, _) = DataRegistry::load_dir(&root).expect("mods load");
    let mut untaught = reg
        .doctrine("bounding_overwatch")
        .expect("the base mod ships bounding overwatch")
        .clone();
    untaught.id = "untaught".into();
    untaught.teaches.clear();
    // No shipped doctrine teaches the play yet (it has not been shown to win),
    // so the instrument teaches it here: the same doctrine, one play richer.
    let mut taught = untaught.clone();
    taught.id = "taught".into();
    taught.teaches = vec!["fix_and_flank".into()];
    reg.doctrines.insert(untaught.id.clone(), untaught);
    // PLANS_SET="playout_samples=4,playout_margin=0.1" overrides planner
    // numbers for a quick comparison without editing the mod.
    if let Ok(set) = std::env::var("PLANS_SET") {
        for pair in set.split(',') {
            let Some((k, v)) = pair.split_once('=') else {
                continue;
            };
            match k {
                "playout_samples" => reg.planner.playout_samples = v.parse().unwrap(),
                "playout_rounds" => reg.planner.playout_rounds = v.parse().unwrap(),
                "playout_margin" => reg.planner.playout_margin = v.parse().unwrap(),
                other => panic!("PLANS_SET knows no `{other}`"),
            }
        }
    }
    reg.doctrines.insert(taught.id.clone(), taught);

    let args: Vec<u64> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    if std::env::args().any(|a| a == "--maps") {
        let games = args.first().copied().unwrap_or(24);
        maps(&reg, games, args.get(1).copied().unwrap_or(0));
        return;
    }
    let games = args.first().copied().unwrap_or(48);
    let offset = args.get(1).copied().unwrap_or(0);
    for (label, a_force, b_force) in [
        ("mediums against mediums, two groups", MEDIUMS, MEDIUMS),
        ("mediums against heavies, two groups", MEDIUMS, HEAVIES),
        (
            "mediums against mediums, three groups",
            MEDIUMS_3,
            MEDIUMS_3,
        ),
        (
            "mediums against heavies, three groups",
            MEDIUMS_3,
            HEAVIES_3,
        ),
    ] {
        println!(
            "ridge arena, {label}: A is 4 medium tanks, two formations a side, command planner, {games} battles a row\n"
        );
        println!(
            "{:<12} {:>5} {:>5} {:>5} {:>6} {:>8} {:>8} {:>8} {:>9} {:>9}",
            "row", "A", "B", "draw", "rounds", "A plans", "A go", "B plans", "flank%", "A flank%"
        );
        for row in ROWS {
            let seeds: Vec<u64> = (offset..offset + games).collect();
            let tallies = run_all(&seeds, |s| battle(&reg, row, a_force, b_force, *s));
            let mut t = Tally::default();
            for x in tallies {
                t.a += x.a;
                t.b += x.b;
                t.draws += x.draws;
                t.rounds += x.rounds;
                t.a_plans += x.a_plans;
                t.a_goes += x.a_goes;
                t.b_plans += x.b_plans;
                t.hits += x.hits;
                t.flank_hits += x.flank_hits;
                t.a_hits += x.a_hits;
                t.a_flank_hits += x.a_flank_hits;
                t.by_score += x.by_score;
            }
            eprint!("[{} by score] ", t.by_score);
            let per = |n: u32| n as f32 / games as f32;
            println!(
                "{:<12} {:>5} {:>5} {:>5} {:>6.1} {:>8.2} {:>8.2} {:>8.2} {:>8.0}% {:>8.0}%",
                row.0,
                t.a,
                t.b,
                t.draws,
                per(t.rounds),
                per(t.a_plans),
                per(t.a_goes),
                per(t.b_plans),
                100.0 * t.flank_hits as f32 / t.hits.max(1) as f32,
                100.0 * t.a_flank_hits as f32 / t.a_hits.max(1) as f32,
            );
        }
        println!();
    }
    println!(
        "\n  `plans` and `go` are per battle: plans adopted (a replacement counts again) and\n  \
         words to go given. `flank%` is the share of all hits and bounces that struck a side\n  \
         or rear plate; `A flank%` the same for side A's guns. At {games} battles a level\n  \
         pairing wanders several wins either way — sweep the seed offset before believing a gap."
    );
}
