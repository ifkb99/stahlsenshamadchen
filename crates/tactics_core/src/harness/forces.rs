//! Two forces staged on a battlefield (REIMAGINED.md, *Infantry*).
//!
//! A [`ForceDef`] says what a side brings; this puts it on the ground. Each
//! side's vehicles take that side's deployment hexes in the order the force
//! lists them, and any beyond the deployment stand on the nearest free
//! ground they can stand on — nearest the side's own deployment, then
//! furthest from the enemy's, then the coordinate. Passengers start aboard
//! the vehicle that carries them. The crews are anonymous: these battles are
//! for measuring and training, not for the academy roll.
//!
//! Each platoon of a force (a line, or a share of one: `ForceUnit::platoons`)
//! is a formation, led by its first vehicle, its passengers in it with their
//! carriers; the formations are declared side 0's first, in deployment
//! order, which is their seniority.

use crate::Hex;
use crate::battle::{BattleState, Muster, SideState, UnitId};
use crate::data::{DataRegistry, ForceDef};
use crate::map::{Battlefield, FormationDef, Scenario, UnitPlacement};
use crate::roster::Roster;
use crate::world::World;
use crate::worldgen::GeneratedWorld;

/// Stage `forces[0]` against `forces[1]` on `field`, deploying on
/// `deployment` (pairs of side 0's hex and side 1's). `None` when a vehicle
/// finds nowhere to stand.
pub fn stage(
    reg: &DataRegistry,
    mut field: Battlefield,
    deployment: &[(Hex, Hex)],
    forces: [&ForceDef; 2],
    seed: u64,
) -> Option<BattleState> {
    let ground = |h: Hex| field.terrain.get(h).map(|t| t.terrain.to_string());
    let (placements, formations) = placements(reg, &ground, &unzip(deployment), forces)?;
    for formation in formations {
        field.scenario.push_formation(formation);
    }
    let (roster, crews) = Roster::stamp_for(reg, &placements);
    BattleState::from_placements(
        reg,
        field,
        sides(forces),
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
    .ok()
}

/// [`stage`] on a window onto a whole generated world (`harness::open`):
/// the battle loads the ground round wherever its crews are and forgets
/// the rest, so the two forces can start further apart than one battle map.
pub fn stage_on_world(
    reg: &DataRegistry,
    world: std::sync::Arc<GeneratedWorld>,
    scenario: Scenario,
    deployment: &[(Hex, Hex)],
    forces: [&ForceDef; 2],
    seed: u64,
) -> Option<BattleState> {
    stage_sides_on_world(reg, world, scenario, &unzip(deployment), forces, &[], seed)
}

/// Each side's hexes of a list of pairs, side 0's first.
fn unzip(deployment: &[(Hex, Hex)]) -> [Vec<Hex>; 2] {
    let (a, b) = deployment.iter().copied().unzip();
    [a, b]
}

/// [`stage_on_world`] with each side's deployment its own list, as it is
/// when the two sides are not facing each other across a line but one is
/// standing on ground the other is coming to take (`harness::sector`).
/// Each list is that side's wanted hexes, in the order its force deploys;
/// `obstacles` are tiles written over the world's for this battle alone
/// (`World::edit`), the world itself being shared.
pub fn stage_sides_on_world(
    reg: &DataRegistry,
    world: std::sync::Arc<GeneratedWorld>,
    mut scenario: Scenario,
    deployment: &[Vec<Hex>; 2],
    forces: [&ForceDef; 2],
    obstacles: &[(Hex, String)],
    seed: u64,
) -> Option<BattleState> {
    // The ground as the battle will have it: the world's, with whatever a
    // defender dug or strung written over it, so nobody deploys into a
    // ditch.
    let ground = |h: Hex| {
        obstacles
            .iter()
            .find(|(o, _)| *o == h)
            .map(|(_, t)| t.clone())
            .or_else(|| world.tile(h).map(|(id, _)| id.to_string()))
    };
    // A side starting in prepared positions had the hours to get off its
    // carriers: its riflemen start on the ground, where a trench is, rather
    // than aboard a hull that one shell empties.
    let dismounted = [scenario.prepared(0), scenario.prepared(1)];
    let (placements, formations) = placements_with(reg, &ground, deployment, forces, dismounted)?;
    for formation in formations {
        scenario.push_formation(formation);
    }
    let (roster, crews) = Roster::stamp_for(reg, &placements);
    let ids: Vec<UnitId> = (0..placements.len() as u32).map(UnitId).collect();
    let mut window = World::window_onto(world.clone());
    for (hex, terrain) in obstacles {
        let level = world.tile(*hex).map_or(0, |(_, l)| l);
        window.edit(reg, *hex, terrain, level);
    }
    BattleState::from_muster_on(
        reg,
        window,
        scenario,
        sides(forces),
        Muster {
            placements: &placements,
            crews: &crews,
            ids: &ids,
        },
        std::sync::Arc::new(roster),
        seed,
    )
    .ok()
}

fn sides(forces: [&ForceDef; 2]) -> Vec<SideState> {
    forces
        .iter()
        .map(|f| SideState {
            name: f.name.clone(),
            ai: None,
        })
        .collect()
}

/// Where every vehicle of both forces stands, `ground` saying what terrain
/// is where, and the platoons they stand in. `None` when a vehicle finds
/// nowhere to stand.
fn placements(
    reg: &DataRegistry,
    ground: &dyn Fn(Hex) -> Option<String>,
    deployment: &[Vec<Hex>; 2],
    forces: [&ForceDef; 2],
) -> Option<(Vec<UnitPlacement>, Vec<FormationDef>)> {
    placements_with(reg, ground, deployment, forces, [false, false])
}

/// [`placements`], with a side whose `dismounted` is set starting its
/// passengers on the ground on their carrier's hex instead of aboard.
fn placements_with(
    reg: &DataRegistry,
    ground: &dyn Fn(Hex) -> Option<String>,
    deployment: &[Vec<Hex>; 2],
    forces: [&ForceDef; 2],
    dismounted: [bool; 2],
) -> Option<(Vec<UnitPlacement>, Vec<FormationDef>)> {
    let mut taken: Vec<Hex> = Vec::new();
    let mut placements: Vec<UnitPlacement> = Vec::new();
    let mut formations: Vec<FormationDef> = Vec::new();
    for (side, force) in forces.iter().enumerate() {
        let side = side as u8;
        let ours: Vec<Hex> = deployment[usize::from(side)].clone();
        let theirs: Vec<Hex> = deployment[usize::from(side == 0)].clone();
        let near = |h: Hex| ours.iter().map(|o| o.distance_to(h)).min().unwrap_or(0);
        let far = |h: Hex| theirs.iter().map(|o| o.distance_to(h)).min().unwrap_or(0);
        let mut slots = ours.clone().into_iter();
        let mut index = 0;
        for line in &force.units {
            let vehicle = reg.vehicle(&line.vehicle)?;
            let class = vehicle.movement.class;
            let stands = |h: Hex| {
                ground(h)
                    .and_then(|t| reg.terrain(&t))
                    .and_then(|t| t.cost_for(class))
                    .is_some()
            };
            // The line's platoons, declared as it deploys, and which of
            // them each of its vehicles is in.
            let mut member_of: Vec<String> = Vec::new();
            for size in line.platoon_sizes() {
                let number = formations.iter().filter(|f| f.side == side).count() + 1;
                let id = format!("side{side}_platoon{number}");
                member_of.extend(std::iter::repeat_n(id.clone(), size as usize));
                formations.push(FormationDef {
                    id,
                    name: format!("{} platoon {number}", force.name),
                    side,
                    doctrine: None,
                });
            }
            for formation in &member_of {
                // The next deployment hex if it will do, else the nearest
                // free ground that will.
                let wanted = slots.next();
                let hex = match wanted {
                    Some(h) if stands(h) && !taken.contains(&h) => h,
                    _ => {
                        let from = wanted.unwrap_or(ours[0]);
                        let mut ring: Vec<Hex> = (0..8u32)
                            .flat_map(|r| {
                                if r == 0 {
                                    vec![from]
                                } else {
                                    from.ring(r).collect()
                                }
                            })
                            .filter(|h| stands(*h) && !taken.contains(h))
                            .collect();
                        ring.sort_by_key(|h| (near(*h), std::cmp::Reverse(far(*h)), h.x, h.y));
                        *ring.first()?
                    }
                };
                taken.push(hex);
                index += 1;
                let name = format!("{} {index}", force.name);
                placements.push(UnitPlacement {
                    aboard_at: None,
                    at: crate::hex_to_offset(hex),
                    side,
                    vehicle: line.vehicle.clone(),
                    crew: Vec::new(),
                    name: Some(name.clone()),
                    facing: None,
                    formation: Some(formation.clone()),
                    leads: false,
                });
                if let Some(rider) = &line.carries {
                    placements.push(UnitPlacement {
                        aboard_at: (!dismounted[usize::from(side)])
                            .then(|| crate::hex_to_offset(hex)),
                        at: crate::hex_to_offset(hex),
                        side,
                        vehicle: rider.clone(),
                        crew: Vec::new(),
                        name: Some(format!("{name}'s squad")),
                        facing: None,
                        formation: Some(formation.clone()),
                        leads: false,
                    });
                }
            }
        }
    }
    Some((placements, formations))
}
