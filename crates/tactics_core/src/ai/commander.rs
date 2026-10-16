//! What a learned commander sees: one mission decision, written down as
//! numbers (REIMAGINED.md, L3).
//!
//! The crews' goal decision ([`super::features`]) one level up, and the same
//! shape on purpose: a row of [`CONTEXT`] about the platoon being given its
//! orders beside a row of [`CANDIDATE`] per mission she could be given, and
//! a network scores each row. A new kind of order is a new kind of row.
//!
//! **One platoon at a time, and each sees the ones before it.** A review
//! walks her platoons in the order of battle and decides each in turn; what
//! an earlier platoon was just given counts as its orders when the next is
//! described ([`Plan`]), so "who else is going there, from which side, and
//! with how much" is a fact about every candidate. That is how a fix and a
//! flank can be chosen without either being named: two platoons converging
//! on one objective from different bearings is a cosine, not a template.
//!
//! **What she knows is her picture** (`Knower::Commander`): the enemies her
//! side has reported to her, never what a crew out of contact can see.
//!
//! No coordinate and no side number reach a feature, for the reason the
//! crews' description gives: a network shown a coordinate learns which way
//! is west. Objectives are offered nearest her platoon first, coordinate
//! last, because list order is the deterministic chooser's tie-break.

use super::command::SideCommand;
use super::features::{GoalRecord, cosine};
use crate::battle::{BattleState, Knower, Mission, Unit, UnitId, incoming};
use crate::data::DataRegistry;
use crate::map::ObjectiveKind;
use hexx::Hex;

/// Hexes standing in for "nobody there" in a distance feature.
const FAR: f32 = 40.0;

/// How near a known enemy has to be to an objective to count as on it or
/// covering it, in hexes: inside every direct gun's reach on this scale.
const NEAR_OBJECTIVE: i32 = 8;

/// How near a friend has to be to ground to count as already there.
const THERE: i32 = 2;

/// The context features, in order.
pub const CONTEXT: &[&str] = &[
    // The platoon.
    "vehicles",
    "strength",
    "substance_full",
    "speed",
    "reach",
    "armour",
    "infantry",
    "carriers",
    "indirect",
    "eyes",
    "has_mission",
    "nearest_enemy",
    "in_contact",
    // Her doctrine, which is how its crews will carry an order out.
    "aggression",
    "initiative",
    "delegation",
    // The battle.
    "round",
    "own_strength",
    "known_enemies",
    "known_strength",
    "margin",
    "own_score",
    "their_score",
    "held_ours",
    "held_theirs",
    "platoons",
    "decided_before",
    // An assault (ASSAULT.md): which end of it she is on, how far the
    // attacker is from what she needs, and how far each side is from the
    // point it breaks at.
    "attacking",
    "defending",
    "attack_short",
    "own_break_margin",
    "their_break_margin",
];

/// The candidate features, in order.
pub const CANDIDATE: &[&str] = &[
    // What the order is. `keep` is saying nothing (her standing orders, or
    // none); `free` is having none, so her crews choose for themselves.
    "keep",
    "free",
    "hold_here",
    "advance",
    "assault",
    "hold",
    "recon",
    "support",
    // The ground it names, if any.
    "objective",
    "objective_value",
    "objective_ours",
    "objective_theirs",
    "distance",
    "rounds",
    "enemies_near",
    "enemy_strength_near",
    "enemy_nearest",
    "incoming",
    "friends_there",
    // Who else is sent there (standing orders, and this review's so far).
    "committed",
    "committed_strength",
    "converging",
    // A base of fire: whom she would shoot for.
    "supported_strength",
    "supported_objective",
    "supported_distance",
];

/// A platoon's orders after a review so far: what each formation (by index)
/// will be doing — the ones already decided this review by what they were
/// just given, the rest by what they were last told.
pub type Plan = Vec<Option<Mission>>;

