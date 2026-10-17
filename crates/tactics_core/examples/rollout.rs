//! Rollouts for reinforcement learning (REIMAGINED.md L1.3).
//!
//! A learner — the utility executor with a network choosing goals by
//! *sampling* its softmax (`GoalBrain::Explore`) — fights the utility AI on a
//! mirrored arena, seats alternating by game so neither end's lean is
//! learned. Every learner decision is recorded with its log-probability, and
//! every round a ledger of every crew's condition and both sides' scores, so
//! the trainer can compute whatever reward it likes after the fact without
//! the engine having an opinion about rewards at all.
//!
//! ```sh
//! cargo run --release -p tactics_core --example rollout -- \
//!     --net target/goal_net.json --games 512 --out target/rl/iter0.jsonl
//! ```
//!
//! Inference runs here, in Rust, on every core: the network is a few
//! microseconds a decision, so there is nothing to gain by sending decisions
//! to a GPU one at a time, and the trainer only ever sees finished battles.

use std::io::Write;
use std::sync::{Arc, Mutex};
use tactics_core::ai::commander::load_command_net;
use tactics_core::ai::features::{GoalRecord, GoalTap};
use tactics_core::ai::learned::{GoalBrain, load_goal_net};
use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, CommandBrain, SideCommand, UtilityPlanner};
use tactics_core::battle::{BattleState, Event, Order};
use tactics_core::data::ArmorFacing;
use tactics_core::data::DataRegistry;
use tactics_core::force::{Purchase, buy};
use tactics_core::harness::arena::{Arena, DEFAULT_ARENA, arena_named};
use tactics_core::harness::open::{self, OpenLayout, Worlds};
use tactics_core::harness::parallel::run_all;
use tactics_core::harness::sector::{self, SectorRules, SectorSize};
use tactics_core::harness::{forces, generated};

const USAGE: &str = "\
rollout: battles for reinforcement learning, learner against the utility AI

  --net PATH          the learner's goal network (required); `utility` makes
                      the learner the utility AI itself, the control a
                      network's per-force record is read against
  --games N           battles (default 256)
  --seed S            first battle (default 0)
  --temperature T     the learner's sampling temperature (default 1.0)
  --opponents A,B     league networks the learner also fights, in turn with
                      the utility AI (two games each, so both seatings)
  --events LIST       planner.goal_events: contact,hit,friend_lost,pinned,order, or all
  --places N          planner.goal_places (default 16)
  --approach N        planner.approach_places: places an enemy, round his arcs (default 0)
  --arena NAME        battlefield (default ridge_arena); with --ground generated,
                      whose forces and deployment are moved onto generated ground
  --ground generated  a fresh generated world every two games (one per seat)
  --ground open       worlds several battle maps across, fought on a window onto
                      them with the forces 3-6 km apart at a bearing drawn every
                      two games, sixteen such fields a world (needs --forces)
  --ground sector     an attack on a defended sector (harness::sector): side 0
                      attacks zones side 1 holds, dug in, mined and wired on
                      three fields in four; both buy to the sector's budget
  --counts C,U        with --ground sector, every side counts a cut-off crew as
                      C percent lost and an unnerved one as U toward its break
  --sizes LIST        with --ground sector, the sizes drawn per field: small,
                      medium, large (default all three)
  --arrays            write the decisions' numbers as flat f32 files beside the
                      battles (OUT.context.f32, OUT.candidates.f32, OUT.counts.u32,
                      OUT.arrays.json) and leave them out of the JSON
  --spread PERCENT    with --ground generated, the deployment's distance from
                      the centre in percent of the arena's (default 100)
  --turned            with --ground generated, turn each world's axis of
                      advance by a sixth of a circle a step, drawn per world
  --forces LIST|all   draw each world's matchup from these forces (forces/ in
                      the mod), each side fighting by its force's doctrine
  --forces bought     each world, both sides buy a force from the platoon
                      catalogue (platoons/ in the mod) to the same budget
  --budget LO,HI      with --forces bought, the budget drawn per world (default 70,100)
  --commander KIND    who gives the learner's platoons their missions: none
                      (default; every crew chooses for herself), utility (the
                      hand-written commander), random (a coin, per review),
                      learned (the network --commander-net names, sampling at
                      --commander-temperature unless --commander-deterministic) or
                      uniform (a coin over the learned commander's menu,
                      recorded as a learned one is), or orders (none, utility,
                      random and uniform in turn by world: crews learning to
                      fight under every kind of order, L2.1)
  --commander-net PATH  the learned commander's network
  --commander-deterministic   the learned commander takes her best choice
  --commander-temperature T   its sampling temperature (default 1.0)
  --opponent-commander KIND   the same for the opponent, or mix: none, utility
                      and random in turn by world
  --set PATH=VALUE    any mod field by its json path, as `balance --set` takes it
  --deterministic     the learner takes its best choice instead of sampling
                      (evaluation; no log-probabilities are recorded)
  --difficulty D      the opponent's difficulty (default 5)
  --out PATH          JSON lines, one battle each (default target/rl/rollouts.jsonl)";

