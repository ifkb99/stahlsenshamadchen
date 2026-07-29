//! Combat resolution: accuracy, armor facings, terrain cover, elevation
//! advantage, blind fire, and counterattacks.

use super::{fog, stats, BattleState, Event, UnitId};
use crate::data::{ArmorFacing, DamageType, DataRegistry, TerrainDef, WeaponDef};
use hexx::Hex;
use rand::RngExt;

/// Which armor arc a shot from `attacker_pos` strikes on a target at
/// `target_pos` facing `facing`.
pub fn struck_facing(
    target_pos: Hex,
    target_facing: hexx::EdgeDirection,
    attacker_pos: Hex,
) -> ArmorFacing {
    let incoming = target_pos.main_direction_to(attacker_pos);
    let diff = (incoming.index() as i32 - target_facing.index() as i32).rem_euclid(6);
    match diff {
        0 | 1 | 5 => ArmorFacing::Front,
        2 | 4 => ArmorFacing::Side,
        _ => ArmorFacing::Rear,
    }
}

fn terrain_at<'r>(registry: &'r DataRegistry, state: &BattleState, pos: Hex) -> Option<&'r TerrainDef> {
    state
        .map
        .get(pos)
        .and_then(|tile| registry.terrain(&tile.terrain))
}

fn elevation_at(state: &BattleState, pos: Hex) -> i32 {
    state.map.get(pos).map(|t| t.elevation).unwrap_or(0)
}

/// Hit chance percentage, clamped to 5..=95 so nothing is ever certain.
/// `from` is passed explicitly so AI can evaluate hypothetical positions.
pub fn hit_chance(
    registry: &DataRegistry,
    state: &BattleState,
    attacker: UnitId,
    from: Hex,
    weapon: &WeaponDef,
    target_pos: Hex,
    blind: bool,
) -> i32 {
    let Some(att) = state.unit(attacker) else {
        return 0;
    };
    let dist = from.distance_to(target_pos).max(1);
    let mut chance = weapon.accuracy - weapon.accuracy_falloff * (dist - 1);
    chance += stats::gunnery(registry, att) * 3;
    if elevation_at(state, from) > elevation_at(state, target_pos) {
        chance += 10;
    }
    if let Some(terrain) = terrain_at(registry, state, target_pos) {
        chance -= terrain.cover / 2;
    }
    if blind {
        chance -= 40;
    }
    chance.clamp(5, 95)
}

/// Damage a hit would deal, before the RNG decides whether it hits.
pub fn raw_damage(
    registry: &DataRegistry,
    state: &BattleState,
    weapon: &WeaponDef,
    attacker_pos: Hex,
    target: UnitId,
) -> i32 {
    let Some(tgt) = state.unit(target) else {
        return 0;
    };
    let Some(vehicle) = registry.vehicle(&tgt.vehicle) else {
        return 0;
    };
    let facing = struck_facing(tgt.pos, tgt.facing, attacker_pos);
    let armor = vehicle.armor.value(facing);
    let effective_armor = match weapon.damage_type {
        DamageType::Kinetic => armor,
        DamageType::Explosive => armor / 2,
        DamageType::SmallArms => armor * 2,
    }
    .max(0);
    let pen = weapon.penetration.max(1) as f32;
    let mut dmg = weapon.damage as f32 * (pen / (pen + effective_armor as f32));
    if let Some(terrain) = terrain_at(registry, state, tgt.pos) {
        dmg *= (100 - terrain.cover) as f32 / 100.0;
    }
    if elevation_at(state, attacker_pos) > elevation_at(state, tgt.pos) {
        dmg *= 1.1;
    }
    (dmg.round() as i32).max(1)
}

