//! What the numbers in `assets/mods` actually do.
//!
//! ```sh
//! cargo run --release -p tactics_core --example balance          # instant
//! cargo run --release -p tactics_core --example balance -- --sim # + fought out
//! cargo run --release -p tactics_core --example balance -- --sim --games 40
//! ```
//!
//! Two passes, deliberately, because balance iteration has two speeds.
//!
//! The **analytic** pass answers "what did that number just do" without playing
//! anything: it stands two vehicles on an empty field and asks the real combat
//! code what would happen. It runs in milliseconds, so it belongs in the tight
//! loop where you are editing a json file and want to know whether the 88 can
//! still hurt a Löwe from the front.
//!
//! The **simulated** pass (`--sim`) fights whole battles and reports what
//! actually happened — who won, how long it took, which vehicles died. It is
//! slower and noisier, and it is the check at the end, because the analytic
//! numbers can all look reasonable while the fights they produce are terrible.
//!
//! Everything here goes through `preview_attack` and `resolve_round` rather
//! than reimplementing the formulas, so the report cannot drift from the game.
//! A harness that computes its own damage would eventually be measuring a
//! second, imaginary game.

use std::collections::HashMap;
use tactics_core::ai::{AiConfig, AiPlanner, make_battle_planner};
use tactics_core::battle::{
    BattleState, EndReason, Event, Order, SideState, UnitId, preview_attack,
};
use tactics_core::data::{ArmorFacing, DamageType, DataRegistry};
use tactics_core::map::{Facing, HexMap, MapFile, UnitPlacement};
use tactics_core::roster::Roster;

/// How far apart the analytic duel stands, in hexes. Inside every weapon's
/// band except the howitzer's minimum, and far enough that range falloff is
/// doing something.
const DUEL_RANGE: i32 = 4;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sim = args.iter().any(|a| a == "--sim");
    let games: usize = flag(&args, "--games")
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);

    if cfg!(debug_assertions) {
        eprintln!("note: debug build. Fine for the analytic pass, slow for --sim.");
    }

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (registry, _) = DataRegistry::load_dir(&root).expect("mods load");

    roster_table(&registry);
    gun_table(&registry);
    flags(&registry);
    if sim {
        simulate(&registry, games);
    } else {
        println!("\n(pass --sim to fight {games} battles and see what these numbers do)");
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).map(|s| s.as_str())
}

fn heading(title: &str) {
    println!("\n{title}");
    println!("{}", "-".repeat(title.len().max(60)));
}

// --- the analytic pass ----------------------------------------------------

fn roster_table(reg: &DataRegistry) {
    heading("vehicles");
    println!(
        "{:<16} {:>3} {:>10} {:>9} {:>8} {:>6} {:>5}",
        "id", "hp", "armour F/S/R", "speed", "sight", "safety", "cost"
    );
    let mut ids: Vec<_> = reg.vehicles.keys().collect();
    ids.sort();
    for id in ids {
        let v = &reg.vehicles[id];
        println!(
            "{:<16} {:>3} {:>10} {:>9} {:>8} {:>6} {:>5}",
            id,
            v.max_hp,
            format!("{}/{}/{}", v.armor.front, v.armor.side, v.armor.rear),
            reg.scale.format_speed(v.movement.points),
            reg.scale.format_distance(v.vision_range as i32),
            v.safety,
            v.cost,
        );
    }
}

