//! What a vehicle is worth, measured from battles rather than asserted
//! (REIMAGINED.md, *Pricing*).
//!
//! A price is a claim that a side with that much more of something wins that
//! much more often, and the honest way to set one is to ask the battles. This
//! draws two **random** forces for every world — so many of each chassis,
//! squads aboard carriers where there is room and on foot where there is not
//! — fights them with the utility AI on both sides, on generated ground, twice
//! a world with the forces exchanged between the ends, and fits
//!
//! ```text
//! P(side 0 wins) = σ(seat + Σ_type worth_type × (count₀ − count₁))
//! ```
//!
//! by maximum likelihood. A type's `worth` is log-odds per vehicle: what one
//! more of it does to the odds of winning. Read as a price it is quoted
//! against the medium tank's listed cost, so a price that matches its listing
//! is a price the battles agree with.
//!
//! Two kinds of squad are counted apart — aboard a carrier at the start and on
//! foot — because a squad that walks a battle map at one hex a round is not
//! the squad that rides to the fight, and lumping them would price the
//! carrier's seat into the squad.
//!
//! What it assumes, said out loud: worth is additive, so it is a first-order
//! reading of a fight that is not (two tanks are not twice one tank to a side
//! with no anti-tank weapon). The held-out check is how far that carries:
//! the model fitted on half the worlds predicts the other half, and the
//! report says how much better than a coin it does.
//!
//! ```sh
//! cargo run --release -p tactics_core --example prices -- --worlds 4096
//! ```

use tactics_core::ai::{AiConfig, AiDriver, make_battle_planner};
use tactics_core::battle::BattleState;
use tactics_core::data::{DataRegistry, ForceDef, ForceUnit};
use tactics_core::harness::arena::arena_named;
use tactics_core::harness::parallel::run_all;
use tactics_core::harness::{forces, generated};

