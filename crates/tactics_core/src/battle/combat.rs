//! Combat resolution: accuracy, armor facings, terrain cover, elevation
//! advantage, blind fire, and opportunity fire.

use super::{BattleState, Event, FireIntent, UnitId, fog, stats};
use crate::data::{ArmorFacing, DamageType, DataRegistry, Scale, TerrainDef, WeaponDef};
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

fn terrain_at<'r>(
    registry: &'r DataRegistry,
    state: &BattleState,
    pos: Hex,
) -> Option<&'r TerrainDef> {
    state
        .map
        .get(pos)
        .and_then(|tile| registry.terrain(&tile.terrain))
}

fn elevation_at(state: &BattleState, pos: Hex) -> i32 {
    state.map.get(pos).map(|t| t.elevation).unwrap_or(0)
}

/// Lowest and highest hit chance the engine will ever report: nothing is a
/// sure thing and nothing is hopeless.
pub const MIN_HIT: i32 = 5;
pub const MAX_HIT: i32 = 95;

/// Hit chance percentage, clamped to [`MIN_HIT`]..=[`MAX_HIT`].
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
    // The no-op closure compiles away, keeping this the allocation-free
    // path that AI search hammers.
    hit_chance_inner(
        registry,
        state,
        attacker,
        from,
        weapon,
        target_pos,
        blind,
        |_, _| {},
    )
}

/// Where one accuracy adjustment came from. An enum rather than a string so
/// the calculation never formats anything for callers that only want the
/// number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitFactor<'a> {
    /// Accuracy lost to distance.
    Range { hexes: i32 },
    /// The crew's gunnery skill.
    Gunnery,
    /// Shooting down onto the target.
    Downhill,
    /// Cover from the terrain the target occupies.
    Cover { terrain: &'a str },
    /// Firing at a tile rather than a spotted unit.
    Blind,
}

impl HitFactor<'_> {
    /// Render this factor for a player. Takes the scale rather than
    /// implementing `Display` so that a range can be stated in metres: a
    /// breakdown reading "Range (12 hexes)" tells the player nothing about
    /// whether the shot is a long one, and there is deliberately no
    /// scale-free way to format it.
    pub fn label(&self, scale: &Scale) -> String {
        match self {
            HitFactor::Range { hexes } => {
                format!("Range ({}, {hexes} hexes)", scale.format_distance(*hexes))
            }
            HitFactor::Gunnery => "Crew gunnery".to_string(),
            HitFactor::Downhill => "Firing downhill".to_string(),
            HitFactor::Cover { terrain } => format!("{terrain} cover"),
            HitFactor::Blind => "Blind fire".to_string(),
        }
    }
}

/// One named contribution to a hit chance, for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitModifier {
    pub label: String,
    pub delta: i32,
}

/// A hit chance with its arithmetic shown, so the player can see why a shot
/// is good or bad rather than just being handed a number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HitBreakdown {
    /// The weapon's base accuracy before anything is applied.
    pub base: i32,
    pub modifiers: Vec<HitModifier>,
    /// Final chance after clamping.
    pub total: i32,
    /// Whether the clamp actually bit, which is worth flagging in the UI.
    pub clamped: bool,
}

/// Hit chance with every modifier itemised. Mirrors [`hit_chance`] exactly:
/// both run the same code, this one just listens in.
pub fn hit_breakdown(
    registry: &DataRegistry,
    state: &BattleState,
    attacker: UnitId,
    from: Hex,
    weapon: &WeaponDef,
    target_pos: Hex,
    blind: bool,
) -> HitBreakdown {
    let mut modifiers = Vec::new();
    let total = hit_chance_inner(
        registry,
        state,
        attacker,
        from,
        weapon,
        target_pos,
        blind,
        |factor, delta| {
            if delta != 0 {
                modifiers.push(HitModifier {
                    label: factor.label(&registry.scale),
                    delta,
                });
            }
        },
    );
    let raw: i32 = weapon.accuracy + modifiers.iter().map(|m| m.delta).sum::<i32>();
    HitBreakdown {
        base: weapon.accuracy,
        modifiers,
        total,
        clamped: raw != total,
    }
}

