//! A headless seat for the player: fight one side of a battle by hand, a
//! round at a time, from what that side's commander knows.
//!
//! ```text
//! cargo run --release -p tactics_core --example command -- start <map> <seed> <dir>
//! cargo run --release -p tactics_core --example command -- turn <dir> [orders.json]
//! cargo run --release -p tactics_core --example command -- look <dir>
//! ```
//!
//! Every other instrument here fights AI against AI, so none of them can say
//! what a battle is like from the commander's chair, and that is the question
//! the slice (SLICE.md) is asking: whether a player can form an expectation
//! of how an order will play out, and see why it didn't. This is the chair
//! without the Bevy layer. The battle lives in `<dir>/battle.json` as an
//! ordinary save, so a turn is load, order, resolve, save.
//!
//! **What is the player's and what is the machine's.** The seat is side 0.
//! Its *commander* is whoever writes the orders file: section missions
//! (a job and ground, `Order::SetMission`), and, where she wants, a
//! commander's word to one crew (`Order::Radio`), a mount or a dismount.
//! Its *crews* are the utility planner, which plans only the units the
//! commander did not radio, inside the missions she set (`next_unplanned_unit`)
//! — the slice's "orders are general" seat. The other side is whatever AI
//! the map gives it.
//!
//! **What is shown.** Only what side 0 would know: events pass through
//! `Event::heard_by`, the one log filter the game uses, and enemies are
//! `known_enemies(Knower::Commander)`. Coordinates are the map file's
//! `[column, row]`, and cadets are named.
//!
//! The orders file:
//!
//! ```json
//! { "missions": [ {"formation": "sec1", "mission": "advance", "to": [13, 9]},
//!                 {"formation": "sec2", "mission": "hold"} ],
//!   "radio":    [ {"unit": "1st Carrier", "to": [12, 6], "binding": true} ],
//!   "dismount": [ "2nd Squad" ],
//!   "mount":    [ {"unit": "3rd Squad", "into": "3rd Carrier"} ],
//!   "release":  [ "sec3" ],
//!   "stay":     [ "3rd Carrier" ],
//!   "stay_aboard": [ "1st Squad" ] }
//! ```
//!
//! `stay` holds a crew on the hex she stands on, binding; `stay_aboard`
//! keeps a passenger in her carrier this round; `fire` orders a shot
//! (`{"unit": "2nd Squad", "at": "Wiesel", "weapon": "rpg"}`) at an enemy
//! her commander knows of.
//!
//! **The preview.** `plan` runs the same planning `turn` does and prints
//! each crew's plan under the orders file, saving nothing: where she means
//! to go, whether that keeps her order, and whether her passengers mean to
//! get out. Change the orders and plan again; `turn` with the same file
//! fights exactly what the last `plan` showed.
//!
//! **The forecast.** Every sitrep ends with every shot either side could
//! take from where it stands (`preview_attack`, the game's hover panel, for
//! every pair with a line and the range): the hit chance with its named
//! modifiers, the chance through the plate it would strike, and "hurts",
//! their product. A pair with a line and nothing that can hurt is listed as
//! such, because "no shot" and "a shot that does nothing" are different
//! things to know. In the preview, an ordered shot carries its odds from
//! where she stands and from where her route ends, or says she has no line:
//! a fire order with no line is accepted and waits.
//!
//! Missions are `advance`, `assault`, `hold` (with or without `to`),
//! `recon`, and `support` (base of fire for the formation named in `of`). Planners are rebuilt each turn from the seed and the round, so a
//! hand-fought battle is reproducible from its orders files, though not
//! identical to the same battle played in one process.
//!
//! Each turn also appends the round's whole event stream, unfiltered, to
//! `<dir>/rounds.jsonl` with where every unit stood after it, so a hand game
//! can be read afterwards by `examples/heatmap --seat <dir>`. That file is
//! the post-mortem's, not the commander's: it holds what she was not told.

#[path = "heatmap/log.rs"]
mod log;

use std::path::{Path, PathBuf};

use hexx::Hex;
use serde::Deserialize;
use tactics_core::ai::{AiConfig, AiDriver, make_battle_planner};
use tactics_core::battle::{BattleState, Event, Knower, Latitude, Mission, Order, UnitId};
use tactics_core::data::DataRegistry;
use tactics_core::save::{self, SaveGame};
use tactics_core::{hex_to_offset, offset_to_hex};

/// The side this seat plays.
const SEAT: u8 = 0;