const USAGE: &str = "\
prices: what each chassis is worth, fitted from random forces fought out

  --worlds N        generated worlds, two battles each (default 2048)
  --seed S          first world (default 2000000)
  --events LIST     planner.goal_events (default all, as training plays)
  --places N        planner.goal_places (default 16, as training plays)
  --difficulty D    both sides' difficulty (default 5)
  --victory N       the score that wins outright (default the generated field's 40; 0 none)
  --max N           most of any one type a force may draw (default 3; squads twice that)
  --set PATH=VALUE  a content override, as balance takes it (repeatable)

  PRICES_TRACE=<world> in the environment prints, for that world, every foot
  unit ending a round with an enemy vehicle inside rocket range: whether her
  side has spotted it, whether she has a sight line, her ammunition, rung and
  fire orders. It is how the squads' silence was found to be ground.";

/// A battle no side has finished by now is a stalemate; the harness's cap.
const MAX_ROUNDS: u32 = 60;

/// The chassis priced, in the order the report prints them. The squads are
/// two columns: aboard and on foot.
const TYPES: &[&str] = &[
    "medium_tank",
    "light_tank",
    "tank_destroyer",
    "recon_car",
    "apc",
    "halftrack",
];

/// Doctrines the random forces fight by, one drawn per world and shared by
/// both sides, so it cancels in the difference rather than being priced into
/// whatever the doctrine happens to like. `drill` is a fixture, not a
/// commander.
const DOCTRINES: &[&str] = &[
    "massed_armor",
    "bounding_overwatch",
    "recon_pull",
    "delaying_action",
    "elastic_defense",
];

/// Columns of the design: the vehicle types, then squads aboard, then
/// squads on foot.
const COLUMNS: usize = TYPES.len() + 2;

/// SplitMix64, written out: the draw must not move with the platform.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A random force: its counts by column and the force itself.
fn draw(world: u64, side: u64, max: u32) -> ([f64; COLUMNS], ForceDef) {
    let mut state = mix(world.wrapping_mul(2).wrapping_add(side) ^ 0x5EED);
    let mut next = |n: u32| {
        state = mix(state);
        (state % (n as u64 + 1)) as u32
    };
    loop {
        let counts: Vec<u32> = TYPES.iter().map(|_| next(max)).collect();
        let squads = next(2 * max);
        let vehicles: u32 = counts.iter().sum::<u32>() + squads;
        // A force of nobody, or of a crowd too big for the deployment, is
        // not a question worth fighting out.
        if !(3..=12).contains(&vehicles) {
            continue;
        }
        let carriers: u32 = TYPES
            .iter()
            .zip(&counts)
            .filter(|(t, _)| matches!(**t, "apc" | "halftrack"))
            .map(|(_, n)| *n)
            .sum();
        let aboard = squads.min(carriers);
        let mut row = [0.0; COLUMNS];
        let mut units = Vec::new();
        let mut riders = aboard;
        for (i, (vehicle, &n)) in TYPES.iter().zip(&counts).enumerate() {
            row[i] = n as f64;
            if n == 0 {
                continue;
            }
            let carrier = matches!(*vehicle, "apc" | "halftrack");
            let loaded = if carrier { riders.min(n) } else { 0 };
            riders -= loaded;
            if loaded > 0 {
                units.push(ForceUnit {
                    vehicle: vehicle.to_string(),
                    count: loaded,
                    carries: Some("rifle_squad".into()),
                });
            }
            if n > loaded {
                units.push(ForceUnit {
                    vehicle: vehicle.to_string(),
                    count: n - loaded,
                    carries: None,
                });
            }
        }
        if squads > aboard {
            units.push(ForceUnit {
                vehicle: "rifle_squad".into(),
                count: squads - aboard,
                carries: None,
            });
        }
        row[TYPES.len()] = aboard as f64;
        row[TYPES.len() + 1] = (squads - aboard) as f64;
        let force = ForceDef {
            id: format!("random_{world}_{side}"),
            name: format!("Force {side}"),
            description: String::new(),
            doctrine: String::new(),
            units,
        };
        return (row, force);
    }
}

/// One battle: side 0's counts minus side 1's, and side 0's result (1 a
/// win, 0 a loss, ½ a draw).
struct Fought {
    world: u64,
    diff: [f64; COLUMNS],
    result: f64,
    census: Census,
}

/// What each kind of unit did, summed over battles: the account a price
/// comes with. Keyed by chassis, and a squad by whether she started aboard.
type Census = std::collections::BTreeMap<String, Deeds>;

#[derive(Default, Clone)]
struct Deeds {
    units: u32,
    shots: u32,
    hits: u32,
    damage: i64,
    /// Damage dealt to targets that are not on foot: vehicles of any kind.
    on_vehicles: i64,
    kills: u32,
    lost: u32,
    /// Lost while still aboard her carrier: the shared fate.
    lost_aboard: u32,
    dismounted: u32,
    /// Summed distance to the nearest enemy on the field at each dismount.
    dismount_range: u32,
    /// Rounds ended on the field (not aboard), and of those: with an enemy
    /// inside her shortest weapon's reach of something she could hurt, and
    /// on a rung that pins.
    on_field: u32,
    in_reach: u32,
    pinned: u32,
    /// Rounds ended on the field with an enemy vehicle inside her anti-armour
    /// weapon's reach (`rpg`), and shots she fired with it.
    vehicle_in_rpg: u32,
    rpg_shots: u32,
}

impl Deeds {
    fn add(&mut self, o: &Deeds) {
        self.units += o.units;
        self.shots += o.shots;
        self.hits += o.hits;
        self.damage += o.damage;
        self.on_vehicles += o.on_vehicles;
        self.kills += o.kills;
        self.lost += o.lost;
        self.lost_aboard += o.lost_aboard;
        self.dismounted += o.dismounted;
        self.dismount_range += o.dismount_range;
        self.on_field += o.on_field;
        self.in_reach += o.in_reach;
        self.pinned += o.pinned;
        self.vehicle_in_rpg += o.vehicle_in_rpg;
        self.rpg_shots += o.rpg_shots;
    }
}

fn fight(
    reg: &DataRegistry,
    world: u64,
    swap: bool,
    max: u32,
    difficulty: u8,
    victory: Option<u32>,
) -> Option<Fought> {
    let arena = arena_named("ridge_arena")?;
    let mut made = generated::field(reg, arena, world)?;
    if let Some(score) = victory {
        made.field.scenario = made.field.scenario.with_victory_score(score);
    }
    let doctrine = DOCTRINES[(mix(world ^ 0xD0C) % DOCTRINES.len() as u64) as usize];
    let (mut a, mut fa) = draw(world, 0, max);
    let (mut b, mut fb) = draw(world, 1, max);
    if swap {
        std::mem::swap(&mut a, &mut b);
        std::mem::swap(&mut fa, &mut fb);
    }
    fa.doctrine = doctrine.into();
    fb.doctrine = doctrine.into();
    let game = world * 2 + swap as u64;
    let mut state: BattleState =
        forces::stage(reg, made.field, &made.deployment, [&fa, &fb], 9000 + game)?;
    let mut ai = AiDriver::new();
    for side in 0..2u8 {
        let config = AiConfig {
            planner: "utility".into(),
            difficulty,
            doctrine: Some(doctrine.into()),
        };
        let seed = game.wrapping_mul(2).wrapping_add(side as u64);
        ai.insert(side, make_battle_planner(&config, seed, reg));
    }
    use tactics_core::battle::Event;
    let foot = |vehicle: &str| {
        reg.vehicle(vehicle)
            .is_some_and(|v| v.movement.class == tactics_core::data::MovementClass::Foot)
    };
    let label: std::collections::BTreeMap<u32, String> = state
        .units
        .iter()
        .map(|u| {
            let name = if foot(&u.vehicle) {
                let how = if u.aboard.is_some() {
                    "aboard"
                } else {
                    "on foot"
                };
                format!("{} {how}", u.vehicle)
            } else {
                u.vehicle.clone()
            };
            (u.id.0, name)
        })
        .collect();
    let mut census = Census::new();
    for name in label.values() {
        census.entry(name.clone()).or_default().units += 1;
    }
    let mut last_hit: std::collections::BTreeMap<u32, u32> = Default::default();
    let mut rounds = 0;
    while !state.is_over() && rounds < MAX_ROUNDS {
        ai.plan_round(reg, &mut state);
        // Who rode into this round aboard: a passenger destroyed in it died
        // with her carrier (a squad that got out and was shot is not).
        let riding: Vec<u32> = state
            .units
            .iter()
            .filter(|u| u.aboard.is_some())
            .map(|u| u.id.0)
            .collect();
        for event in state.resolve_round(reg) {
            match event {
                Event::ShotFired {
                    attacker, weapon, ..
                } => {
                    if let Some(n) = label.get(&attacker.0) {
                        let d = census.entry(n.clone()).or_default();
                        d.shots += 1;
                        d.rpg_shots += (weapon == "rpg") as u32;
                    }
                }
                Event::ShotHit {
                    attacker,
                    target,
                    damage,
                    ..
                } => {
                    last_hit.insert(target.0, attacker.0);
                    let on_vehicle = state.unit(target).is_some_and(|t| !foot(&t.vehicle));
                    if let Some(n) = label.get(&attacker.0) {
                        let d = census.entry(n.clone()).or_default();
                        d.hits += 1;
                        d.damage += damage as i64;
                        if on_vehicle {
                            d.on_vehicles += damage as i64;
                        }
                    }
                }
                Event::Dismounted { unit, at } => {
                    let side = state.unit(unit).map(|u| u.side);
                    let range = state
                        .alive_units()
                        .filter(|e| Some(e.side) != side && e.aboard.is_none())
                        .map(|e| e.pos.distance_to(at) as u32)
                        .min()
                        .unwrap_or(0);
                    if let Some(n) = label.get(&unit.0) {
                        let d = census.entry(n.clone()).or_default();
                        d.dismounted += 1;
                        d.dismount_range += range;
                    }
                }
                Event::UnitDestroyed { unit, .. } => {
                    let aboard = riding.contains(&unit.0)
                        && state.unit(unit).is_none_or(|u| u.aboard.is_some());
                    if let Some(n) = label.get(&unit.0) {
                        let d = census.entry(n.clone()).or_default();
                        d.lost += 1;
                        if aboard {
                            d.lost_aboard += 1;
                        }
                    }
                    if let Some(n) = last_hit.get(&unit.0).and_then(|a| label.get(a)) {
                        census.entry(n.clone()).or_default().kills += 1;
                    }
                }
                _ => {}
            }
        }
        // Where every foot unit on the field stands at the end of the round:
        // within reach of an enemy (her longest weapon), and pinned.
        for u in state
            .alive_units()
            .filter(|u| u.aboard.is_none() && foot(&u.vehicle))
        {
            let reach = reg
                .vehicle(&u.vehicle)
                .map(|v| {
                    v.weapons
                        .iter()
                        .filter_map(|w| reg.weapon(w))
                        .map(|w| w.range[1])
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            let near = state.alive_units().any(|e| {
                e.side != u.side && e.aboard.is_none() && e.pos.distance_to(u.pos) as u32 <= reach
            });
            let pinned = reg.morale.rung(u.pressure).pinned;
            let rpg = reg.weapon("rpg").map_or(0, |w| w.range[1]);
            let vehicle_close = state.alive_units().any(|e| {
                e.side != u.side
                    && e.aboard.is_none()
                    && !foot(&e.vehicle)
                    && e.pos.distance_to(u.pos) as u32 <= rpg
            });
            if vehicle_close && std::env::var("PRICES_TRACE").is_ok_and(|w| w == world.to_string())
            {
                let fog = state.fog.side(u.side);
                for e in state.alive_units().filter(|e| {
                    e.side != u.side
                        && e.aboard.is_none()
                        && !foot(&e.vehicle)
                        && e.pos.distance_to(u.pos) as u32 <= rpg
                }) {
                    let rung = reg.morale.rung(u.pressure);
                    eprintln!(
                        "round {rounds} {} ({:?}) d={} -> {} {}: spotted {} sight {} seen-by-them {} ammo {:?} rung {} obeys {} fire {:?} cooldowns {:?} goal {:?}",
                        label[&u.id.0],
                        u.id,
                        e.pos.distance_to(u.pos),
                        e.vehicle,
                        e.id.0,
                        fog.spotted.contains(&e.id),
                        state.world.sight_clear(u.pos, e.pos),
                        state.fog.side(e.side).spotted.contains(&u.id),
                        u.ammo,
                        rung.name,
                        rung.obeys,
                        u.intent.fire,
                        u.cooldowns,
                        u.goal,
                    );
                }
            }
            if let Some(n) = label.get(&u.id.0) {
                let d = census.entry(n.clone()).or_default();
                d.on_field += 1;
                d.in_reach += near as u32;
                d.pinned += pinned as u32;
                d.vehicle_in_rpg += vehicle_close as u32;
            }
        }
        rounds += 1;
    }
    let result = match state.over.and_then(|r| r.winner).or_else(|| state.leader()) {
        Some(0) => 1.0,
        Some(_) => 0.0,
        None => 0.5,
    };
    let mut diff = [0.0; COLUMNS];
    for i in 0..COLUMNS {
        diff[i] = a[i] - b[i];
    }
    Some(Fought {
        world,
        diff,
        result,
        census,
    })
}

/// Logistic regression by Newton's method, with an intercept (the seat) and
/// a whisper of ridge so a type no battle varied cannot run off to infinity.
/// Returns the coefficients, intercept first, and their standard errors.
fn fit(rows: &[&Fought]) -> (Vec<f64>, Vec<f64>) {
    let k = COLUMNS + 1;
    let x = |r: &Fought, j: usize| if j == 0 { 1.0 } else { r.diff[j - 1] };
    let mut beta = vec![0.0; k];
    let ridge = 1e-3;
    let mut hessian = vec![vec![0.0; k]; k];
    for _ in 0..50 {
        let mut grad = vec![0.0; k];
        hessian = vec![vec![0.0; k]; k];
        for r in rows {
            let eta: f64 = (0..k).map(|j| beta[j] * x(r, j)).sum();
            let p = 1.0 / (1.0 + (-eta).exp());
            let w = p * (1.0 - p);
            for (i, row) in hessian.iter_mut().enumerate() {
                grad[i] += (r.result - p) * x(r, i);
                for (j, cell) in row.iter_mut().enumerate() {
                    *cell += w * x(r, i) * x(r, j);
                }
            }
        }
        for i in 0..k {
            grad[i] -= ridge * beta[i];
            hessian[i][i] += ridge;
        }
        let step = solve(&hessian, &grad);
        let size: f64 = step.iter().map(|s| s.abs()).fold(0.0, f64::max);
        for i in 0..k {
            beta[i] += step[i];
        }
        if size < 1e-9 {
            break;
        }
    }
    let se = (0..k)
        .map(|i| {
            let mut e = vec![0.0; k];
            e[i] = 1.0;
            solve(&hessian, &e)[i].max(0.0).sqrt()
        })
        .collect();
    (beta, se)
}

/// Gaussian elimination with partial pivoting; small and dense.
fn solve(a: &[Vec<f64>], b: &[f64]) -> Vec<f64> {
    let n = b.len();
    let mut m: Vec<Vec<f64>> = a
        .iter()
        .zip(b)
        .map(|(row, &v)| row.iter().copied().chain([v]).collect())
        .collect();
    for c in 0..n {
        let p = (c..n)
            .max_by(|&i, &j| m[i][c].abs().total_cmp(&m[j][c].abs()))
            .unwrap_or(c);
        m.swap(c, p);
        let pivot = m[c][c];
        if pivot.abs() < 1e-12 {
            continue;
        }
        let pivot_row = m[c].clone();
        for (r, row) in m.iter_mut().enumerate() {
            if r != c {
                let f = row[c] / pivot;
                for (cell, p) in row.iter_mut().zip(&pivot_row).skip(c) {
                    *cell -= f * p;
                }
            }
        }
    }
    (0..n)
        .map(|i| {
            if m[i][i].abs() < 1e-12 {
                0.0
            } else {
                m[i][n] / m[i][i]
            }
        })
        .collect()
}

/// Mean log-loss of the model on `rows`.
fn log_loss(beta: &[f64], rows: &[&Fought]) -> f64 {
    let loss: f64 = rows
        .iter()
        .map(|r| {
            let eta = beta[0] + (0..COLUMNS).map(|j| beta[j + 1] * r.diff[j]).sum::<f64>();
            let p = (1.0 / (1.0 + (-eta).exp())).clamp(1e-9, 1.0 - 1e-9);
            -(r.result * p.ln() + (1.0 - r.result) * (1.0 - p).ln())
        })
        .sum();
    loss / rows.len().max(1) as f64
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return;
    }
    let parse = |name: &str, default: u64| -> u64 {
        flag(&args, name).map_or(default, |s| s.parse().unwrap_or_else(|_| panic!("{name}")))
    };
    let worlds = parse("--worlds", 2048);
    let first = parse("--seed", 2_000_000);
    let difficulty = parse("--difficulty", 5) as u8;
    let max = parse("--max", 3) as u32;
    let victory = flag(&args, "--victory").map(|v| v.parse::<u32>().expect("--victory"));

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (mut reg, _) = DataRegistry::load_dir(&root).expect("mods load");
    reg.planner.goal_places = parse("--places", 16) as u32;
    reg.planner.goal_events =
        tactics_core::data::GoalEvents::parse(flag(&args, "--events").unwrap_or("all"))
            .unwrap_or_else(|e| {
                eprintln!("error: --events: {e}");
                std::process::exit(1);
            });
    for (_, text) in args
        .iter()
        .zip(args.iter().skip(1))
        .filter(|(a, _)| *a == "--set")
    {
        let applied = tactics_core::harness::overrides::Override::parse(text)
            .and_then(|ov| tactics_core::harness::overrides::apply_override(&mut reg, &ov));
        if let Err(e) = applied {
            eprintln!("error: --set {text}: {e}");
            std::process::exit(1);
        }
    }

    let jobs: Vec<(u64, bool)> = (first..first + worlds)
        .flat_map(|w| [(w, false), (w, true)])
        .collect();
    let fought: Vec<Fought> = run_all(&jobs, |&(w, swap)| {
        fight(&reg, w, swap, max, difficulty, victory)
    })
    .into_iter()
    .flatten()
    .collect();
    let draws = fought.iter().filter(|f| f.result == 0.5).count();
    println!(
        "{} battles on {} generated worlds ({} drawn), random forces, utility against utility at difficulty {difficulty}",
        fought.len(),
        worlds,
        draws
    );

    // Fit on even worlds, test on odd ones; then fit on everything for the
    // prices themselves.
    let train: Vec<&Fought> = fought.iter().filter(|f| f.world % 2 == 0).collect();
    let test: Vec<&Fought> = fought.iter().filter(|f| f.world % 2 == 1).collect();
    let (half, _) = fit(&train);
    let coin = std::f64::consts::LN_2;
    let held = log_loss(&half, &test);
    let right = test
        .iter()
        .filter(|r| r.result != 0.5)
        .filter(|r| {
            let eta = half[0] + (0..COLUMNS).map(|j| half[j + 1] * r.diff[j]).sum::<f64>();
            (eta > 0.0) == (r.result == 1.0)
        })
        .count();
    let decided = test.iter().filter(|r| r.result != 0.5).count();
    println!(
        "held out: log-loss {held:.3} against a coin's {coin:.3}; {right} of {decided} decided battles called ({:.1}%)",
        100.0 * right as f64 / decided.max(1) as f64
    );

    let all: Vec<&Fought> = fought.iter().collect();
    let (beta, se) = fit(&all);
    let anchor = TYPES
        .iter()
        .position(|t| *t == "medium_tank")
        .expect("medium tank");
    let listed_anchor = reg.vehicle("medium_tank").map_or(16, |v| v.cost) as f64;
    let rate = listed_anchor / beta[anchor + 1];
    println!("seat (side 0): {:+.3} ± {:.3} log-odds", beta[0], se[0]);
    println!();
    println!(
        "  {:22} {:>16} {:>8} {:>8}",
        "type", "worth (log-odds)", "price", "listed"
    );
    let names: Vec<String> = TYPES
        .iter()
        .map(|t| t.to_string())
        .chain(["rifle_squad aboard".into(), "rifle_squad on foot".into()])
        .collect();
    for (j, name) in names.iter().enumerate() {
        let vehicle = name.split(' ').next().unwrap_or(name);
        let listed = reg.vehicle(vehicle).map_or(0, |v| v.cost);
        println!(
            "  {name:22} {:>+8.3} ± {:.3} {:>8.1} {:>8}",
            beta[j + 1],
            se[j + 1],
            beta[j + 1] * rate,
            listed
        );
    }

    // What each kind of unit did, per unit fielded.
    let mut census = Census::new();
    for f in &fought {
        for (name, d) in &f.census {
            census.entry(name.clone()).or_default().add(d);
        }
    }
    println!();
    println!(
        "  {:22} {:>7} {:>7} {:>7} {:>8} {:>9} {:>7} {:>7} {:>8} {:>9} {:>9}",
        "per unit fielded",
        "units",
        "shots",
        "hits",
        "damage",
        "on hulls",
        "kills",
        "lost",
        "aboard",
        "dismount",
        "at range"
    );
    let mut field_lines = Vec::new();
    for (name, d) in &census {
        let n = d.units.max(1) as f64;
        println!(
            "  {name:22} {:>7} {:>7.1} {:>7.1} {:>8.1} {:>9.1} {:>7.2} {:>6.0}% {:>7.0}% {:>8.0}% {:>9}",
            d.units,
            d.shots as f64 / n,
            d.hits as f64 / n,
            d.damage as f64 / n,
            d.on_vehicles as f64 / n,
            d.kills as f64 / n,
            100.0 * d.lost as f64 / n,
            100.0 * d.lost_aboard as f64 / n,
            100.0 * d.dismounted as f64 / n,
            if d.dismounted == 0 {
                "-".to_string()
            } else {
                format!("{:.1}", d.dismount_range as f64 / d.dismounted as f64)
            },
        );
        if d.on_field > 0 {
            field_lines.push(format!(
                "  {name:22} {:.1} rounds on the field a unit; {:.0}% of them with an enemy in reach, {:.0}% pinned; \
                 {:.2} rounds a unit with a vehicle inside RPG range, {:.2} RPG shots a unit",
                d.on_field as f64 / n,
                100.0 * d.in_reach as f64 / d.on_field as f64,
                100.0 * d.pinned as f64 / d.on_field as f64,
                d.vehicle_in_rpg as f64 / n,
                d.rpg_shots as f64 / n,
            ));
        }
    }

    println!();
    for line in field_lines {
        println!("{line}");
    }

    // The shipped forces at both prices: what the listing says they cost,
    // and what the battles say they are worth.
    println!();
    println!("  {:22} {:>8} {:>8}", "force", "listed", "fitted");
    let mut ids: Vec<&String> = reg.forces.keys().collect();
    ids.sort();
    for id in ids {
        let force = &reg.forces[id];
        let mut listed = 0;
        let mut worth = 0.0;
        for line in &force.units {
            let cost = reg.vehicle(&line.vehicle).map_or(0, |v| v.cost);
            listed += cost * line.count as i32;
            if let Some(j) = TYPES.iter().position(|t| *t == line.vehicle) {
                worth += beta[j + 1] * line.count as f64;
            }
            if let Some(rider) = &line.carries {
                listed += reg.vehicle(rider).map_or(0, |v| v.cost) * line.count as i32;
                if rider == "rifle_squad" {
                    worth += beta[TYPES.len() + 1] * line.count as f64;
                }
            }
        }
        println!("  {id:22} {listed:>8} {:>8.1}", worth * rate);
    }
}