/// The shared accuracy calculation. `note` is called with every adjustment;
/// callers that only want the number pass a closure that throws them away.
#[allow(clippy::too_many_arguments)]
fn hit_chance_inner(
    registry: &DataRegistry,
    state: &BattleState,
    attacker: UnitId,
    from: Hex,
    weapon: &WeaponDef,
    target_pos: Hex,
    blind: bool,
    mut note: impl FnMut(HitFactor<'_>, i32),
) -> i32 {
    let Some(att) = state.unit(attacker) else {
        return 0;
    };
    let mut chance = weapon.accuracy;

    let dist = from.distance_to(target_pos).max(1);
    let falloff = -weapon.accuracy_falloff * (dist - 1);
    note(HitFactor::Range { hexes: dist }, falloff);
    chance += falloff;

    let gunnery = registry.balance.accuracy(stats::gunnery(registry, att));
    note(HitFactor::Gunnery, gunnery);
    chance += gunnery;

    if elevation_at(state, from) > elevation_at(state, target_pos) {
        note(HitFactor::Downhill, 10);
        chance += 10;
    }
    if let Some(terrain) = terrain_at(registry, state, target_pos) {
        let cover = -(terrain.cover / 2);
        note(
            HitFactor::Cover {
                terrain: &terrain.name,
            },
            cover,
        );
        chance += cover;
    }
    if blind {
        note(HitFactor::Blind, -40);
        chance -= 40;
    }
    chance.clamp(MIN_HIT, MAX_HIT)
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

/// Everything the player should know before committing to a shot.
#[derive(Debug, Clone, PartialEq)]
pub struct AttackPreview {
    pub weapon_name: String,
    /// The weapon's `[min, max]` range band, in hexes.
    pub weapon_range: [u32; 2],
    pub target_name: String,
    /// Display name of the target's chassis, e.g. "Panzer IV".
    pub target_vehicle: String,
    /// Display name of the side the target belongs to.
    pub target_side: String,
    pub target_hp: i32,
    pub target_max_hp: i32,
    pub distance: i32,
    /// `false` when the target sits outside the weapon's range band, in
    /// which case the numbers below are hypothetical.
    pub in_range: bool,
    pub hit: HitBreakdown,
    /// Damage a hit would deal.
    pub damage: i32,
    /// Which armour arc the shot lands on, and how thick it is against this
    /// weapon's damage type.
    pub facing: ArmorFacing,
    pub effective_armor: i32,
    /// Hit chance times damage.
    pub expected_damage: f32,
    /// Whether this shot would finish the target outright.
    pub lethal: bool,
    /// Whether the target is able to shoot back, and for how much.
    pub counter: Option<CounterPreview>,
}

/// The return fire a shot would invite. Nothing special resolves this: it is
/// ordinary opportunity fire from a target that can see you and has a loaded
/// gun that reaches.
#[derive(Debug, Clone, PartialEq)]
pub struct CounterPreview {
    pub weapon_name: String,
    pub hit_chance: i32,
    pub damage: i32,
}

/// Whether `unit`'s weapon at `index` has finished reloading.
pub fn weapon_ready(state: &BattleState, unit: UnitId, index: usize) -> bool {
    state
        .unit(unit)
        .and_then(|u| u.cooldowns.get(index).copied())
        .is_some_and(|cd| cd == 0)
}

/// Work out what an attack would do, without touching the simulation.
///
/// `weapon_index` indexes the attacker's vehicle weapon list. Returns
/// `None` only when the units or their definitions are missing.
pub fn preview_attack(
    registry: &DataRegistry,
    state: &BattleState,
    attacker: UnitId,
    weapon_index: usize,
    target: UnitId,
    blind: bool,
) -> Option<AttackPreview> {
    let att = state.unit(attacker)?;
    let tgt = state.unit(target)?;
    let att_vehicle = registry.vehicle(&att.vehicle)?;
    let tgt_vehicle = registry.vehicle(&tgt.vehicle)?;
    let weapon = att_vehicle
        .weapons
        .get(weapon_index)
        .and_then(|w| registry.weapon(w))?;

    let distance = att.pos.distance_to(tgt.pos);
    let in_range = (weapon.range[0] as i32..=weapon.range[1] as i32).contains(&distance);
    let hit = hit_breakdown(registry, state, attacker, att.pos, weapon, tgt.pos, blind);
    let damage = raw_damage(registry, state, weapon, att.pos, target);
    let facing = struck_facing(tgt.pos, tgt.facing, att.pos);
    let armor = tgt_vehicle.armor.value(facing);
    let effective_armor = match weapon.damage_type {
        DamageType::Kinetic => armor,
        DamageType::Explosive => armor / 2,
        DamageType::SmallArms => armor * 2,
    }
    .max(0);

    // Return fire is just opportunity fire: the target needs to see the
    // attacker and own a loaded direct-fire weapon that reaches.
    let counter = {
        let can_see = state.fog.side(tgt.side).spotted.contains(&attacker)
            && state.sight.clear(tgt.pos, att.pos);
        if can_see {
            tgt_vehicle
                .weapons
                .iter()
                .enumerate()
                .filter_map(|(i, w)| registry.weapon(w).map(|w| (i, w)))
                .find(|(i, w)| {
                    !w.indirect
                        && (w.range[0] as i32..=w.range[1] as i32).contains(&distance)
                        && weapon_ready(state, target, *i)
                })
                .map(|(_, w)| CounterPreview {
                    weapon_name: w.name.clone(),
                    hit_chance: hit_chance(registry, state, target, tgt.pos, w, att.pos, false),
                    damage: raw_damage(registry, state, w, tgt.pos, attacker),
                })
        } else {
            None
        }
    };

    Some(AttackPreview {
        weapon_name: weapon.name.clone(),
        weapon_range: weapon.range,
        target_name: tgt.name.clone(),
        target_vehicle: tgt_vehicle.name.clone(),
        target_side: state
            .sides
            .get(tgt.side as usize)
            .map(|s| s.name.clone())
            .unwrap_or_default(),
        target_hp: tgt.hp.max(0),
        target_max_hp: tgt_vehicle.max_hp,
        distance,
        in_range,
        expected_damage: hit.total as f32 / 100.0 * damage as f32,
        lethal: damage >= tgt.hp,
        hit,
        damage,
        facing,
        effective_armor,
        counter,
    })
}

/// Resolve one shot from `attacker` at `target`. Damage lands immediately but
/// death does not: see [`reap`]. Reloading and fog are handled by the callers
/// below.
#[allow(clippy::too_many_arguments)]
fn resolve_shot(
    registry: &DataRegistry,
    state: &mut BattleState,
    attacker: UnitId,
    weapon: &WeaponDef,
    target: UnitId,
    blind: bool,
    opportunity: bool,
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
        opportunity,
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
}

/// Take the wrecks off the board at the end of a tick.
///
/// Nothing is removed while a tick is still resolving, which is what lets two
/// crews that fired at each other in the same instant both get their shot off
/// — and both lose. Deciding it by whoever happened to be processed first
/// would be an artefact of the loop order, not a rule of the game.
pub fn reap(state: &mut BattleState, events: &mut Vec<Event>) {
    for unit in state.units.iter_mut().filter(|u| u.alive && u.hp <= 0) {
        unit.alive = false;
        events.push(Event::UnitDestroyed {
            unit: unit.id,
            at: unit.pos,
        });
    }
}

/// Whether `weapon` fired from `from` can reach `target_pos` at all: inside
/// the range band, and either in line of sight or indirect with a spotter.
fn shot_exists(
    state: &BattleState,
    weapon: &WeaponDef,
    from: Hex,
    target_pos: Hex,
    spotted: bool,
) -> bool {
    let dist = from.distance_to(target_pos);
    if !(weapon.range[0] as i32..=weapon.range[1] as i32).contains(&dist) {
        return false;
    }
    if weapon.indirect {
        // Indirect fire needs somebody watching, not its own eyes.
        spotted
    } else {
        state.sight.clear(from, target_pos)
    }
}

fn weapon_at<'r>(
    registry: &'r DataRegistry,
    state: &BattleState,
    unit: UnitId,
    index: usize,
) -> Option<&'r WeaponDef> {
    let u = state.unit(unit)?;
    registry
        .vehicle(&u.vehicle)
        .and_then(|v| v.weapons.get(index))
        .and_then(|w| registry.weapon(w))
}

