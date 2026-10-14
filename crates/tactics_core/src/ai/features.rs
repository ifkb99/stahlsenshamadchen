//! What a learned goal chooser sees: one goal decision, written down as
//! numbers.
//!
//! REIMAGINED.md, L0.2. A learned policy replaces [`super::goal::GoalChooser`]
//! and nothing else, and it does so by **scoring candidates** rather than by
//! picking from a fixed menu: every candidate goal arrives as a row of
//! [`CANDIDATE`] features beside one row of [`CONTEXT`] features about the
//! crew choosing, and the network scores each row. That shape is what makes a
//! new *kind* of goal a fine-tune rather than a new network — it is a new kind
//! of row, not a new output slot.
//!
//! # What goes in
//!
//! Three families, and the first is there on purpose:
//!
//! - **The teacher's own reading** of each candidate
//!   ([`super::goal::Reading`]): worth to her doctrine, the drive by crow and
//!   by road, the exposure along it, who gets there first, whether it sets
//!   her orders aside. The first network is trained to imitate the utility
//!   chooser, and giving it the chooser's arithmetic is what lets it get
//!   there in hours instead of rediscovering `score_tile`.
//! - **Facts the teacher does not weigh directly**: the fire she would take
//!   standing there, the climb, the cover, the friends nearby, whose
//!   objective it is. Reinforcement learning has something to find in these
//!   that the hand-written chooser does not use.
//! - **Who she is and what she was told**: her condition, her nerve, her
//!   order's verb, and her doctrine's numbers — the doctrine stands in for
//!   the trait vector until cadets carry one (L1.4), since in the utility AI
//!   the doctrine *is* the personality.
//!
//! # What does not go in
//!
//! **No coordinate, ever.** Every feature is a distance, a count, a share, a
//! rule's answer or a flag, which a reflection of the battlefield preserves —
//! the tiebreak invariant in CLAUDE.md applied to a policy's inputs. A
//! network shown a coordinate learns which way is west, and it would learn it
//! on every map. No side number either, for the same reason one end of a
//! mirrored arena must not differ from the other.
//!
//! Values are raw rather than normalised. The trainer standardises from the
//! data, and a constant chosen here would be a second copy of a scale the
//! data already has. The one exception is infinity — "nobody is coming" —
//! which is capped at [`NOBODY_COMING`] rounds and flagged beside it.

use super::Evaluator;
use super::goal::{Reading, read_candidates};
use crate::battle::{BattleState, Goal, Knower, Mission, UnitId, along_the_bearing, incoming};
use crate::data::DataRegistry;
use crate::map::ObjectiveKind;
use hexx::Hex;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// Rounds standing in for "no known enemy could get there at all", so a
/// feature is never infinite. Far beyond any battle's length (they are over
/// in fifteen or so), and flagged in `nobody_coming` beside it.
pub const NOBODY_COMING: f32 = 60.0;

/// How near a friend has to be to a candidate to count as beside it, in
/// hexes. Two is the overwatch distance a section keeps on this scale.
const FRIENDS_NEAR: u32 = 2;

/// Hexes standing in for "nobody there at all" in a distance feature: past
/// any weapon's reach and any battle map's width that matters.
const FAR: f32 = 40.0;

/// Who is where, read once per decision rather than once per candidate.
///
/// Enemies are the ones *she* knows of (`Knower::Crew`), with the way each
/// hull faces and how far each one's guns reach; friends are every crew of
/// her side on the field but her, with where she means to go.
struct Scene {
    reach: i32,
    enemies: Vec<Party>,
    friends: Vec<Party>,
}

struct Party {
    pos: Hex,
    /// The hull's facing as a direction, for the aspect a place shows her.
    facing: Hex,
    reach: i32,
    /// Where a friend's goal is taking her, if anywhere.
    heading: Option<Hex>,
}

