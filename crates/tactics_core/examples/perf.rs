//! Performance harness for the hot paths, so that a claim about speed is a
//! command anyone can run rather than a number someone remembers.
//!
//! The fog rewrite was measured ad hoc and its results survive only as prose in
//! CLAUDE.md, which means the next round of work on the same code — the
//! occupancy index, shadowcasting FOV, cutting the MCTS budget — has nothing to
//! compare against. This prints the same handful of numbers every time, in a
//! shape that pastes into a commit message.
//!
//! ```sh
//! cargo run --release -p tactics_core --example perf
//! cargo run --release -p tactics_core --example perf -- --seeds 1,2,3,4
//! cargo run --release -p tactics_core --example perf -- --mcts
//! ```
//!
//! MCTS is opt-in because at roughly three seconds per order it dominates the
//! run time by two orders of magnitude, and the default invocation should be
//! something you are willing to run before every commit.
//!
//! **`--release` is not optional.** `[profile.dev]` builds this workspace at
//! `opt-level = 1`, and the planners are several times slower there, so a dev
//! build measures a machine nobody ships.
//!
//! What is measured and why:
//!
//! - **Round resolution** is the number the fog work moved (14.05 -> 1.02
//!   ms/round) and the one a rendering-side change can silently regress.
//! - **`reachable()`** is O(hexes x units) twice over and sits inside the MCTS
//!   inner loop, so it is the thing the planned occupancy index has to beat.
//! - **`roads()`** is what the goal chooser pays to price every candidate goal
//!   at once, and the number `ai::goal::HORIZON` is really about: it scales
//!   with the tiles inside the horizon, not with the number of candidates.
//! - **Planner cost per order** is what decides whether a difficulty is
//!   shippable against a human at all. MCTS is the reason the shipped scenario
//!   still names `utility`.
//! - **`unit_vision`** is one unit's look, computed cold. Fog is recomputed
//!   after every shot, so this is multiplied by the rate of fire rather than by
//!   the number of rounds — and it is the number a shadowcasting FOV would
//!   move.

use std::time::{Duration, Instant};
use tactics_core::ai::{AiConfig, AiPlanner, make_battle_planner};
use tactics_core::battle::{BattleState, Order, reachable, roads, unit_vision};
use tactics_core::data::DataRegistry;

const MAP: &str = "river_crossing";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seeds = flag(&args, "--seeds")
        .map(|v| {
            v.split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect::<Vec<u64>>()
        })
        .unwrap_or_else(|| vec![1, 2, 3, 4]);
    let with_mcts = args.iter().any(|a| a == "--mcts");

    if cfg!(debug_assertions) {
        eprintln!(
            "warning: this is a debug build. The workspace builds at opt-level 1 in dev and the \
             planners are several times slower; rerun with --release before quoting anything."
        );
    }

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (registry, _) = DataRegistry::load_dir(&root).expect("mods load");

    let probe = BattleState::from_map(&registry, MAP, seeds[0]).expect("battle");
    println!(
        "map {MAP}: {} tiles, {} units",
        probe.world.len(),
        probe.units.len()
    );
    println!("seeds {seeds:?}\n");

    let rounds = bench_round_resolution(&registry, &seeds);
    let reach = bench_reachable(&registry, &seeds);
    let road = bench_roads(&registry, &seeds);
    let fog = bench_fog(&registry, &seeds);
    let (region, whole) = bench_ground(&registry, &seeds);

    row("measurement", "result", "notes");
    println!("{}", "-".repeat(78));
    row(
        "round resolution",
        &fmt_ms(rounds.mean),
        "plan + 12 ticks, utility vs utility",
    );
    row(
        "  spread over seeds",
        &fmt_ms_range(rounds.min, rounds.max),
        "",
    );
    row(
        "reachable() per call",
        &fmt_us(reach),
        "selected unit, full move range",
    );
    row(
        "roads() per call",
        &fmt_us(road),
        "goal chooser's whole candidate list, one Dijkstra",
    );
    row(
        "unit_vision per unit (cold)",
        &fmt_us(fog),
        "raycasts every tile in range",
    );
    row(
        "terrain reading per region (cold)",
        &fmt_ms(region),
        "61 tiles, each counting what it sees",
    );
    row(
        "vantages, whole map (cold)",
        &fmt_ms(whole),
        "a commander's area of interest is smaller",
    );

    for difficulty in [1u8, 3, 5] {
        let t = bench_planner(&registry, &seeds, "utility", difficulty, 16);
        row(
            &format!("utility order (difficulty {difficulty})"),
            &fmt_ms(t),
            "",
        );
    }
    if with_mcts {
        // Few orders and few seeds: each one is seconds, and the point is the
        // order of magnitude, not the third significant figure.
        let seeds = &seeds[..seeds.len().min(2)];
        for difficulty in [3u8, 4] {
            let t = bench_planner(&registry, seeds, "mcts", difficulty, 4);
            row(
                &format!("mcts order (difficulty {difficulty})"),
                &fmt_ms(t),
                "the reason the shipped scenario uses utility",
            );
        }
    } else {
        row("mcts order", "skipped", "pass --mcts; it takes minutes");
    }
    campaign_rows(&registry);
}

