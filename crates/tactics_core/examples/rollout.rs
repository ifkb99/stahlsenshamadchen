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
use tactics_core::harness::arena::{Arena, DEFAULT_ARENA, arena_named};
use tactics_core::harness::parallel::run_all;
use tactics_core::harness::{forces, generated};

const USAGE: &str = "\
rollout: battles for reinforcement learning, learner against the utility AI

  --net PATH          the learner's goal network (required); `utility` makes
                      the learner the utility AI itself, the control a
                      network's per-force record is read against
  --games N           battles (default 256)
  --seed S            first battle (default 0)
  --temperature T     the learner's sampling temperature (default 1.0)
  --opponents A,B     league networks the learner also fights, in turn with
                      the utility AI (two games each, so both seatings)
  --events LIST       planner.goal_events: contact,hit,friend_lost,pinned,order, or all
  --places N          planner.goal_places (default 16)
  --approach N        planner.approach_places: places an enemy, round his arcs (default 0)
  --arena NAME        battlefield (default ridge_arena); with --ground generated,
                      whose forces and deployment are moved onto generated ground
  --ground generated  a fresh generated world every two games (one per seat)
  --spread PERCENT    with --ground generated, the deployment's distance from
                      the centre in percent of the arena's (default 100)
  --turned            with --ground generated, turn each world's axis of
                      advance by a sixth of a circle a step, drawn per world
  --forces LIST|all   draw each world's matchup from these forces (forces/ in
                      the mod), each side fighting by its force's doctrine
  --deterministic     the learner takes its best choice instead of sampling
                      (evaluation; no log-probabilities are recorded)
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
    /// The score that wins the battle outright, if the scenario names one:
    /// what a point of objective margin is a share of.
    victory_score: Option<u32>,
    /// Who the learner fought: `utility`, or a league network's file name.
    opponent: String,
    /// The two forces, side 0's first, when drawn from `--forces`.
    forces: Option<[String; 2]>,
    /// Strikes on armour — hits and bounces — by the face they struck,
    /// `[front, side, rear]`: the learner's on the enemy, and the enemy's on
    /// the learner. The instrument for whether she learned to get round, a
    /// label read after the fact and never shown to the network.
    struck_by_learner: [u32; 3],
    struck_on_learner: [u32; 3],
    /// Fix-and-flank, read after the fact, `[learner, opponent]`: a target
    /// struck on his front by one crew and then on his side or rear by
    /// another of the same side within three rounds. A label for the
    /// measurer, never shown to the network.
    fix_and_flank: [u32; 2],
    /// Shots aimed at somebody, `[learner, opponent]`, and of those the ones
    /// fired from beyond the target's own longest direct reach: standoff.
    aimed: [u32; 2],
    standoff: [u32; 2],
}

/// A unit's longest direct weapon reach, in hexes: what she could answer
/// with.
fn reach(reg: &DataRegistry, state: &BattleState, unit: tactics_core::battle::UnitId) -> i32 {
    state
        .unit(unit)
        .and_then(|u| reg.vehicle(&u.vehicle))
        .map(|v| {
            v.weapons
                .iter()
                .filter_map(|w| reg.weapon(w))
                .filter(|w| !w.indirect)
                .map(|w| w.range[1] as i32)
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
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
    /// The league: past networks the learner also fights, each taking its
    /// turn with the utility AI by game number (`--opponents`). A learner
    /// that only ever meets one opponent learns that opponent.
    opponents: Vec<(String, std::sync::Arc<tactics_core::data::GoalNet>)>,
    /// Fight on a world the generator makes from the game's seed
    /// (`harness::generated`) instead of on the arena's own ground: two games
    /// a world, so the learner has each seat on it.
    generated: bool,
    /// How the deployment is laid on generated ground (`--spread`, `--turned`).
    layout: generated::Layout,
    /// The forces battles are drawn from (`--forces`): each world gets a
    /// matchup, both seatings, each side fighting by its force's doctrine.
    /// Empty is the arena's own symmetric force.
    forces: Vec<&'a tactics_core::data::ForceDef>,
}

/// Who the learner fights in `game`: the utility AI, or one of the league's
/// networks, in turn.
fn opponent<'a>(
    setup: &'a Setup,
    game: u64,
) -> Option<&'a (String, std::sync::Arc<tactics_core::data::GoalNet>)> {
    let turn = (game / 2) as usize % (setup.opponents.len() + 1);
    turn.checked_sub(1).and_then(|i| setup.opponents.get(i))
}

