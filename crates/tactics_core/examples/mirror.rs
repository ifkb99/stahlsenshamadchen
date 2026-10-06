//! Does an arena actually *play* mirrored, and where does it stop?
//!
//! An arena is built symmetric under two reflections — through the centre,
//! which swaps the ends, and across the axis of advance, which swaps left and
//! right — and every table that fights on one quotes that symmetry in its own
//! heading. But symmetric *ground* is not the claim. The claim is that a
//! battle fought on it is symmetric, and that is a property of the rules
//! rather than of the map: at difficulty 5 the planner's blur is exactly zero,
//! so with mirrored ground and a mirrored force every decision one end makes
//! must be the reflection of the decision its twin makes.
//!
//! Where that stops being true, some rule is reading a quantity the reflection
//! does not preserve — the invariants' own prohibition, and this probe is how
//! you find out *which round* and *which crew*. It has already earned its
//! keep: it is what turned "side B wins 46 of 72 equal battles and nobody
//! knows why" into two named defects, `ai/goal.rs::candidates` pushing an
//! objective's hexes in map-file order and `sight_line_clear` rounding a ray
//! that runs along a hex boundary by an absolute rule.
//!
//! ```sh
//! cargo run --release -p tactics_core --example mirror
//! cargo run --release -p tactics_core --example mirror -- --arena ridge_arena
//! cargo run --release -p tactics_core --example mirror -- --seeds 0,1,2,3,4 --rounds 12
//! cargo run --release -p tactics_core --example mirror -- --difficulty 3
//! cargo run --release -p tactics_core --example mirror -- --map battle_town
//! ```
//!
//! `--map` plays a *shipped* battle map instead of an arena: one of the
//! generated ones (`battle_hills`, `battle_town`), whose ground and orders of
//! battle are exact west–east reflections of each other in map-file columns
//! (`tools/make_battle_maps.py` asserts it). Their reflection is across the
//! north–south axis rather than through the centre, each crew's twin is the
//! crew the map placed on her reflected hex, and crews are stamped anonymous
//! so that nobody's twin is better trained than she is — the arrangement the
//! `ground` table fights, but at a difficulty with no blur. It exists because
//! that table read `battle_town` leaning two to one east once each side had a
//! reserve, with every placement mirrored and the lean surviving the ids
//! being exchanged.
//!
//! `--release` is not required for correctness — this is a determinism probe,
//! not a timing one — but a dozen rounds of two planners is a few seconds in
//! dev and a blink in release.
//!
//! # What the report distinguishes, and why that is the whole point
//!
//! Not every divergence is a defect. The invariants permit a total order whose
//! **coordinate key goes last**, on the stated ground that the compass then
//! decides only a left-or-right choice off the line of advance. `candidates`
//! is written that way today: an objective's hexes are sorted by distance from
//! the crew, and `(x, y)` breaks the remaining ties. So two crews standing on
//! reflected ground, offered two hexes they are *equally far from*, will pick
//! hexes that are not each other's reflection — and that is the documented
//! residual rather than a bug.
//!
//! The report therefore says which kind it found:
//!
//! - **equal-key tiebreak** — the goal one crew took and the reflection of the
//!   goal her twin took are the same distance from her. Nothing has read
//!   anything a reflection does not preserve except the coordinate that is
//!   allowed to be read last. Expected; not a defect.
//! - **different key** — the two are at *different* distances, so a real
//!   ordering key disagreed and the tiebreak never came into it. That is a
//!   rule reading the compass, and it is the thing this probe exists to find.
//!
//! A position that diverges without a goal divergence in front of it is
//! reported as its own kind: goals are chosen before ground is crossed, so a
//! crew who is not where her twin's reflection is while both mean to go to
//! reflected places has been moved differently by pathing, occupancy or
//! opportunity fire.
//!
//! # Only the first divergence is a finding
//!
//! Planning walks units in id order, and the arena stages the i-th vehicle of
//! each side on the i-th deployment pair, so **pair order is decision order**.
//! Once one pair has broken the mirror, everything decided after it that round
//! is downstream of it: a crew's candidate list drops hexes a friend has
//! already claimed, her path avoids where friends are standing, and her fire
//! reads the whole board. So a second pair choosing unreflected ground in the
//! same round is a consequence, not a second defect.
//!
//! The report therefore names the **root** — the lowest-numbered pair in the
//! first round that broke — and lists the rest of that round as downstream.
//! The verdict counts roots only. Without that rule the ridge arena reports
//! four "a rule read the compass" findings that are all the same tie, seen
//! four crews later.