/// Expected damage per shot and rounds-to-kill, for every gun against every
/// vehicle's frontal armour. The single most useful table for balance: it says
/// what can hurt what, and how long it takes.
fn gun_table(reg: &DataRegistry) {
    heading(&format!(
        "expected damage per shot at {DUEL_RANGE} hexes ({}), front armour",
        reg.scale.format_distance(DUEL_RANGE)
    ));

    let mut vehicles: Vec<_> = reg.vehicles.keys().cloned().collect();
    vehicles.sort();
    let mut weapons: Vec<_> = reg.weapons.keys().cloned().collect();
    weapons.sort();

    print!("{:<14}", "");
    for v in &vehicles {
        print!("{:>13}", short(v));
    }
    println!();

    for w in &weapons {
        print!("{:<14}", w);
        for v in &vehicles {
            match duel(reg, w, v, Facing::West) {
                Some(shot) => {
                    let ttk = rounds_to_kill(reg, w, v, shot.expected);
                    print!("{:>13}", format!("{:.1} ({})", shot.expected, ttk));
                }
                None => print!("{:>13}", "-"),
            }
        }
        println!();
    }
    println!("\n  cell is `expected damage per shot (rounds to kill)`; `inf` means it cannot");
    println!("  get through in any useful time, `-` means the gun is not carried in range");

    heading("the same guns from behind, for comparison");
    print!("{:<14}", "");
    for v in &vehicles {
        print!("{:>13}", short(v));
    }
    println!();
    for w in &weapons {
        print!("{:<14}", w);
        for v in &vehicles {
            match duel(reg, w, v, Facing::East) {
                Some(shot) => print!("{:>13}", format!("{:.1}", shot.expected)),
                None => print!("{:>13}", "-"),
            }
        }
        println!();
    }
}

/// Things worth a second look. Not failures — this game has not decided all of
/// these — but the shapes that usually mean a number is wrong.
fn flags(reg: &DataRegistry) {
    heading("worth a look");
    let mut said = false;

    let mut vehicles: Vec<_> = reg.vehicles.keys().cloned().collect();
    vehicles.sort();
    let mut weapons: Vec<_> = reg.weapons.keys().cloned().collect();
    weapons.sort();

    for w in &weapons {
        let Some(def) = reg.weapon(w) else { continue };
        for v in &vehicles {
            let Some(veh) = reg.vehicle(v) else { continue };
            let Some(shot) = duel(reg, w, v, Facing::West) else {
                continue;
            };
            let ttk = rounds_to_kill(reg, w, v, shot.expected);
            // Small arms that can kill armour at all is the `.max(1)` damage
            // floor showing through: a non-penetrating hit should do nothing,
            // and instead it chips. Worst where the armour is thickest.
            if def.damage_type == DamageType::SmallArms && veh.armor.front >= 5 && ttk != "inf" {
                println!(
                    "  {w} ({:?}) kills {v} (front armour {}) in {ttk} rounds — the \
                     `.max(1)` damage floor",
                    def.damage_type, veh.armor.front
                );
                said = true;
            }
            // A gun that needs longer than a battle is effectively no gun.
            if def.damage_type != DamageType::SmallArms && ttk == "inf" {
                println!("  {w} cannot meaningfully hurt {v} from the front");
                said = true;
            }
        }
    }

    // A vehicle that outranges its own eyes needs a spotter, which is a
    // deliberate design point for gun tanks and an accident anywhere else.
    for v in &vehicles {
        let Some(veh) = reg.vehicle(v) else { continue };
        let reach = veh
            .weapons
            .iter()
            .filter_map(|w| reg.weapon(w))
            .map(|w| w.range[1])
            .max()
            .unwrap_or(0);
        if reach > veh.vision_range {
            println!(
                "  {v} shoots {} but sees {} — needs a spotter",
                reg.scale.format_distance(reach as i32),
                reg.scale.format_distance(veh.vision_range as i32),
            );
            said = true;
        }
    }
    if !said {
        println!("  nothing stood out");
    }
}

struct Shot {
    expected: f32,
}

/// Stand two vehicles on an empty field and ask the real combat code what one
/// shot would do. `target_facing` selects the armour arc: the attacker is west
/// of the target, so a target facing west is hit head-on and one facing east
/// is hit from behind.
fn duel(
    reg: &DataRegistry,
    weapon: &str,
    target_vehicle: &str,
    target_facing: Facing,
) -> Option<Shot> {
    let attacker_vehicle = reg
        .vehicles
        .values()
        .find(|v| v.weapons.iter().any(|w| w == weapon))?;
    let weapon_index = attacker_vehicle.weapons.iter().position(|w| w == weapon)?;

    let state = two_unit_field(reg, &attacker_vehicle.id, target_vehicle, target_facing)?;
    let preview = preview_attack(reg, &state, UnitId(0), weapon_index, UnitId(1), false)?;
    if !preview.in_range {
        return None;
    }
    debug_assert_eq!(preview.facing, expected_arc(target_facing));
    Some(Shot {
        expected: preview.expected_damage,
    })
}