/// The plan a review starts from: everybody's latest orders.
pub fn plan_now(state: &BattleState) -> Plan {
    state
        .command
        .formations()
        .iter()
        .map(|f| f.latest_mission().cloned())
        .collect()
}

/// One option on the menu.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    /// Say nothing: her standing orders, or none, stand.
    Keep,
    /// Hand her back her own judgment ([`crate::battle::Order::ReleaseMission`]).
    Release,
    /// Give her this mission.
    Give(Mission),
}

impl Choice {
    /// The mission this option gives, if it gives one.
    pub fn mission(&self) -> Option<&Mission> {
        match self {
            Self::Give(mission) => Some(mission),
            Self::Keep | Self::Release => None,
        }
    }
}

/// The ground a mission names, if any: the hex it sends her to.
fn ground_of(state: &BattleState, mission: &Mission) -> Option<Hex> {
    match mission {
        Mission::Advance { to } | Mission::Assault { to } | Mission::Recon { toward: to } => {
            Some(*to)
        }
        Mission::Hold { at } => *at,
        Mission::Support { formation } => state
            .command
            .formations()
            .iter()
            .find(|f| f.id == *formation)
            .and_then(|f| f.leader)
            .and_then(|id| state.unit(id))
            .map(|u| u.pos),
        Mission::Withdraw { .. } => None,
    }
}

/// The objective a mission is about, by index: the one holding its ground,
/// or for a base of fire the one the supported platoon was sent to.
fn objective_of(state: &BattleState, plan: &Plan, mission: &Mission) -> Option<usize> {
    let hex = match mission {
        Mission::Support { formation } => {
            let index = state
                .command
                .formations()
                .iter()
                .position(|f| f.id == *formation)?;
            match plan.get(index)?.as_ref()? {
                Mission::Support { .. } => return None,
                other => ground_of(state, other)?,
            }
        }
        other => ground_of(state, other)?,
    };
    state
        .scenario
        .objectives()
        .iter()
        .position(|o| o.kind == ObjectiveKind::Hold && o.contains(hex))
}

/// Everybody alive in a formation.
fn members<'a>(
    state: &'a BattleState,
    formation: &'a crate::battle::Formation,
) -> impl Iterator<Item = &'a Unit> {
    formation.members.iter().filter_map(|id| state.unit(*id))
}

/// A formation's substance left and its full complement, the dead counting
/// their full complement against it.
fn strength(registry: &DataRegistry, state: &BattleState, index: usize) -> (f32, f32) {
    let Some(formation) = state.command.formations().get(index) else {
        return (0.0, 0.0);
    };
    let (mut left, mut full) = (0u32, 0u32);
    for id in &formation.members {
        let Some(unit) = state.lookup(*id) else {
            continue;
        };
        let (have, total) = state.substance(registry, unit);
        if unit.alive() {
            left += have;
        }
        full += total;
    }
    (left as f32, full as f32)
}

/// Where a formation is, for distances: its leader, else its first member
/// still on the field.
fn where_is(state: &BattleState, index: usize) -> Option<(UnitId, Hex)> {
    let formation = state.command.formations().get(index)?;
    formation
        .leader
        .and_then(|id| state.unit(id))
        .or_else(|| members(state, formation).next())
        .map(|u| (u.id, u.pos))
}

