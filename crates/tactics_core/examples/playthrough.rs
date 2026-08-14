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
        // Planning: every side writes orders for all of its units. Orders
        // are mostly silent, but a mission being assigned is worth hearing
        // as it happens rather than buried in the resolution.
        ai.plan_round_with(&registry, &mut state, |d| {
            if let Some(e) = &d.rejected {
                println!("!! side {} illegal order {:?}: {e}", d.side, d.order);
            }
            for ev in &d.events {
                if let Event::MissionAssigned { formation, mission } = ev {
                    println!("[side {}] {formation} ordered to {mission:?}", d.side);
                }
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
                Event::TookCover { unit, at } => {
                    println!("{} breaks for cover at {at:?}", name(&state, *unit))
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
                // The gap between this and the `fires` line above is the
                // artillery rework made watchable: the shell was aimed at
                // ground ticks ago, and the narration has to show the ticks
                // passing or a target that walked out of the beaten zone
                // reads as the gun having missed.
                Event::ShellLanded { at, ammo, .. } => {
                    println!("   ** {ammo} lands at {at:?} **")
                }
                Event::ShotHit {
                    target,
                    damage,
                    facing,
                    ..
                } => println!(
                    "   PENETRATES {} on the {facing:?} ({damage} pts of effect)",
                    name(&state, *target)
                ),
                Event::CrewHit { unit, girl, out } => println!(
                    "   ** crew hit aboard {}: girl #{} is {} **",
                    name(&state, *unit),
                    girl.0,
                    if *out { "OUT" } else { "wounded" }
                ),
                Event::ModuleHit {
                    unit,
                    module,
                    destroyed,
                } => println!(
                    "   {}'s {module} is {}",
                    name(&state, *unit),
                    if *destroyed { "destroyed" } else { "damaged" }
                ),
                Event::BrewedUp { unit } => {
                    println!("   ** {} BREWS UP **", name(&state, *unit))
                }
                Event::Abandoned { unit } => {
                    println!("   >> the crew abandons {}", name(&state, *unit))
                }
                Event::Mounted { unit, into } => println!(
                    "   >> {} mounts up in {}",
                    name(&state, *unit),
                    name(&state, *into)
                ),
                Event::Dismounted { unit, at } => {
                    println!("   >> {} dismounts at {at:?}", name(&state, *unit))
                }
                Event::ShotMissed { .. } => println!("   miss"),
                Event::ShotBounced { target, facing, .. } => {
                    println!("   BOUNCES off {} ({facing:?})", name(&state, *target))
                }
                Event::WeaponDry { unit, weapon } => {
                    println!(
                        "   >> {} has fired her last {weapon} round",
                        name(&state, *unit)
                    )
                }
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
                Event::MissionAssigned { formation, mission } => {
                    println!("   >> {formation} ordered to {mission:?}")
                }
                // The two halves of an order travelling: sent above, arrived
                // here. A narrator that printed only the first would make a
                // formation look disobedient for the ticks in between, which
                // is exactly the confusion the latency events exist to
                // prevent.
                Event::MissionReceived { formation, mission } => {
                    println!("   >> {formation} receives its orders: {mission:?}")
                }
                Event::MissionCompleted { formation, mission } => {
                    println!("   >> {formation} completes {mission:?} and takes up the next order")
                }
                Event::OutOfContact { unit } => {
                    println!("   >> {} is out of contact", name(&state, *unit))
                }
                // The two halves of a direct order waiting for a wire. Only
                // ever a human commander's, so the narrator never prints them
                // — an AI side's units decide for themselves and need no
                // radio to hear their own minds.
                Event::OrdersWaiting { unit } => {
                    println!(
                        "   >> orders for {} are waiting at the radio",
                        name(&state, *unit)
                    )
                }
                Event::OrdersDelivered { unit } => {
                    println!("   >> orders reach {}", name(&state, *unit))
                }
                Event::ContactRestored { unit } => {
                    println!("   >> {} is back in contact", name(&state, *unit))
                }
                Event::ContactReported { unit, by, at } => println!(
                    "   >> {} reports {} at {at:?}",
                    name(&state, *by),
                    name(&state, *unit)
                ),
                // Named on both ends: the narrator is the standing proof that
                // every event carries its own story, and "command passes" with
                // nobody in it would be exactly the archaeology that discipline
                // exists to prevent.
                Event::CommandPassed {
                    formation,
                    from,
                    to,
                } => println!(
                    "   >> command of {formation} passes from {} to {}",
                    name(&state, *from),
                    name(&state, *to)
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
            "{} {} - {}% condition{}",
            state.sides[u.side as usize].name,
            u.name,
            (state.condition(&registry, u) * 100.0).round(),
            if u.alive { "" } else { " (destroyed)" }
        );
    }
}
