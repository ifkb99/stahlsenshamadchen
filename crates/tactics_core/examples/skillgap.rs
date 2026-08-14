//! Does skill win cleanly? A one-question instrument.
//!
//! Same map, same vehicles, same doctrine on both sides — the only thing
//! that differs is how well each side executes (`difficulty`). If command
//! skill in this simulation works the way it did at Rossbach or 73 Easting,
//! the gap should show up not just in who wins but in how little the better
//! side pays for it.
//!
//!   cargo run --release -p tactics_core --example skillgap [games]

use tactics_core::ai::{AiConfig, AiDriver, make_battle_planner};
use tactics_core::battle::BattleState;
use tactics_core::data::DataRegistry;

fn main() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (reg, _) = DataRegistry::load_dir(&root).expect("mods load");
    let games: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(36);

    println!("skill gap: identical forces and doctrine, only execution differs");
    println!("{games} battles per pairing on river_crossing, planner=utility\n");
    println!(
        "{:<12} {:>5} {:>5} {:>6} {:>12} {:>12} {:>8}",
        "pairing", "A won", "B won", "draws", "A lost/game", "B lost/game", "ratio"
    );

    for (a, b) in [
        (5, 5),
        (1, 1),
        (5, 3),
        (3, 5),
        (5, 1),
        (1, 5),
        (3, 1),
        (1, 3),
    ] {
        let (mut a_wins, mut b_wins, mut draws) = (0, 0, 0);
        let (mut a_losses, mut b_losses) = (0usize, 0usize);
        for seed in 0..games as u64 {
            let mut state =
                BattleState::from_map(&reg, "river_crossing", 9000 + seed).expect("battle");
            let mut ai = AiDriver::new();
            for (side, diff) in [(0u8, a), (1u8, b)] {
                ai.insert(
                    side,
                    make_battle_planner(
                        &AiConfig {
                            planner: "utility".into(),
                            difficulty: diff,
                            doctrine: None,
                        },
                        seed * 2 + side as u64,
                        &reg,
                    ),
                );
            }
            let mut rounds = 0;
            while !state.is_over() && rounds < 60 {
                ai.plan_round(&reg, &mut state);
                state.resolve_round(&reg);
                rounds += 1;
            }
            match state.over.and_then(|r| r.winner) {
                Some(0) => a_wins += 1,
                Some(_) => b_wins += 1,
                None => draws += 1,
            }
            a_losses += state.lost_units().filter(|u| u.side == 0).count();
            b_losses += state.lost_units().filter(|u| u.side == 1).count();
        }
        let per = |l: usize| l as f32 / games as f32;
        println!(
            "{:<12} {:>5} {:>5} {:>6} {:>12.2} {:>12.2} {:>8}",
            format!("{a} vs {b}"),
            a_wins,
            b_wins,
            draws,
            per(a_losses),
            per(b_losses),
            format!(
                "1:{:.1}",
                if a_losses > 0 {
                    b_losses as f32 / a_losses as f32
                } else {
                    f32::INFINITY
                }
            ),
        );
    }
}