/// The generated campaign's march (WORLD.md W2): what it costs to hold the
/// world's ground, to ask how far a column reaches in a day, and to march
/// one across the world. These are the numbers the hierarchical-routing
/// question is answered with.
fn campaign_rows(registry: &DataRegistry) {
    use tactics_core::overworld::{OverworldOrder, OverworldState};
    let time = |f: &mut dyn FnMut()| {
        let t = std::time::Instant::now();
        f();
        t.elapsed().as_secs_f64() * 1e3
    };
    let mut state = None;
    let made = time(&mut || {
        state = Some(OverworldState::from_map(registry, "frontier_world", 1).expect("campaign"))
    });
    let state = state.unwrap();
    let tiles = state.world.as_ref().map_or(0, |w| w.chunks().count()) * 1261;
    row(
        "campaign world made",
        &format!("{made:.0} ms"),
        &format!("frontier_world, {tiles} tiles, skeleton and summary"),
    );
    let army = tactics_core::overworld::ElementId(0);
    let cold = time(&mut || {
        state.reachable(registry, army);
    });
    row(
        "campaign reach, cold",
        &format!("{cold:.0} ms"),
        "builds the ground and every passage it needs",
    );
    let warm = time(&mut || {
        state.reachable(registry, army);
    });
    row(
        "campaign reach, warm",
        &format!("{warm:.2} ms"),
        "passages cached",
    );
    let far = state
        .map
        .iter()
        .map(|(h, _)| h)
        .max_by_key(|h| {
            (
                h.unsigned_distance_to(state.army(army).unwrap().pos),
                h.x,
                h.y,
            )
        })
        .unwrap();
    let mut trial = state.clone();
    let march = time(&mut || {
        let _ = trial.apply(registry, &OverworldOrder::MoveArmy { army, to: far });
    });
    row(
        "campaign march, across the world",
        &format!("{march:.1} ms"),
        "coarse A*, then tile A* to the next day's waypoint",
    );
    let mut again = state.clone();
    let warm = time(&mut || {
        let _ = again.apply(registry, &OverworldOrder::MoveArmy { army, to: far });
    });
    row(
        "campaign march, warm",
        &format!("{warm:.1} ms"),
        "the same march again: passages cached, as a campaign runs",
    );
}

fn row(label: &str, value: &str, notes: &str) {
    println!("{label:<34} {value:>12}  {notes}");
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).map(|s| s.as_str())
}

struct Stats {
    mean: Duration,
    min: Duration,
    max: Duration,
}

