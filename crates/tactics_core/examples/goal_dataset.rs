//! The imitation set for a learned goal chooser (REIMAGINED.md L0.3).
//!
//! Fights battles under the utility AI on every shipped battle map and
//! records every goal decision either side's crews take — the crew's
//! context, a row per candidate, and which one she chose — as JSON lines.
//! A learned chooser trained to agree with these is the warm start every
//! later stage begins from.
//!
//! ```sh
//! cargo run --release -p tactics_core --example goal_dataset -- \
//!     --games 256 --out target/goal_dataset/train.jsonl
//! ```
//!
//! Beside the data it writes `<out>.schema.json`: the feature names in
//! order, so the trainer never restates them.
//!
//! **Difficulty 5 by default**, because the teacher is being imitated and
//! below 5 her choices carry a blur the student would have to learn as noise.
//! `--difficulty` changes it. Half the battles are fought under the
//! `command` planner (missions, so orders appear in the data) and half under
//! the bare `utility` executor; doctrines are drawn from the shipped list per
//! battle, so the student sees every personality the teacher has.

use std::io::Write;
use std::sync::{Arc, Mutex};
use tactics_core::ai::features::{CANDIDATE, GoalRecord, GoalTap, context_names};
use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, SideCommand, UtilityPlanner};
use tactics_core::battle::{BattleState, Order};
use tactics_core::data::DataRegistry;
use tactics_core::harness::parallel::run_all;
use tactics_core::map::MapKind;

const USAGE: &str = "\
goal_dataset: record the utility AI's goal decisions for imitation

  --games N        battles to fight (default 64)
  --seed S         first battle's seed (default 0)
  --difficulty D   the teacher's difficulty, 1..=5 (default 5)
  --out PATH       where to write JSON lines (default target/goal_dataset/train.jsonl)";

/// The doctrines a battle draws from. `drill` is left out for the reason the
/// `mustered` table leaves it out: a fixture with no appetite and no interest
/// in ground, which would teach the student nothing about choosing ground.
const DOCTRINES: &[&str] = &[
    "massed_armor",
    "elastic_defense",
    "recon_pull",
    "bounding_overwatch",
    "delaying_action",
];

/// A battle no side has finished by now is a stalemate; the balance harness
/// uses the same cap.
const MAX_ROUNDS: u32 = 60;

#[derive(serde::Serialize)]
struct Line<'a> {
    game: u64,
    map: &'a str,
    #[serde(flatten)]
    record: &'a GoalRecord,
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn planner(
    reg: &DataRegistry,
    seed: u64,
    command: bool,
    doctrine: &str,
    difficulty: u8,
    tap: &GoalTap,
) -> Box<dyn AiPlanner<BattleState, Order>> {
    let config = AiConfig {
        planner: if command { "command" } else { "utility" }.into(),
        difficulty,
        doctrine: Some(doctrine.into()),
    };
    if command {
        Box::new(SideCommand::from_config(&config, seed, reg).tapped(tap.clone()))
    } else {
        Box::new(UtilityPlanner::from_config(&config, seed, reg).tapped(tap.clone()))
    }
}

/// Fight one battle and hand back every decision taken in it.
fn fight(
    reg: &DataRegistry,
    maps: &[&str],
    game: u64,
    difficulty: u8,
) -> (String, Vec<GoalRecord>) {
    let map = maps[game as usize % maps.len()];
    let command = (game / maps.len() as u64).is_multiple_of(2);
    let pick = |n: u64| DOCTRINES[(n % DOCTRINES.len() as u64) as usize];
    let doctrines = [pick(game.wrapping_mul(7)), pick(game.wrapping_mul(13) + 3)];
    let tap: GoalTap = Arc::new(Mutex::new(Vec::new()));
    let mut state = BattleState::from_map(reg, map, game).expect("battle");
    let mut ai = AiDriver::new();
    for side in 0..2u8 {
        let seed = game.wrapping_mul(2).wrapping_add(side as u64);
        ai.insert(
            side,
            planner(
                reg,
                seed,
                command,
                doctrines[side as usize],
                difficulty,
                &tap,
            ),
        );
    }
    let mut rounds = 0;
    while !state.is_over() && rounds < MAX_ROUNDS {
        ai.plan_round(reg, &mut state);
        state.resolve_round(reg);
        rounds += 1;
    }
    drop(ai);
    let records = Arc::try_unwrap(tap)
        .map(|m| m.into_inner().unwrap_or_default())
        .unwrap_or_else(|shared| shared.lock().map(|r| r.clone()).unwrap_or_default());
    (map.to_string(), records)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return;
    }
    let games: u64 = flag(&args, "--games").map_or(64, |s| s.parse().expect("--games"));
    let first: u64 = flag(&args, "--seed").map_or(0, |s| s.parse().expect("--seed"));
    let difficulty: u8 =
        flag(&args, "--difficulty").map_or(5, |s| s.parse().expect("--difficulty"));
    let out =
        std::path::PathBuf::from(flag(&args, "--out").unwrap_or("target/goal_dataset/train.jsonl"));

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (reg, _) = DataRegistry::load_dir(&root).expect("mods load");
    let mut maps: Vec<&str> = reg
        .maps
        .values()
        .filter(|m| m.kind == MapKind::Battle)
        .map(|m| m.id.as_str())
        .collect();
    maps.sort_unstable();

    let jobs: Vec<u64> = (first..first + games).collect();
    let fought = run_all(&jobs, |&game| fight(&reg, &maps, game, difficulty));

    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).expect("create output directory");
    }
    let mut file = std::io::BufWriter::new(std::fs::File::create(&out).expect("open output"));
    let mut decisions = 0usize;
    let mut dropped = 0usize;
    let mut candidates = 0usize;
    for (game, (map, records)) in jobs.iter().zip(&fought) {
        for record in records {
            if record.chosen.is_none() {
                dropped += 1;
                continue;
            }
            decisions += 1;
            candidates += record.candidates.len();
            let line = Line {
                game: *game,
                map,
                record,
            };
            serde_json::to_writer(&mut file, &line).expect("write record");
            file.write_all(b"\n").expect("write record");
        }
    }
    file.flush().expect("flush output");

    let schema = serde_json::json!({
        "context": context_names(),
        "candidate": CANDIDATE,
        "difficulty": difficulty,
        "games": games,
        "first_seed": first,
    });
    let schema_path = out.with_extension("schema.json");
    std::fs::write(
        &schema_path,
        serde_json::to_string_pretty(&schema).expect("schema"),
    )
    .expect("write schema");

    println!(
        "{games} battles, {decisions} decisions ({:.1} candidates each), {dropped} dropped",
        candidates as f32 / decisions.max(1) as f32
    );
    println!("wrote {} and {}", out.display(), schema_path.display());
}
