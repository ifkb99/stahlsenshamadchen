//! Rollouts for reinforcement learning (REIMAGINED.md L1.3).
//!
//! A learner — the utility executor with a network choosing goals by
//! *sampling* its softmax (`GoalBrain::Explore`) — fights the utility AI on a
//! mirrored arena, seats alternating by game so neither end's lean is
//! learned. Every learner decision is recorded with its log-probability, and
//! every round a ledger of every crew's condition and both sides' scores, so
//! the trainer can compute whatever reward it likes after the fact without
//! the engine having an opinion about rewards at all.
//!
//! ```sh
//! cargo run --release -p tactics_core --example rollout -- \
//!     --net target/goal_net.json --games 512 --out target/rl/iter0.jsonl
//! ```
//!
//! Inference runs here, in Rust, on every core: the network is a few
//! microseconds a decision, so there is nothing to gain by sending decisions
//! to a GPU one at a time, and the trainer only ever sees finished battles.

use std::io::Write;
use std::sync::{Arc, Mutex};
use tactics_core::ai::features::{GoalRecord, GoalTap};
use tactics_core::ai::learned::{GoalBrain, load_goal_net};
use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, UtilityPlanner, make_battle_planner};
use tactics_core::battle::{BattleState, Event, Order};
use tactics_core::data::ArmorFacing;
use tactics_core::data::DataRegistry;
use tactics_core::harness::arena::{DEFAULT_ARENA, arena_named};
use tactics_core::harness::parallel::run_all;

const USAGE: &str = "\
rollout: battles for reinforcement learning, learner against the utility AI

  --net PATH          the learner's goal network (required)
  --games N           battles (default 256)
  --seed S            first battle (default 0)
  --temperature T     the learner's sampling temperature (default 1.0)
  --events LIST       planner.goal_events: contact,hit,friend_lost,pinned,order, or all
  --places N          planner.goal_places (default 16)
  --arena NAME        battlefield (default ridge_arena)
  --difficulty D      the opponent's difficulty (default 5)
  --out PATH          JSON lines, one battle each (default target/rl/rollouts.jsonl)";

/// A battle no side has finished by now is a stalemate; the harness's cap.
const MAX_ROUNDS: u32 = 60;

#[derive(serde::Serialize)]
struct Entry {
    round: u32,
    unit: u32,
    side: u8,
    left: u32,
    full: u32,
    fighting: bool,
}

#[derive(serde::Serialize)]
struct Battle {
    game: u64,
    learner: u8,
    winner: Option<u8>,
    rounds: u32,
    decisions: Vec<GoalRecord>,
    ledger: Vec<Entry>,
    /// `[round, side 0's score, side 1's score]`, round 0 being the start.
    scores: Vec<[u32; 3]>,
    /// Strikes on armour — hits and bounces — by the face they struck,
    /// `[front, side, rear]`: the learner's on the enemy, and the enemy's on
    /// the learner. The instrument for whether she learned to get round, a
    /// label read after the fact and never shown to the network.
    struck_by_learner: [u32; 3],
    struck_on_learner: [u32; 3],
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn ledger(reg: &DataRegistry, state: &BattleState, round: u32, out: &mut Vec<Entry>) {
    for unit in &state.units {
        let (left, full) = state.substance(reg, unit);
        out.push(Entry {
            round,
            unit: unit.id.0,
            side: unit.side,
            left: if unit.alive() { left } else { 0 },
            full,
            fighting: unit.alive(),
        });
    }
}

struct Setup<'a> {
    reg: &'a DataRegistry,
    arena: &'static tactics_core::harness::arena::Arena,
    brain: GoalBrain,
    difficulty: u8,
}