/// Expected damage (hit chance x damage), the currency of AI scoring.
/// `from` is where the attacker would fire from (hypothetical or real).
pub fn expected_damage(
    registry: &DataRegistry,
    state: &BattleState,
    attacker: UnitId,
    from: Hex,
    weapon: &WeaponDef,
    target: UnitId,
    blind: bool,
) -> f32 {
    let Some(tgt) = state.unit(target) else {
        return 0.0;
    };
    let p = hit_chance(registry, state, attacker, from, weapon, tgt.pos, blind) as f32 / 100.0;
    p * raw_damage(registry, state, weapon, from, target) as f32
}

/// Resolve one shot from `attacker` at `target` (which is known to be at
/// `at`). Appends granular events; kills are marked here. Does not handle
/// counterattacks -- see [`resolve_attack`].
fn resolve_shot(
    registry: &DataRegistry,
    state: &mut BattleState,
    attacker: UnitId,
    weapon: &WeaponDef,
    target: UnitId,
    blind: bool,
    counter: bool,
    events: &mut Vec<Event>,
) {
    let (att_pos, tgt_pos) = match (state.unit(attacker), state.unit(target)) {
        (Some(a), Some(t)) => (a.pos, t.pos),
        _ => return,
    };
    events.push(Event::ShotFired {
        attacker,
        from: att_pos,
        at: tgt_pos,
        weapon: weapon.id.clone(),
        blind,
        counter,
    });

    let chance = hit_chance(registry, state, attacker, att_pos, weapon, tgt_pos, blind);
    let roll = state.rng.random_range(0..100);
    if roll >= chance {
        events.push(Event::ShotMissed {
            attacker,
            at: tgt_pos,
        });
        return;
    }

    let damage = raw_damage(registry, state, weapon, att_pos, target);
    let facing = {
        let tgt = state.unit(target).expect("target checked above");
        struck_facing(tgt.pos, tgt.facing, att_pos)
    };
    let tgt = state.unit_mut(target).expect("target checked above");
    tgt.hp -= damage;
    let remaining = tgt.hp;
    events.push(Event::ShotHit {
        attacker,
        target,
        damage,
        facing,
        remaining_hp: remaining.max(0),
    });
    if remaining <= 0 {
        let at = tgt.pos;
        tgt.alive = false;
        events.push(Event::UnitDestroyed { unit: target, at });
    }
}

/// Full attack resolution: the shot, the muzzle-flash reveal, and the
/// counterattack if the target survives and can answer.
pub fn resolve_attack(
    registry: &DataRegistry,
    state: &mut BattleState,
    attacker: UnitId,
    weapon: &WeaponDef,
    target: UnitId,
    blind: bool,
) -> Vec<Event> {
    let mut events = Vec::new();
    resolve_shot(registry, state, attacker, weapon, target, blind, false, &mut events);

    // Firing gives away your position.
    fog::reveal_to_all(state, attacker);
    fog::recompute(registry, state);

    // Counterattack: the target answers with its first weapon that can
    // reach, requiring line of sight (indirect weapons don't snap-fire).
    if let (Some(att), Some(tgt)) = (state.unit(attacker), state.unit(target)) {
        let dist = tgt.pos.distance_to(att.pos);
        let can_see = state.fog.side(tgt.side).spotted.contains(&attacker)
            && fog::los_clear(registry, &state.map, tgt.pos, att.pos);
        if can_see && !tgt.acted {
            let counter_weapon = registry.vehicle(&tgt.vehicle).and_then(|v| {
                v.weapons.iter().filter_map(|w| registry.weapon(w)).find(|w| {
                    !w.indirect && (w.range[0] as i32..=w.range[1] as i32).contains(&dist)
                })
            });
            if let Some(weapon) = counter_weapon.cloned() {
                // Face the attacker before returning fire.
                let (att_pos, tgt_id) = (att.pos, tgt.id);
                if let Some(t) = state.unit_mut(tgt_id) {
                    if t.pos != att_pos {
                        t.facing = t.pos.main_direction_to(att_pos);
                    }
                }
                resolve_shot(registry, state, target, &weapon, attacker, false, true, &mut events);
                fog::reveal_to_all(state, target);
                fog::recompute(registry, state);
            }
        }
    }
    events
}
