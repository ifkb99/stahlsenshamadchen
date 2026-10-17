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
//!
//! `--set PATH=VALUE` changes any mod field, as `balance --set` does.
//!
//! `--counts C,U` has every side count a cut-off crew as C percent lost and
//! an unnerved one as U percent toward its break (`BreakPoint`), on a map
//! or a sector; the summary then says how many breaks the count decided —
//! a side that broke whose strength alone had not.

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
    let (mut reg, _) = DataRegistry::load_dir(&root).expect("mods load");
    // `--set PATH=VALUE`, any number of times: a mod field by its json path,
    // as `balance --set` takes it.
    for (_, text) in args
        .iter()
        .zip(args.iter().skip(1))
        .filter(|(a, _)| *a == "--set")
    {
        let applied = tactics_core::harness::overrides::Override::parse(text)
            .and_then(|ov| tactics_core::harness::overrides::apply_override(&mut reg, &ov));
        if let Err(e) = applied {
            panic!("--set {text}: {e}");
        }
    }
    let reg = reg;
    let size = flag("--size")
        .map(|n| SectorSize::named(&n).unwrap_or_else(|| panic!("--size: no size `{n}`")))
        .unwrap_or(SectorSize::Medium);
    let worlds = Worlds::default();
    let counts: [u32; 2] = flag("--counts").map_or([0, 0], |s| {
        let v: Vec<u32> = s
            .split(',')
            .map(|x| x.parse().expect("--counts C,U"))
            .collect();
        [v[0], v[1]]
    });
    let rules = SectorRules {
        cut_off_counts: counts[0],
        unnerved_counts: counts[1],
        ..SectorRules::default()
    };
    let counting = |state: &mut BattleState| {
        let mut scenario = (*state.scenario).clone();
        for side in 0..2u8 {
            if let Some(point) = scenario.breaking_of(side).copied() {
                scenario = scenario.with_breaking(tactics_core::map::BreakPoint {
                    cut_off_percent: counts[0],
                    unnerved_percent: counts[1],
                    ..point
                });
            }
        }
        state.scenario = std::sync::Arc::new(scenario);
    };
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
        let mut by_count = 0;
        let mut ends: std::collections::BTreeMap<String, u32> = Default::default();
        for game in 0..games {
            let mut state = BattleState::from_map(&reg, &map, 7000 + game)
                .unwrap_or_else(|e| panic!("--map {map}: {e:?}"));
            counting(&mut state);
            let outcome = fight(&reg, &mut state, &doctrines, &commander, game, games == 1);
            by_count += u32::from(outcome.by_count);
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
            "  {:.1} rounds; attacker lost {:.0}%, defender {:.0}%; attacker held {:.2} zones at the end; {:.1} mines struck; {by_count} broke on crews cut off or unnerved",
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
            match sector::sector(&reg, &worlds, field, size, rules, [&pair[0], &pair[1]]) {
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
    let made = sector::sector(&reg, &worlds, field, size, rules, [&pair[0], &pair[1]])
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
    /// A side broke whose strength lost alone had not reached its point:
    /// the crews it could not count on decided it.
    by_count: bool,
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
        // Crews each side cannot count on: cut off, and unnerved.
        let shaken = |side: u8| {
            let mine = state.units.iter().filter(|u| u.side == side && u.alive());
            mine.fold((0, 0), |(c, n), u| {
                (
                    c + usize::from(u.cut_off),
                    n + usize::from(!reg.morale.rung(u.pressure).obeys),
                )
            })
        };
        let (shaken_a, shaken_d) = (shaken(0), shaken(1));
        println!(
            "round {round:3}: attacker lost {:3}% / will {:3}% (breaks {attacker_breaks}; cut off {}, unnerved {}), defender {:3}% / will {:3}% (breaks {defender_breaks}; cut off {}, unnerved {}); attacker holds {held}; defender dug in {dug}/{standing}{}{}",
            state.strength_lost(reg, 0) / 10,
            state.will_lost(reg, 0) / 10,
            shaken_a.0,
            shaken_a.1,
            state.strength_lost(reg, 1) / 10,
            state.will_lost(reg, 1) / 10,
            shaken_d.0,
            shaken_d.1,
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
        by_count: state
            .over
            .is_some_and(|r| r.reason == tactics_core::battle::EndReason::Broken)
            && (0..2u8).any(|side| {
                state.scenario.break_point(side).is_some_and(|p| {
                    state.will_lost(reg, side) >= p * 10 && state.strength_lost(reg, side) < p * 10
                })
            }),
    }
}