/// The two forces that meet on world `game / 2`, side 0's first: drawn from
/// the list by the world's seed, the same for both games on it, so the
/// learner fights each matchup from both ends.
fn matchup<'a>(setup: &Setup<'a>, game: u64) -> Option<[&'a tactics_core::data::ForceDef; 2]> {
    let n = setup.forces.len() as u64;
    if n == 0 {
        return None;
    }
    // SplitMix64, written out: the draw must not move with the platform.
    let mix = |mut z: u64| {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let world = game / 2;
    let a = (mix(world) % n) as usize;
    let b = (mix(world ^ 0xA5A5_A5A5) % n) as usize;
    Some([setup.forces[a], setup.forces[b]])
}

fn fight(setup: &Setup, game: u64) -> Option<Battle> {
    let reg = setup.reg;
    let matchup = matchup(setup, game);
    let (field, deployment) = if setup.generated {
        let made = generated::field_laid_out(reg, setup.arena, game / 2, setup.layout)?;
        (made.field, made.deployment)
    } else {
        (setup.arena.map()?, setup.arena.deployment())
    };
    let mut state = match matchup {
        Some(pair) => forces::stage(reg, field, &deployment, pair, 9000 + game)?,
        None => Arena::stage_on(reg, field, deployment, 9000 + game),
    };
    let learner = (game % 2) as u8;
    let tap: GoalTap = Arc::new(Mutex::new(Vec::new()));
    let mut ai = AiDriver::new();
    for side in 0..2u8 {
        // Each side fights by its own force's doctrine: the doctrine comes
        // from what it has.
        let config = AiConfig {
            planner: "utility".into(),
            difficulty: setup.difficulty,
            doctrine: matchup.map(|pair| pair[side as usize].doctrine.clone()),
        };
        let seed = game.wrapping_mul(2).wrapping_add(side as u64);
        let planner: Box<dyn AiPlanner<BattleState, Order>> = if side == learner {
            Box::new(
                UtilityPlanner::from_config(&config, seed, reg)
                    .with_brain(setup.brain.clone())
                    .tapped(tap.clone()),
            )
        } else {
            match opponent(setup, game) {
                Some((_, net)) => {
                    Box::new(UtilityPlanner::from_config(&config, seed, reg).with_net(net.clone()))
                }
                None => make_battle_planner(&config, seed, reg),
            }
        };
        ai.insert(side, planner);
    }
    let mut entries = Vec::new();
    let mut scores = vec![[0, state.score(0), state.score(1)]];
    ledger(reg, &state, 0, &mut entries);
    let mut rounds = 0;
    let (mut by, mut on) = ([0u32; 3], [0u32; 3]);
    let (mut f4, mut aimed, mut standoff) = ([0u32; 2], [0u32; 2], [0u32; 2]);
    // Per target: the last round a crew of each side struck his front, and
    // which crew.
    let mut fronted: std::collections::BTreeMap<(u32, u8), (u32, u32)> = Default::default();
    while !state.is_over() && rounds < MAX_ROUNDS {
        ai.plan_round(reg, &mut state);
        for event in state.resolve_round(reg) {
            // Who was aimed at whom, for the standoff count.
            let aimed_at = match &event {
                Event::ShotHit {
                    attacker, target, ..
                }
                | Event::ShotBounced {
                    attacker, target, ..
                } => Some((*attacker, *target)),
                Event::ShotMissed {
                    attacker,
                    target: Some(target),
                    ..
                } => Some((*attacker, *target)),
                _ => None,
            };
            if let Some((a, t)) = aimed_at
                && let (Some(au), Some(tu)) = (state.lookup(a), state.lookup(t))
            {
                let who = (au.side != learner) as usize;
                aimed[who] += 1;
                if au.pos.distance_to(tu.pos) > reach(reg, &state, t) {
                    standoff[who] += 1;
                }
            }
            let (attacker, facing, target) = match event {
                Event::ShotHit {
                    attacker,
                    facing,
                    target,
                    ..
                }
                | Event::ShotBounced {
                    attacker,
                    facing,
                    target,
                    ..
                } => (attacker, facing, target),
                _ => continue,
            };
            if let Some(side) = state.lookup(attacker).map(|u| u.side) {
                let who = (side != learner) as usize;
                let key = (target.0, side);
                if facing == ArmorFacing::Front {
                    fronted.insert(key, (rounds, attacker.0));
                } else if fronted
                    .get(&key)
                    .is_some_and(|(r, fixer)| rounds - r <= 3 && *fixer != attacker.0)
                {
                    f4[who] += 1;
                    fronted.remove(&key);
                }
            }
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
        fix_and_flank: f4,
        aimed,
        standoff,
        victory_score: state.scenario.victory_score(),
        opponent: opponent(setup, game).map_or("utility".to_string(), |(name, _)| name.clone()),
        forces: matchup.map(|pair| [pair[0].id.clone(), pair[1].id.clone()]),
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
    reg.planner.approach_places = parse("--approach", 0.0) as u32;
    if let Some(list) = flag(&args, "--events") {
        reg.planner.goal_events = tactics_core::data::GoalEvents::parse(list).unwrap_or_else(|e| {
            eprintln!("error: --events: {e}");
            std::process::exit(1);
        });
    }
    let path = flag(&args, "--net").expect("--net");
    // `utility` is the control: the teacher in the learner's seat, so a
    // force's record can be told apart from what the force is worth. A force
    // the teacher also loses with is the force; one only the network loses
    // with is the network.
    let brain = if path == "utility" {
        GoalBrain::Utility
    } else {
        let net = load_goal_net(std::path::Path::new(path)).unwrap_or_else(|err| {
            eprintln!("error: --net {path}: {err}");
            std::process::exit(1);
        });
        if args.iter().any(|a| a == "--deterministic") {
            GoalBrain::Learned(net)
        } else {
            GoalBrain::Explore { net, temperature }
        }
    };
    let setup = Setup {
        reg: &reg,
        arena,
        brain,
        difficulty,
        generated: flag(&args, "--ground") == Some("generated"),
        layout: generated::Layout {
            spread_percent: flag(&args, "--spread").map_or(100, |s| {
                s.parse()
                    .unwrap_or_else(|_| panic!("--spread takes a percent, got {s}"))
            }),
            turned: args.iter().any(|a| a == "--turned"),
        },
        forces: match flag(&args, "--forces") {
            None => Vec::new(),
            Some("all") => {
                let mut all: Vec<_> = reg.forces.values().collect();
                all.sort_by(|a, b| a.id.cmp(&b.id));
                all
            }
            Some(list) => list
                .split(',')
                .map(|id| {
                    reg.force(id).unwrap_or_else(|| {
                        eprintln!("error: --forces: no force `{id}`");
                        std::process::exit(1);
                    })
                })
                .collect(),
        },
        opponents: flag(&args, "--opponents")
            .map(|list| {
                list.split(',')
                    .filter(|p| !p.is_empty())
                    .map(|p| {
                        let net = load_goal_net(std::path::Path::new(p)).unwrap_or_else(|err| {
                            eprintln!("error: --opponents {p}: {err}");
                            std::process::exit(1);
                        });
                        let name = std::path::Path::new(p)
                            .file_name()
                            .map_or(p.to_string(), |n| n.to_string_lossy().into_owned());
                        (name, net)
                    })
                    .collect()
            })
            .unwrap_or_default(),
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
    let mut against: std::collections::BTreeMap<&str, [u32; 3]> = Default::default();
    for battle in fought.iter().flatten() {
        let row = against.entry(battle.opponent.as_str()).or_default();
        match battle.winner {
            Some(w) if w == battle.learner => row[0] += 1,
            Some(_) => row[1] += 1,
            None => row[2] += 1,
        }
    }
    for (name, [w, l, d]) in &against {
        println!("  against {name}: won {w}, lost {l}, drew {d}");
    }
    let mut with: std::collections::BTreeMap<&str, [u32; 3]> = Default::default();
    for battle in fought.iter().flatten() {
        let Some(pair) = &battle.forces else { continue };
        let row = with
            .entry(pair[battle.learner as usize].as_str())
            .or_default();
        match battle.winner {
            Some(w) if w == battle.learner => row[0] += 1,
            Some(_) => row[1] += 1,
            None => row[2] += 1,
        }
    }
    for (name, [w, l, d]) in &with {
        println!("  learner as {name}: won {w}, lost {l}, drew {d}");
    }
    println!("  struck by the learner: {}", share(by));
    println!("  struck on the learner: {}", share(on));
    let (mut f4, mut aimed, mut off) = ([0u32; 2], [0u32; 2], [0u32; 2]);
    for battle in fought.iter().flatten() {
        for i in 0..2 {
            f4[i] += battle.fix_and_flank[i];
            aimed[i] += battle.aimed[i];
            off[i] += battle.standoff[i];
        }
    }
    let n = fought.iter().flatten().count().max(1) as f32;
    for (i, who) in ["learner", "opponent"].iter().enumerate() {
        println!(
            "  {who}: fix and flank {:.2} a battle; {:.0}% of {} aimed shots from beyond the target's reach",
            f4[i] as f32 / n,
            100.0 * off[i] as f32 / aimed[i].max(1) as f32,
            aimed[i]
        );
    }
}
