//! Headless narrated playthrough: pits two AI planners against each other on
//! a battle map and prints the event stream as a readable battle log.
//!
//! Useful for eyeballing balance changes, debugging the sim without the UI,
//! and (eventually) batch balance runs.
//!
//! Usage:
//!   cargo run -p tactics_core --example playthrough            # seed 42
//!   cargo run -p tactics_core --example playthrough 1234       # custom seed

use tactics_core::ai::{AiConfig, AiDriver, make_battle_planner};
use tactics_core::battle::{BattleState, Event};
use tactics_core::data::DataRegistry;

fn main() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (registry, _) = DataRegistry::load_dir(&root).expect("mods load");
    let seed: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(42);

    let mut state = BattleState::from_map(&registry, "river_crossing", seed).expect("battle");
    let mut ai = AiDriver::new();
    ai.insert(
        0,
        make_battle_planner(
            &AiConfig {
                planner: "mcts".into(),
                difficulty: 4,
                doctrine: Some("massed_armor".into()),
            },
            seed,
            &registry,
        ),
    );
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "utility".into(),
                difficulty: 4,
                doctrine: Some("elastic_defense".into()),
            },
            seed + 1,
            &registry,
        ),
    );

    let name = |st: &BattleState, id: tactics_core::battle::UnitId| -> String {
        let u = &st.units[id.index()];
        format!(
            "[{}] {} ({})",
            st.sides[u.side as usize].name, u.name, u.vehicle
        )
    };

    let mut rounds = 0usize;
    while !state.is_over() && rounds < 200 {
        // Planning: every side writes orders for all of its units.
        ai.plan_round_with(&registry, &mut state, |d| {
            if let Some(e) = &d.rejected {
                println!("!! side {} illegal order {:?}: {e}", d.side, d.order);
            }
        });
        rounds += 1;

        // Resolution: everyone moves and shoots at once. Ticks are only
        // announced when something actually happened in them, so a quiet
        // approach march does not bury the fighting.
        let mut tick = 0;
        let mut announced = true;
        for ev in &state.resolve_round(&registry) {
            if !matches!(ev, Event::TickStarted { .. }) && !announced {
                println!("  tick {tick}");
                announced = true;
            }
            match ev {
                Event::RoundStarted { round } => println!("--- round {round} ---"),
                Event::TickStarted { tick: t } => {
                    tick = *t;
                    announced = false;
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
                Event::ShotFired {
                    attacker,
                    weapon,
                    blind,
                    opportunity,
                    at,
                    ..
                } => println!(
                    "{} fires {weapon}{}{} at {at:?}",
                    name(&state, *attacker),
                    if *blind { " (blind)" } else { "" },
                    if *opportunity { " (opportunity)" } else { "" },
                ),
                Event::ShotHit {
                    target,
                    damage,
                    facing,
                    remaining_hp,
                    ..
                } => println!(
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
                Event::MoraleChanged { unit, rung, obeys } => println!(
                    "{} is {rung}{}",
                    name(&state, *unit),
                    if *obeys { "" } else { " and will not advance" }
                ),
                Event::OrderRefused { unit, rung } => {
                    println!("{} refuses to advance ({rung})", name(&state, *unit))
                }
                Event::UnitExited {
                    unit, objective, ..
                } => println!(
                    "   >> {} drives off the map by {objective}",
                    name(&state, *unit)
                ),
                Event::ObjectiveTaken {
                    objective, side, ..
                } => println!(
                    "   >> {objective} taken by {}",
                    match side {
                        Some(s) => state.sides[*s as usize].name.clone(),
                        None => "nobody - contested".into(),
                    }
                ),
                Event::BattleEnded { winner, reason } => println!(
                    "=== battle over after {rounds} rounds: {:?} wins ({reason:?}), score {:?} ===",
                    winner.map(|w| state.sides[w as usize].name.clone()),
                    state.score
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
