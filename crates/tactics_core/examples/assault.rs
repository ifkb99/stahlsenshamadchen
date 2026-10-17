//! One attack on a defended sector, narrated (ASSAULT.md): the instrument
//! for reading what `rollout --ground sector` only totals.
//!
//! ```sh
//! cargo run --release -p tactics_core --example assault -- 2 --size small
//! cargo run --release -p tactics_core --example assault -- 2 --size large --commander utility
//! cargo run --release -p tactics_core --example assault -- --survey 64 --size medium
//! ```
//!
//! The first argument is the field (the same number `rollout` fights as
//! `game / 2`). Both sides buy to the sector's budget, each fights by its
//! force's doctrine, and both are the utility AI — with the hand-written
//! commander over them if `--commander utility` says so. It prints the
//! sector, then every round: what each side has lost against where it
//! breaks, the zones the attacker holds, and the round's mines, wrecks and
//! zones changing hands; then how and why it ended.
//!
//! `--survey N` lays out N fields without fighting them and says, for each
//! that cannot be staged, why.

use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, SideCommand, UtilityPlanner};
use tactics_core::battle::{BattleState, Event, Order};
use tactics_core::data::DataRegistry;
use tactics_core::force::{Purchase, buy};
use tactics_core::harness::open::Worlds;
use tactics_core::harness::sector::{self, SectorRules, SectorSize};

fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| -> Option<String> {
        let i = args.iter().position(|a| a == name)?;
        args.get(i + 1).cloned()
    };
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (reg, _) = DataRegistry::load_dir(&root).expect("mods load");
    let size = flag("--size")
        .map(|n| SectorSize::named(&n).unwrap_or_else(|| panic!("--size: no size `{n}`")))
        .unwrap_or(SectorSize::Medium);
    let worlds = Worlds::default();
    let forces_for = |field: u64| {
        let purchase = Purchase {
            points: sector::budget(size, field),
            max_vehicles: size.shape().max_vehicles,
        };
        Some([
            buy(&reg, purchase, mix(field))?,
            buy(&reg, purchase, mix(field ^ 0xA5A5_A5A5))?,
        ])
    };

    // A shipped scenario map, fought `--games` times on successive seeds:
    // the attacker's record, how the battles ended, and what they cost.
    if let Some(map) = flag("--map") {
        let games: u64 = flag("--games").and_then(|s| s.parse().ok()).unwrap_or(1);
        let commander = flag("--commander").unwrap_or_else(|| "none".into());
        let doctrine = |side: usize, default: &str| {
            flag(if side == 0 {
                "--attacker-doctrine"
            } else {
                "--defender-doctrine"
            })
            .or_else(|| {
                reg.map(&map)
                    .and_then(|m| m.sides.get(side))
                    .and_then(|s| s.ai.as_ref())
                    .and_then(|ai| ai.doctrine.clone())
            })
            .unwrap_or_else(|| default.to_string())
        };
        let doctrines = [doctrine(0, "massed_armor"), doctrine(1, "elastic_defense")];
        let (mut won, mut rounds, mut lost, mut held, mut mines) = (0, 0, [0u64; 2], 0, 0);
        let mut ends: std::collections::BTreeMap<String, u32> = Default::default();
        for game in 0..games {
            let mut state = BattleState::from_map(&reg, &map, 7000 + game)
                .unwrap_or_else(|e| panic!("--map {map}: {e:?}"));
            let outcome = fight(&reg, &mut state, &doctrines, &commander, game, games == 1);
            won += u32::from(outcome.end.is_some_and(|(w, _)| w == Some(0)));
            rounds += outcome.rounds;
            lost[0] += u64::from(outcome.lost[0]);
            lost[1] += u64::from(outcome.lost[1]);
            held += outcome.held;
            mines += outcome.mines;
            let why = outcome.end.map_or("cut off".to_string(), |(_, r)| {
                format!("{r:?}").to_lowercase()
            });
            *ends.entry(why).or_default() += 1;
        }
        let n = games.max(1) as f64;
        let ends: Vec<String> = ends.iter().map(|(r, k)| format!("{r} {k}")).collect();
        println!(
            "{map}, {games} battles ({} vs {}, commander {commander}): attacker won {won} ({:.0}%); ended by {}",
            doctrines[0],
            doctrines[1],
            100.0 * f64::from(won) / n,
            ends.join(", ")
        );
        println!(
            "  {:.1} rounds; attacker lost {:.0}%, defender {:.0}%; attacker held {:.2} zones at the end; {:.1} mines struck",
            f64::from(rounds) / n,
            lost[0] as f64 / n / 10.0,
            lost[1] as f64 / n / 10.0,
            held as f64 / n,
            f64::from(mines) / n
        );
        return;
    }

    if let Some(n) = flag("--survey").and_then(|s| s.parse::<u64>().ok()) {
        let (mut ok, mut failed) = (0, 0);
        for field in 0..n {
            let Some(pair) = forces_for(field) else {
                println!("field {field}: nothing affordable");
                failed += 1;
                continue;
            };
            match sector::sector(
                &reg,
                &worlds,
                field,
                size,
                SectorRules::default(),
                [&pair[0], &pair[1]],
            ) {
                Err(e) => {
                    println!("field {field}: {e}");
                    failed += 1;
                }
                Ok(made) => match sector::stage(&reg, &made, [&pair[0], &pair[1]], 9000) {
                    None => {
                        println!("field {field}: a vehicle found nowhere to stand");
                        failed += 1;
                    }
                    Some(_) => ok += 1,
                },
            }
        }
        println!("{ok} staged, {failed} not");
        return;
    }

    let field: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let pair = forces_for(field).expect("both sides can buy something");
    let made = sector::sector(
        &reg,
        &worlds,
        field,
        size,
        SectorRules::default(),
        [&pair[0], &pair[1]],
    )
    .unwrap_or_else(|e| panic!("field {field}: {e}"));
    let mut state = sector::stage(&reg, &made, [&pair[0], &pair[1]], 9000 + field * 2)
        .expect("every vehicle finds ground");

    println!(
        "field {field}: a {} sector, {} defence, {} points a side",
        size.name(),
        if made.prepared { "prepared" } else { "hasty" },
        made.budget
    );
    for (i, z) in made.zones.iter().enumerate() {
        let (terrain, level) = made.world.tile(*z).unwrap_or(("?", 0));
        println!("  zone {}: {terrain} at level {level}", i + 1);
    }
    let needs = state.scenario.assault().map_or(0, |a| a.needs);
    println!("  the attacker needs {needs} of {}", made.zones.len());
    println!(
        "  {} minefield(s), {} obstacle tile(s)",
        state.scenario.minefields().len(),
        made.obstacles.len()
    );
    for (side, force) in pair.iter().enumerate() {
        let units: Vec<String> = force
            .units
            .iter()
            .map(|u| match &u.carries {
                Some(r) => format!("{} {} carrying {r}", u.count, u.vehicle),
                None => format!("{} {}", u.count, u.vehicle),
            })
            .collect();
        println!(
            "  {} ({}, {}): {}",
            if side == 0 { "attacker" } else { "defender" },
            force.id,
            force.doctrine,
            units.join(", ")
        );
    }

    let commander = flag("--commander").unwrap_or_else(|| "none".into());
    let doctrines = [pair[0].doctrine.clone(), pair[1].doctrine.clone()];
    fight(&reg, &mut state, &doctrines, &commander, field, true);
}

/// How one battle ended: who won, why, after how many rounds (`None` for
/// a battle cut off at the cap), and each side's losses in thousandths.
struct Outcome {
    end: Option<(Option<u8>, tactics_core::battle::EndReason)>,
    rounds: u32,
    lost: [u32; 2],
    held: usize,
    mines: u32,
}