fn expected_arc(facing: Facing) -> ArmorFacing {
    match facing {
        Facing::West => ArmorFacing::Front,
        Facing::East => ArmorFacing::Rear,
        _ => ArmorFacing::Side,
    }
}

/// A strip of grass with one vehicle at each end. `shape: free` because this
/// is deliberately not a whole overworld tile.
fn two_unit_field(
    reg: &DataRegistry,
    attacker: &str,
    target: &str,
    target_facing: Facing,
) -> Option<BattleState> {
    let width = (DUEL_RANGE + 3) as usize;
    let row = "g".repeat(width);
    let file: MapFile = serde_json::from_value(serde_json::json!({
        "id": "balance_field",
        "kind": "battle",
        "shape": "free",
        "palette": { "g": "grass" },
        "rows": [row, row, row],
    }))
    .ok()?;
    let map = HexMap::from_map_file(&file).ok()?;

    let placements = vec![
        UnitPlacement {
            at: [1, 1],
            side: 0,
            vehicle: attacker.to_string(),
            crew: Vec::new(),
            name: Some("attacker".into()),
            facing: Some(Facing::East),
        },
        UnitPlacement {
            at: [1 + DUEL_RANGE, 1],
            side: 1,
            vehicle: target.to_string(),
            crew: Vec::new(),
            name: Some("target".into()),
            facing: Some(target_facing),
        },
    ];
    let sides = vec![
        SideState {
            name: "A".into(),
            ai: None,
        },
        SideState {
            name: "B".into(),
            ai: None,
        },
    ];
    let (roster, crews) = Roster::stamp_for(reg, &placements);
    Some(BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        1,
    ))
}

/// Rounds to remove a target's hit points, given a weapon's cadence.
fn rounds_to_kill(reg: &DataRegistry, weapon: &str, vehicle: &str, per_shot: f32) -> String {
    let (Some(w), Some(v)) = (reg.weapon(weapon), reg.vehicle(vehicle)) else {
        return "-".into();
    };
    let shots = (reg.scale.ticks_per_round as f32 / w.reload(&reg.scale) as f32).max(0.0);
    let per_round = per_shot * shots;
    if per_round <= 0.01 {
        return "inf".into();
    }
    let rounds = v.max_hp as f32 / per_round;
    // Longer than a battle lasts is the same as never, for balance purposes.
    if rounds > 40.0 {
        "inf".into()
    } else {
        format!("{rounds:.0}")
    }
}

/// Column heading that fits, without mangling short names.
fn short(id: &str) -> String {
    if id.len() <= 12 {
        id.to_string()
    } else {
        id.chars().take(11).chain(std::iter::once('.')).collect()
    }
}

// --- the simulated pass ---------------------------------------------------

#[derive(Default)]
struct Tally {
    wins: HashMap<String, usize>,
    draws: usize,
    stalemates: usize,
    rounds: Vec<u32>,
    kills: HashMap<String, usize>,
    deaths: HashMap<String, usize>,
    shots: u32,
    hits: u32,
    hits_by_arc: HashMap<String, u32>,
}

