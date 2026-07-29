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
    hit_chance_inner(registry, state, attacker, from, weapon, target_pos, blind, |_, _| {})
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

impl std::fmt::Display for HitFactor<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HitFactor::Range { hexes } => write!(f, "Range ({hexes} hexes)"),
            HitFactor::Gunnery => write!(f, "Crew gunnery"),
            HitFactor::Downhill => write!(f, "Firing downhill"),
            HitFactor::Cover { terrain } => write!(f, "{terrain} cover"),
            HitFactor::Blind => write!(f, "Blind fire"),
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
                    label: factor.to_string(),
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

    let gunnery = stats::gunnery(registry, att) * 3;
    note(HitFactor::Gunnery, gunnery);
    chance += gunnery;

    if elevation_at(state, from) > elevation_at(state, target_pos) {
        note(HitFactor::Downhill, 10);
        chance += 10;
    }
    if let Some(terrain) = terrain_at(registry, state, target_pos) {
        let cover = -(terrain.cover / 2);
        note(HitFactor::Cover { terrain: &terrain.name }, cover);
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

/// The return fire a shot would invite.
#[derive(Debug, Clone, PartialEq)]
pub struct CounterPreview {
    pub weapon_name: String,
    pub hit_chance: i32,
    pub damage: i32,
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

    // Return fire mirrors the rule in `resolve_attack`: the target has to
    // see the attacker, still have its action, and own a direct-fire weapon
    // that reaches.
    let counter = {
        let can_see = state.fog.side(tgt.side).spotted.contains(&attacker)
            && fog::los_clear(registry, &state.map, tgt.pos, att.pos);
        if can_see && !tgt.acted {
            tgt_vehicle
                .weapons
                .iter()
                .filter_map(|w| registry.weapon(w))
                .find(|w| {
                    !w.indirect && (w.range[0] as i32..=w.range[1] as i32).contains(&distance)
                })
                .map(|w| CounterPreview {
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