/// Fire one weapon at a unit, spending its reload and giving away the
/// shooter's position.
fn fire_at_unit(
    registry: &DataRegistry,
    state: &mut BattleState,
    attacker: UnitId,
    weapon_index: usize,
    target: UnitId,
    opportunity: bool,
    events: &mut Vec<Event>,
) {
    let Some(weapon) = weapon_at(registry, state, attacker, weapon_index).cloned() else {
        return;
    };
    let Some(tgt_pos) = state.unit(target).map(|t| t.pos) else {
        return;
    };
    if let Some(att) = state.unit_mut(attacker) {
        if att.pos != tgt_pos {
            att.facing = att.pos.main_direction_to(tgt_pos);
        }
        if let Some(cd) = att.cooldowns.get_mut(weapon_index) {
            *cd = weapon.reload(&registry.scale);
        }
    }
    resolve_shot(
        registry,
        state,
        attacker,
        &weapon,
        target,
        false,
        opportunity,
        events,
    );
    fog::reveal_to_all(state, attacker);
    events.extend(fog::recompute(registry, state));
}

/// Shell a tile. Hits whoever happens to be standing there, at a heavy
/// accuracy penalty.
fn fire_at_tile(
    registry: &DataRegistry,
    state: &mut BattleState,
    attacker: UnitId,
    weapon_index: usize,
    at: Hex,
    events: &mut Vec<Event>,
) {
    let Some(weapon) = weapon_at(registry, state, attacker, weapon_index).cloned() else {
        return;
    };
    let Some((att_pos, side)) = state.unit(attacker).map(|a| (a.pos, a.side)) else {
        return;
    };
    if let Some(att) = state.unit_mut(attacker) {
        if att.pos != at {
            att.facing = att.pos.main_direction_to(at);
        }
        if let Some(cd) = att.cooldowns.get_mut(weapon_index) {
            *cd = weapon.reload(&registry.scale);
        }
    }
    match state.unit_at(at).filter(|t| t.side != side).map(|t| t.id) {
        Some(target) => resolve_shot(
            registry, state, attacker, &weapon, target, true, false, events,
        ),
        None => {
            events.push(Event::ShotFired {
                attacker,
                from: att_pos,
                at,
                weapon: weapon.id.clone(),
                blind: true,
                opportunity: false,
            });
            events.push(Event::ShotMissed { attacker, at });
        }
    }
    fog::reveal_to_all(state, attacker);
    events.extend(fog::recompute(registry, state));
}