// Every order struct denies unknown fields. A seat writes this file by
// hand, and a misspelled key (`"dismout"`) would otherwise be dropped
// without a word: the crew does nothing and the commander reads it as the
// crew's choice. None of them may use `#[serde(flatten)]`, which does not
// combine with `deny_unknown_fields`.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Orders {
    missions: Vec<MissionOrder>,
    radio: Vec<RadioOrder>,
    dismount: Vec<String>,
    mount: Vec<MountOrder>,
    release: Vec<String>,
    /// Hold exactly here, meant to the letter: a binding radio order to
    /// the hex she stands on. The overrule for a crew whose plan leaves
    /// ground you want held.
    stay: Vec<String>,
    /// Passengers who stay aboard this round: the overrule for a squad
    /// whose plan is to get out where the carrier starts the round.
    stay_aboard: Vec<String>,
    /// A shot ordered: this crew, at this known enemy, with this weapon (by
    /// id; her first that can if none is named).
    fire: Vec<FireOrder>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FireOrder {
    unit: String,
    at: String,
    #[serde(default)]
    weapon: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MissionOrder {
    formation: String,
    mission: String,
    #[serde(default)]
    to: Option<[i32; 2]>,
    /// For `support`: the formation to cover.
    #[serde(default)]
    of: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RadioOrder {
    unit: String,
    to: [i32; 2],
    /// Meant to the letter: she may not break the route off for cover
    /// (`Latitude::Binding`). A player's order; the AI never gives one.
    #[serde(default)]
    binding: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MountOrder {
    unit: String,
    into: String,
}

/// Why an orders file could not be taken: both name the file, so a seat
/// juggling several knows which one to fix.
#[derive(Debug, thiserror::Error)]
enum OrdersError {
    #[error("{}: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// serde's text names the offending field and where it stands.
    #[error("{}: {source}", path.display())]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
}

/// An orders document, read strictly: see the note on `Orders`. `path` is
/// only for the error.
fn parse_orders(text: &str, path: &Path) -> Result<Orders, OrdersError> {
    serde_json::from_str(text).map_err(|source| OrdersError::Parse {
        path: path.to_path_buf(),
        source,
    })
}

fn read_orders(path: &Path) -> Result<Orders, OrdersError> {
    let text = std::fs::read_to_string(path).map_err(|source| OrdersError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    parse_orders(&text, path)
}

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (registry, _) = DataRegistry::load_dir(&root).expect("mods load");
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: command start <map> <seed> <dir> | plan <dir> [orders.json] | turn <dir> [orders.json] | look <dir>";
    let orders_at = |path: Option<&String>| -> Orders {
        match path {
            // A bad file stops the turn before anything is planned or
            // saved: half an order set fought is worse than none.
            Some(path) => read_orders(Path::new(path)).unwrap_or_else(|e| {
                eprintln!("orders refused: {e}");
                std::process::exit(2)
            }),
            None => Orders::default(),
        }
    };
    match args.first().map(String::as_str) {
        Some("start") if args.len() == 4 => {
            let seed: u64 = args[2].parse().expect("seed is a number");
            let state = BattleState::from_map(&registry, &args[1], seed).expect("battle");
            let dir = PathBuf::from(&args[3]);
            std::fs::write(dir_seed(&dir), seed.to_string()).ok();
            std::fs::write(dir.join("rounds.jsonl"), "").ok();
            append(
                &dir,
                &log::Round {
                    round: 0,
                    events: Vec::new(),
                    units: log::placed(&state),
                    intentions: Vec::new(),
                },
            );
            store(&registry, &dir, state.clone());
            sitrep(&registry, &state);
        }
        // The preview: plan the round under these orders and show it, and
        // save nothing. Change the orders and plan again until the plans
        // are the ones you want, then `turn` with the same file.
        Some("plan") if args.len() >= 2 => {
            let dir = PathBuf::from(&args[1]);
            let mut state = load(&registry, &dir);
            if state.is_over() {
                println!("the battle is over");
                return;
            }
            let planned = plan(&registry, &mut state, &orders_at(args.get(2)), &dir);
            preview(&registry, &state, &planned);
        }
        Some("turn") if args.len() >= 2 => {
            let dir = PathBuf::from(&args[1]);
            let mut state = load(&registry, &dir);
            let orders = orders_at(args.get(2));
            turn(&registry, &mut state, &orders, &dir);
            store(&registry, &dir, state.clone());
            sitrep(&registry, &state);
        }
        Some("look") if args.len() == 2 => {
            sitrep(&registry, &load(&registry, Path::new(&args[1])));
        }
        _ => eprintln!("{usage}"),
    }
}

fn dir_seed(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).ok();
    dir.join("seed")
}

fn store(registry: &DataRegistry, dir: &Path, state: BattleState) {
    save::write(
        dir.join("battle.json"),
        &SaveGame::new(registry, None, Some(state)),
    )
    .expect("save writes");
}

fn load(registry: &DataRegistry, dir: &Path) -> BattleState {
    let (game, warnings) = save::read(registry, dir.join("battle.json")).expect("save reads");
    for w in warnings {
        eprintln!("save: {w}");
    }
    game.battle.expect("the save holds a battle")
}

/// What planning produced: what the commander's orders did on the way in,
/// and every crew's intention as the round stands committed.
struct Planned {
    /// Orders refused, and the engine's word on those accepted (an order
    /// held at the radio says so here).
    notes: Vec<String>,
    intentions: Vec<log::Intention>,
}

/// The commander's orders, then the crews and the enemy plan. Everything
/// up to the moment of commitment, and nothing after: `plan` stops here and
/// `turn` goes on to fight it. The planners are seeded from the battle's
/// seed and the round, so the intentions a `plan` shows are exactly the
/// ones the same orders' `turn` will fight.
fn plan(registry: &DataRegistry, state: &mut BattleState, orders: &Orders, dir: &Path) -> Planned {
    let seed: u64 = std::fs::read_to_string(dir_seed(dir))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);

    let mut notes = Vec::new();
    for order in commander_orders(registry, state, orders) {
        match state.apply(registry, &order) {
            Err(e) => notes.push(format!(
                "REFUSED {}: {e}",
                describe_text(state, order_text(&order))
            )),
            Ok(events) => {
                for ev in events {
                    notes.push(match ev {
                        Event::OrdersWaiting { unit } => format!(
                            "your order to {} is held at the radio: it reaches her at the top of next round",
                            name_of(state, unit)
                        ),
                        other => describe(state, &other),
                    });
                }
            }
        }
    }

    let mut ai = AiDriver::new();
    let round_seed = seed ^ (u64::from(state.round) << 32);
    ai.insert(
        SEAT,
        make_battle_planner(
            &AiConfig {
                planner: "utility".into(),
                difficulty: 4,
                doctrine: Some("bounding_overwatch".into()),
            },
            round_seed,
            registry,
        ),
    );
    for side in 0..state.sides.len() as u8 {
        if side == SEAT {
            continue;
        }
        if let Some(config) = state.sides[side as usize].ai.clone() {
            ai.insert(
                side,
                make_battle_planner(&config, round_seed + 1 + u64::from(side), registry),
            );
        }
    }
    let mut events = Vec::new();
    ai.plan_round_with(registry, state, |d| events.extend(d.events.iter().cloned()));
    let intentions = log::intentions(state, &events);
    Planned { notes, intentions }
}

/// The planning preview: each of her crews, the ground her orders name,
/// what she means to do about it, and whether that keeps the order. Only
/// side 0's: the enemy's plans are not hers to see.
fn preview(registry: &DataRegistry, state: &BattleState, planned: &Planned) {
    println!(
        "== round {}: your crews' plans (not yet committed)",
        state.round
    );
    for note in &planned.notes {
        println!("   !! {note}");
    }
    // A mission travels: it reaches the crews some ticks into the round,
    // after they have planned it. Said here, or the plans below read as
    // crews ignoring an order they have not yet heard.
    for f in state.formations().iter().filter(|f| f.side == SEAT) {
        if let Some((change, ticks)) = &f.incoming {
            let what = change
                .mission()
                .map(mission_text)
                .unwrap_or_else(|| "release".into());
            let then = if registry
                .command
                .as_ref()
                .is_some_and(|c| c.replan_on_arrival)
            {
                "the plans below are until then, and they re-plan for it when it lands"
            } else {
                "so this round they plan on what they had"
            };
            println!(
                "   !! {}'s new orders ({what}) are on the radio: they reach her crews {} s into the round; {then}",
                f.id,
                ticks * 5
            );
        }
    }
    for i in planned.intentions.iter().filter(|i| i.side == SEAT) {
        let Some(u) = state.lookup(UnitId(i.unit)) else {
            continue;
        };
        let order = match i.ordered {
            Some(g) => format!("{} ({},{})", i.order, g[0], g[1]),
            None => "no order".into(),
        };
        let verdict = match log::keeps_order(i) {
            Some(true) => "keeps it",
            Some(false) => "DEVIATES",
            None => "",
        };
        let route = if i.intends == i.from {
            "stays".to_string()
        } else {
            format!(
                "to ({},{}), {} hexes",
                i.intends[0],
                i.intends[1],
                log::distance(i.from, i.intends)
            )
        };
        let fire = match u.intent.fire {
            tactics_core::battle::FireIntent::Target { target, .. } => {
                let end = u.intent.path.last().copied().unwrap_or(u.pos);
                format!(
                    ", firing at {} ({})",
                    name_of(state, target),
                    fire_odds(registry, state, u.id, target, u.pos, end)
                )
            }
            _ => String::new(),
        };
        let doing = if i.doing.is_empty() {
            String::new()
        } else {
            format!(" \"{}\"", i.doing)
        };
        println!(
            "   {:<14} at ({},{}) under {order}: {route}{fire}{doing}  {verdict}",
            u.name, i.from[0], i.from[1]
        );
        // Who rides with her, and whether they mean to get out.
        for p in state
            .units
            .iter()
            .filter(|p| p.aboard == Some(u.id) && p.fate.alive())
        {
            println!(
                "      {} aboard{}",
                p.name,
                if p.dismounting {
                    ": GETS OUT at the start of the round, beside the carrier where it stands now, and walks"
                } else {
                    ", staying aboard"
                }
            );
        }
    }
}

/// The shot forecast: every shot either side could take this instant, from
/// where each crew stands, with the arithmetic behind it. Two halves: yours
/// at what you know of, and theirs at you from where you know them to be.
///
/// It is `preview_attack`, the game's own hover panel, asked for every pair
/// that has a weapon in range and a line between them (`best_weapon_from`
/// picks the gun the crew would pick). "Hurts" is hit times through: the
/// chance one shot does anything structural. A miss and a bounce still
/// frighten, which this does not count.
///
/// **The second half is not fog-honest.** `preview_attack` reads the
/// shooter as she is: her crew's gunnery, her nerve (a pinned or breaking
/// rung is a named modifier) and her racks. For your own crews that is what
/// you know; for theirs it is what the post-mortem knows, and the printout
/// says so. The game's hover panel has the same leak in its return-fire
/// line, which matters more there (SLICE.md, playtest 3).
fn forecast(
    registry: &DataRegistry,
    state: &BattleState,
    mine: &[&tactics_core::battle::Unit],
    known: &[&tactics_core::battle::Unit],
) {
    let standing: Vec<_> = mine.iter().filter(|u| u.aboard.is_none()).collect();
    // The gun she would pick; failing that, any direct-fire gun with a line
    // and the range, which is a shot that cannot hurt (the plate is too
    // thick for anything she carries). That second case is listed rather
    // than left out: "no shot" and "a shot that does nothing" are different
    // things to know about the tank beside you.
    let shot = |from: &tactics_core::battle::Unit, to: &tactics_core::battle::Unit| {
        let weapon = match tactics_core::battle::best_weapon_from(
            registry, state, from.id, from.pos, to.id, to.pos,
        ) {
            Some((i, _)) => i,
            None => {
                let dist = from.pos.distance_to(to.pos);
                let vehicle = registry.vehicle(&from.vehicle)?;
                vehicle.weapons.iter().position(|w| {
                    registry.weapon(w).is_some_and(|w| {
                        !w.indirect
                            && (w.range[0] as i32..=w.range[1] as i32).contains(&dist)
                            && state.sees_through(from.pos, to.pos)
                    })
                })?
            }
        };
        tactics_core::battle::preview_attack(registry, state, from.id, weapon, to.id, false)
    };
    let line = |p: &tactics_core::battle::AttackPreview, shooter: &str, target: &str| {
        let mods: Vec<String> = std::iter::once(format!("base {}", p.hit.base))
            .chain(
                p.hit
                    .modifiers
                    .iter()
                    .map(|m| format!("{:+} {}", m.delta, m.label)),
            )
            .collect();
        let hurts = p.hit.total * p.pen_chance / 100;
        let through = match &p.ammo {
            Some(round) => format!(
                "{round} {}% through {:?} ({})",
                p.pen_chance, p.facing, p.effective_armor
            ),
            None => "NO AMMUNITION".into(),
        };
        format!(
            "  {shooter:12} -> {target:14} {} hex  {:10} hit {:>2}%{}  {through}  => hurts {hurts}%{}\n      [{}]",
            p.distance,
            p.weapon_name,
            p.hit.total,
            if p.hit.clamped { " (capped)" } else { "" },
            if hurts == 0 {
                ": CAN'T HURT HER"
            } else if p.lethal {
                ", could finish her"
            } else {
                ""
            },
            mods.join(", ")
        )
    };
    println!("\n== shot forecast, from where everyone stands now");
    let mut any = false;
    for u in &standing {
        for e in known {
            if let Some(p) = shot(u, e) {
                println!("{}", line(&p, &u.name, &e.name));
                any = true;
            }
        }
    }
    if !any {
        println!("   you have no shot at anything you know of");
    }
    println!(
        "   -- and at you (their crews' real gunnery, nerve and racks, which you would not know):"
    );
    let mut any = false;
    for e in known {
        for u in &standing {
            if let Some(p) = shot(e, u) {
                println!("{}", line(&p, &e.name, &u.name));
                any = true;
            }
        }
    }
    if !any {
        println!("   nothing you know of has a shot at you");
    }
}

/// The odds on an ordered shot, from where she stands and from where her
/// route ends: the gun she would pick and its hit chance, or that she has
/// no line. A fire order with no line is not refused (that would tell the
/// commander something about the ground she may not have read), but she
/// cannot carry it out until one opens, and the plan must say so or it
/// reads as a shot that will be taken.
fn fire_odds(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    target: UnitId,
    from: Hex,
    end: Hex,
) -> String {
    let Some(at) = state.unit(target).map(|t| t.pos) else {
        return "gone".into();
    };
    let odds = |from: Hex| {
        let vehicle = registry.vehicle(&state.unit(unit)?.vehicle)?;
        // As in the forecast: the gun she would pick, else a gun that has
        // the line and the range but cannot hurt what it is pointed at.
        let (index, harmless) =
            match tactics_core::battle::best_weapon_from(registry, state, unit, from, target, at) {
                Some((i, _)) => (i, false),
                None => {
                    let dist = from.distance_to(at);
                    let i = vehicle.weapons.iter().position(|w| {
                        registry.weapon(w).is_some_and(|w| {
                            !w.indirect
                                && (w.range[0] as i32..=w.range[1] as i32).contains(&dist)
                                && state.sees_through(from, at)
                        })
                    })?;
                    (i, true)
                }
            };
        let weapon = registry.weapon(vehicle.weapons.get(index)?)?;
        let hit = tactics_core::battle::hit_breakdown(
            registry, state, unit, from, weapon, target, at, false,
        );
        Some(format!(
            "{} hit {}% at {} hex{}",
            weapon.name,
            hit.total,
            from.distance_to(at),
            if harmless { ", but CAN'T HURT HER" } else { "" }
        ))
    };
    let here = odds(from);
    let there = (end != from).then(|| odds(end));
    match (here, there) {
        (Some(h), None) => h,
        (Some(h), Some(Some(t))) => format!("now {h}; from {} {t}", fmt(end)),
        (Some(h), Some(None)) => format!("now {h}; NO LINE from {}", fmt(end)),
        (None, Some(Some(t))) => format!("no line now; from {} {t}", fmt(end)),
        (None, _) => "NO LINE: she holds it until one opens".into(),
    }
}

/// One round: plan, show the plans, then fight it and log it.
fn turn(registry: &DataRegistry, state: &mut BattleState, orders: &Orders, dir: &Path) {
    if state.is_over() {
        println!("the battle is over");
        return;
    }
    let planned = plan(registry, state, orders, dir);
    preview(registry, state, &planned);
    let round = state.round;
    let events = state.resolve_round(registry);
    append(
        dir,
        &log::Round {
            round,
            events: events.clone(),
            units: log::placed(state),
            intentions: planned.intentions,
        },
    );
    println!(
        "== round {} as heard by your side",
        state.round.saturating_sub(1)
    );
    for ev in &events {
        if quiet(ev) || !ev.heard_by(state, SEAT) {
            continue;
        }
        println!("   {}", describe(state, ev));
    }
}

/// One line of `rounds.jsonl` (`heatmap/log.rs` is its shape).
fn append(dir: &Path, round: &log::Round) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("rounds.jsonl"))
    {
        writeln!(
            f,
            "{}",
            serde_json::to_string(round).expect("a round serialises")
        )
        .ok();
    }
}