/// A full round: both sides plan with the utility planner, then twelve ticks
/// resolve. Averaged over the first few rounds of each seed, which is where
/// the unit count is highest and so the cost is worst.
fn bench_round_resolution(registry: &DataRegistry, seeds: &[u64]) -> Stats {
    const ROUNDS: usize = 8;
    let mut per_seed = Vec::new();
    for &seed in seeds {
        let mut state = BattleState::from_map(registry, MAP, seed).expect("battle");
        let mut planners = both_sides(registry, seed, "utility", 3);
        let mut total = Duration::ZERO;
        let mut counted = 0;
        for _ in 0..ROUNDS {
            if state.is_over() {
                break;
            }
            plan_round(registry, &mut state, &mut planners);
            let t = Instant::now();
            let _ = state.resolve_round(registry);
            total += t.elapsed();
            counted += 1;
        }
        if counted > 0 {
            per_seed.push(total / counted);
        }
    }
    Stats {
        mean: mean(&per_seed),
        min: per_seed.iter().copied().min().unwrap_or_default(),
        max: per_seed.iter().copied().max().unwrap_or_default(),
    }
}

/// `reachable()` for every living unit, which is what the search does
/// repeatedly while expanding move candidates.
/// One `roads` call: what the goal chooser pays to price every candidate at
/// once, and the number `ai::goal::HORIZON` is really about. Only paid when a
/// crew's goal *finishes*, so this is not a per-round cost.
fn bench_roads(registry: &DataRegistry, seeds: &[u64]) -> Duration {
    // Matches `ai::goal::HORIZON`. A benchmark that drifted from it would
    // mislabel itself rather than break anything, which is why it is a
    // literal with a comment rather than a new public constant.
    const HORIZON: u32 = 4;
    let mut samples = Vec::new();
    for &seed in seeds {
        let state = BattleState::from_map(registry, MAP, seed).expect("battle");
        let ids: Vec<_> = state.alive_units().map(|u| u.id).collect();
        for &id in &ids {
            let _ = roads(registry, &state, id, HORIZON);
        }
        let t = Instant::now();
        const REPS: u32 = 20;
        for _ in 0..REPS {
            for &id in &ids {
                std::hint::black_box(roads(registry, &state, id, HORIZON));
            }
        }
        samples.push(t.elapsed() / (REPS * ids.len() as u32));
    }
    mean(&samples)
}

fn bench_reachable(registry: &DataRegistry, seeds: &[u64]) -> Duration {
    let mut samples = Vec::new();
    for &seed in seeds {
        let state = BattleState::from_map(registry, MAP, seed).expect("battle");
        let ids: Vec<_> = state.alive_units().map(|u| u.id).collect();
        // A warm pass first: the point is steady-state cost, not the first
        // touch of a cold allocator.
        for &id in &ids {
            let _ = reachable(registry, &state, id);
        }
        let t = Instant::now();
        const REPS: u32 = 20;
        for _ in 0..REPS {
            for &id in &ids {
                std::hint::black_box(reachable(registry, &state, id));
            }
        }
        samples.push(t.elapsed() / (REPS * ids.len() as u32));
    }
    mean(&samples)
}

/// One unit's vision, computed cold.
///
/// This is deliberately `unit_vision` rather than a whole `fog::recompute`.
/// The recompute a real battle runs is mostly cache hits — that is the entire
/// point of the fog commit, and timing it would mostly measure the cache
/// saying "nothing moved". What a future change actually alters is the cost of
/// one look, since vision currently raycasts every tile in range
/// independently; a shadowcasting FOV would move *this* number, and would move
/// it by about an order of magnitude.
fn bench_fog(registry: &DataRegistry, seeds: &[u64]) -> Duration {
    let mut samples = Vec::new();
    for &seed in seeds {
        let state = BattleState::from_map(registry, MAP, seed).expect("battle");
        let ids: Vec<_> = state.alive_units().map(|u| u.id).collect();
        for &id in &ids {
            let _ = unit_vision(registry, &state, id);
        }
        const REPS: u32 = 20;
        let t = Instant::now();
        for _ in 0..REPS {
            for &id in &ids {
                std::hint::black_box(unit_vision(registry, &state, id));
            }
        }
        samples.push(t.elapsed() / (REPS * ids.len() as u32));
    }
    mean(&samples)
}

