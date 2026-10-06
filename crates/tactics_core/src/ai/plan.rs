//! Plans: a commander matching a template she knows onto the ground in front
//! of her. PLANNING.md step 4.
//!
//! **Who plans** is each operational command's commander
//! ([`BattleState::operational_commands`]) — the senior cadet her orders can
//! reach from — not a brain for the whole side. **What she can plan** is her
//! repertoire: the templates her academy teaches ([`DoctrineDef::teaches`])
//! and the ones she knows herself ([`crate::roster::Cadet::templates`]).
//! **How well she plans** is her own stats, the designer's ruling
//! ([`Strength`]): her `command` skill decides how many options she weighs
//! and how exactly she judges them, and her `will` how stubbornly she holds a
//! plan once she has one.
//!
//! **Plans change.** Each review re-checks what the plan rests on — the
//! commander still commands both formations, neither is beaten, the target is
//! still known — and re-scores it against the ground as it now stands; a new
//! plan replaces it only if it beats that by her commitment. And every round,
//! whether or not she reviews, the plan's trigger is checked: the word to go is
//! given when the manoeuvre element is at its assembly point and the fixing
//! element is in place or engaged, or at once if the flankers are found.
//!
//! **What a plan produces is ordinary orders**: missions on the net at its
//! ordinary speed, and a [`Plan`] record on the battle so a saved battle
//! remembers what she meant. Nothing in the engine obeys a plan; formations
//! obey missions.

use crate::battle::{
    BattleState, FormationId, Knower, Latitude, Mission, OperationalCommand, Order, Plan,
    PlanPhase, Unit, UnitId, move_points, struck_facing, threatened,
};
use crate::data::{
    AVERAGE, ArmorFacing, CheckContext, DataRegistry, DoctrineDef, FixAndFlank, TemplateDef,
    TemplateKind,
};
use crate::ground::{Area, CoveredRoutes, firing_positions, watched};
use crate::roster::CadetId;
use hexx::Hex;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;

/// The skill a commander plans with. An id rather than data for the reason
/// `signals` is one: skill ids are the engine's contract with the base mod.
pub const PLAN_SKILL: &str = "command";
/// The core that decides how stubbornly she holds a plan.
pub const RESOLVE_CORE: &str = "will";

/// How well a commander plans, from who she is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strength {
    /// Her level in [`PLAN_SKILL`].
    pub level: i32,
    /// How many options she weighs for each role.
    pub breadth: usize,
    /// How far she may misjudge an option's worth, either way, in plan-score
    /// units.
    pub misjudge: f32,
    /// How much better a new plan must look before she drops the one she has.
    pub commitment: f32,
    /// The same, for a plan that has been played out, in position value.
    pub playout_margin: f32,
    /// How far she misreads a playout's result, either way.
    pub playout_noise: f32,
}

impl Strength {
    /// The strength of the cadet commanding from `commander`'s hull: the
    /// senior cadet aboard and still fighting (the one who gives the hull its
    /// rank), or an average officer for a hull nobody named.
    pub fn of(registry: &DataRegistry, state: &BattleState, commander: UnitId) -> Self {
        let p = &registry.planner;
        let cadet = commanding_cadet(registry, state, commander);
        let level = cadet
            .and_then(|c| {
                let unit = state.unit(commander)?;
                let vehicle = registry.vehicle(&unit.vehicle);
                let ctx = CheckContext {
                    terrain: state.terrain_at(unit.pos),
                    vehicle_class: vehicle.map(|v| v.class.as_str()),
                    crew_size: unit.crew.len(),
                };
                state.roster.skill_level(registry, c, PLAN_SKILL, &ctx)
            })
            .unwrap_or(AVERAGE);
        let will = cadet
            .and_then(|c| state.roster.get(c))
            .and_then(|c| {
                registry
                    .core_index
                    .get(RESOLVE_CORE)
                    .and_then(|i| c.cores.get(i).copied())
            })
            .unwrap_or(AVERAGE);
        Self {
            level,
            breadth: (1 + (level - p.plan_breadth_base).max(0) / p.plan_breadth_step.max(1))
                as usize,
            misjudge: (p.plan_sure_level - level).max(0) as f32 * p.plan_misjudge_per_level,
            commitment: p.plan_commitment * will as f32 / AVERAGE as f32,
            playout_margin: p.playout_margin * will as f32 / AVERAGE as f32,
            playout_noise: (p.plan_sure_level - level).max(0) as f32 * p.playout_noise_per_level,
        }
    }
}