use std::sync::Arc;
use tactics_core::Hex;
use tactics_core::ai::{AiConfig, AiDriver, make_battle_planner};
use tactics_core::battle::UnitId;
use tactics_core::battle::{BattleState, Goal, SideState};
use tactics_core::data::DataRegistry;
use tactics_core::harness::arena::{ARENAS, Arena, DEFAULT_ARENA, arena_named};
use tactics_core::map::Battlefield;
use tactics_core::map::UnitPlacement;
use tactics_core::roster::Roster;

/// One vehicle per deployment pair, and the pairs come in flip twins, so this
/// list has to name the same chassis twice running or the *force* is not
/// laterally symmetric even though the ground is. See `Arena::deployment`.
const ROSTER: [&str; 6] = [
    "medium_tank",
    "medium_tank",
    "tank_destroyer",
    "tank_destroyer",
    "light_tank",
    "light_tank",
];

/// What the probe plays on: an arena, or a shipped battle map mirrored west
/// to east. Everything the comparison needs is the reflection and a centre to
/// print coordinates from.
struct Board {
    id: String,
    blurb: String,
    mirror: Box<dyn Fn(Hex) -> Hex>,
    centre: Hex,
    map: Option<String>,
    arena: Option<&'static Arena>,
}

impl Board {
    fn of_arena(arena: &'static Arena) -> Self {
        Board {
            id: arena.id.to_string(),
            blurb: arena.blurb.to_string(),
            mirror: Box::new(move |h| arena.mirror(h)),
            centre: arena.centre_hex(),
            map: None,
            arena: Some(arena),
        }
    }

    /// A generated battle map: reflected across the north–south axis, which
    /// in map-file terms keeps the row and sends column `c` to
    /// `2·radius − row % 2 − c` — the generator's own doubled lateral
    /// coordinate, negated.
    fn of_map(registry: &DataRegistry, id: &str) -> Self {
        let file = registry.map(id).unwrap_or_else(|| {
            eprintln!("error: --map {id}: no such map");
            std::process::exit(1);
        });
        let _ = file;
        // A battle map is the regulation hexagon, 2·radius + 1 across.
        let span = 2 * registry.scale.battle_map_radius() as i32;
        Board {
            id: id.to_string(),
            blurb: "a shipped battle map, mirrored west to east".into(),
            mirror: Box::new(move |h| {
                let [c, r] = tactics_core::hex_to_offset(h);
                tactics_core::offset_to_hex(span - r.rem_euclid(2) - c, r)
            }),
            centre: tactics_core::offset_to_hex(span / 2, span / 2),
            map: Some(id.to_string()),
            arena: None,
        }
    }

    fn mirror(&self, h: Hex) -> Hex {
        (self.mirror)(h)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return;
    }
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (mut registry, _) = DataRegistry::load_dir(&root).expect("mods load");
    // A search roll is the first die most battles throw, and the rng is drawn
    // in unit-id order, so the two ends read different dice from it: a
    // divergence at first contact is a coin until this is ruled out.
    if args.iter().any(|a| a == "--no-search") {
        registry.balance.detection_certain_percent = 100;
    }
    let arena = match flag(&args, "--arena") {
        None => DEFAULT_ARENA,
        Some(name) => arena_named(name).unwrap_or_else(|| {
            eprintln!(
                "error: --arena {name}: no such arena. There is: {}",
                ARENAS.iter().map(|a| a.id).collect::<Vec<_>>().join(", ")
            );
            std::process::exit(1);
        }),
    };
    let seeds: Vec<u64> = flag(&args, "--seeds")
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![0, 1, 2, 3]);
    // Five by default because that is where the blur is *exactly* zero and a
    // divergence therefore has nowhere else to come from. Anything lower is
    // asking a different question — how much noise hides the residual — and is
    // worth asking, which is why the flag exists.
    let difficulty: u8 = flag(&args, "--difficulty")
        .and_then(|v| v.parse().ok())
        .unwrap_or(5);
    let rounds: usize = flag(&args, "--rounds")
        .and_then(|v| v.parse().ok())
        .unwrap_or(8);

    let board = match flag(&args, "--map") {
        Some(id) => Board::of_map(&registry, id),
        None => Board::of_arena(arena),
    };

    println!(
        "mirror: {} ({}), difficulty {difficulty} both sides, {} rounds, seeds {seeds:?}",
        board.id, board.blurb, rounds
    );
    if board.arena.is_some() {
        println!(
            "  {} vehicles a side on {} deployment pairs\n",
            ROSTER.len().min(arena.deployment().len()),
            arena.deployment().len()
        );
    } else {
        println!("  the map's own orders of battle, anonymously crewed\n");
    }

    let mut findings: Vec<Finding> = Vec::new();
    for &seed in &seeds {
        findings.extend(play(&registry, &board, seed, difficulty, rounds));
    }
    report(&board, &seeds, &findings);
}

