//! Two forces staged on a battlefield (REIMAGINED.md, *Infantry*).
//!
//! A [`ForceDef`] says what a side brings; this puts it on the ground. Each
//! side's vehicles take that side's deployment hexes in the order the force
//! lists them, and any beyond the deployment stand on the nearest free
//! ground they can stand on — nearest the side's own deployment, then
//! furthest from the enemy's, then the coordinate. Passengers start aboard
//! the vehicle that carries them. The crews are anonymous: these battles are
//! for measuring and training, not for the academy roll.

use crate::Hex;
use crate::battle::{BattleState, SideState};
use crate::data::{DataRegistry, ForceDef};
use crate::map::{Battlefield, UnitPlacement};
use crate::roster::Roster;

/// Stage `forces[0]` against `forces[1]` on `field`, deploying on
/// `deployment` (pairs of side 0's hex and side 1's). `None` when a vehicle
/// finds nowhere to stand.
pub fn stage(
    reg: &DataRegistry,
    field: Battlefield,
    deployment: &[(Hex, Hex)],
    forces: [&ForceDef; 2],
    seed: u64,
) -> Option<BattleState> {
    let mut taken: Vec<Hex> = Vec::new();
    let mut placements: Vec<UnitPlacement> = Vec::new();
    for (side, force) in forces.iter().enumerate() {
        let side = side as u8;
        let ours: Vec<Hex> = deployment
            .iter()
            .map(|(a, b)| if side == 0 { *a } else { *b })
            .collect();
        let theirs: Vec<Hex> = deployment
            .iter()
            .map(|(a, b)| if side == 0 { *b } else { *a })
            .collect();
        let near = |h: Hex| ours.iter().map(|o| o.distance_to(h)).min().unwrap_or(0);
        let far = |h: Hex| theirs.iter().map(|o| o.distance_to(h)).min().unwrap_or(0);
        let mut slots = ours.clone().into_iter();
        let mut index = 0;
        for line in &force.units {
            let vehicle = reg.vehicle(&line.vehicle)?;
            let class = vehicle.movement.class;
            let stands = |h: Hex| {
                field
                    .terrain
                    .get(h)
                    .and_then(|t| reg.terrain(t.terrain))
                    .and_then(|t| t.cost_for(class))
                    .is_some()
            };
            for _ in 0..line.count {
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
                    formation: None,
                    leads: false,
                });
                if let Some(rider) = &line.carries {
                    placements.push(UnitPlacement {
                        aboard_at: Some(crate::hex_to_offset(hex)),
                        at: crate::hex_to_offset(hex),
                        side,
                        vehicle: rider.clone(),
                        crew: Vec::new(),
                        name: Some(format!("{name}'s squad")),
                        facing: None,
                        formation: None,
                        leads: false,
                    });
                }
            }
        }
    }
    let sides = forces
        .iter()
        .map(|f| SideState {
            name: f.name.clone(),
            ai: None,
        })
        .collect();
    let (roster, crews) = Roster::stamp_for(reg, &placements);
    BattleState::from_placements(
        reg,
        field,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
    .ok()
}