fn name_of(state: &BattleState, id: UnitId) -> String {
    state
        .lookup(id)
        .map(|u| {
            if u.side == SEAT {
                u.name.clone()
            } else {
                format!("enemy {}", u.name)
            }
        })
        .unwrap_or_else(|| format!("unit {}", id.0))
}

/// An order's debug text, for `describe` to put names into.
fn order_text(order: &Order) -> String {
    format!("{order:?}")
}

/// The commander's orders file, turned into engine orders.
fn commander_orders(registry: &DataRegistry, state: &BattleState, orders: &Orders) -> Vec<Order> {
    let mut out = Vec::new();
    let formation = |id: &str| {
        state
            .formations()
            .iter()
            .position(|f| f.id == id && f.side == SEAT)
            .map(|i| tactics_core::battle::FormationId(i as u32))
            .unwrap_or_else(|| panic!("no formation {id} on your side"))
    };
    let unit = |name: &str| -> UnitId {
        state
            .units
            .iter()
            .find(|u| u.side == SEAT && u.name == name)
            .map(|u| u.id)
            .unwrap_or_else(|| panic!("no unit called {name} on your side"))
    };
    let hex = |at: [i32; 2]| offset_to_hex(at[0], at[1]);
    for m in &orders.missions {
        let to = m.to.map(hex);
        let need = || to.unwrap_or_else(|| panic!("{} needs a `to`", m.mission));
        let mission = match m.mission.as_str() {
            "advance" => Mission::Advance { to: need() },
            "assault" => Mission::Assault { to: need() },
            "hold" => Mission::Hold { at: to },
            "recon" => Mission::Recon { toward: need() },
            "support" => Mission::Support {
                formation: m.of.clone().expect("support needs `of`"),
            },
            other => panic!("unknown mission {other}"),
        };
        out.push(Order::SetMission {
            formation: formation(&m.formation),
            mission,
            latitude: Latitude::default(),
        });
    }
    for f in &orders.release {
        out.push(Order::ReleaseMission {
            formation: formation(f),
        });
    }
    for name in &orders.dismount {
        out.push(Order::Dismount { unit: unit(name) });
    }
    for m in &orders.mount {
        out.push(Order::Mount {
            unit: unit(&m.unit),
            into: unit(&m.into),
        });
    }
    for name in &orders.stay {
        let u = unit(name);
        let here = state.lookup(u).map(|u| u.pos).expect("a unit has a place");
        out.push(Order::Radio {
            unit: u,
            to: Some(here),
            fire: None,
            latitude: Latitude::Binding,
        });
    }
    for name in &orders.stay_aboard {
        out.push(Order::SetFire {
            unit: unit(name),
            fire: tactics_core::battle::FireIntent::Hold,
        });
    }
    for f in &orders.fire {
        let shooter = unit(&f.unit);
        let target = state
            .known_enemies(registry, Knower::Commander(SEAT))
            .iter()
            .find(|u| u.name == f.at)
            .map(|u| u.id)
            .unwrap_or_else(|| panic!("no enemy called {} in your picture", f.at));
        let weapons = state
            .lookup(shooter)
            .and_then(|u| registry.vehicle(&u.vehicle))
            .map(|v| v.weapons.clone())
            .unwrap_or_default();
        let weapon = match &f.weapon {
            Some(w) => weapons
                .iter()
                .position(|x| x == w)
                .unwrap_or_else(|| panic!("{} carries no {w}", f.unit)),
            None => 0,
        };
        out.push(Order::Radio {
            unit: shooter,
            to: None,
            fire: Some(tactics_core::battle::FireIntent::Target { target, weapon }),
            latitude: Latitude::default(),
        });
    }
    for r in &orders.radio {
        out.push(Order::Radio {
            unit: unit(&r.unit),
            to: Some(hex(r.to)),
            fire: None,
            latitude: if r.binding {
                Latitude::Binding
            } else {
                Latitude::default()
            },
        });
    }
    out
}