/// A battle no side has finished by now is a stalemate; the harness's cap.
const MAX_ROUNDS: u32 = 60;

/// The cap for an assault, whose approach alone can take a rifle platoon
/// thirty rounds: a cap is the harness's, not a rule of the battle, and an
/// assault is judged when it stops (ASSAULT.md), so every battle cut off
/// here is counted and reported.
const ASSAULT_ROUNDS: u32 = 120;

#[derive(serde::Serialize)]
struct Entry {
    round: u32,
    unit: u32,
    side: u8,
    left: u32,
    full: u32,
    fighting: bool,
    /// The ground her orders name, as `[x, y]`, if they name any: the
    /// trainer pays the `tactical` tier only while one order stands.
    #[serde(skip_serializing_if = "Option::is_none")]
    order: Option<[i32; 2]>,
    /// Whether her side holds the objective that ground lies in; `None`
    /// with no such objective.
    #[serde(skip_serializing_if = "Option::is_none")]
    order_held: Option<bool>,
}

#[derive(serde::Serialize)]
struct Battle {
    game: u64,
    learner: u8,
    winner: Option<u8>,
    rounds: u32,
    decisions: Vec<GoalRecord>,
    ledger: Vec<Entry>,
    /// `[round, side 0's score, side 1's score]`, round 0 being the start.
    scores: Vec<[u32; 3]>,
    /// The score that wins the battle outright, if the scenario names one:
    /// what a point of objective margin is a share of.
    victory_score: Option<u32>,
    /// Who the learner fought: `utility`, or a league network's file name.
    opponent: String,
    /// The two forces, side 0's first, when drawn from `--forces`.
    forces: Option<[String; 2]>,
    /// Strikes on armour — hits and bounces — by the face they struck,
    /// `[front, side, rear]`: the learner's on the enemy, and the enemy's on
    /// the learner. The instrument for whether she learned to get round, a
    /// label read after the fact and never shown to the network.
    struck_by_learner: [u32; 3],
    struck_on_learner: [u32; 3],
    /// Fix-and-flank, read after the fact, `[learner, opponent]`: a target
    /// struck on his front by one crew and then on his side or rear by
    /// another of the same side within three rounds. A label for the
    /// measurer, never shown to the network.
    fix_and_flank: [u32; 2],
    /// Shots aimed at somebody, `[learner, opponent]`, and of those the ones
    /// fired from beyond the target's own longest direct reach: standoff.
    aimed: [u32; 2],
    standoff: [u32; 2],
    /// Missions given, `[learner's side, opponent's]`: zero for a side
    /// nobody commands.
    missions: [u32; 2],
    /// The learner's commander's decisions, when a learned or uniform
    /// commander gave them (`--commander learned|uniform`).
    commands: Vec<GoalRecord>,
    /// Who commanded the opponent.
    opponent_commander: &'static str,
    /// Why it ended (`EndReason`), or `cut off` at the harness's cap.
    reason: String,
    /// With `--ground sector`: its size, whether the defender prepared,
    /// and how many zones the attacker held at the end, of how many.
    sector: Option<SectorRecord>,
}

#[derive(serde::Serialize, Clone, Copy)]
struct SectorRecord {
    size: &'static str,
    prepared: bool,
    held: u32,
    zones: u32,
    needs: u32,
}

