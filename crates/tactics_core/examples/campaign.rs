//! Play the shipped campaign out headlessly, every side a machine, and say
//! what happened: how it ended, what it cost each academy, and every battle
//! it fought along the way.
//!
//! ```sh
//! cargo run --release -p tactics_core --example campaign              # seeds 0..8
//! cargo run --release -p tactics_core --example campaign -- 3         # one seed, every battle
//! cargo run --release -p tactics_core --example campaign -- 0 32      # seeds 0..32, summary only
//! cargo run --release -p tactics_core --example campaign -- --map frontier 0 32  # the drawn one
//! cargo run --release -p tactics_core --example campaign -- --trace 3  # ...and every event
//! ```
//!
//! With no `--map` it plays the campaign the game opens on (`campaign` in
//! `mod.json`), so the instrument measures what a player plays.
//!
//! The first instrument this project has had that reads the *campaign*
//! rather than a battle: before `tactics_core::field` existed the path from a
//! clash back to the map ran through the game crate's screens. The machinery
//! is `tactics_core::harness::campaign`, which `tests/harness.rs` holds to
//! its contract (it ends, it declines nothing on shipped content, and the
//! same seed is the same war); this file is layout.

use tactics_core::data::DataRegistry;
use tactics_core::harness::campaign::{CampaignOptions, CampaignRun, play};
use tactics_core::harness::parallel::run_all;

/// The campaign played when `--map` names none and no mod names one.
const MAP: &str = "frontier";

fn main() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (registry, _) = DataRegistry::load_dir(&root).expect("mods load");
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let map = raw
        .iter()
        .position(|a| a == "--map")
        .and_then(|i| raw.get(i + 1))
        .cloned()
        .or_else(|| registry.campaign.clone())
        .unwrap_or_else(|| MAP.to_string());
    let trace = raw.iter().any(|a| a == "--trace");
    let args: Vec<u64> = raw
        .iter()
        .filter(|a| *a != "--map" && **a != map && *a != "--trace")
        .map(|a| a.parse().expect("seeds are numbers"))
        .collect();
    let map = map.as_str();
    let (from, to) = match args.as_slice() {
        [] => (0, 8),
        [one] => (*one, *one + 1),
        [from, to, ..] => (*from, *to),
    };
    let options = CampaignOptions {
        trace,
        ..CampaignOptions::default()
    };
    let seeds: Vec<u64> = (from..to).collect();
    let started = std::time::Instant::now();
    let runs: Vec<CampaignRun> = run_all(&seeds, |seed| {
        play(&registry, map, *seed, &options).expect("the shipped campaign builds")
    });
    let elapsed = started.elapsed();
    // Every event of every run, seed first: the check that a change to how
    // the campaign is stored left what it does alone.
    for (seed, run) in seeds.iter().zip(&runs) {
        for event in &run.events {
            println!("{seed} {event:?}");
        }
    }

    let names: Vec<String> = tactics_core::overworld::OverworldState::from_map(&registry, map, 0)
        .expect("the shipped campaign builds")
        .sides
        .iter()
        .map(|s| s.name.clone())
        .collect();

    if runs.len() == 1 {
        let run = &runs[0];
        println!(
            "{:>4} {:>18} {:>8} {:>7} {:>7} {:>9} {:>9}",
            "day", "battlefield", "attacker", "winner", "rounds", "lost", "withdrew"
        );
        for b in &run.battles {
            let winner = b
                .winner
                .map_or("-".to_string(), |w| names[w as usize].clone());
            let lost = b
                .hulls_lost
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("/");
            println!(
                "{:>4} {:>18} {:>8} {:>7} {:>6}{} {:>9} {:>9}",
                b.day,
                b.map_id,
                names[b.attacker_side as usize],
                winner,
                b.rounds,
                if b.cut_off { "!" } else { " " },
                lost,
                b.withdrew
            );
        }
        println!();
    }

    println!(
        "{:>5} {:>5} {:>14} {:>13} {:>8} {:>13} {:>13}",
        "seed", "days", "winner", "by", "battles", "killed", "hurt"
    );
    let mut wins = vec![0u32; names.len()];
    let mut undecided = 0;
    for (seed, run) in seeds.iter().zip(&runs) {
        let (winner, by) = match run.end {
            Some((Some(w), reason)) => {
                wins[w as usize] += 1;
                (names[w as usize].clone(), format!("{reason:?}"))
            }
            Some((None, reason)) => ("nobody".into(), format!("{reason:?}")),
            None => {
                undecided += 1;
                ("-".into(), "unfinished".into())
            }
        };
        let per_side = |v: &[u32]| {
            v.iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("/")
        };
        println!(
            "{:>5} {:>5} {:>14} {:>13} {:>8} {:>13} {:>13}",
            seed,
            run.days,
            winner,
            by,
            run.battles.len(),
            per_side(&run.killed),
            per_side(&run.hurt)
        );
        if run.declined > 0 {
            println!(
                "      declined {} clash(es) it could not stage",
                run.declined
            );
        }
    }
    println!();
    for (name, won) in names.iter().zip(&wins) {
        println!("{name}: won {won} of {}", runs.len());
    }
    if undecided > 0 {
        println!("{undecided} ran past {} days undecided", options.max_days);
    }
    let battles: usize = runs.iter().map(|r| r.battles.len()).sum();
    let cut: usize = runs
        .iter()
        .flat_map(|r| &r.battles)
        .filter(|b| b.cut_off)
        .count();
    if cut > 0 {
        println!(
            "{cut} of {battles} battles hit the {}-round cap",
            options.max_rounds
        );
    }
    println!(
        "{} campaign(s), {battles} battles, in {:.1?}",
        runs.len(),
        elapsed
    );
}