/// The cadet commanding from a hull: the highest rank aboard and fighting,
/// the first seat among equals.
pub fn commanding_cadet(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
) -> Option<CadetId> {
    let u = state.unit(unit)?;
    let mut best: Option<(CadetId, Option<usize>)> = None;
    for (i, id) in u.crew.iter().enumerate() {
        if !u.crew_state.get(i).is_none_or(|c| c.fighting()) {
            continue;
        }
        let Some(cadet) = state.roster.get(*id) else {
            continue;
        };
        let rank = registry.rank_index(cadet.rank.as_deref());
        if best.is_none_or(|(_, b)| rank > b) {
            best = Some((*id, rank));
        }
    }
    best.map(|(id, _)| id)
}

/// The templates `commander` knows: what her academy teaches and what she
/// has learned herself, sorted by id so the order they are tried in is the
/// same on every machine.
pub fn repertoire<'r>(
    registry: &'r DataRegistry,
    state: &BattleState,
    academy: &DoctrineDef,
    commander: UnitId,
) -> Vec<&'r TemplateDef> {
    let own: Vec<String> = commanding_cadet(registry, state, commander)
        .and_then(|c| state.roster.get(c))
        .map(|c| c.templates.clone())
        .unwrap_or_default();
    let mut ids: Vec<&String> = academy.teaches.iter().chain(own.iter()).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.into_iter()
        .filter_map(|id| registry.template(id))
        .collect()
}

/// A plan worth considering, with the orders that would carry it out.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub plan: Plan,
    /// What she thinks it is worth, misjudgement included.
    pub score: f32,
    /// Where the manoeuvre element drives on its way to the assembly point.
    pub waypoints: Vec<Hex>,
}

fn leader(state: &BattleState, formation: usize) -> Option<&Unit> {
    state
        .formations()
        .get(formation)?
        .leader
        .and_then(|id| state.unit(id))
}