const USAGE: &str = "\
where a mirrored battle stops being mirrored

  cargo run --release -p tactics_core --example mirror [-- FLAGS]

  --arena NAME       which battlefield (default skill_arena)
  --map ID           a generated battle map instead (battle_hills, battle_town)
  --seeds A,B,C      which battles (default 0,1,2,3)
  --difficulty N     both sides (default 5, where the blur is exactly zero)
  --rounds N         how far to follow each battle (default 8)
  --no-search        find everybody in sight at once: no search roll, so a
                     divergence before the first shot is a rule, not a die

  At difficulty 5 every divergence is a rule reading something the
  reflection does not preserve, except a tiebreak between candidates whose
  real ordering keys are equal — which the invariants permit and which this
  report labels `equal-key tiebreak` rather than counting as a defect.";

/// What went wrong, and of which kind.
struct Finding {
    seed: u64,
    round: usize,
    /// Index into the deployment pairing: unit `2i` is side A's, `2i + 1` is
    /// side B's twin.
    pair: usize,
    vehicle: String,
    kind: Kind,
    detail: String,
    /// Decided after another pair had already broken the mirror this round, so
    /// it is a consequence of that one rather than a finding of its own. See
    /// the module doc.
    downstream: bool,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    /// A real ordering key disagreed: the two crews are choosing between
    /// candidates they are *not* equally far from. A rule read the compass.
    DifferentKey,
    /// Two candidates at the same distance, settled by the coordinate that the
    /// invariants allow to be read last. Documented residual.
    EqualKeyTiebreak,
    /// Goals still mirror; the crews do not stand where each other's
    /// reflections do.
    Position,
    /// One crew has a goal and her twin has none, or the goals are of
    /// different kinds entirely.
    Shape,
}

impl Kind {
    fn label(&self) -> &'static str {
        match self {
            Kind::DifferentKey => "different key  (a rule read the compass)",
            Kind::EqualKeyTiebreak => "equal-key tiebreak (documented residual)",
            Kind::Position => "position only  (movement, not choice)",
            Kind::Shape => "goal shape     (one has one, the twin does not)",
        }
    }
}

/// Stage the arena with a laterally and centrally symmetric force.
fn stage(registry: &DataRegistry, arena: &Arena, seed: u64) -> BattleState {
    let map = arena.map().expect("the arena builds");
    let mut placements = Vec::new();
    for ((west, east), vehicle) in arena.deployment().into_iter().zip(ROSTER) {
        for (side, hex) in [(0u8, west), (1u8, east)] {
            placements.push(UnitPlacement {
                aboard_at: None,
                at: tactics_core::hex_to_offset(hex),
                side,
                vehicle: vehicle.to_string(),
                crew: Vec::new(),
                name: Some(format!("{vehicle} {side}")),
                facing: None,
                formation: None,
                leads: false,
            });
        }
    }
    let sides = vec![
        SideState {
            name: "A".into(),
            ai: None,
        },
        SideState {
            name: "B".into(),
            ai: None,
        },
    ];
    let (roster, crews) = Roster::stamp_for(registry, &placements);
    BattleState::from_placements(
        registry,
        map,
        sides,
        &placements,
        &crews,
        Arc::new(roster),
        seed,
    )
    .expect("an arena's own placements are on its own map")
}