/// The terrain reader, cold: one region's readings, and every vantage on the
/// whole map. Cold because a reading is cached per region and paid once per
/// planner; the whole map is the upper bound a real area of interest stays
/// under.
fn bench_ground(registry: &DataRegistry, seeds: &[u64]) -> (Duration, Duration) {
    use tactics_core::ground::{Area, TerrainReader};
    let (mut regions, mut wholes) = (Vec::new(), Vec::new());
    for &seed in seeds {
        let state = BattleState::from_map(registry, MAP, seed).expect("battle");
        let centre = state.world.center();
        const REPS: u32 = 5;
        let t = Instant::now();
        for _ in 0..REPS {
            let mut reader = TerrainReader::new(registry);
            std::hint::black_box(reader.tile(registry, &state, centre));
        }
        regions.push(t.elapsed() / REPS);
        let area = Area::of(state.world.iter().map(|(h, _)| h));
        let t = Instant::now();
        let mut reader = TerrainReader::new(registry);
        std::hint::black_box(reader.vantages(registry, &state, &area));
        wholes.push(t.elapsed());
    }
    (mean(&regions), mean(&wholes))
}

/// Time per `next_order` call, which is the unit a human waits on.
fn bench_planner(
    registry: &DataRegistry,
    seeds: &[u64],
    planner: &str,
    difficulty: u8,
    max_orders: u32,
) -> Duration {
    let mut samples = Vec::new();
    for &seed in seeds {
        let state = BattleState::from_map(registry, MAP, seed).expect("battle");
        let mut planners = both_sides(registry, seed, planner, difficulty);
        let mut total = Duration::ZERO;
        let mut orders = 0u32;
        // One planning pass for side 0 only: enough orders to average over
        // without paying for a whole battle at MCTS speeds.
        let mut scratch = state.clone();
        for _ in 0..max_orders {
            if scratch.has_committed(0) || !scratch.is_planning() {
                break;
            }
            let t = Instant::now();
            let order = planners[0].next_order(registry, &scratch, 0);
            total += t.elapsed();
            orders += 1;
            if scratch.apply(registry, &order).is_err() {
                break;
            }
        }
        if orders > 0 {
            samples.push(total / orders);
        }
    }
    mean(&samples)
}

type Planners = [Box<dyn AiPlanner<BattleState, Order>>; 2];

fn both_sides(registry: &DataRegistry, seed: u64, planner: &str, difficulty: u8) -> Planners {
    let make = |offset: u64, doctrine: &str| {
        make_battle_planner(
            &AiConfig {
                planner: planner.into(),
                difficulty,
                doctrine: Some(doctrine.into()),
            },
            seed + offset,
            registry,
        )
    };
    [make(0, "massed_armor"), make(1, "elastic_defense")]
}

fn plan_round(registry: &DataRegistry, state: &mut BattleState, planners: &mut Planners) {
    for side in state.living_sides() {
        for _ in 0..64 {
            if state.has_committed(side) || !state.is_planning() {
                break;
            }
            let order = planners[side as usize].next_order(registry, state, side);
            if state.apply(registry, &order).is_err() {
                let _ = state.apply(registry, &Order::Commit { side });
                break;
            }
        }
    }
}

fn mean(samples: &[Duration]) -> Duration {
    if samples.is_empty() {
        return Duration::ZERO;
    }
    samples.iter().sum::<Duration>() / samples.len() as u32
}

fn fmt_ms(d: Duration) -> String {
    format!("{:.2} ms", d.as_secs_f64() * 1000.0)
}

fn fmt_ms_range(min: Duration, max: Duration) -> String {
    format!(
        "{:.2}-{:.2} ms",
        min.as_secs_f64() * 1000.0,
        max.as_secs_f64() * 1000.0
    )
}

fn fmt_us(d: Duration) -> String {
    format!("{:.1} us", d.as_secs_f64() * 1_000_000.0)
}