/// The options a review offers formation `index`: saying nothing first —
/// which is "carry on" under orders and "choose for yourselves" without —
/// then, under orders, handing her back her own judgment, then holding
/// where she stands, then for every objective nearest her
/// first — each named by its hex nearest her — an advance, an assault, a
/// hold and a reconnaissance on it, then a
/// base of fire for each other platoon of her side still on the field. An
/// order equal to her standing one is not offered twice.
pub fn options(state: &BattleState, side: u8, index: usize) -> Vec<Choice> {
    let Some((_, at)) = where_is(state, index) else {
        return Vec::new();
    };
    let current = state
        .command
        .formations()
        .get(index)
        .and_then(|f| f.latest_mission().cloned());
    let mut out = vec![Choice::Keep];
    // Handing her back her judgment is only news to a platoon under orders;
    // without any, it is saying nothing.
    if current.is_some() {
        out.push(Choice::Release);
    }
    let mut push = |mission: Mission| {
        if current.as_ref() != Some(&mission) {
            out.push(Choice::Give(mission));
        }
    };
    push(Mission::Hold { at: None });
    // Each objective by the hex of it nearest her, never by its anchor: the
    // anchor is the first hex a map file lists, so a platoon and her mirror
    // twin would be sent to ground at different distances from each of
    // them, which is the compass in another coat. Her standing order's hex
    // is kept when it lies in the objective, so carrying on is not offered
    // as a new order every time she has moved.
    let standing = current.as_ref().and_then(|m| ground_of(state, m));
    let mut ground: Vec<Hex> = state
        .objectives()
        .filter(|(o, _)| o.kind == ObjectiveKind::Hold && o.open_to(side))
        .filter_map(|(o, _)| match standing {
            Some(hex) if o.contains(hex) => Some(hex),
            _ => o
                .hexes
                .iter()
                .copied()
                .min_by_key(|h| (h.distance_to(at), h.x, h.y)),
        })
        .collect();
    ground.sort_by_key(|hex| (hex.distance_to(at), hex.x, hex.y));
    for to in ground {
        push(Mission::Advance { to });
        push(Mission::Assault { to });
        push(Mission::Hold { at: Some(to) });
        push(Mission::Recon { toward: to });
    }
    for (other, formation) in state.command.formations().iter().enumerate() {
        if other == index || formation.side != side || where_is(state, other).is_none() {
            continue;
        }
        push(Mission::Support {
            formation: formation.id.clone(),
        });
    }
    out
}

fn flag(b: bool) -> f32 {
    if b { 1.0 } else { 0.0 }
}

/// Which decision is being described: whose platoon, against which plan,
/// how many of her platoons were decided before it this review, and the
/// doctrine its crews will carry the order out under.
pub struct Asking<'a> {
    pub side: u8,
    pub index: usize,
    pub plan: &'a Plan,
    pub decided_before: usize,
    pub doctrine: &'a crate::data::DoctrineDef,
}

