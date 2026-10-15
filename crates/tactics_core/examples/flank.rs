//! Does a flank pay in this engine, before anybody has to get there?
//! (REIMAGINED.md, *Tactics from equipment*.)
//!
//! The fix-and-flank play reads neutral, and a neutral play says nothing
//! about the tactic until the two halves are told apart: *being* on a flank,
//! and *getting* there. This stages the first half alone. Two tanks a side on
//! open grass, the defenders facing west; the attackers start either both in
//! front of them, or one in front and one already off their side or behind
//! them, at the same distance. Utility against utility at difficulty 5, no
//! objectives, so the battle is about the shooting and nothing else.
//!
//! If the flank start wins far more than the head-on start, position pays
//! and the problem is the road there. If it does not, a learner has nothing
//! to find by going round, whatever features it is given.
//!
//! ```sh
//! cargo run --release -p tactics_core --example flank -- 400
//! cargo run --release -p tactics_core --example flank -- 800 --hedge \
//!     --set balance.cover_lip_percent=100 --set vehicle.medium_tank.traverse_seconds=20
//! ```
//!
//! `--hedge` puts a hedge line one hex in front of the defenders, so the
//! guns in front fire across it and the flanker does not; `--set` takes any
//! content override, as `balance` does, which is how the flank rules
//! (`cover_lip_percent`, `hull_down_cover`, `traverse_seconds`,
//! `flank_search_percent`, `buttoned_flank_search_percent`) are measured
//! before any of them is shipped.

use tactics_core::ai::{AiConfig, AiDriver, make_battle_planner};
use tactics_core::battle::{BattleState, Event, SideState};
use tactics_core::data::{ArmorFacing, DataRegistry};
use tactics_core::harness::parallel::run_all;
use tactics_core::map::{Battlefield, Facing, HexMap, Scenario, UnitPlacement};
use tactics_core::roster::Roster;

/// Where the second attacker starts, in offset coordinates; the first is
/// always eight hexes in front of the defenders.
/// Each faces her targets: east from the front, west from behind, and from
/// the side "at the enemy" (no hex edge points due south).
const STARTS: &[(&str, [i32; 2], Option<Facing>)] = &[
    ("head-on", [12, 18], Some(Facing::East)),
    ("flank", [20, 7], None),
    ("rear", [28, 16], Some(Facing::West)),
];

#[derive(Default, Clone, Copy)]
struct Tally {
    won: u32,
    lost: u32,
    drew: u32,
    hits: u32,
    flank_hits: u32,
    rounds: u32,
}

fn battle(
    reg: &DataRegistry,
    hedge: bool,
    vehicle: &str,
    second: [i32; 2],
    second_facing: Option<Facing>,
    seed: u64,
) -> Tally {
    let mut terrain = HexMap::default();
    for col in 0..36 {
        for row in 0..30 {
            let ground = if hedge && col == 19 && (15..=19).contains(&row) {
                "hedgerow"
            } else {
                "grass"
            };
            terrain.insert(tactics_core::offset_to_hex(col, row), ground, 0);
        }
    }
    let field = Battlefield {
        terrain,
        scenario: Scenario::default(),
    };
    let place = |at: [i32; 2], side: u8, facing: Option<Facing>| UnitPlacement {
        aboard_at: None,
        at,
        side,
        vehicle: vehicle.into(),
        crew: Vec::new(),
        name: None,
        facing,
        formation: None,
        leads: false,
    };
    let placements = vec![
        place([12, 16], 0, Some(Facing::East)),
        place(second, 0, second_facing),
        place([20, 16], 1, Some(Facing::West)),
        place([20, 18], 1, Some(Facing::West)),
    ];
    let (roster, crews) = Roster::stamp_for(reg, &placements);
    let sides = ["Attackers", "Defenders"]
        .map(|name| SideState {
            name: name.into(),
            ai: None,
        })
        .to_vec();
    let Ok(mut state) = BattleState::from_placements(
        reg,
        field,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    ) else {
        return Tally::default();
    };
    let mut ai = AiDriver::new();
    for side in 0..2u8 {
        let config = AiConfig {
            planner: "utility".into(),
            difficulty: 5,
            doctrine: Some("bounding_overwatch".into()),
        };
        ai.insert(
            side,
            make_battle_planner(&config, seed * 2 + side as u64, reg),
        );
    }
    let mut t = Tally::default();
    while !state.is_over() && t.rounds < 60 {
        ai.plan_round(reg, &mut state);
        for e in state.resolve_round(reg) {
            if let Event::ShotHit {
                attacker, facing, ..
            }
            | Event::ShotBounced {
                attacker, facing, ..
            } = e
                && state.lookup(attacker).map(|u| u.side) == Some(0)
            {
                t.hits += 1;
                t.flank_hits += (facing != ArmorFacing::Front) as u32;
            }
        }
        t.rounds += 1;
    }
    match state.over.and_then(|r| r.winner) {
        Some(0) => t.won = 1,
        Some(_) => t.lost = 1,
        None => t.drew = 1,
    }
    t
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let games: u64 = args.first().and_then(|a| a.parse().ok()).unwrap_or(400);
    let hedge = args.iter().any(|a| a == "--hedge");
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (mut reg, _) = DataRegistry::load_dir(&root).expect("mods load");
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
    let only: Option<&str> = args
        .iter()
        .position(|a| a == "--only")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str);
    for vehicle in ["medium_tank", "light_tank", "heavy_tank"]
        .into_iter()
        .filter(|v| only.is_none_or(|o| o == *v))
    {
        println!("{vehicle}, two a side, {games} battles a row");
        println!(
            "  {:10} {:>5} {:>5} {:>5} {:>7} {:>7}",
            "second", "won", "lost", "drew", "rounds", "flank%"
        );
        for &(name, second, facing) in STARTS {
            let seeds: Vec<u64> = (0..games).collect();
            let mut t = Tally::default();
            for x in run_all(&seeds, |s| battle(&reg, hedge, vehicle, second, facing, *s)) {
                t.won += x.won;
                t.lost += x.lost;
                t.drew += x.drew;
                t.hits += x.hits;
                t.flank_hits += x.flank_hits;
                t.rounds += x.rounds;
            }
            println!(
                "  {name:10} {:>5} {:>5} {:>5} {:>7.1} {:>6.0}%",
                t.won,
                t.lost,
                t.drew,
                t.rounds as f32 / games as f32,
                100.0 * t.flank_hits as f32 / t.hits.max(1) as f32
            );
        }
        println!();
    }
}