/// The best shot this unit could take on its own initiative: the loaded
/// direct-fire weapon and spotted enemy promising the most damage. Indirect
/// weapons do not snap-fire, so artillery holds unless it was given a target.
pub fn best_opportunity_shot(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
) -> Option<(usize, UnitId)> {
    let att = state.unit(unit)?;
    let vehicle = registry.vehicle(&att.vehicle)?;
    let spotted = &state.fog.side(att.side).spotted;

    let mut best: Option<(usize, UnitId, f32)> = None;
    for (index, weapon_id) in vehicle.weapons.iter().enumerate() {
        let Some(weapon) = registry.weapon(weapon_id) else {
            continue;
        };
        if weapon.indirect || !weapon_ready(state, unit, index) {
            continue;
        }
        // Enemies in id order, so ties resolve the same way in every replay.
        for enemy in state.alive_units().filter(|e| e.side != att.side) {
            if !spotted.contains(&enemy.id) || !shot_exists(state, weapon, att.pos, enemy.pos, true)
            {
                continue;
            }
            let value = expected_damage(registry, state, unit, att.pos, weapon, enemy.id, false);
            if best.is_none_or(|(_, _, v)| value > v) {
                best = Some((index, enemy.id, value));
            }
        }
    }
    best.map(|(w, t, _)| (w, t))
}

/// Take this unit's shot for the current tick, if it has one.
///
/// The ordered target comes first. When there is no order, or the order
/// cannot be carried out right now (target destroyed, lost in the fog, out of
/// range, or the gun is still reloading), the crew falls back to opportunity
/// fire. That fallback is what makes return fire happen without a special
/// case for it.
pub fn fire_if_able(
    registry: &DataRegistry,
    state: &mut BattleState,
    unit: UnitId,
    events: &mut Vec<Event>,
) {
    let Some(att) = state.unit(unit) else { return };
    let (intent, att_pos, side) = (att.intent.fire, att.pos, att.side);

    match intent {
        FireIntent::Target { target, weapon } => {
            let spotted = state.fog.side(side).spotted.contains(&target);
            let ordered_shot = weapon_ready(state, unit, weapon)
                && spotted
                && state
                    .unit(target)
                    .filter(|t| t.side != side)
                    .and_then(|t| {
                        weapon_at(registry, state, unit, weapon)
                            .map(|w| shot_exists(state, w, att_pos, t.pos, spotted))
                    })
                    .unwrap_or(false);
            if ordered_shot {
                fire_at_unit(registry, state, unit, weapon, target, false, events);
                return;
            }
        }
        FireIntent::Area { at, weapon } => {
            let can_shell = weapon_ready(state, unit, weapon)
                && weapon_at(registry, state, unit, weapon)
                    .is_some_and(|w| shot_exists(state, w, att_pos, at, true));
            if can_shell {
                fire_at_tile(registry, state, unit, weapon, at, events);
                return;
            }
        }
        FireIntent::Hold => {}
    }

    if let Some((weapon, target)) = best_opportunity_shot(registry, state, unit) {
        fire_at_unit(registry, state, unit, weapon, target, true, events);
    }
}