/// Stage a shipped map with every crew stamped anonymous, and pair each of
/// side A's crews with the side-B crew the map placed on her reflected hex.
fn stage_map(
    registry: &DataRegistry,
    board: &Board,
    id: &str,
    seed: u64,
) -> (BattleState, Vec<(UnitId, UnitId)>) {
    let file = registry.map(id).expect("the map is in the registry");
    let map = Battlefield::from_map_file(file).expect("the map builds");
    let mut placements = file.units.clone();
    for p in &mut placements {
        p.crew.clear();
    }
    let sides = file
        .sides
        .iter()
        .map(|s| SideState {
            name: s.name.clone(),
            ai: None,
        })
        .collect();
    let (roster, crews) = Roster::stamp_for(registry, &placements);
    let state = BattleState::from_placements(
        registry,
        map,
        sides,
        &placements,
        &crews,
        Arc::new(roster),
        seed,
    )
    .expect("a shipped map's placements stage");
    let mut pairs = Vec::new();
    for a in state.units.iter().filter(|u| u.side == 0) {
        let twin = state.units.iter().find(|b| {
            b.side == 1
                && b.vehicle == a.vehicle
                && b.pos == board.mirror(a.pos)
                && b.aboard.is_some() == a.aboard.is_some()
        });
        match twin {
            Some(b) => pairs.push((a.id, b.id)),
            None => {
                eprintln!(
                    "error: {id}: {} at {:?} has no twin on the reflected hex; the map is not mirrored",
                    a.vehicle,
                    tactics_core::hex_to_offset(a.pos)
                );
                std::process::exit(1);
            }
        }
    }
    (state, pairs)
}

/// Play one battle and report the first round in which any pair breaks.
///
/// Stops at the first broken round rather than carrying on, and that is not
/// impatience: once two crews are standing on unreflected ground everything
/// downstream of them diverges too, so a second round of output is a
/// consequence of the first and not a second finding.
fn play(
    registry: &DataRegistry,
    board: &Board,
    seed: u64,
    difficulty: u8,
    rounds: usize,
) -> Vec<Finding> {
    // The same offset `balance`'s own arena duels use, so a divergence found
    // here is one of the battles the skill table actually fought.
    let (mut state, pairs) = match (board.arena, &board.map) {
        (Some(arena), _) => {
            let state = stage(registry, arena, 9000 + seed);
            let pairs = (0..state.units.len() / 2)
                .map(|i| (UnitId(i as u32 * 2), UnitId(i as u32 * 2 + 1)))
                .collect();
            (state, pairs)
        }
        (None, Some(id)) => stage_map(registry, board, id, 9000 + seed),
        (None, None) => unreachable!("a board is an arena or a map"),
    };
    let mut ai = AiDriver::new();
    for side in [0u8, 1] {
        let cfg = AiConfig {
            planner: "utility".into(),
            difficulty,
            doctrine: None,
        };
        ai.insert(
            side,
            make_battle_planner(&cfg, seed * 2 + side as u64, registry),
        );
    }

    for round in 1..=rounds {
        ai.plan_round(registry, &mut state);
        // Read the plan before it is resolved: a divergence in where a crew
        // has *decided* to go precedes one in where she ends up, and reporting
        // the second when the first is available names the wrong rule.
        let mut found: Vec<Finding> = Vec::new();
        for (pair, &(a, b)) in pairs.iter().enumerate() {
            let (Some(a), Some(b)) = (
                state.units.iter().find(|u| u.id == a && u.alive()),
                state.units.iter().find(|u| u.id == b && u.alive()),
            ) else {
                continue;
            };
            if let Some((kind, detail)) = compare(board, a.pos, a.goal, b.pos, b.goal) {
                found.push(Finding {
                    seed,
                    round,
                    pair,
                    vehicle: a.vehicle.clone(),
                    kind,
                    detail,
                    downstream: false,
                });
            }
        }
        if !found.is_empty() {
            // Pair order is decision order, so everything after the first
            // broken pair inherits a board that is already unmirrored.
            for f in found.iter_mut().skip(1) {
                f.downstream = true;
            }
            return found;
        }
        state.resolve_round(registry);
        if state.is_over() {
            break;
        }
    }
    Vec::new()
}

