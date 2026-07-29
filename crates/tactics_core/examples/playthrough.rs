//! Headless narrated playthrough: pits two AI planners against each other on
//! a battle map and prints the event stream as a readable battle log.
//!
//! Useful for eyeballing balance changes, debugging the sim without the UI,
//! and (eventually) batch balance runs.
//!
//! Usage:
//!   cargo run -p tactics_core --example playthrough            # seed 42
//!   cargo run -p tactics_core --example playthrough 1234       # custom seed

use tactics_core::ai::{make_battle_planner, AiConfig};
use tactics_core::battle::{BattleState, Event, Order};
use tactics_core::data::DataRegistry;

fn main() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (registry, _) = DataRegistry::load_dir(&root).expect("mods load");
    let seed: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(42);

    let mut state = BattleState::from_map(&registry, "river_crossing", seed).expect("battle");
    let mut planners = [
        make_battle_planner(
            &AiConfig { planner: "mcts".into(), difficulty: 4 },
            seed,
        ),
        make_battle_planner(
            &AiConfig { planner: "utility".into(), difficulty: 4 },
            seed + 1,
        ),
    ];

    let name = |st: &BattleState, id: tactics_core::battle::UnitId| -> String {
        let u = &st.units[id.index()];
        format!("[{}] {} ({})", st.sides[u.side as usize].name, u.name, u.vehicle)
    };

    let mut orders = 0usize;
    while !state.is_over() && orders < 2000 {
        let side = state.active_side;
        let order = planners[side as usize].next_order(&registry, &state, side);
        orders += 1;
        let events = match state.apply(&registry, &order) {
            Ok(ev) => ev,
            Err(e) => {
                println!("!! side {side} illegal order {order:?}: {e}");
                state.apply(&registry, &Order::EndTurn).unwrap()
            }
        };
        for ev in &events {
            match ev {
                Event::TurnStarted { side, turn } => {
                    println!("--- round {turn}: {} ---", state.sides[*side as usize].name)
                }
                Event::UnitMoved { unit, path } => println!(
                    "{} moves {} hexes to {:?}",
                    name(&state, *unit),
                    path.len() - 1,
                    path.last().unwrap()
                ),
                Event::UnitTrapped { unit, .. } => {
                    println!("{} AMBUSHED mid-move!", name(&state, *unit))
                }
                Event::ShotFired { attacker, weapon, blind, counter, at, .. } => println!(
                    "{} fires {weapon}{}{} at {at:?}",
                    name(&state, *attacker),
                    if *blind { " (blind)" } else { "" },
                    if *counter { " (return fire)" } else { "" },
                ),
                Event::ShotHit { target, damage, facing, remaining_hp, .. } => println!(
                    "   HIT {} on the {facing:?} for {damage}, {remaining_hp} hp left",
                    name(&state, *target)
                ),
                Event::ShotMissed { .. } => println!("   miss"),
                Event::UnitDestroyed { unit, .. } => {
                    println!("   ** {} DESTROYED **", name(&state, *unit))
                }
                Event::UnitSpotted { unit, by_side, .. } => println!(
                    "{} spotted by {}",
                    name(&state, *unit),
                    state.sides[*by_side as usize].name
                ),
                Event::BattleEnded { winner, reason } => println!(
                    "=== battle over after {} orders: {:?} wins ({reason:?}) ===",
                    orders,
                    winner.map(|w| state.sides[w as usize].name.clone())
                ),
            }
        }
    }
    for u in &state.units {
        println!(
            "{} {} - {} hp{}",
            state.sides[u.side as usize].name,
            u.name,
            u.hp.max(0),
            if u.alive { "" } else { " (destroyed)" }
        );
    }
}