/// A unit's longest direct weapon reach, in hexes: what she could answer
/// with.
fn reach(reg: &DataRegistry, state: &BattleState, unit: tactics_core::battle::UnitId) -> i32 {
    state
        .unit(unit)
        .and_then(|u| reg.vehicle(&u.vehicle))
        .map(|v| {
            v.weapons
                .iter()
                .filter_map(|w| reg.weapon(w))
                .filter(|w| !w.indirect)
                .map(|w| w.range[1] as i32)
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// One stream of decisions as flat little-endian files beside the battles
/// (`--arrays`), in the order the battles and their decisions are written,
/// the JSON keeping everything else: parsing 800 MB of numbers out of text
/// was most of what a training update spent before it learned anything.
struct Arrays {
    context: std::io::BufWriter<std::fs::File>,
    candidates: std::io::BufWriter<std::fs::File>,
    counts: std::io::BufWriter<std::fs::File>,
}

impl Arrays {
    fn open(out: &std::path::Path, stream: &str, context: Vec<String>, candidate: &[&str]) -> Self {
        let beside = |ext: &str| {
            let mut name = out.as_os_str().to_owned();
            name.push(stream);
            name.push(ext);
            std::path::PathBuf::from(name)
        };
        std::fs::write(
            beside(".arrays.json"),
            serde_json::json!({
                "context": context.len(),
                "candidate": candidate.len(),
                "context_names": context,
                "candidate_names": candidate,
            })
            .to_string(),
        )
        .expect("write the arrays' shape");
        let open = |ext: &str| {
            std::io::BufWriter::new(std::fs::File::create(beside(ext)).expect("open an array"))
        };
        Self {
            context: open(".context.f32"),
            candidates: open(".candidates.f32"),
            counts: open(".counts.u32"),
        }
    }

    /// Write each record's numbers and take them out of the record.
    fn write(&mut self, records: &mut [GoalRecord]) {
        for d in records {
            for v in &d.context {
                self.context
                    .write_all(&v.to_le_bytes())
                    .expect("write an array");
            }
            for row in &d.candidates {
                for v in row {
                    self.candidates
                        .write_all(&v.to_le_bytes())
                        .expect("write an array");
                }
            }
            self.counts
                .write_all(&(d.candidates.len() as u32).to_le_bytes())
                .expect("write an array");
            d.context = Vec::new();
            d.candidates = Vec::new();
        }
    }

    fn flush(&mut self) {
        for f in [&mut self.context, &mut self.candidates, &mut self.counts] {
            f.flush().expect("flush an array");
        }
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn ledger(reg: &DataRegistry, state: &BattleState, round: u32, out: &mut Vec<Entry>) {
    for unit in &state.units {
        let (left, full) = state.substance(reg, unit);
        let order = match tactics_core::ai::goal::ordered_goal(state, unit.id) {
            Some(tactics_core::battle::Goal::Take(hex)) => Some(hex),
            _ => None,
        };
        let order_held = order.and_then(|hex| {
            state
                .objectives()
                .find(|(o, _)| o.hexes.contains(&hex))
                .map(|(_, held)| held == Some(unit.side))
        });
        out.push(Entry {
            round,
            unit: unit.id.0,
            side: unit.side,
            left: if unit.alive() { left } else { 0 },
            full,
            fighting: unit.alive(),
            order: order.map(|h| [h.x, h.y]),
            order_held,
        });
    }
}

struct Setup<'a> {
    /// `--counts C,U`: what a sector's break points count a cut-off and an
    /// unnerved crew as, in percent lost (`SectorRules`).
    counts: [u32; 2],
    reg: &'a DataRegistry,
    arena: &'static tactics_core::harness::arena::Arena,
    brain: GoalBrain,
    difficulty: u8,
    /// The league: past networks the learner also fights, each taking its
    /// turn with the utility AI by game number (`--opponents`). A learner
    /// that only ever meets one opponent learns that opponent.
    opponents: Vec<(String, std::sync::Arc<tactics_core::data::GoalNet>)>,
    /// Fight on a world the generator makes from the game's seed
    /// (`harness::generated`) instead of on the arena's own ground: two games
    /// a world, so the learner has each seat on it.
    generated: bool,
    /// How the deployment is laid on generated ground (`--spread`, `--turned`).
    layout: generated::Layout,
    /// Battles on a window onto a whole world, the forces kilometres apart
    /// (`--ground open`, `harness::open`).
    open: bool,
    /// The worlds those battles are fought on, each made once.
    worlds: Worlds,
    /// The forces battles are drawn from (`--forces`): each world gets a
    /// matchup, both seatings, each side fighting by its force's doctrine.
    /// Empty is the arena's own symmetric force.
    forces: Vec<&'a tactics_core::data::ForceDef>,
    /// `--forces bought`: the budget range both sides buy to, per world.
    bought: Option<(i32, i32)>,
    /// Who commands the learner's side and the opponent's (`--commander`,
    /// `--opponent-commander`).
    commanders: [Commander; 2],
    /// The learner's commander, when it is a network or the uniform coin.
    command_brain: CommandBrain,
    /// `--ground sector`: the sizes drawn from, per field.
    sector: Option<Vec<SectorSize>>,
}

/// The size of sector field `field`, drawn from `sizes`.
fn sector_size(sizes: &[SectorSize], field: u64) -> SectorSize {
    let pick = (field.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 33) as usize % sizes.len().max(1);
    sizes[pick]
}

/// Who gives a side's platoons their missions.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Commander {
    /// Nobody: every crew chooses for herself among the battle's objectives.
    None,
    /// The hand-written commander (`SideCommand`).
    Utility,
    /// A coin at every review (`SideCommand::with_random_missions`).
    Random,
    /// A network, or the coin over its menu (`Setup::command_brain`).
    Learned,
    /// For the opponent: none, utility and random in turn by world.
    Mix,
    /// For the learner's crews to learn to fight under orders (L2.1): none,
    /// utility, random and a coin over the learned commander's menu in turn
    /// by world, so they meet every kind of order and none.
    Orders,
}

impl Commander {
    fn parse(flag: Option<&str>) -> Self {
        match flag {
            None | Some("none") => Self::None,
            Some("utility") => Self::Utility,
            Some("random") => Self::Random,
            Some("learned") | Some("uniform") => Self::Learned,
            Some("mix") => Self::Mix,
            Some("orders") => Self::Orders,
            Some(other) => {
                eprintln!(
                    "error: a commander is none, utility, random, learned, uniform, mix or orders, not `{other}`"
                );
                std::process::exit(1);
            }
        }
    }
}

/// Who the learner fights in `game`: the utility AI, or one of the league's
/// networks, in turn.
fn opponent<'a>(
    setup: &'a Setup,
    game: u64,
) -> Option<&'a (String, std::sync::Arc<tactics_core::data::GoalNet>)> {
    let turn = (game / 2) as usize % (setup.opponents.len() + 1);
    turn.checked_sub(1).and_then(|i| setup.opponents.get(i))
}

/// The two forces that meet on world `game / 2`, side 0's first: drawn from
/// the list by the world's seed, or bought to one budget drawn from it, the
/// same for both games on it, so the learner fights each matchup from both
/// ends.
fn matchup(setup: &Setup, game: u64) -> Option<[tactics_core::data::ForceDef; 2]> {
    // SplitMix64, written out: the draw must not move with the platform.
    let mix = |mut z: u64| {
        z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    let world = game / 2;
    if let Some(sizes) = &setup.sector {
        let size = sector_size(sizes, world);
        let purchase = Purchase {
            points: sector::budget(size, world),
            max_vehicles: size.shape().max_vehicles,
        };
        let a = buy(setup.reg, purchase, mix(world))?;
        let b = buy(setup.reg, purchase, mix(world ^ 0xA5A5_A5A5))?;
        return Some([a, b]);
    }
    if let Some((lo, hi)) = setup.bought {
        let points = lo + (mix(world ^ 0xB06E7) % (hi - lo + 1).max(1) as u64) as i32;
        let purchase = Purchase {
            points,
            max_vehicles: 16,
        };
        let a = buy(setup.reg, purchase, mix(world))?;
        let b = buy(setup.reg, purchase, mix(world ^ 0xA5A5_A5A5))?;
        return Some([a, b]);
    }
    let n = setup.forces.len() as u64;
    if n == 0 {
        return None;
    }
    let a = (mix(world) % n) as usize;
    let b = (mix(world ^ 0xA5A5_A5A5) % n) as usize;
    Some([setup.forces[a].clone(), setup.forces[b].clone()])
}

fn fight(setup: &Setup, game: u64) -> Option<Battle> {
    let reg = setup.reg;
    let matchup = matchup(setup, game);
    let mut placed = None;
    let mut state = if let Some(sizes) = &setup.sector {
        let pair = matchup.as_ref()?;
        let size = sector_size(sizes, game / 2);
        let made = sector::sector(
            reg,
            &setup.worlds,
            game / 2,
            size,
            SectorRules {
                cut_off_counts: setup.counts[0],
                unnerved_counts: setup.counts[1],
                ..SectorRules::default()
            },
            [&pair[0], &pair[1]],
        )
        .ok()?;
        placed = Some((size, made.prepared, made.zones.len() as u32));
        sector::stage(reg, &made, [&pair[0], &pair[1]], 9000 + game)?
    } else if setup.open {
        // Open ground needs forces: the arena's own four tanks a side are a
        // battle-map fixture.
        let pair = matchup.as_ref()?;
        let made = open::field(
            reg,
            setup.arena,
            &setup.worlds,
            game / 2,
            OpenLayout::default(),
        )?;
        forces::stage_on_world(
            reg,
            made.world,
            made.scenario,
            &made.deployment,
            [&pair[0], &pair[1]],
            9000 + game,
        )?
    } else {
        let (field, deployment) = if setup.generated {
            let made = generated::field_laid_out(reg, setup.arena, game / 2, setup.layout)?;
            (made.field, made.deployment)
        } else {
            (setup.arena.map()?, setup.arena.deployment())
        };
        match &matchup {
            Some(pair) => {
                forces::stage(reg, field, &deployment, [&pair[0], &pair[1]], 9000 + game)?
            }
            None => Arena::stage_on(reg, field, deployment, 9000 + game),
        }
    };
    let learner = (game % 2) as u8;
    let tap: GoalTap = Arc::new(Mutex::new(Vec::new()));
    let command_tap: GoalTap = Arc::new(Mutex::new(Vec::new()));
    let opponent_commander = match setup.commanders[1] {
        Commander::None => "none",
        Commander::Utility => "utility",
        Commander::Random => "random",
        Commander::Learned => "learned",
        Commander::Mix => ["none", "utility", "random"][(game / 2 % 3) as usize],
        Commander::Orders => ["none", "utility", "random", "learned"][(game / 2 % 4) as usize],
    };
    let mut ai = AiDriver::new();
    for side in 0..2u8 {
        // Each side fights by its own force's doctrine: the doctrine comes
        // from what it has.
        let config = AiConfig {
            planner: "utility".into(),
            difficulty: setup.difficulty,
            doctrine: matchup
                .as_ref()
                .map(|pair| pair[side as usize].doctrine.clone()),
        };
        let seed = game.wrapping_mul(2).wrapping_add(side as u64);
        let commander = match setup.commanders[usize::from(side != learner)] {
            Commander::Mix => {
                [Commander::None, Commander::Utility, Commander::Random][(game / 2 % 3) as usize]
            }
            Commander::Orders => [
                Commander::None,
                Commander::Utility,
                Commander::Random,
                Commander::Learned,
            ][(game / 2 % 4) as usize],
            other => other,
        };
        let brain = if side == learner {
            Some(setup.brain.clone())
        } else {
            opponent(setup, game).map(|(_, net)| GoalBrain::Learned(net.clone()))
        };
        let tapped = (side == learner).then(|| tap.clone());
        let planner: Box<dyn AiPlanner<BattleState, Order>> = if commander == Commander::None {
            let mut planner = UtilityPlanner::from_config(&config, seed, reg);
            if let Some(brain) = brain {
                planner = planner.with_brain(brain);
            }
            if let Some(tap) = tapped {
                planner = planner.tapped(tap);
            }
            Box::new(planner)
        } else {
            let mut planner = SideCommand::from_config(&config, seed, reg);
            if commander == Commander::Random {
                planner = planner.with_random_missions();
            }
            if commander == Commander::Learned {
                planner = planner
                    .with_command_brain(setup.command_brain.clone())
                    .command_tapped(command_tap.clone());
            }
            if let Some(brain) = brain {
                planner = planner.with_brain(brain);
            }
            if let Some(tap) = tapped {
                planner = planner.tapped(tap);
            }
            Box::new(planner)
        };
        ai.insert(side, planner);
    }
    let mut entries = Vec::new();
    let mut scores = vec![[0, state.score(0), state.score(1)]];
    ledger(reg, &state, 0, &mut entries);
    let mut rounds = 0;
    let (mut by, mut on) = ([0u32; 3], [0u32; 3]);
    let (mut f4, mut aimed, mut standoff) = ([0u32; 2], [0u32; 2], [0u32; 2]);
    let mut missions = [0u32; 2];
    // Per target: the last round a crew of each side struck his front, and
    // which crew.
    let mut fronted: std::collections::BTreeMap<(u32, u8), (u32, u32)> = Default::default();
    let cap = if placed.is_some() {
        ASSAULT_ROUNDS
    } else {
        MAX_ROUNDS
    };
    while !state.is_over() && rounds < cap {
        ai.plan_round_with(reg, &mut state, |decision| {
            if matches!(decision.order, Order::SetMission { .. }) && decision.rejected.is_none() {
                missions[usize::from(decision.side != learner)] += 1;
            }
        });
        for event in state.resolve_round(reg) {
            // Who was aimed at whom, for the standoff count.
            let aimed_at = match &event {
                Event::ShotHit {
                    attacker, target, ..
                }
                | Event::ShotBounced {
                    attacker, target, ..
                } => Some((*attacker, *target)),
                Event::ShotMissed {
                    attacker,
                    target: Some(target),
                    ..
                } => Some((*attacker, *target)),
                _ => None,
            };
            if let Some((a, t)) = aimed_at
                && let (Some(au), Some(tu)) = (state.lookup(a), state.lookup(t))
            {
                let who = (au.side != learner) as usize;
                aimed[who] += 1;
                if au.pos.distance_to(tu.pos) > reach(reg, &state, t) {
                    standoff[who] += 1;
                }
            }
            let (attacker, facing, target) = match event {
                Event::ShotHit {
                    attacker,
                    facing,
                    target,
                    ..
                }
                | Event::ShotBounced {
                    attacker,
                    facing,
                    target,
                    ..
                } => (attacker, facing, target),
                _ => continue,
            };
            if let Some(side) = state.lookup(attacker).map(|u| u.side) {
                let who = (side != learner) as usize;
                let key = (target.0, side);
                if facing == ArmorFacing::Front {
                    fronted.insert(key, (rounds, attacker.0));
                } else if fronted
                    .get(&key)
                    .is_some_and(|(r, fixer)| rounds - r <= 3 && *fixer != attacker.0)
                {
                    f4[who] += 1;
                    fronted.remove(&key);
                }
            }
            let face = match facing {
                ArmorFacing::Front => 0,
                ArmorFacing::Side => 1,
                ArmorFacing::Rear => 2,
            };
            match state.unit(attacker).map(|u| u.side) {
                Some(side) if side == learner => by[face] += 1,
                Some(_) => on[face] += 1,
                None => {}
            }
        }
        rounds += 1;
        ledger(reg, &state, rounds, &mut entries);
        scores.push([rounds, state.score(0), state.score(1)]);
    }
    drop(ai);
    let reason = state.over.map_or("cut off".to_string(), |r| {
        format!("{:?}", r.reason).to_lowercase()
    });
    let sector = placed.map(|(size, prepared, zones)| SectorRecord {
        size: size.name(),
        prepared,
        held: state
            .objectives()
            .filter(|(o, h)| o.kind == tactics_core::map::ObjectiveKind::Hold && *h == Some(0))
            .count() as u32,
        zones,
        needs: state.scenario.assault().map_or(0, |a| a.needs),
    });
    let decisions = tap.lock().map(|r| r.clone()).unwrap_or_default();
    let commands = command_tap.lock().map(|r| r.clone()).unwrap_or_default();
    Some(Battle {
        game,
        learner,
        winner: state.over.and_then(|r| r.winner),
        rounds,
        decisions,
        ledger: entries,
        scores,
        struck_by_learner: by,
        struck_on_learner: on,
        fix_and_flank: f4,
        aimed,
        standoff,
        missions,
        commands,
        opponent_commander,
        reason,
        sector,
        // An assault declares no score to win by; the trainer prices a point
        // of margin as a share of one, so it is forty rounds of holding
        // every zone: the shaping stays smaller than the verdict it leads to.
        victory_score: state
            .scenario
            .victory_score()
            .or_else(|| placed.map(|(_, _, zones)| zones * 40)),
        opponent: opponent(setup, game).map_or("utility".to_string(), |(name, _)| name.clone()),
        forces: matchup.map(|pair| [pair[0].id.clone(), pair[1].id.clone()]),
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") || flag(&args, "--net").is_none() {
        println!("{USAGE}");
        return;
    }
    let parse = |name: &str, default: f64| -> f64 {
        flag(&args, name).map_or(default, |s| s.parse().unwrap_or_else(|_| panic!("{name}")))
    };
    let games = parse("--games", 256.0) as u64;
    let first = parse("--seed", 0.0) as u64;
    let temperature = parse("--temperature", 1.0) as f32;
    let difficulty = parse("--difficulty", 5.0) as u8;
    let out = std::path::PathBuf::from(flag(&args, "--out").unwrap_or("target/rl/rollouts.jsonl"));
    let arena = match flag(&args, "--arena") {
        None => arena_named("ridge_arena").unwrap_or(DEFAULT_ARENA),
        Some(name) => arena_named(name).unwrap_or_else(|| panic!("--arena {name}: no such arena")),
    };

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (mut reg, _) = DataRegistry::load_dir(&root).expect("mods load");
    reg.planner.goal_places = parse("--places", 16.0) as u32;
    reg.planner.approach_places = parse("--approach", 0.0) as u32;
    if let Some(list) = flag(&args, "--events") {
        reg.planner.goal_events = tactics_core::data::GoalEvents::parse(list).unwrap_or_else(|e| {
            eprintln!("error: --events: {e}");
            std::process::exit(1);
        });
    }
    // Any field of any block by its json path, as `balance --set` takes it,
    // applied after the flags above so it has the last word.
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
    let path = flag(&args, "--net").expect("--net");
    // `utility` is the control: the teacher in the learner's seat, so a
    // force's record can be told apart from what the force is worth. A force
    // the teacher also loses with is the force; one only the network loses
    // with is the network.
    let brain = if path == "utility" {
        GoalBrain::Utility
    } else {
        let net = load_goal_net(std::path::Path::new(path)).unwrap_or_else(|err| {
            eprintln!("error: --net {path}: {err}");
            std::process::exit(1);
        });
        if args.iter().any(|a| a == "--deterministic") {
            GoalBrain::Learned(net)
        } else {
            GoalBrain::Explore { net, temperature }
        }
    };
    let setup = Setup {
        reg: &reg,
        arena,
        brain,
        difficulty,
        generated: flag(&args, "--ground") == Some("generated"),
        open: flag(&args, "--ground") == Some("open"),
        sector: (flag(&args, "--ground") == Some("sector")).then(|| {
            flag(&args, "--sizes").map_or(SectorSize::ALL.to_vec(), |list| {
                list.split(',')
                    .map(|n| {
                        SectorSize::named(n).unwrap_or_else(|| {
                            eprintln!("error: --sizes: no size `{n}`");
                            std::process::exit(1);
                        })
                    })
                    .collect()
            })
        }),
        worlds: Worlds::default(),
        layout: generated::Layout {
            spread_percent: flag(&args, "--spread").map_or(100, |s| {
                s.parse()
                    .unwrap_or_else(|_| panic!("--spread takes a percent, got {s}"))
            }),
            turned: args.iter().any(|a| a == "--turned"),
        },
        forces: match flag(&args, "--forces") {
            None | Some("bought") => Vec::new(),
            Some("all") => {
                let mut all: Vec<_> = reg.forces.values().collect();
                all.sort_by(|a, b| a.id.cmp(&b.id));
                all
            }
            Some(list) => list
                .split(',')
                .map(|id| {
                    reg.force(id).unwrap_or_else(|| {
                        eprintln!("error: --forces: no force `{id}`");
                        std::process::exit(1);
                    })
                })
                .collect(),
        },
        bought: (flag(&args, "--forces") == Some("bought")).then(|| {
            let range = flag(&args, "--budget").unwrap_or("70,100");
            let (lo, hi) = range
                .split_once(',')
                .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
                .unwrap_or_else(|| panic!("--budget takes LO,HI, got {range}"));
            (lo, hi)
        }),
        counts: flag(&args, "--counts").map_or([0, 0], |s| {
            let v: Vec<u32> = s
                .split(',')
                .map(|x| x.parse().expect("--counts C,U"))
                .collect();
            [v[0], v[1]]
        }),
        commanders: [
            Commander::parse(flag(&args, "--commander")),
            Commander::parse(flag(&args, "--opponent-commander")),
        ],
        command_brain: match flag(&args, "--commander") {
            Some("uniform") | Some("orders") => CommandBrain::Uniform,
            Some("learned") => {
                let path = flag(&args, "--commander-net").unwrap_or_else(|| {
                    eprintln!("error: --commander learned needs --commander-net");
                    std::process::exit(1);
                });
                let net = load_command_net(std::path::Path::new(path)).unwrap_or_else(|err| {
                    eprintln!("error: --commander-net {path}: {err}");
                    std::process::exit(1);
                });
                if args.iter().any(|a| a == "--commander-deterministic") {
                    CommandBrain::Learned(net)
                } else {
                    CommandBrain::Explore {
                        net,
                        temperature: parse("--commander-temperature", 1.0) as f32,
                    }
                }
            }
            _ => CommandBrain::Utility,
        },
        opponents: flag(&args, "--opponents")
            .map(|list| {
                list.split(',')
                    .filter(|p| !p.is_empty())
                    .map(|p| {
                        let net = load_goal_net(std::path::Path::new(p)).unwrap_or_else(|err| {
                            eprintln!("error: --opponents {p}: {err}");
                            std::process::exit(1);
                        });
                        let name = std::path::Path::new(p)
                            .file_name()
                            .map_or(p.to_string(), |n| n.to_string_lossy().into_owned());
                        (name, net)
                    })
                    .collect()
            })
            .unwrap_or_default(),
    };

    let jobs: Vec<u64> = (first..first + games).collect();
    let mut fought = run_all(&jobs, |&game| fight(&setup, game));

    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).expect("create output directory");
    }
    let mut file = std::io::BufWriter::new(std::fs::File::create(&out).expect("open output"));
    // With `--arrays` the decisions' numbers go to flat little-endian files
    // beside the battles, in the order the battles and their decisions are
    // written, and the JSON keeps everything else: parsing 800 MB of
    // numbers out of text was most of what a training update spent before
    // it learned anything.
    // The crews' decisions under the plain suffixes, the commander's under
    // `.cmd` ones: two streams with two widths.
    let mut arrays = args.iter().any(|a| a == "--arrays").then(|| {
        (
            Arrays::open(
                &out,
                "",
                tactics_core::ai::features::context_names(),
                tactics_core::ai::features::CANDIDATE,
            ),
            Arrays::open(
                &out,
                ".cmd",
                tactics_core::ai::commander::context_names(),
                tactics_core::ai::commander::CANDIDATE,
            ),
        )
    });
    let (mut won, mut lost, mut drawn, mut decisions) = (0, 0, 0, 0usize);
    let (mut by, mut on) = ([0u32; 3], [0u32; 3]);
    for battle in fought.iter_mut().flatten() {
        if let Some((crews, command)) = &mut arrays {
            crews.write(&mut battle.decisions);
            command.write(&mut battle.commands);
        }
        match battle.winner {
            Some(w) if w == battle.learner => won += 1,
            Some(_) => lost += 1,
            None => drawn += 1,
        }
        decisions += battle.decisions.len();
        for i in 0..3 {
            by[i] += battle.struck_by_learner[i];
            on[i] += battle.struck_on_learner[i];
        }
        serde_json::to_writer(&mut file, battle).expect("write battle");
        file.write_all(b"\n").expect("write battle");
    }
    file.flush().expect("flush output");
    if let Some((mut crews, mut command)) = arrays {
        crews.flush();
        command.flush();
    }
    println!(
        "{games} battles on {}: learner won {won}, lost {lost}, drew {drawn}; {decisions} decisions",
        arena.id
    );
    let share = |f: [u32; 3]| {
        let total = f.iter().sum::<u32>().max(1) as f32;
        format!(
            "front {:.0}% side {:.0}% rear {:.0}% of {}",
            100.0 * f[0] as f32 / total,
            100.0 * f[1] as f32 / total,
            100.0 * f[2] as f32 / total,
            f.iter().sum::<u32>()
        )
    };
    let mut against: std::collections::BTreeMap<&str, [u32; 3]> = Default::default();
    for battle in fought.iter().flatten() {
        let row = against.entry(battle.opponent.as_str()).or_default();
        match battle.winner {
            Some(w) if w == battle.learner => row[0] += 1,
            Some(_) => row[1] += 1,
            None => row[2] += 1,
        }
    }
    for (name, [w, l, d]) in &against {
        println!("  against {name}: won {w}, lost {l}, drew {d}");
    }
    let mut commanded: std::collections::BTreeMap<&str, [u32; 3]> = Default::default();
    for battle in fought.iter().flatten() {
        let row = commanded.entry(battle.opponent_commander).or_default();
        match battle.winner {
            Some(w) if w == battle.learner => row[0] += 1,
            Some(_) => row[1] += 1,
            None => row[2] += 1,
        }
    }
    for (name, [w, l, d]) in &commanded {
        println!("  against a commander {name}: won {w}, lost {l}, drew {d}");
    }
    let commands: usize = fought.iter().flatten().map(|b| b.commands.len()).sum();
    if commands > 0 {
        println!("  the learner's commander decided {commands} times");
    }
    let mut with: std::collections::BTreeMap<&str, [u32; 3]> = Default::default();
    for battle in fought.iter().flatten() {
        let Some(pair) = &battle.forces else { continue };
        let row = with
            .entry(pair[battle.learner as usize].as_str())
            .or_default();
        match battle.winner {
            Some(w) if w == battle.learner => row[0] += 1,
            Some(_) => row[1] += 1,
            None => row[2] += 1,
        }
    }
    for (name, [w, l, d]) in &with {
        println!("  learner as {name}: won {w}, lost {l}, drew {d}");
    }
    // An assault read by role, size, preparation and how it ended: the
    // learner attacks from side 0 and defends from side 1.
    let mut by_role: std::collections::BTreeMap<String, [u32; 3]> = Default::default();
    let mut ended: std::collections::BTreeMap<String, u32> = Default::default();
    for battle in fought.iter().flatten() {
        *ended.entry(battle.reason.clone()).or_default() += 1;
        let Some(record) = battle.sector else {
            continue;
        };
        let role = if battle.learner == 0 {
            "attacking"
        } else {
            "defending"
        };
        let tally = |row: &mut [u32; 3]| match battle.winner {
            Some(w) if w == battle.learner => row[0] += 1,
            Some(_) => row[1] += 1,
            None => row[2] += 1,
        };
        tally(by_role.entry(format!("learner {role}")).or_default());
        tally(
            by_role
                .entry(format!(
                    "learner {role} against a commander {}",
                    battle.opponent_commander
                ))
                .or_default(),
        );
        tally(
            by_role
                .entry(format!("learner {role}, {}", record.size))
                .or_default(),
        );
        let prep = if record.prepared { "prepared" } else { "hasty" };
        tally(
            by_role
                .entry(format!("learner {role}, {prep}"))
                .or_default(),
        );
    }
    for (name, [w, l, d]) in &by_role {
        println!("  {name}: won {w}, lost {l}, drew {d}");
    }
    let ends: Vec<String> = ended.iter().map(|(r, n)| format!("{r} {n}")).collect();
    println!("  ended by: {}", ends.join(", "));
    println!("  struck by the learner: {}", share(by));
    println!("  struck on the learner: {}", share(on));
    let (mut f4, mut aimed, mut off, mut missions) = ([0u32; 2], [0u32; 2], [0u32; 2], [0u32; 2]);
    for battle in fought.iter().flatten() {
        for i in 0..2 {
            f4[i] += battle.fix_and_flank[i];
            aimed[i] += battle.aimed[i];
            off[i] += battle.standoff[i];
            missions[i] += battle.missions[i];
        }
    }
    let n = fought.iter().flatten().count().max(1) as f32;
    for (i, who) in ["learner", "opponent"].iter().enumerate() {
        println!(
            "  {who}: fix and flank {:.2} a battle; {:.0}% of {} aimed shots from beyond the target's reach; {:.1} missions a battle",
            f4[i] as f32 / n,
            100.0 * off[i] as f32 / aimed[i].max(1) as f32,
            aimed[i],
            missions[i] as f32 / n
        );
    }
}