fn reach_of(registry: &DataRegistry, unit: &crate::battle::Unit) -> i32 {
    registry
        .vehicle(&unit.vehicle)
        .map(|v| {
            v.weapons
                .iter()
                .filter_map(|w| registry.weapon(w))
                .map(|w| w.range[1] as i32)
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

fn scene(registry: &DataRegistry, state: &BattleState, unit: UnitId) -> Scene {
    let Some(me) = state.unit(unit) else {
        return Scene {
            reach: 0,
            enemies: Vec::new(),
            friends: Vec::new(),
        };
    };
    let party = |u: &crate::battle::Unit| Party {
        pos: u.pos,
        facing: Hex::from(u.facing),
        reach: reach_of(registry, u),
        heading: match u.goal {
            Some(Goal::Take(hex)) => Some(hex),
            _ => None,
        },
    };
    Scene {
        reach: reach_of(registry, me),
        enemies: state
            .known_enemies(registry, Knower::Crew(unit))
            .into_iter()
            .map(party)
            .collect(),
        friends: state
            .alive_units()
            .filter(|u| u.side == me.side && u.id != unit && u.aboard.is_none())
            .map(party)
            .collect(),
    }
}

/// The cosine of the angle between two hex differences, through the cube
/// dot product — a quantity a reflection preserves, because it negates
/// neither or both. Zero for a zero vector.
fn cosine(a: Hex, b: Hex) -> f32 {
    let (ab, aa, bb) = (
        along_the_bearing(a, b) as f32,
        along_the_bearing(a, a) as f32,
        along_the_bearing(b, b) as f32,
    );
    if aa == 0.0 || bb == 0.0 {
        0.0
    } else {
        ab / (aa * bb).sqrt()
    }
}

/// The context features, in order. The doctrine's numbers come after the
/// state, then one flag per verb of the order vocabulary
/// ([`Mission::vocabulary`]), so the width follows the vocabulary rather
/// than being restated here.
const CONTEXT_FIXED: &[&str] = &[
    "substance_share",
    "substance_full",
    "movement_points",
    "longest_shot",
    "pressure",
    "rung",
    "obeys",
    "pinned",
    "buttoned",
    "moved",
    "detached",
    "known_enemies",
    "friends",
    "round",
    "aggression",
    "cover_value",
    "elevation_value",
    "concentration",
    "scouting",
    "objective_value",
    "indirect_appetite",
    "withdraw_threshold",
    "initiative",
    "delegation",
    "route_caution",
    "contest_aversion",
    "screening",
    "has_order",
];

/// The candidate features, in order.
pub const CANDIDATE: &[&str] = &[
    "hold",
    "ordered",
    "deviates",
    "objective",
    "objective_value",
    "objective_ours",
    "objective_theirs",
    "exit",
    "distance",
    "worth",
    "crow",
    "road",
    "exposed",
    "theirs",
    "nobody_coming",
    "late",
    "incoming",
    "incoming_share",
    "climb",
    "cover",
    "friends_near",
    // The relational facts (L1.2): what makes a place a flank, a crossfire,
    // an overwatch or a mistake, written as geometry and never as a name.
    "enemy_distance",
    "closing",
    "seen_by",
    "targets",
    "aspect_min",
    "aspect_nearest",
    "crossfire",
    "friends_heading_near",
    "friend_distance",
];

/// Every context feature's name, in order: the fixed ones, then
/// `order_<verb>` for each verb in the vocabulary.
pub fn context_names() -> Vec<String> {
    CONTEXT_FIXED
        .iter()
        .map(|s| s.to_string())
        .chain(
            Mission::vocabulary()
                .iter()
                .map(|(verb, _)| format!("order_{verb}")),
        )
        .collect()
}

/// One goal decision as numbers: the crew's context, a row per candidate in
/// the candidate list's order, and — when recorded from a chooser — which
/// row she took.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GoalRecord {
    /// The round the decision was taken in. Bookkeeping, not a feature: a
    /// trainer that wants it splits by battle with it.
    pub round: u32,
    /// The crew, so a trainer can follow one cadet's decisions through a
    /// battle. Also not a feature.
    pub unit: u32,
    pub context: Vec<f32>,
    pub candidates: Vec<Vec<f32>>,
    /// Index into `candidates` of the goal chosen. `None` when the choice
    /// was not on the list, which a chooser should never do and a trainer
    /// should drop.
    pub chosen: Option<usize>,
    /// Her side, so a trainer can keep the learner's decisions and drop the
    /// opponent's.
    #[serde(default)]
    pub side: u8,
    /// The log-probability the chooser gave `chosen`, when it sampled rather
    /// than took the best (`GoalBrain::Explore`) — the old policy's half of
    /// the ratio a policy-gradient update needs. `None` for a deterministic
    /// choice.
    #[serde(default)]
    pub logp: Option<f32>,
}

/// Where a tapped planner writes its decisions. Shared because the harness
/// fights battles across every core and a side's executors are several
/// planners; a `Mutex` because recording is the rare path and contention on
/// it is not worth designing around.
pub type GoalTap = Arc<Mutex<Vec<GoalRecord>>>;

/// Describe one goal decision: the crew's context and a row per candidate.
///
/// Reads the candidates through [`read_candidates`] under `evaluator`, the
/// same reading [`super::goal::UtilityChooser`] takes, so that the teacher's
/// arithmetic in a record is exactly the arithmetic the teacher judged.
/// Takes no rng and changes nothing.
pub fn describe(
    registry: &DataRegistry,
    state: &BattleState,
    evaluator: &Evaluator,
    unit: UnitId,
    candidates: &[Goal],
) -> GoalRecord {
    let readings = read_candidates(registry, state, evaluator, unit, candidates);
    GoalRecord {
        round: state.round,
        unit: unit.index() as u32,
        context: context(registry, state, evaluator, unit),
        candidates: {
            let scene = scene(registry, state, unit);
            readings
                .iter()
                .map(|reading| candidate(registry, state, unit, &scene, reading))
                .collect()
        },
        chosen: None,
        side: state.unit(unit).map(|u| u.side).unwrap_or(0),
        logp: None,
    }
}

fn flag(b: bool) -> f32 {
    if b { 1.0 } else { 0.0 }
}

fn context(
    registry: &DataRegistry,
    state: &BattleState,
    evaluator: &Evaluator,
    unit: UnitId,
) -> Vec<f32> {
    let width = CONTEXT_FIXED.len() + Mission::vocabulary().len();
    let Some(me) = state.unit(unit) else {
        return vec![0.0; width];
    };
    let (left, full) = state.substance(registry, me);
    let vehicle = registry.vehicle(&me.vehicle);
    let movement = vehicle.map(|v| v.movement.points).unwrap_or(0) as f32;
    let reach = vehicle
        .map(|v| {
            v.weapons
                .iter()
                .filter_map(|w| registry.weapon(w))
                .map(|w| w.range[1])
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0) as f32;
    let morale = &registry.morale;
    let rung = morale.rung(me.pressure);
    let rung_index = morale
        .rungs
        .iter()
        .position(|r| r.id == rung.id)
        .unwrap_or(0) as f32;
    let known = state.known_enemies(registry, Knower::Crew(unit)).len() as f32;
    let friends = state
        .alive_units()
        .filter(|u| u.side == me.side && u.id != unit && u.aboard.is_none())
        .count() as f32;
    let mission = state
        .command
        .formation_of(unit)
        .and_then(|f| f.mission_for(unit));
    let d = &evaluator.doctrine;

    let mut out = vec![
        if full == 0 {
            0.0
        } else {
            left as f32 / full as f32
        },
        full as f32,
        movement,
        reach,
        me.pressure as f32,
        rung_index,
        flag(rung.obeys),
        flag(rung.pinned),
        flag(me.buttoned > 0),
        me.moved as f32,
        flag(me.detached()),
        known,
        friends,
        state.round as f32,
        d.aggression,
        d.cover_value,
        d.elevation_value,
        d.concentration,
        d.scouting,
        d.objective_value,
        d.indirect_appetite,
        d.withdraw_threshold,
        d.initiative,
        d.delegation,
        d.route_caution,
        d.contest_aversion,
        d.screening,
        flag(mission.is_some()),
    ];
    let verb = mission.map(|m| m.verb());
    out.extend(
        Mission::vocabulary()
            .iter()
            .map(|(v, _)| flag(verb == Some(*v))),
    );
    debug_assert_eq!(out.len(), width);
    out
}

fn candidate(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    scene: &Scene,
    reading: &Reading,
) -> Vec<f32> {
    let Some(me) = state.unit(unit) else {
        return vec![0.0; CANDIDATE.len()];
    };
    let hex = reading.hex;
    let ordered = super::goal::ordered_goal(state, unit) == Some(reading.goal);
    // The richest objective this hex belongs to, if any, and who holds it.
    let objective = state
        .objectives()
        .filter(|(o, _)| o.hexes.contains(&hex))
        .max_by_key(|(o, _)| o.value);
    let (value, ours, theirs, exit) = match objective {
        Some((o, held)) => (
            o.value as f32,
            held == Some(me.side),
            held.is_some_and(|s| s != me.side),
            o.kind == ObjectiveKind::Exit,
        ),
        None => (0.0, false, false, false),
    };
    let coming = reading.theirs.is_finite();
    let theirs_rounds = if coming {
        reading.theirs.min(NOBODY_COMING)
    } else {
        NOBODY_COMING
    };
    let fire = incoming(registry, state, unit, hex).worth;
    let (left, _) = state.substance(registry, me);
    let elevation = |h| state.world.get(h).map(|t| t.elevation).unwrap_or(0);
    let cover = state
        .terrain_at(hex)
        .and_then(|id| registry.terrain(id))
        .map(|t| t.cover)
        .unwrap_or(0) as f32;
    let friends_near = state
        .alive_units()
        .filter(|u| {
            u.side == me.side
                && u.id != unit
                && u.aboard.is_none()
                && u.pos.distance_to(hex) as u32 <= FRIENDS_NEAR
        })
        .count() as f32;

    // The relational facts. Distances and counts, and cosines of angles
    // between differences — never a bearing, which a reflection turns round.
    let sight = |from: Hex, to: Hex| state.world.sight_clear(from, to);
    let nearest_enemy = scene
        .enemies
        .iter()
        .min_by_key(|e| (e.pos.distance_to(hex), e.pos.distance_to(me.pos)));
    let enemy_distance = nearest_enemy.map_or(FAR, |e| (e.pos.distance_to(hex) as f32).min(FAR));
    // How much nearer the nearest enemy this place takes her.
    let closing = nearest_enemy.map_or(0.0, |e| {
        e.pos.distance_to(me.pos) as f32 - e.pos.distance_to(hex) as f32
    });
    let seen_by = scene
        .enemies
        .iter()
        .filter(|e| e.pos.distance_to(hex) <= e.reach && sight(e.pos, hex))
        .count() as f32;
    let targets: Vec<&Party> = scene
        .enemies
        .iter()
        .filter(|e| e.pos.distance_to(hex) <= scene.reach && sight(hex, e.pos))
        .collect();
    // The aspect she would show an enemy's hull from here: +1 straight in
    // front of him, −1 straight behind. The least over everyone she could
    // shoot from here is how far round him this place gets her.
    let aspect = |e: &Party| cosine(e.facing, hex - e.pos);
    let aspect_min = targets.iter().map(|e| aspect(e)).fold(1.0f32, f32::min);
    let aspect_min = if targets.is_empty() { 0.0 } else { aspect_min };
    let aspect_nearest = nearest_enemy.map_or(0.0, aspect);
    // Crossfire: for each enemy she could shoot from here, the widest angle
    // between her line onto him and a friend's who can shoot him too, as
    // (1 − cos) / 2 — zero side by side, one from opposite sides.
    let crossfire = targets
        .iter()
        .flat_map(|e| {
            scene
                .friends
                .iter()
                .filter(|f| f.pos.distance_to(e.pos) <= f.reach && sight(f.pos, e.pos))
                .map(|f| (1.0 - cosine(hex - e.pos, f.pos - e.pos)) / 2.0)
        })
        .fold(0.0f32, f32::max);
    let friends_heading_near = scene
        .friends
        .iter()
        .filter(|f| {
            f.heading
                .is_some_and(|h| h.distance_to(hex) as u32 <= FRIENDS_NEAR)
        })
        .count() as f32;
    let friend_distance = scene
        .friends
        .iter()
        .map(|f| f.pos.distance_to(hex) as f32)
        .fold(FAR, f32::min);

    let out = vec![
        flag(reading.goal == Goal::Hold),
        flag(ordered),
        flag(reading.deviates),
        flag(objective.is_some()),
        value,
        flag(ours),
        flag(theirs),
        flag(exit),
        me.pos.distance_to(hex) as f32,
        reading.worth,
        reading.crow,
        reading.road,
        reading.exposed,
        theirs_rounds,
        flag(!coming),
        (reading.road - theirs_rounds).max(0.0),
        fire,
        if left == 0 { 0.0 } else { fire / left as f32 },
        (elevation(hex) - elevation(me.pos)) as f32,
        cover,
        friends_near,
        enemy_distance,
        closing,
        seen_by,
        targets.len() as f32,
        aspect_min,
        aspect_nearest,
        crossfire,
        friends_heading_near,
        friend_distance,
    ];
    debug_assert_eq!(out.len(), CANDIDATE.len());
    out
}