/// Events too fine-grained to read a round by: the sitrep shows where
/// everybody ended up.
fn quiet(ev: &Event) -> bool {
    matches!(
        ev,
        Event::TickStarted { .. }
            | Event::RoundStarted { .. }
            | Event::UnitMoved { .. }
            | Event::Pivoted { .. }
            | Event::Traversed { .. }
    )
}

/// An event in words a player can follow: its debug text with every unit
/// id replaced by her name and every hex by its map coordinate.
fn describe(state: &BattleState, ev: &Event) -> String {
    describe_text(state, format!("{ev:?}"))
}

/// Debug text with names and map coordinates put in.
fn describe_text(state: &BattleState, mut text: String) -> String {
    text = replace_ids(&text, "UnitId(", |n| {
        state
            .lookup(UnitId(n))
            .map(|u| {
                let side = if u.side == SEAT { "" } else { "enemy " };
                format!("{side}{}", u.name)
            })
            .unwrap_or_else(|| format!("unit {n}"))
    });
    text = replace_ids(&text, "CadetId(", |n| {
        state
            .roster
            .get(tactics_core::roster::CadetId(n))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| format!("cadet {n}"))
    });
    replace_hexes(&text)
}

fn replace_ids(text: &str, prefix: &str, name: impl Fn(u32) -> String) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(prefix) {
        out.push_str(&rest[..i]);
        let after = &rest[i + prefix.len()..];
        let Some(close) = after.find(')') else {
            out.push_str(&rest[i..]);
            return out;
        };
        match after[..close].parse::<u32>() {
            Ok(n) => out.push_str(&name(n)),
            Err(_) => out.push_str(&rest[i..i + prefix.len() + close + 1]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

fn replace_hexes(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find("Hex { x: ") {
        out.push_str(&rest[..i]);
        let after = &rest[i..];
        let Some(close) = after.find('}') else {
            out.push_str(after);
            return out;
        };
        let nums: Vec<i32> = after[..close]
            .split(|c: char| !(c.is_ascii_digit() || c == '-'))
            .filter_map(|s| s.parse().ok())
            .collect();
        if nums.len() >= 2 {
            let [c, r] = hex_to_offset(Hex::new(nums[0], nums[1]));
            out.push_str(&format!("({c},{r})"));
        } else {
            out.push_str(&after[..=close]);
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// What the commander knows, as a page: the score, the ground, her own
/// people, and the enemy she has been told about.
fn sitrep(registry: &DataRegistry, state: &BattleState) {
    println!();
    println!(
        "== round {}  score {} (yours) to {}{}",
        state.round,
        state.score(SEAT),
        state.score(1 - SEAT),
        if state.is_over() {
            "  -- BATTLE OVER"
        } else {
            ""
        }
    );
    for (o, holder) in state.objectives() {
        let at: Vec<String> = o.hexes.iter().map(|h| fmt(*h)).collect();
        // A way off the map is ground nobody scores for holding.
        if o.kind == tactics_core::map::ObjectiveKind::Exit {
            let whose = if o.open_to(SEAT) {
                "your"
            } else {
                "the enemy's"
            };
            println!("   {} is {whose} way out: {}", o.name, at.join(" "));
            continue;
        }
        let who = match holder {
            Some(s) if s == SEAT => "yours",
            Some(_) => "THE ENEMY'S",
            None => "nobody's",
        };
        println!(
            "   {} ({} pts/round) is {who}: {}",
            o.name,
            o.value,
            at.join(" ")
        );
    }

    // What she has been told is coming: the window, never the roll.
    for a in state.arrivals.iter().filter(|a| a.def.side == SEAT) {
        let names: Vec<&str> = a
            .placements
            .iter()
            .map(|p| p.name.as_deref().unwrap_or("?"))
            .collect();
        println!(
            "   on the way: {} ({}), expected round {}{}",
            a.def.name,
            names.join(", "),
            a.expected,
            if a.spread > 0 {
                format!(" give or take {}", a.spread)
            } else {
                String::new()
            }
        );
    }
    let known: Vec<_> = state.known_enemies(registry, Knower::Commander(SEAT));
    let mine: Vec<_> = state
        .units
        .iter()
        .filter(|u| u.side == SEAT && u.fate.alive())
        .collect();

    // The board, with your units as letters and known enemies as capitals.
    let mut marks: std::collections::BTreeMap<[i32; 2], char> = Default::default();
    let mut legend = Vec::new();
    for (i, u) in mine.iter().enumerate() {
        if u.aboard.is_some() {
            continue;
        }
        let c = (b'a' + i as u8) as char;
        marks.insert(hex_to_offset(u.pos), c);
        legend.push((c, *u));
    }
    for (i, u) in known.iter().enumerate() {
        let c = (b'A' + i as u8) as char;
        marks.insert(hex_to_offset(u.pos), c);
    }
    let objective_hexes: std::collections::BTreeSet<[i32; 2]> = state
        .objectives()
        .flat_map(|(o, _)| o.hexes.iter().map(|h| hex_to_offset(*h)))
        .collect();
    let (mut w, mut h) = (0, 0);
    for (hex, _) in state.world.iter() {
        let [c, r] = hex_to_offset(hex);
        w = w.max(c + 1);
        h = h.max(r + 1);
    }
    print!("\n     ");
    for c in 0..w {
        print!(
            "{}",
            if c % 5 == 0 {
                (b'0' + (c / 10) as u8) as char
            } else {
                ' '
            }
        );
        print!(
            "{}",
            if c % 5 == 0 {
                (b'0' + (c % 10) as u8) as char
            } else {
                ' '
            }
        );
    }
    println!();
    for r in 0..h {
        print!("{r:3}  {}", if r & 1 == 1 { " " } else { "" });
        for c in 0..w {
            let glyph = match marks.get(&[c, r]) {
                Some(m) => *m,
                None => match state.world.get(offset_to_hex(c, r)) {
                    Some(t) if objective_hexes.contains(&[c, r]) && t.terrain != "road" => '*',
                    Some(t) => terrain_glyph(t.terrain),
                    None => ' ',
                },
            };
            print!("{glyph} ");
        }
        println!();
    }
    println!(
        "     . fields  , meadow  = road  # town  ^ wood  \" hedge  ~ brook  _ mud  * objective"
    );

    println!("\n== your units");
    for u in &mine {
        let mark = legend
            .iter()
            .find(|(_, v)| v.id == u.id)
            .map(|(c, _)| *c)
            .unwrap_or(' ');
        let crew: Vec<String> = u
            .crew
            .iter()
            .filter_map(|c| state.roster.get(*c).map(|c| c.name.clone()))
            .collect();
        let rung = &registry.morale.rung(u.pressure).name;
        let condition = (state.condition(registry, u) * 100.0).round();
        let place = match u.aboard.and_then(|c| state.lookup(c)) {
            Some(c) => format!("aboard {}", c.name),
            None => fmt(u.pos),
        };
        let mission = state
            .formations()
            .iter()
            .find(|f| f.members.contains(&u.id))
            .map(|f| match &f.mission {
                Some(m) => format!("{}: {}", f.id, mission_text(m)),
                None => format!("{}: no mission", f.id),
            })
            .unwrap_or_default();
        let ammo: Vec<String> = u.ammo.iter().map(|(k, v)| format!("{k} {v}")).collect();
        println!(
            "  {mark} {:12} {:14} {:10} {:>3}% {:8} [{}]  {}  ammo: {}",
            u.name,
            u.vehicle,
            place,
            condition,
            rung,
            crew.join(", "),
            mission,
            ammo.join(", ")
        );
    }

    println!("\n== enemy, as reported to you");
    if known.is_empty() {
        println!("   none in sight");
    }
    for (i, u) in known.iter().enumerate() {
        let c = (b'A' + i as u8) as char;
        println!("  {c} {} ({}) at {}", u.name, u.vehicle, fmt(u.pos));
    }
    // The commander's ghosts: contacts nobody is re-reporting, kept at the
    // last place they were seen. "We lost sight of it" is information.
    for contact in state.picture(SEAT) {
        if contact.fresh || known.iter().any(|u| u.id == contact.unit) {
            continue;
        }
        let what = state
            .lookup(contact.unit)
            .map(|u| format!("{} ({})", u.name, u.vehicle))
            .unwrap_or_else(|| "something".into());
        println!(
            "  ? {what}: lost sight of it at {}, round {}",
            fmt(contact.at),
            contact.round
        );
    }
    forecast(registry, state, &mine, &known);
}

fn mission_text(m: &Mission) -> String {
    match m {
        Mission::Advance { to } => format!("advance to {}", fmt(*to)),
        Mission::Assault { to } => format!("assault {}", fmt(*to)),
        Mission::Hold { at: Some(at) } => format!("hold {}", fmt(*at)),
        Mission::Hold { at: None } => "hold where you are".into(),
        Mission::Recon { toward } => format!("recon toward {}", fmt(*toward)),
        Mission::Withdraw { via } => format!("withdraw via {via}"),
        Mission::Support { formation } => format!("base of fire for {formation}"),
    }
}

fn fmt(h: Hex) -> String {
    let [c, r] = hex_to_offset(h);
    format!("({c},{r})")
}

fn terrain_glyph(terrain: &str) -> char {
    match terrain {
        "plains" => '.',
        "grass" => ',',
        "road" => '=',
        "town" => '#',
        "forest" | "deep_forest" => '^',
        "hedgerow" => '"',
        "stream" | "water" => '~',
        "mud" => '_',
        _ => '?',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rejection(text: &str) -> String {
        parse_orders(text, Path::new("round3.json"))
            .expect_err("a typo'd orders file is refused")
            .to_string()
    }

    #[test]
    fn a_misspelled_order_is_refused_naming_the_field_and_the_file() {
        let err = rejection(r#"{ "dismout": ["2nd Squad"] }"#);
        assert!(err.contains("dismout"), "{err}");
        assert!(err.contains("round3.json"), "{err}");
    }

    #[test]
    fn a_misspelled_field_inside_an_order_is_refused_too() {
        for (text, field) in [
            (
                r#"{ "missions": [{"formation": "sec1", "mision": "hold"}] }"#,
                "mision",
            ),
            (
                r#"{ "radio": [{"unit": "1st Carrier", "to": [12, 6], "bindng": true}] }"#,
                "bindng",
            ),
            (
                r#"{ "mount": [{"unit": "3rd Squad", "in": "3rd Carrier"}] }"#,
                "`in`",
            ),
            (
                r#"{ "fire": [{"unit": "2nd Squad", "at": "Wiesel", "weapn": "rpg"}] }"#,
                "weapn",
            ),
        ] {
            let err = rejection(text);
            assert!(err.contains(field), "{text}: {err}");
            assert!(err.contains("round3.json"), "{err}");
        }
    }

    #[test]
    fn the_documented_orders_file_still_reads() {
        let orders = parse_orders(
            r#"{ "missions": [ {"formation": "sec1", "mission": "advance", "to": [13, 9]},
                               {"formation": "sec2", "mission": "hold"},
                               {"formation": "sec3", "mission": "support", "of": "sec1"} ],
                 "radio":    [ {"unit": "1st Carrier", "to": [12, 6], "binding": true} ],
                 "dismount": [ "2nd Squad" ],
                 "mount":    [ {"unit": "3rd Squad", "into": "3rd Carrier"} ],
                 "release":  [ "sec3" ],
                 "stay":     [ "3rd Carrier" ],
                 "stay_aboard": [ "1st Squad" ],
                 "fire":     [ {"unit": "2nd Squad", "at": "Wiesel", "weapon": "rpg"} ] }"#,
            Path::new("orders.json"),
        )
        .expect("every documented field is known");
        assert_eq!(orders.missions.len(), 3);
        assert_eq!(orders.dismount, ["2nd Squad"]);
        assert!(parse_orders("{}", Path::new("empty.json")).is_ok());
    }
}