/// Describe formation `index`'s decision: her context and a row per option,
/// read against `plan` (this review so far).
pub fn describe(
    registry: &DataRegistry,
    state: &BattleState,
    asking: &Asking,
    options: &[Choice],
) -> GoalRecord {
    let Asking {
        side,
        index,
        plan,
        decided_before,
        doctrine,
    } = *asking;
    let formations = state.command.formations();
    let formation = &formations[index];
    let (leader, at) = where_is(state, index).unwrap_or((UnitId(0), Hex::ZERO));
    let alive: Vec<&Unit> = members(state, formation).collect();
    let (left, full) = strength(registry, state, index);
    let vehicles = alive.len() as f32;
    let share = |pick: &dyn Fn(&Unit) -> bool| {
        alive.iter().filter(|u| pick(u)).count() as f32 / vehicles.max(1.0)
    };
    let def = |u: &Unit| registry.vehicle(&u.vehicle);
    let foot =
        |u: &Unit| def(u).is_some_and(|v| v.movement.class == crate::data::MovementClass::Foot);
    let speed = alive
        .iter()
        .filter(|u| u.aboard.is_none())
        .filter_map(|u| def(u).map(|v| v.movement.points))
        .min()
        .unwrap_or(0) as f32;
    let reach_of = |u: &Unit| {
        def(u)
            .map(|v| {
                v.weapons
                    .iter()
                    .filter_map(|w| registry.weapon(w))
                    .filter(|w| !w.indirect)
                    .map(|w| w.range[1] as i32)
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0)
    };
    let reach = alive.iter().map(|u| reach_of(u)).max().unwrap_or(0);
    let known = state.known_enemies(registry, Knower::Commander(side));
    let nearest_to = |hex: Hex| {
        known
            .iter()
            .map(|e| e.pos.distance_to(hex))
            .min()
            .map_or(FAR, |d| (d as f32).min(FAR))
    };
    let in_contact = alive.iter().any(|u| {
        let r = reach_of(u);
        known.iter().any(|e| e.pos.distance_to(u.pos) <= r)
    });

    let side_full: f32 = (0..formations.len())
        .filter(|i| formations[*i].side == side)
        .map(|i| strength(registry, state, i).1)
        .sum::<f32>()
        .max(1.0);
    let side_left: f32 = (0..formations.len())
        .filter(|i| formations[*i].side == side)
        .map(|i| strength(registry, state, i).0)
        .sum();
    let known_left: f32 = known
        .iter()
        .map(|e| state.substance(registry, e).0 as f32)
        .sum();
    let victory = state.scenario.victory_score().unwrap_or(40).max(1) as f32;
    let (ours, theirs) = (state.score(side) as f32, state.score(1 - side) as f32);
    let held = |who: &dyn Fn(Option<u8>) -> bool| {
        state
            .objectives()
            .filter(|(o, h)| o.kind == ObjectiveKind::Hold && who(*h))
            .count() as f32
    };
    let assault = state.scenario.assault();
    // What a side can still lose before it breaks, as a share of its
    // strength: the whole of what it has left when it has no break point.
    let break_margin = |who: u8| {
        let lost = state.strength_lost(registry, who) as f32 / 1000.0;
        let point = state.scenario.break_point(who).unwrap_or(100) as f32 / 100.0;
        point - lost
    };
    let platoons = (0..formations.len())
        .filter(|i| formations[*i].side == side && where_is(state, *i).is_some())
        .count() as f32;
    let context = vec![
        vehicles,
        left / full.max(1.0),
        full,
        speed,
        reach as f32,
        share(&|u| def(u).is_some_and(|v| v.armor.front >= 3) && !foot(u)),
        share(&foot),
        share(&|u| def(u).is_some_and(|v| v.capacity > 0)),
        flag(alive.iter().any(|u| {
            def(u).is_some_and(|v| {
                v.weapons
                    .iter()
                    .any(|w| registry.weapon(w).is_some_and(|w| w.indirect))
            })
        })),
        SideCommand::eyes_share(registry, state, formation),
        flag(formation.latest_mission().is_some()),
        nearest_to(at),
        flag(in_contact),
        doctrine.aggression,
        doctrine.initiative,
        doctrine.delegation,
        state.round as f32,
        side_left / side_full,
        known.len() as f32,
        known_left / side_full,
        (ours - theirs) / victory,
        ours / victory,
        theirs / victory,
        held(&|h| h == Some(side)),
        held(&|h| h.is_some_and(|h| h != side)),
        platoons,
        decided_before as f32,
        flag(assault.is_some_and(|a| a.attacker == side)),
        flag(assault.is_some_and(|a| a.attacker != side)),
        assault.map_or(0.0, |a| a.needs as f32 - held(&|h| h == Some(a.attacker))),
        break_margin(side),
        break_margin(1 - side),
    ];
    debug_assert_eq!(context.len(), CONTEXT.len());

    let current = formation.latest_mission();
    let candidates = options
        .iter()
        .map(|choice| {
            // Saying nothing leaves her on what she was told.
            let mission = match choice {
                Choice::Keep => current,
                Choice::Release => None,
                Choice::Give(mission) => Some(mission),
            };
            let hold_here = flag(matches!(mission, Some(Mission::Hold { at: None })));
            let hold_at = flag(matches!(mission, Some(Mission::Hold { at: Some(_) })));
            let target = mission.and_then(|m| ground_of(state, m));
            let objective = mission.and_then(|m| objective_of(state, plan, m));
            let (value, mine, theirs_held) = objective
                .and_then(|i| state.objectives().nth(i))
                .map_or((0.0, 0.0, 0.0), |(o, h)| {
                    (
                        o.value as f32,
                        flag(h == Some(side)),
                        flag(h.is_some_and(|h| h != side)),
                    )
                });
            let distance = target.map_or(0.0, |t| t.distance_to(at) as f32);
            let near: Vec<&&Unit> = target
                .map(|t| {
                    known
                        .iter()
                        .filter(|e| e.pos.distance_to(t) <= NEAR_OBJECTIVE)
                        .collect()
                })
                .unwrap_or_default();
            let near_strength: f32 = near
                .iter()
                .map(|e| state.substance(registry, e).0 as f32)
                .sum::<f32>()
                / full.max(1.0);
            let fire = target.map_or(0.0, |t| {
                incoming(registry, state, leader, t).worth
                    / state
                        .unit(leader)
                        .map_or(1.0, |u| state.substance(registry, u).0.max(1) as f32)
            });
            let friends_there = target.map_or(0.0, |t| {
                state
                    .side_units(side)
                    .filter(|u| u.aboard.is_none() && u.pos.distance_to(t) <= THERE)
                    .count() as f32
            });
            // Who else is bound for the same objective, and from which way.
            let (mut committed, mut committed_strength, mut converging) = (0.0, 0.0, 1.0f32);
            if let Some(objective) = objective {
                for (other, orders) in plan.iter().enumerate() {
                    if other == index || formations.get(other).is_none_or(|f| f.side != side) {
                        continue;
                    }
                    let Some(orders) = orders else { continue };
                    if objective_of(state, plan, orders) != Some(objective) {
                        continue;
                    }
                    let Some((_, there)) = where_is(state, other) else {
                        continue;
                    };
                    committed += 1.0;
                    committed_strength += strength(registry, state, other).0 / side_full;
                    let goal = target.unwrap_or(at);
                    converging = converging.min(cosine(goal - at, goal - there));
                }
            }
            let (supported_strength, supported_objective, supported_distance) = match mission {
                Some(Mission::Support { formation: id }) => {
                    let other = formations.iter().position(|f| f.id == *id);
                    match other {
                        Some(other) => (
                            strength(registry, state, other).0 / side_full,
                            flag(objective.is_some()),
                            where_is(state, other).map_or(FAR, |(_, p)| p.distance_to(at) as f32),
                        ),
                        None => (0.0, 0.0, FAR),
                    }
                }
                _ => (0.0, 0.0, 0.0),
            };
            let row = vec![
                flag(*choice == Choice::Keep),
                flag(mission.is_none()),
                hold_here,
                flag(matches!(mission, Some(Mission::Advance { .. }))),
                flag(matches!(mission, Some(Mission::Assault { .. }))),
                hold_at,
                flag(matches!(mission, Some(Mission::Recon { .. }))),
                flag(matches!(mission, Some(Mission::Support { .. }))),
                flag(objective.is_some()),
                value,
                mine,
                theirs_held,
                distance,
                distance / speed.max(1.0),
                near.len() as f32,
                near_strength,
                target.map_or(FAR, nearest_to),
                fire,
                friends_there,
                committed,
                committed_strength,
                converging,
                supported_strength,
                supported_objective,
                supported_distance,
            ];
            debug_assert_eq!(row.len(), CANDIDATE.len());
            row
        })
        .collect();
    GoalRecord {
        round: state.round,
        unit: index as u32,
        context,
        candidates,
        chosen: None,
        side,
        logp: None,
    }
}

/// Every context feature's name, in order, for a network's contract.
pub fn context_names() -> Vec<String> {
    CONTEXT.iter().map(|s| s.to_string()).collect()
}

/// Read a trained commander network and check it against the features this
/// engine describes.
pub fn load_command_net(
    path: &std::path::Path,
) -> Result<std::sync::Arc<crate::data::GoalNet>, crate::data::NetError> {
    let net = crate::data::GoalNet::from_path(path)?;
    net.check(&context_names(), CANDIDATE)?;
    Ok(std::sync::Arc::new(net))
}