fn fight(setup: &Setup, game: u64) -> Option<Battle> {
    let reg = setup.reg;
    let mut state = setup.arena.stage(reg, 9000 + game)?;
    let learner = (game % 2) as u8;
    let tap: GoalTap = Arc::new(Mutex::new(Vec::new()));
    let config = AiConfig {
        planner: "utility".into(),
        difficulty: setup.difficulty,
        doctrine: None,
    };
    let mut ai = AiDriver::new();
    for side in 0..2u8 {
        let seed = game.wrapping_mul(2).wrapping_add(side as u64);
        let planner: Box<dyn AiPlanner<BattleState, Order>> = if side == learner {
            Box::new(
                UtilityPlanner::from_config(&config, seed, reg)
                    .with_brain(setup.brain.clone())
                    .tapped(tap.clone()),
            )
        } else {
            make_battle_planner(&config, seed, reg)
        };
        ai.insert(side, planner);
    }
    let mut entries = Vec::new();
    let mut scores = vec![[0, state.score(0), state.score(1)]];
    ledger(reg, &state, 0, &mut entries);
    let mut rounds = 0;
    let (mut by, mut on) = ([0u32; 3], [0u32; 3]);
    while !state.is_over() && rounds < MAX_ROUNDS {
        ai.plan_round(reg, &mut state);
        for event in state.resolve_round(reg) {
            let (attacker, facing) = match event {
                Event::ShotHit {
                    attacker, facing, ..
                }
                | Event::ShotBounced {
                    attacker, facing, ..
                } => (attacker, facing),
                _ => continue,
            };
            let face = match facing {
                ArmorFacing::Front => 0,
                ArmorFacing::Side => 1,
                ArmorFacing::Rear => 2,
            };
            match state.unit(attacker).map(|u| u.side) {
                Some(side) if side == learner => by[face] += 1,
                Some(_) => on[face] += 1,
                None => {}
            }
        }
        rounds += 1;
        ledger(reg, &state, rounds, &mut entries);
        scores.push([rounds, state.score(0), state.score(1)]);
    }
    drop(ai);
    let decisions = tap.lock().map(|r| r.clone()).unwrap_or_default();
    Some(Battle {
        game,
        learner,
        winner: state.over.and_then(|r| r.winner),
        rounds,
        decisions,
        ledger: entries,
        scores,
        struck_by_learner: by,
        struck_on_learner: on,
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") || flag(&args, "--net").is_none() {
        println!("{USAGE}");
        return;
    }
    let parse = |name: &str, default: f64| -> f64 {
        flag(&args, name).map_or(default, |s| s.parse().unwrap_or_else(|_| panic!("{name}")))
    };
    let games = parse("--games", 256.0) as u64;
    let first = parse("--seed", 0.0) as u64;
    let temperature = parse("--temperature", 1.0) as f32;
    let difficulty = parse("--difficulty", 5.0) as u8;
    let out = std::path::PathBuf::from(flag(&args, "--out").unwrap_or("target/rl/rollouts.jsonl"));
    let arena = match flag(&args, "--arena") {
        None => arena_named("ridge_arena").unwrap_or(DEFAULT_ARENA),
        Some(name) => arena_named(name).unwrap_or_else(|| panic!("--arena {name}: no such arena")),
    };

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (mut reg, _) = DataRegistry::load_dir(&root).expect("mods load");
    reg.planner.goal_places = parse("--places", 16.0) as u32;
    if let Some(list) = flag(&args, "--events") {
        reg.planner.goal_events = tactics_core::data::GoalEvents::parse(list).unwrap_or_else(|e| {
            eprintln!("error: --events: {e}");
            std::process::exit(1);
        });
    }
    let path = flag(&args, "--net").expect("--net");
    let net = load_goal_net(std::path::Path::new(path)).unwrap_or_else(|err| {
        eprintln!("error: --net {path}: {err}");
        std::process::exit(1);
    });
    let setup = Setup {
        reg: &reg,
        arena,
        brain: GoalBrain::Explore { net, temperature },
        difficulty,
    };

    let jobs: Vec<u64> = (first..first + games).collect();
    let fought = run_all(&jobs, |&game| fight(&setup, game));

    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).expect("create output directory");
    }
    let mut file = std::io::BufWriter::new(std::fs::File::create(&out).expect("open output"));
    let (mut won, mut lost, mut drawn, mut decisions) = (0, 0, 0, 0usize);
    let (mut by, mut on) = ([0u32; 3], [0u32; 3]);
    for battle in fought.iter().flatten() {
        match battle.winner {
            Some(w) if w == battle.learner => won += 1,
            Some(_) => lost += 1,
            None => drawn += 1,
        }
        decisions += battle.decisions.len();
        for i in 0..3 {
            by[i] += battle.struck_by_learner[i];
            on[i] += battle.struck_on_learner[i];
        }
        serde_json::to_writer(&mut file, battle).expect("write battle");
        file.write_all(b"\n").expect("write battle");
    }
    file.flush().expect("flush output");
    println!(
        "{games} battles on {}: learner won {won}, lost {lost}, drew {drawn}; {decisions} decisions",
        arena.id
    );
    let share = |f: [u32; 3]| {
        let total = f.iter().sum::<u32>().max(1) as f32;
        format!(
            "front {:.0}% side {:.0}% rear {:.0}% of {}",
            100.0 * f[0] as f32 / total,
            100.0 * f[1] as f32 / total,
            100.0 * f[2] as f32 / total,
            f.iter().sum::<u32>()
        )
    };
    println!("  struck by the learner: {}", share(by));
    println!("  struck on the learner: {}", share(on));
}