/// Is side B's crew doing the reflection of what side A's is doing?
///
/// Returns `None` when she is. Otherwise the kind of divergence and a line
/// naming both halves of it.
fn compare(
    arena: &Board,
    a_pos: Hex,
    a_goal: Option<Goal>,
    b_pos: Hex,
    b_goal: Option<Goal>,
) -> Option<(Kind, String)> {
    let want = arena.mirror(a_pos);
    match (a_goal, b_goal) {
        (Some(Goal::Take(here)), Some(Goal::Take(there))) => {
            let want_goal = arena.mirror(here);
            if want_goal == there {
                // Same intention. Anything left is where they are standing.
                return (b_pos != want).then(|| {
                    (
                        Kind::Position,
                        format!(
                            "A at {:?} -> Take{:?} | B at {:?}, wanted {:?}",
                            rel(arena, a_pos),
                            rel(arena, here),
                            rel(arena, b_pos),
                            rel(arena, want)
                        ),
                    )
                });
            }
            // The two crews chose different ground. The question the whole
            // probe turns on is whether a real ordering key separated the two
            // candidates, or whether they were equal and the coordinate — which
            // the invariants permit as a last key — settled it.
            //
            // `candidates` sorts an objective's hexes by distance from the
            // crew's own position. So measure both choices from A's own hex,
            // reflecting B's back: if the two are the same distance from her,
            // the sort had nothing to separate them.
            let mine = here.distance_to(a_pos);
            let hers = arena.mirror(there).distance_to(a_pos);
            let kind = if mine == hers {
                Kind::EqualKeyTiebreak
            } else {
                Kind::DifferentKey
            };
            Some((
                kind,
                format!(
                    "A at {:?} -> Take{:?} ({mine} away) | B -> Take{:?}, reflecting to {:?} ({hers} away)",
                    rel(arena, a_pos),
                    rel(arena, here),
                    rel(arena, there),
                    rel(arena, arena.mirror(there)),
                ),
            ))
        }
        (x, y) if goal_shape(&x) != goal_shape(&y) => Some((
            Kind::Shape,
            format!(
                "A at {:?} has {} | B at {:?} has {}",
                rel(arena, a_pos),
                goal_shape(&x),
                rel(arena, b_pos),
                goal_shape(&y)
            ),
        )),
        _ => (b_pos != want).then(|| {
            (
                Kind::Position,
                format!(
                    "A at {:?} | B at {:?}, wanted {:?}",
                    rel(arena, a_pos),
                    rel(arena, b_pos),
                    rel(arena, want)
                ),
            )
        }),
    }
}

fn goal_shape(g: &Option<Goal>) -> &'static str {
    match g {
        None => "no goal",
        Some(Goal::Take(_)) => "Take",
        Some(other) => match format!("{other:?}").split('(').next().unwrap_or("goal") {
            "Hold" => "Hold",
            _ => "another goal",
        },
    }
}

/// Arena-relative coordinates, because an absolute axial pair on a map whose
/// centre is at (15, 10) tells the reader nothing about which end it is.
fn rel(arena: &Board, h: Hex) -> (i32, i32) {
    let d = h - arena.centre;
    (d.x, d.y)
}

fn report(arena: &Board, seeds: &[u64], findings: &[Finding]) {
    if findings.is_empty() {
        println!(
            "clean: {} battles on {} played mirrored the whole way through.",
            seeds.len(),
            arena.id
        );
        return;
    }

    let roots: Vec<&Finding> = findings.iter().filter(|f| !f.downstream).collect();

    let mut ordered: Vec<&Finding> = findings.iter().collect();
    ordered.sort_by(|x, y| {
        (x.downstream, &x.kind, x.seed, x.round, x.pair).cmp(&(
            y.downstream,
            &y.kind,
            y.seed,
            y.round,
            y.pair,
        ))
    });
    let mut last: Option<(bool, &Kind)> = None;
    for f in &ordered {
        if last != Some((f.downstream, &f.kind)) {
            println!(
                "{}{}",
                if f.downstream { "downstream — " } else { "" },
                f.kind.label()
            );
            last = Some((f.downstream, &f.kind));
        }
        println!(
            "  seed {:<4} round {:<3} pair {} ({:<15}) {}",
            f.seed, f.round, f.pair, f.vehicle, f.detail
        );
    }

    let count = |k: Kind| roots.iter().filter(|f| f.kind == k).count();
    let real = count(Kind::DifferentKey) + count(Kind::Position) + count(Kind::Shape);
    println!(
        "\n{} divergences over {} battles, of which {} are roots: {} equal-key tiebreak, \
         {} different key, {} position only, {} goal shape.",
        findings.len(),
        seeds.len(),
        roots.len(),
        count(Kind::EqualKeyTiebreak),
        count(Kind::DifferentKey),
        count(Kind::Position),
        count(Kind::Shape),
    );
    if real == 0 {
        println!(
            "\nEvery root is the documented residual: two candidates the same distance from\n\
             the crew, settled by the coordinate key the invariants allow to go last.\n\
             Nothing here read the compass to break a tie it could have broken on distance,\n\
             and everything else listed was decided on a board another pair had already\n\
             unmirrored."
        );
    } else {
        println!(
            "\n{real} root(s) are NOT the documented residual. A rule is reading a quantity\n\
             the reflection does not preserve; find it before quoting anything measured on\n\
             this arena at difficulty 5."
        );
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).map(|s| s.as_str())
}