/// Fight `state` out, both sides the utility AI by `doctrines` (under the
/// hand-written commander if `commander` is `utility`), narrating every
/// round if `verbose`.
fn fight(
    reg: &DataRegistry,
    state: &mut BattleState,
    doctrines: &[String; 2],
    commander: &str,
    seed: u64,
    verbose: bool,
) -> Outcome {
    let mut ai = AiDriver::new();
    for side in 0..2u8 {
        let config = AiConfig {
            planner: "utility".into(),
            difficulty: 5,
            doctrine: Some(doctrines[side as usize].clone()),
        };
        let seed = seed.wrapping_mul(2).wrapping_add(side as u64);
        let planner: Box<dyn AiPlanner<BattleState, Order>> = if commander == "utility" {
            Box::new(SideCommand::from_config(&config, seed, reg))
        } else {
            Box::new(UtilityPlanner::from_config(&config, seed, reg))
        };
        ai.insert(side, planner);
    }
    let mut mines = 0;
    let name = |state: &BattleState, id| {
        state
            .unit(id)
            .or_else(|| state.lookup(id))
            .map(|u| {
                let who = if u.side == 0 { "A" } else { "D" };
                format!(
                    "{who}:{}{}",
                    u.vehicle,
                    if u.dug_in > 0 { "(dug)" } else { "" }
                )
            })
            .unwrap_or_else(|| format!("unit {}", id.0))
    };
    let mut last_hit: std::collections::BTreeMap<u32, u32> = Default::default();
    let breaks = |side: u8| state.scenario.break_point(side).unwrap_or(100);
    let (attacker_breaks, defender_breaks) = (breaks(0), breaks(1));
    let mut round = 0;
    while !state.is_over() && round < 120 {
        ai.plan_round(reg, state);
        let events = state.resolve_round(reg);
        round += 1;
        let mut notes: Vec<String> = Vec::new();
        for e in &events {
            if let Event::ShotHit {
                attacker, target, ..
            } = e
            {
                last_hit.insert(target.0, attacker.0);
            }
        }
        for e in &events {
            match e {
                Event::MineStruck { unit, .. } => {
                    mines += 1;
                    notes.push(format!("{} hit a mine", name(state, *unit)))
                }
                Event::UnitDestroyed { unit, .. } => notes.push(match last_hit.get(&unit.0) {
                    Some(by) => format!(
                        "{} lost to {}",
                        name(state, *unit),
                        name(state, tactics_core::battle::UnitId(*by))
                    ),
                    None => format!("{} lost", name(state, *unit)),
                }),
                Event::ObjectiveTaken {
                    objective, side, ..
                } => notes.push(match side {
                    Some(0) => format!("the attacker took {objective}"),
                    Some(_) => format!("the defender took {objective} back"),
                    None => format!("{objective} contested"),
                }),
                Event::BattleEnded { winner, reason } => notes.push(format!(
                    "ENDED: {reason:?}, {}",
                    match winner {
                        Some(0) => "the attacker wins",
                        Some(_) => "the defender wins",
                        None => "nobody wins",
                    }
                )),
                _ => {}
            }
        }
        let held = state.objectives().filter(|(_, h)| *h == Some(0)).count();
        // The defender's crews on the field and how many are still in a
        // hole: whether a defence stayed in the positions it prepared.
        let on_field = state
            .units
            .iter()
            .filter(|u| u.side == 1 && u.alive() && u.aboard.is_none());
        let (standing, dug) =
            on_field.fold((0, 0), |(n, d), u| (n + 1, d + usize::from(u.dug_in > 0)));
        if !verbose {
            continue;
        }
        println!(
            "round {round:3}: attacker lost {:3}% (breaks {attacker_breaks}), defender {:3}% (breaks {defender_breaks}); attacker holds {held}; defender dug in {dug}/{standing}{}{}",
            state.strength_lost(reg, 0) / 10,
            state.strength_lost(reg, 1) / 10,
            if notes.is_empty() { "" } else { " — " },
            notes.join("; ")
        );
    }
    if verbose && !state.is_over() {
        println!("cut off at round {round}");
    }
    Outcome {
        end: state.over.map(|r| (r.winner, r.reason)),
        rounds: round,
        lost: [state.strength_lost(reg, 0), state.strength_lost(reg, 1)],
        held: state.objectives().filter(|(_, h)| *h == Some(0)).count(),
        mines,
    }
}