/// Furthest a hull's direct guns reach, in hexes.
fn direct_range(registry: &DataRegistry, unit: &Unit) -> u32 {
    registry
        .vehicle(&unit.vehicle)
        .map(|v| {
            v.weapons
                .iter()
                .filter_map(|w| registry.weapon(w))
                .filter(|w| !w.indirect)
                .map(|w| w.range[1])
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

fn speed(registry: &DataRegistry, state: &BattleState, unit: &Unit) -> u32 {
    move_points(registry, &state.roster, unit, state.terrain_at(unit.pos)).max(1)
}

/// Whether a hex is off the target's frontal arc: a flank at all.
fn off_the_front(target: &Unit, from: Hex) -> bool {
    struck_facing(target.pos, target.facing, from) != ArmorFacing::Front
}

/// What a round of `attacker`'s fire from `from` is worth against `target`
/// where he stands: the resolver's own arithmetic, the currency every other
/// decision is priced in.
fn worth_from(
    registry: &DataRegistry,
    state: &BattleState,
    attacker: UnitId,
    from: Hex,
    target: &Unit,
) -> f32 {
    crate::battle::best_weapon_from(registry, state, attacker, from, target.id, target.pos)
        .map(|(_, v)| v.worth_per_round())
        .unwrap_or(0.0)
}

/// Everybody in formation `index` still fighting, passengers included: the
/// fixing element is what the formation can put on him, not what its
/// leader can. A grenadier section is led by her halftrack, whose belt the
/// proper-equipment rule leaves worthless against a tank, while the RPGs
/// ride in the back — priced by the leader alone, a platoon never fixed
/// anything.
fn element(state: &BattleState, index: usize) -> Vec<&Unit> {
    state
        .formations()
        .get(index)
        .map(|f| {
            f.members
                .iter()
                .filter_map(|id| state.unit(*id))
                .filter(|u| u.alive())
                .collect()
        })
        .unwrap_or_default()
}

/// The best round of fire anybody in the element could put on `target` from
/// `from`.
fn element_worth(
    registry: &DataRegistry,
    state: &BattleState,
    element: &[&Unit],
    from: Hex,
    target: &Unit,
) -> f32 {
    element
        .iter()
        .map(|u| worth_from(registry, state, u.id, from, target))
        .fold(0.0, f32::max)
}

/// Every enemy she knows of, as an observer: where he is and how far he sees.
fn observers(registry: &DataRegistry, state: &BattleState, side: u8) -> Vec<(Hex, u32)> {
    state
        .known_enemies(registry, Knower::Commander(side))
        .into_iter()
        .map(|e| {
            (
                e.pos,
                registry
                    .vehicle(&e.vehicle)
                    .map(|v| v.vision_range)
                    .unwrap_or(0),
            )
        })
        .collect()
}

/// The enemy a fix-and-flank would be aimed at: of those she has been told
/// about, the one nearest the ground she is meant to take (`goal`) — a plan
/// serves the mission, it does not replace it — or, with no ground to take,
/// the one nearest her leaders. Lowest id among equals.
pub fn target(
    registry: &DataRegistry,
    state: &BattleState,
    side: u8,
    command: &OperationalCommand,
    goal: Option<Hex>,
) -> Option<UnitId> {
    let ours: Vec<Hex> = match goal {
        Some(at) => vec![at],
        None => command
            .leaders
            .iter()
            .filter_map(|id| state.unit(*id))
            .map(|u| u.pos)
            .collect(),
    };
    state
        .known_enemies(registry, Knower::Commander(side))
        .into_iter()
        .map(|e| {
            let d = ours
                .iter()
                .map(|p| p.distance_to(e.pos))
                .min()
                .unwrap_or(i32::MAX);
            (d, e.id.index(), e.id)
        })
        .min()
        .map(|(_, _, id)| id)
}

/// The best fix-and-flank she can see against `target`, fixing with one of
/// `fixers` and going round with one of `movers`, or `None` when nothing
/// clears the template's threshold. The cheap model's own verdict; see
/// [`candidates`] for the list a playout chooses from.
#[allow(clippy::too_many_arguments)]
pub fn fix_and_flank(
    registry: &DataRegistry,
    state: &BattleState,
    side: u8,
    commander: UnitId,
    template: &TemplateDef,
    params: &FixAndFlank,
    fixers: &[usize],
    movers: &[usize],
    target: UnitId,
    strength: Strength,
    rng: &mut ChaCha8Rng,
) -> Option<Candidate> {
    candidates(
        registry, state, side, commander, template, params, fixers, movers, target, strength, rng,
    )
    .into_iter()
    .next()
    .filter(|c| c.score >= params.threshold)
}

/// Every fix-and-flank she can see against `target`, fixing with one of
/// `fixers` (a platoon may hold a firing position) and going round with one
/// of `movers` (only what can drive goes round) — every ordered pair, each fixing formation's best firing
/// positions and each manoeuvre formation's best flanks, as many of each as
/// her breadth allows — scored by the cheap model, misjudgement included,
/// best first, and as many kept as her breadth allows. The model nominates;
/// a playout ([`playout`]) decides.
#[allow(clippy::too_many_arguments)]
pub fn candidates(
    registry: &DataRegistry,
    state: &BattleState,
    side: u8,
    commander: UnitId,
    template: &TemplateDef,
    params: &FixAndFlank,
    fixers: &[usize],
    movers: &[usize],
    target: UnitId,
    strength: Strength,
    rng: &mut ChaCha8Rng,
) -> Vec<Candidate> {
    let Some(tgt) = state.unit(target) else {
        return Vec::new();
    };
    let watchers = observers(registry, state, side);
    let [near, far] = params.standoff;
    let mut found: Vec<Candidate> = Vec::new();
    for &f in fixers {
        for &m in movers {
            if f == m {
                continue;
            }
            let (Some(fl), Some(ml)) = (leader(state, f), leader(state, m)) else {
                continue;
            };
            let (f_speed, m_speed) = (speed(registry, state, fl), speed(registry, state, ml));
            // Where the fixing element could shoot from, within two rounds'
            // drive and no nearer than the flank's own standoff: a fix is a
            // base of fire, not an assault.
            let reach = Area::of(
                fl.pos
                    .range(f_speed * 2)
                    .filter(|h| h.distance_to(tgt.pos) >= near as i32),
            );
            let onto = Area::around([tgt.pos], 1);
            let fixing = element(state, f);
            let reach_of = fixing
                .iter()
                .map(|u| direct_range(registry, u))
                .max()
                .unwrap_or(0);
            // Positions the element can hurt him from at all, best first: a
            // platoon's RPG reaches three hexes where her carrier's belt
            // reaches six, and the six are no use against plate.
            let fires: Vec<(Hex, u32, i32)> =
                firing_positions(registry, state, &reach, &onto, reach_of)
                    .into_iter()
                    .filter(|(h, _, _)| element_worth(registry, state, &fixing, *h, tgt) > 0.0)
                    .take(strength.breadth)
                    .collect();
            if fires.is_empty() {
                continue;
            }
            let Some(vehicle) = registry.vehicle(&ml.vehicle) else {
                continue;
            };
            let spread = ml.pos.distance_to(tgt.pos).max(far as i32) as u32 + 3;
            let area = Area::around([ml.pos, tgt.pos], spread);
            let routes = CoveredRoutes::search(
                state,
                vehicle.movement.class,
                vehicle.movement.max_climb,
                ml.pos,
                &watchers,
                &area,
                params.exposure_price,
            );
            // Every hex off his frontal arc in reach, with what going round to
            // it would gain over standing in the line — which is worth
            // judging from the fixing position, where the manoeuvre element
            // would otherwise be.
            let mut flanks: Vec<(Hex, f32, crate::ground::Route)> = tgt
                .pos
                .range(far)
                .filter(|h| h.distance_to(tgt.pos) >= near as i32 && off_the_front(tgt, *h))
                .filter_map(|h| {
                    let route = routes.to(h)?;
                    let there = worth_from(registry, state, ml.id, h, tgt);
                    (there > 0.0).then_some((h, there, route))
                })
                .collect();
            let net = |there: f32, route: &crate::ground::Route| {
                params.rounds_of_fire * there
                    - params.per_hex_exposed * route.exposed as f32
                    - params.per_round * route.cost as f32 / m_speed as f32
            };
            // Best flanks first by what they are worth net of getting there,
            // the coordinate last.
            flanks.sort_by(|a, b| {
                net(b.1, &b.2)
                    .total_cmp(&net(a.1, &a.2))
                    .then(a.0.x.cmp(&b.0.x))
                    .then(a.0.y.cmp(&b.0.y))
            });
            flanks.truncate(strength.breadth);
            for &(fire, _, _) in &fires {
                // The fix must be able to hurt him from there, or it fixes
                // nothing; and the flank is worth what it gains over the
                // manoeuvre element standing here beside it.
                if element_worth(registry, state, &fixing, fire, tgt) <= 0.0 {
                    continue;
                }
                let in_line = worth_from(registry, state, ml.id, fire, tgt);
                // What the fixing element will take standing there alone.
                let alone = crate::battle::incoming(registry, state, fl.id, fire).worth;
                // And what standing there holds: a fix on the objective is
                // also holding it, which the enemy scores if it is not.
                let holds = holding(registry, state, fire);
                for (flank, there, route) in &flanks {
                    let rounds_f = fl.pos.distance_to(fire) as f32 / f_speed as f32;
                    let rounds_m = route.cost as f32 / m_speed as f32;
                    let mut score = params.rounds_of_fire * (there - in_line)
                        - params.per_hex_exposed * route.exposed as f32
                        - params.per_round * rounds_f.max(rounds_m)
                        - params.alone_share * alone * rounds_m
                        + params.rounds_of_fire * holds;
                    if strength.misjudge > 0.0 {
                        score += rng.random_range(-strength.misjudge..=strength.misjudge);
                    }
                    let (assembly, waypoints) = assembly(state, &watchers, tgt.pos, far, route);
                    found.push(Candidate {
                        plan: Plan {
                            side,
                            commander,
                            template: template.id.clone(),
                            fix: FormationId(f as u32),
                            manoeuvre: FormationId(m as u32),
                            target,
                            fire_position: fire,
                            assembly,
                            flank: *flank,
                            phase: PlanPhase::Forming,
                            score: (score * 100.0).round() as i32,
                            go_by: state.round + rounds_m.ceil() as u32 + 2,
                        },
                        score,
                        waypoints,
                    });
                }
            }
        }
    }
    // Best first; among equals the pairing, then the ground, coordinate last.
    found.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then(a.plan.fix.index().cmp(&b.plan.fix.index()))
            .then(a.plan.manoeuvre.index().cmp(&b.plan.manoeuvre.index()))
            .then(a.plan.flank.x.cmp(&b.plan.flank.x))
            .then(a.plan.flank.y.cmp(&b.plan.flank.y))
            .then(a.plan.fire_position.x.cmp(&b.plan.fire_position.x))
            .then(a.plan.fire_position.y.cmp(&b.plan.fire_position.y))
    });
    found.truncate(strength.breadth);
    found
}

/// Where the manoeuvre element waits for the word, and the waypoints that get
/// it there: the last hex of the route outside the flank's standoff that
/// nobody can see, or failing that the one two short of the end.
fn assembly(
    state: &BattleState,
    watchers: &[(Hex, u32)],
    target: Hex,
    far: u32,
    route: &crate::ground::Route,
) -> (Hex, Vec<Hex>) {
    let path = &route.path;
    let at = path
        .iter()
        .rposition(|h| h.distance_to(target) > far as i32 && !watched(state, watchers, *h))
        .unwrap_or(path.len().saturating_sub(3));
    let assembly = path[at.min(path.len() - 1)];
    let waypoints: Vec<Hex> = path[..=at.min(path.len() - 1)]
        .iter()
        .copied()
        .enumerate()
        .filter(|(i, _)| *i > 0 && i % 4 == 0)
        .map(|(_, h)| h)
        .filter(|h| *h != assembly)
        .collect();
    (assembly, waypoints)
}

/// Re-score a plan in flight against the ground as it now stands: its fixing
/// element to the same firing position, its manoeuvre element from where she
/// is now to the same flank. `None` when it no longer can be carried out.
pub fn rescore(
    registry: &DataRegistry,
    state: &BattleState,
    plan: &Plan,
    params: &FixAndFlank,
) -> Option<f32> {
    let tgt = state.unit(plan.target)?;
    let (fl, ml) = (
        leader(state, plan.fix.index())?,
        leader(state, plan.manoeuvre.index())?,
    );
    let watchers = observers(registry, state, plan.side);
    if worth_from(registry, state, fl.id, plan.fire_position, tgt) <= 0.0 {
        return None;
    }
    let in_line = worth_from(registry, state, ml.id, plan.fire_position, tgt);
    let there = if off_the_front(tgt, plan.flank) {
        worth_from(registry, state, ml.id, plan.flank, tgt)
    } else {
        // He has turned to face the flank: it is a front now.
        in_line
    };
    let vehicle = registry.vehicle(&ml.vehicle)?;
    let spread = ml.pos.distance_to(tgt.pos).max(params.standoff[1] as i32) as u32 + 3;
    let route = crate::ground::covered_route(
        state,
        vehicle.movement.class,
        vehicle.movement.max_climb,
        ml.pos,
        plan.flank,
        &watchers,
        &Area::around([ml.pos, tgt.pos], spread),
        params.exposure_price,
    )?;
    let rounds_f =
        fl.pos.distance_to(plan.fire_position) as f32 / speed(registry, state, fl) as f32;
    let rounds_m = route.cost as f32 / speed(registry, state, ml) as f32;
    let alone = crate::battle::incoming(registry, state, fl.id, plan.fire_position).worth;
    Some(
        params.rounds_of_fire * (there - in_line)
            - params.per_hex_exposed * route.exposed as f32
            - params.per_round * rounds_f.max(rounds_m)
            - params.alone_share * alone * rounds_m
            + params.rounds_of_fire * holding(registry, state, plan.fire_position),
    )
}

/// The orders that put a candidate plan into effect: the record, the fixing
/// element to its firing position and holding there, the manoeuvre element
/// along its waypoints to hold at the assembly point.
pub fn adopt(candidate: &Candidate) -> Vec<Order> {
    let plan = &candidate.plan;
    let mut orders = vec![Order::SetPlan {
        side: plan.side,
        commander: plan.commander,
        plan: Some(plan.clone()),
    }];
    let d = Latitude::Delegated;
    orders.push(Order::SetMission {
        formation: plan.fix,
        mission: Mission::Advance {
            to: plan.fire_position,
        },
        latitude: d,
    });
    orders.push(Order::QueueMission {
        formation: plan.fix,
        mission: Mission::Hold {
            at: Some(plan.fire_position),
        },
        latitude: d,
    });
    let mut legs = candidate.waypoints.iter().copied();
    let first = legs.next().unwrap_or(plan.assembly);
    orders.push(Order::SetMission {
        formation: plan.manoeuvre,
        mission: Mission::Advance { to: first },
        latitude: d,
    });
    for to in legs.chain((first != plan.assembly).then_some(plan.assembly)) {
        orders.push(Order::QueueMission {
            formation: plan.manoeuvre,
            mission: Mission::Advance { to },
            latitude: d,
        });
    }
    orders.push(Order::QueueMission {
        formation: plan.manoeuvre,
        mission: Mission::Hold {
            at: Some(plan.assembly),
        },
        latitude: d,
    });
    orders
}

/// Whether the word to go should be given now, and the orders that give it.
///
/// The manoeuvre element must be at its assembly point, and the fixing
/// element in place at its firing position or already engaged — the whole
/// idea is that the enemy is looking at the fix when the flank comes in. If
/// the flankers are found before that, surprise is gone and waiting only
/// gives him time: they go at once.
pub fn trigger(registry: &DataRegistry, state: &BattleState, plan: &Plan) -> Vec<Order> {
    if plan.phase != PlanPhase::Forming {
        return Vec::new();
    }
    let (Some(fl), Some(ml)) = (
        leader(state, plan.fix.index()),
        leader(state, plan.manoeuvre.index()),
    ) else {
        return Vec::new();
    };
    let fixed = fl.pos.distance_to(plan.fire_position) <= 1 || threatened(registry, state, fl.id);
    let gathered = ml.pos.distance_to(plan.assembly) <= 2;
    let found = threatened(registry, state, ml.id);
    let overdue = state.round >= plan.go_by;
    if !((gathered && fixed) || found || overdue) {
        return Vec::new();
    }
    vec![
        Order::SetPlan {
            side: plan.side,
            commander: plan.commander,
            plan: Some(Plan {
                phase: PlanPhase::Going,
                ..plan.clone()
            }),
        },
        Order::SetMission {
            formation: plan.manoeuvre,
            mission: Mission::Assault { to: plan.flank },
            latitude: Latitude::Delegated,
        },
    ]
}

/// What holding `at` is worth a round, in the currency: the value of every
/// piece of scoring ground it is part of, at the planner's exchange rate
/// (`planner.score_worth`).
fn holding(registry: &DataRegistry, state: &BattleState, at: Hex) -> f32 {
    state
        .scenario
        .objectives()
        .iter()
        .filter(|o| o.kind == crate::map::ObjectiveKind::Hold && o.contains(at))
        .map(|o| o.value as f32 * registry.planner.score_worth)
        .sum()
}

/// Whether a plan has run its course: the flankers are in, or the enemy it
/// was aimed at is gone.
pub fn done(state: &BattleState, plan: &Plan) -> bool {
    state.unit(plan.target).is_none()
        || plan.phase == PlanPhase::Going
            && leader(state, plan.manoeuvre.index())
                .is_some_and(|u| u.pos.distance_to(plan.flank) <= 1)
}

/// The world as `side`'s commander knows it: a copy of the battle with
/// every enemy she has not been told about taken off the board, and nobody
/// else's orders known ([`crate::ai::determinize`], narrowed from what her side
/// has spotted to what she has been told — [`Knower::Commander`]). Fresh
/// dice, so a playout cannot read the real battle's future.
pub fn known_world(
    registry: &DataRegistry,
    state: &BattleState,
    side: u8,
    seed: u64,
) -> BattleState {
    let told: Vec<UnitId> = state
        .known_enemies(registry, Knower::Commander(side))
        .iter()
        .map(|u| u.id)
        .collect();
    let mut world = crate::ai::determinize(state, side, seed);
    for unit in &mut world.units {
        if unit.side != side && unit.alive() && !told.contains(&unit.id) {
            unit.withdraw();
        }
    }
    world
}

/// What `orders` are worth to `side`, found by playing them out: the world as
/// her commander knows it, the orders given, and [`PlannerRules::playout_rounds`]
/// rounds of the real engine — her side under a commander who carries plans
/// out but makes none (`SideCommand::for_playout`), everybody else under the
/// ordinary executor — scored by [`crate::ai::Evaluator::position_value`]'s
/// material and ground (never the copy's verdict; see the body) and
/// averaged over [`PlannerRules::playout_samples`] dice.
///
/// **The battle is the evaluator.** The cheap model is a guess at what a plan
/// will do; this asks the game. It is search over *plans*, a handful of
/// playouts a review — not over moves, which is what MCTS tried and parked
/// with a weak evaluator at its leaves.
///
/// [`PlannerRules::playout_rounds`]: crate::data::PlannerRules::playout_rounds
/// [`PlannerRules::playout_samples`]: crate::data::PlannerRules::playout_samples
pub fn playout(
    registry: &DataRegistry,
    state: &BattleState,
    side: u8,
    orders: &[Order],
    ours: &crate::ai::AiConfig,
    seed: u64,
) -> f32 {
    let p = &registry.planner;
    let samples = p.playout_samples.max(1);
    let evaluator = crate::ai::Evaluator::new(crate::ai::resolve_doctrine(ours, registry));
    let mut total = 0.0;
    for sample in 0..samples {
        let dice = seed ^ (sample as u64 + 1).wrapping_mul(0xA24B_AED4_963E_E407);
        let mut world = known_world(registry, state, side, dice);
        for order in orders {
            let _ = world.apply(registry, order);
        }
        let mut driver = crate::ai::AiDriver::new();
        for other in 0..world.sides.len() as u8 {
            if other == side {
                driver.insert(
                    other,
                    Box::new(crate::ai::SideCommand::for_playout(ours, dice, registry)),
                );
            } else {
                let theirs = crate::ai::AiConfig {
                    planner: "utility".into(),
                    difficulty: 3,
                    doctrine: None,
                };
                driver.insert(
                    other,
                    crate::ai::make_battle_planner(&theirs, dice ^ other as u64, registry),
                );
            }
        }
        for _ in 0..p.playout_rounds {
            if world.is_over() {
                break;
            }
            driver.plan_round(registry, &mut world);
            world.resolve_round(registry);
        }
        // Material and ground, never the verdict. The copy holds only the
        // enemies she has been told about, so its battle can "end" the
        // moment they are gone — and did: the first playouts scored almost
        // every option as a won battle, 1.0, and could tell none of them
        // apart. What the options leave behind is what differs.
        world.over = None;
        total += evaluator.position_value(registry, &world, side);
    }
    total / samples as f32
}

/// A deterministic stream for one commander's misjudgements at one review.
pub fn judgement_rng(seed: u64, round: u32, commander: UnitId) -> ChaCha8Rng {
    ChaCha8Rng::seed_from_u64(
        seed ^ (round as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (commander.index() as u64 + 1).wrapping_mul(0xD1B5_4A32_D192_ED03),
    )
}

/// The fix-and-flank parameters of a template, if that is what it is.
pub fn as_fix_and_flank(template: &TemplateDef) -> Option<&FixAndFlank> {
    match &template.kind {
        TemplateKind::FixAndFlank(p) => Some(p),
    }
}