fn simulate(reg: &DataRegistry, games: usize) {
    heading(&format!("fought out: {games} battles on river_crossing"));
    let mut t = Tally::default();

    for game in 0..games {
        let seed = 1000 + game as u64;
        let mut state = BattleState::from_map(reg, "river_crossing", seed).expect("battle");
        let mut planners = [
            planner(reg, seed, "massed_armor"),
            planner(reg, seed + 1, "elastic_defense"),
        ];
        let mut rounds = 0;
        let mut last_hit: HashMap<UnitId, String> = HashMap::new();
        while !state.is_over() && rounds < 60 {
            for side in state.living_sides() {
                for _ in 0..64 {
                    if state.has_committed(side) || !state.is_planning() {
                        break;
                    }
                    let order = planners[side as usize].next_order(reg, &state, side);
                    if state.apply(reg, &order).is_err() {
                        let _ = state.apply(reg, &Order::Commit { side });
                        break;
                    }
                }
            }
            rounds += 1;
            for event in state.resolve_round(reg) {
                match event {
                    Event::ShotFired { .. } => t.shots += 1,
                    Event::ShotHit {
                        attacker,
                        target,
                        facing,
                        ..
                    } => {
                        t.hits += 1;
                        *t.hits_by_arc.entry(format!("{facing:?}")).or_default() += 1;
                        // Remembered so a kill can be credited: `UnitDestroyed`
                        // says who died, not who did it, because death is
                        // reaped at the end of a tick and may have several
                        // contributors.
                        if let Some(a) = state.units.get(attacker.index()) {
                            last_hit.insert(target, a.vehicle.clone());
                        }
                    }
                    Event::UnitDestroyed { unit, .. } => {
                        if let Some(u) = state.units.get(unit.index()) {
                            *t.deaths.entry(u.vehicle.clone()).or_default() += 1;
                        }
                        if let Some(killer) = last_hit.get(&unit) {
                            *t.kills.entry(killer.clone()).or_default() += 1;
                        }
                    }
                    _ => {}
                }
            }
        }
        t.rounds.push(rounds);
        match state.over.map(|r| (r.winner, r.reason)) {
            Some((Some(w), _)) => {
                *t.wins
                    .entry(state.sides[w as usize].name.clone())
                    .or_default() += 1;
                // Everything still alive on the winning side did the killing.
                for u in state.alive_units().filter(|u| u.side == w) {
                    *t.kills.entry(u.vehicle.clone()).or_default() += 0;
                }
            }
            Some((None, EndReason::Stalemate)) => {
                t.stalemates += 1;
                t.draws += 1;
            }
            _ => t.draws += 1,
        }
    }

    let mean = t.rounds.iter().sum::<u32>() as f32 / t.rounds.len().max(1) as f32;
    println!("  outcome:");
    let mut names: Vec<_> = t.wins.keys().cloned().collect();
    names.sort();
    for name in names {
        println!("    {:<22} {} wins", name, t.wins[&name]);
    }
    println!(
        "    {:<22} {} ({} by stalemate)",
        "draws", t.draws, t.stalemates
    );
    println!(
        "  length: {mean:.1} rounds mean, {} shortest, {} longest",
        t.rounds.iter().min().copied().unwrap_or(0),
        t.rounds.iter().max().copied().unwrap_or(0)
    );
    if t.shots > 0 {
        println!(
            "  gunnery: {} shots, {} hits ({:.0}%)",
            t.shots,
            t.hits,
            100.0 * t.hits as f32 / t.shots as f32
        );
    }
    let mut arcs: Vec<_> = t.hits_by_arc.iter().collect();
    arcs.sort();
    if !arcs.is_empty() {
        let total: u32 = t.hits_by_arc.values().sum();
        print!("  hits by arc:");
        for (arc, n) in arcs {
            print!(" {arc} {:.0}%", 100.0 * *n as f32 / total as f32);
        }
        println!();
    }
    if !t.deaths.is_empty() {
        println!("  {:<18} {:>6} {:>6}", "vehicle", "kills", "losses");
        let mut names: Vec<&String> = t.deaths.keys().chain(t.kills.keys()).collect();
        names.sort();
        names.dedup();
        for vehicle in names {
            println!(
                "    {vehicle:<16} {:>6} {:>6}",
                t.kills.get(vehicle).copied().unwrap_or(0),
                t.deaths.get(vehicle).copied().unwrap_or(0)
            );
        }
    }

    // Outcomes worth noticing, in the same spirit as the analytic flags.
    let stalemate_rate = t.stalemates as f32 / games.max(1) as f32;
    if stalemate_rate > 0.3 {
        println!(
            "\n  NOTE {:.0}% of battles ended in stalemate — the sides are losing each \
             other rather than fighting. Worth looking at vision, map size, or the \
             stalemate timeout before reading anything else here as balance.",
            100.0 * stalemate_rate
        );
    }
}

fn planner(
    reg: &DataRegistry,
    seed: u64,
    doctrine: &str,
) -> Box<dyn AiPlanner<BattleState, Order>> {
    make_battle_planner(
        &AiConfig {
            planner: "utility".into(),
            difficulty: 3,
            doctrine: Some(doctrine.into()),
        },
        seed,
        reg,
    )
}
