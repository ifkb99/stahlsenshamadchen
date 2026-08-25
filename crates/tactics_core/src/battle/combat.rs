//! Combat resolution: accuracy, armor facings, terrain cover, elevation
//! advantage, blind fire, opportunity fire, and shells in the air.

use super::{BattleState, Event, FireIntent, Unit, UnitId, fog, stats};
use crate::data::{
    AmmoClass, AmmoDef, ArmorFacing, DamageType, DataRegistry, Scale, TerrainDef, WeaponDef,
};
use hexx::Hex;
use rand::RngExt;
use serde::{Deserialize, Serialize};

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

    let gunnery = registry.balance.accuracy(stats::gunnery(
        registry,
        &state.roster,
        att,
        state.terrain_at(att.pos),
    ));
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

/// The round a weapon would put downrange right now.
///
/// Ammunition is content: a weapon fires the first round in its declared
/// list with anything left in the racks, so the list's order is the
/// loader's standing preference. A weapon whose mod declares no ammunition
/// at all fires forever on its own legacy numbers — that is what keeps a
/// pre-ballistics mod fighting with the relationships its author tuned —
/// while a weapon with a list and empty racks chambers nothing and is
/// silent. Silence is announced once, by [`Event::WeaponDry`] at the moment
/// the last round is spent, not per refused tick.
pub struct Round<'r> {
    /// The ammunition definition. `None` is the legacy path: nothing is
    /// spent by firing, because there is nothing counted to spend.
    pub ammo: Option<&'r AmmoDef>,
    /// Flat penetration for the legacy path, pre-scaled by the weapon's
    /// damage type so old content keeps its old relationships (explosive
    /// always treated armor as half, small arms as double).
    legacy_pen: f32,
    /// What the hit-point ledger loses if this round gets through. The
    /// weapon's damage scaled by the round's behind-armor potency — a
    /// stand-in that the outcome chunk (B2) replaces with effect rolls
    /// against the crew and modules.
    pub damage: i32,
    /// Bullets bouncing off plate frighten nobody buttoned up behind it.
    pub small_arms: bool,
}

impl<'r> Round<'r> {
    /// The round a `weapon` puts downrange when `ammo` is what is chambered.
    ///
    /// Shared by [`chambered`], which reads the racks, and by the shell that
    /// has been in the air since somebody read them a tick or two ago: a
    /// round already fired must arrive as exactly the round that left, and
    /// two constructors would be two chances for it not to.
    fn loaded(ammo: &'r AmmoDef, weapon: &WeaponDef) -> Self {
        Self {
            ammo: Some(ammo),
            legacy_pen: 0.0,
            damage: (weapon.damage as f32 * ammo.post_pen).round().max(0.0) as i32,
            small_arms: matches!(ammo.class, AmmoClass::SmallArms),
        }
    }

    /// Penetration of this round at `dist` hexes, over the firing weapon's
    /// range band. Kinetic ammunition falls off downrange, chemical is
    /// flat, and both of those facts live in the data, not here.
    pub fn pen_at(&self, dist: i32, range: [u32; 2]) -> f32 {
        match self.ammo {
            Some(ammo) => ammo.penetration_at(dist.max(0) as u32, range) as f32,
            None => self.legacy_pen,
        }
    }

    /// The round's behind-armor potency, 1.0 on the legacy path.
    fn post_pen_scale(&self) -> f32 {
        self.ammo.map(|a| a.post_pen).unwrap_or(1.0)
    }

    /// What the campaign's crew-fate roll should believe came aboard.
    fn damage_type(&self) -> Option<DamageType> {
        self.ammo.map(|a| match a.class {
            // A shaped charge's jet and spall are kinetic events for the
            // cadets inside; the distinction that matters downstream is
            // "something sharp came through" versus blast versus bullets.
            AmmoClass::Kinetic | AmmoClass::Chemical => DamageType::Kinetic,
            AmmoClass::Explosive => DamageType::Explosive,
            AmmoClass::SmallArms => DamageType::SmallArms,
        })
    }
}

/// What `unit`'s `weapon` would fire right now, or `None` for a gun with an
/// ammunition list and nothing left on it.
pub fn chambered<'r>(
    registry: &'r DataRegistry,
    state: &BattleState,
    unit: UnitId,
    weapon: &WeaponDef,
) -> Option<Round<'r>> {
    if weapon.ammo.is_empty() {
        let legacy_pen = weapon.penetration.max(1) as f32
            * match weapon.damage_type {
                DamageType::Kinetic => 1.0,
                DamageType::Explosive => 2.0,
                DamageType::SmallArms => 0.5,
            };
        return Some(mustered(
            registry,
            state,
            unit,
            Round {
                ammo: None,
                legacy_pen,
                damage: weapon.damage,
                small_arms: matches!(weapon.damage_type, DamageType::SmallArms),
            },
        ));
    }
    let u = state.unit(unit)?;
    let ammo = weapon
        .ammo
        .iter()
        .filter(|id| u.ammo.get(*id).copied().unwrap_or(0) > 0)
        .find_map(|id| registry.ammo(id))?;
    Some(mustered(registry, state, unit, Round::loaded(ammo, weapon)))
}

/// A platoon shoots with the riflemen she still has. Any unit carrying
/// [`ModuleEffect::Troops`] modules scales every round's effect by the
/// fraction still standing — half the platoon is half the fire, and a
/// remnant with the cadets alone left rounds down to nearly nothing, which
/// is what makes her want the exit rather than the fight. A unit with no
/// troops modules (every tank there is) passes through untouched.
fn mustered<'r>(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    mut round: Round<'r>,
) -> Round<'r> {
    if let Some(u) = state.unit(unit)
        && let Some((have, total)) = u.troops(registry)
    {
        round.damage = (round.damage as u32 * have / total.max(1)) as i32;
    }
    round
}

/// What one round is expected to accomplish against one profile: the
/// penetration chain's value plus what the burst does from outside if the
/// plate holds.
/// The one value function behind both the loader's choice and the AI's shot
/// pricing — if they read different formulas, the crew would load a round
/// the planner did not price, and every number upstream would quietly lie.
///
/// The blast half used to be a flat `0.3 * blast`, and being flat is exactly
/// what was wrong with it: [`overpressure`] reads the plate the burst
/// arrives against and this did not, so every shell a howitzer fired was
/// priced the same against a heavy tank's glacis as against an open-topped
/// carrier's roof. The playthrough review's headline defect is that
/// arithmetic and nothing else — a 105 put thirty-six shells into one tank
/// destroyer's front, twenty-nine of them after the only two things blast
/// could reach out there were already broken, because the number said 1.8
/// every time. It now asks [`blast_worth`], which walks the same three
/// cases `overpressure` walks.
fn round_worth(
    registry: &DataRegistry,
    state: &BattleState,
    target: UnitId,
    profile: &ShotProfile,
    ammo: Option<&AmmoDef>,
) -> f32 {
    let blast = ammo.map(|a| a.blast).unwrap_or(0).max(0);
    profile.pen_chance * profile.damage as f32
        + (1.0 - profile.pen_chance) * blast_worth(registry, state, target, profile.plate, blast)
}

/// What a burst of `blast` against `plate` is expected to be worth, in the
/// same ledger points a penetration spends — the pricing twin of
/// [`overpressure`], case for case, and the reason the two are written
/// beside each other.
///
/// There is no conversion constant here and there deliberately is not one.
/// `points_per_effect` already states what a ledger point buys, and
/// `overpressure` already spends blast through it against a soft target, so
/// the exchange rate between blast and damage is not an opinion anybody has
/// to hold — it is a fact about the resolver, and this reads it off rather
/// than guessing at it.
fn blast_worth(
    registry: &DataRegistry,
    state: &BattleState,
    target: UnitId,
    plate: i32,
    blast: i32,
) -> f32 {
    if blast <= 0 {
        return 0.0;
    }
    if plate == 0 {
        // Splash against no armour cashes straight into casualty rolls at
        // the same rate a penetration's budget does, so a point of blast is
        // worth exactly a point of damage.
        //
        // Today's only caller cannot reach this, and the line is here anyway.
        // `round_worth` prices this term as `1 - pen_chance`, and against no
        // armour the gate always passes, so a direct hit on a platoon is
        // priced through the penetration half with the full budget — which is
        // the same sentence `overpressure` carries about why direct hits
        // never reach ITS plate-zero branch. The trap is what happens
        // without this line: `blast_overmatches(blast, 0)` is true for any
        // positive blast, so the next caller to price area fire or splash
        // through here would silently get "the whole platoon is wrecked",
        // and artillery against infantry is attrition and never a
        // single-event erasure.
        return blast as f32;
    }
    if blast_overmatches(blast, plate) {
        // She is wrecked, gate or no gate, so the shot is worth whatever is
        // left of her — which also means `best_weapon_against` reads it as a
        // kill and the evaluator pays its kill bonus, without either of them
        // learning a special case for blast.
        return state
            .unit(target)
            .map(|u| state.substance(registry, u).0 as f32)
            .unwrap_or(0.0);
    }
    // What is left is the harassing case: some chance of breaking one of the
    // things a burst can reach from outside. A crew whose tracks and antenna
    // are already gone is a crew this shell cannot touch, and saying so is
    // the whole fix — the alternative is a gun that keeps firing at a number
    // rather than at a tank.
    let Some(unit) = state.unit(target) else {
        return 0.0;
    };
    // Summed the way the resolver picks, not merely counted. `overpressure`
    // draws from the weighted list and returns early when the weights come
    // to nothing, so a hull whose exterior modules are all size zero is one
    // blast provably cannot touch — and a pricing that counted entries
    // instead would put high explosive up against solid shot on a plate the
    // HE cannot beat, which is the loader making a choice on a number no
    // resolver will honour.
    let total: u32 = exterior_modules(registry, unit)
        .iter()
        .map(|(_, w)| w)
        .sum();
    if total == 0 {
        return 0.0;
    }
    // One roll's worth of harm, at the odds of getting it: `points_per_effect`
    // is by definition what one effect costs the ledger, so this needs no
    // conversion of its own.
    overpressure_chance(blast, plate) as f32 / 100.0
        * registry.balance.points_per_effect.max(1) as f32
}

/// Blast that does not need the penetration gate's permission: twice the
/// plate it arrives against crushes the hull. One line, shared, because
/// [`blast_worth`] pricing it differently from [`overpressure`] resolving it
/// is precisely the drift this chunk exists to remove. Public because the
/// balance instrument's "worth a look" section asks the same question of a
/// gun's heaviest round against a hull's thinnest plate, and a table that
/// says a gun can hurt a tank while the resolver disagrees is worse than no
/// table.
pub fn blast_overmatches(blast: i32, plate: i32) -> bool {
    blast >= plate * 2
}

/// The odds a burst that neither overmatches nor penetrates breaks
/// something anyway. Shared with [`blast_worth`] for the reason above.
fn overpressure_chance(blast: i32, plate: i32) -> i32 {
    blast * 100 / (blast + plate).max(1)
}

/// What blast can actually reach from outside, weighted like the inside is:
/// running gear and antennas, the things bolted to the hull rather than
/// sheltered by it. Shared between the resolver and the pricing so that a
/// module set a mod invents is priced by whatever it is, not by a list
/// somebody remembered to update twice.
fn exterior_modules(registry: &DataRegistry, unit: &Unit) -> Vec<(String, u32)> {
    unit.modules
        .iter()
        .filter(|(_, hits)| **hits > 0)
        .filter_map(|(id, _)| {
            registry.module(id).and_then(|m| {
                matches!(
                    m.effect,
                    crate::data::ModuleEffect::Mobility | crate::data::ModuleEffect::Radio
                )
                .then(|| (id.clone(), m.size))
            })
        })
        .collect()
}

/// The round the loader picks with a target in her commander's sights: the
/// listed ammunition with stock remaining that expects the most against
/// THAT vehicle, ties to the earlier listing (the loader's standing
/// preference). This is the AP-or-HE decision made where it was really
/// made — at the racks, by the crew, per target — rather than in the order
/// stream: the commander calls the target, the loader picks the round.
/// Legacy-path weapons (no ammo list) have nothing to choose.
pub fn best_round_against<'r>(
    registry: &'r DataRegistry,
    state: &BattleState,
    unit: UnitId,
    weapon: &WeaponDef,
    target: UnitId,
) -> Option<Round<'r>> {
    if weapon.ammo.is_empty() {
        return chambered(registry, state, unit, weapon);
    }
    let u = state.unit(unit)?;
    let mut best: Option<(f32, Round<'r>)> = None;
    for id in &weapon.ammo {
        if u.ammo.get(id).copied().unwrap_or(0) == 0 {
            continue;
        }
        let Some(ammo) = registry.ammo(id) else {
            continue;
        };
        let round = mustered(registry, state, unit, Round::loaded(ammo, weapon));
        let Some(profile) = shot_profile(registry, state, weapon, &round, u.pos, target) else {
            continue;
        };
        let worth = round_worth(registry, state, target, &profile, Some(ammo));
        if best.as_ref().is_none_or(|(w, _)| worth > *w) {
            best = Some((worth, round));
        }
    }
    best.map(|(_, round)| round)
}

/// The round for shelling ground nobody has confirmed: the most blast
/// aboard, ties to the earlier listing. Area fire is suppression and
/// harassment, and solid shot into an empty treeline is a wasted rack slot.
pub fn best_round_for_ground<'r>(
    registry: &'r DataRegistry,
    state: &BattleState,
    unit: UnitId,
    weapon: &WeaponDef,
) -> Option<Round<'r>> {
    if weapon.ammo.is_empty() {
        return chambered(registry, state, unit, weapon);
    }
    let u = state.unit(unit)?;
    let mut best: Option<(i32, Round<'r>)> = None;
    for id in &weapon.ammo {
        if u.ammo.get(id).copied().unwrap_or(0) == 0 {
            continue;
        }
        let Some(ammo) = registry.ammo(id) else {
            continue;
        };
        if best.as_ref().is_none_or(|(b, _)| ammo.blast > *b) {
            best = Some((
                ammo.blast,
                mustered(registry, state, unit, Round::loaded(ammo, weapon)),
            ));
        }
    }
    best.map(|(_, round)| round)
}

/// A round that has left the muzzle and has not arrived yet.
///
/// Only indirect fire is ever in this list. At this scale a direct-fire shot
/// is over before the tick that fired it ends — an 88 crosses its whole 1.6
/// km reach in two seconds against a five-second tick — so the machinery
/// below would be an elaborate way of doing nothing to it. A howitzer shell
/// at 470 m/s crossing 3 km is genuinely in the air for eight seconds, and
/// what it is aimed at is *ground*: the gunner laid the piece on a map
/// reference, and whoever is standing there when it comes down is who it
/// lands on. That is the whole artillery rework in one sentence, and it is
/// why the target is a [`Hex`] rather than a [`UnitId`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellInFlight {
    /// Who fired it. Still named after it lands, so the log can say whose
    /// battery that was even when the gun itself has since been knocked out.
    pub attacker: UnitId,
    /// [`AmmoDef`] id, resolved again at impact. The round is data, and the
    /// shell carries its name rather than a copy of its numbers so that the
    /// thing that lands is the thing the mod describes.
    pub ammo: String,
    /// [`WeaponDef`] id, for the weapon weight that prices the behind-armor
    /// budget.
    pub weapon: String,
    /// Where it was fired from.
    ///
    /// Kept because armor cares about bearing: the plate a shell strikes and
    /// the obliquity it strikes at are read from the line between the gun and
    /// the ground it was laid on, which is this game's model of the shell's
    /// arrival. The battery may be dead by the time it matters, so the
    /// position travels with the shell rather than being looked up.
    pub from: Hex,
    /// The ground it was aimed at.
    pub at: Hex,
    /// Absolute tick it comes down on: `round * ticks_per_round + tick`, the
    /// same clock [`super::SideFog::spotted_since`] runs on.
    pub lands: u64,
}

/// How many ticks a shell of this `velocity` spends crossing `hexes`.
///
/// Never zero: a shell that arrives in the tick that fired it is a
/// direct-fire shot, and the one thing indirect fire must cost is the chance
/// for the target to be somewhere else. A 105 at 470 m/s over 3 km comes out
/// at two ticks, which is the number the design doc names.
pub fn flight_ticks(scale: &Scale, velocity: u32, hexes: i32) -> u64 {
    // A round nobody gave a velocity is not a round that hangs in the air
    // forever; treat it as the minimum flight and let validation complain
    // about the data.
    if velocity == 0 {
        return 1;
    }
    let seconds = scale.meters(hexes.max(0)) / velocity as f32;
    let tick = scale.tick_seconds();
    if tick <= 0.0 {
        return 1;
    }
    ((seconds / tick).ceil() as i64).max(1) as u64
}

/// The shell this shot puts in the air, or `None` when it resolves in-tick.
///
/// Two conditions, and the second one is the additivity contract rather than
/// an optimisation. Flight time is for indirect fire only. And it is for
/// indirect fire **with ammunition data**: a mod whose howitzer declares no
/// `ammo` list has no velocity to fly at, and — more importantly — a mod that
/// declares nothing must get today's game, which resolves artillery
/// instantly. So the legacy path keeps its hitscan howitzer forever, and only
/// rounds that a mod actually wrote down go up in the air.
fn shell_in_flight(
    registry: &DataRegistry,
    state: &BattleState,
    attacker: UnitId,
    weapon: &WeaponDef,
    round: &Round<'_>,
    at: Hex,
) -> Option<ShellInFlight> {
    if !weapon.indirect {
        return None;
    }
    let ammo = round.ammo?;
    let from = state.unit(attacker)?.pos;
    let lands = state.absolute_tick(registry)
        + flight_ticks(&registry.scale, ammo.velocity, from.distance_to(at));
    Some(ShellInFlight {
        attacker,
        ammo: ammo.id.clone(),
        weapon: weapon.id.clone(),
        from,
        at,
        lands,
    })
}

/// Bring down every shell whose time has come, in the order they were fired.
///
/// Called at the top of a tick, before anybody drives, so a shell lands on
/// whoever was standing there when the tick opened rather than on whoever
/// happens to arrive during it. The Vec's order is the firing order and the
/// iteration is over it directly — no map, no set — because these rolls reach
/// the event stream.
///
/// Nothing is reaped here. A crew the shell just killed still moves and
/// shoots this tick, which is the same simultaneity bargain every other
/// source of damage keeps: death is reaped at the end of the tick, never
/// eagerly.
pub(super) fn resolve_shells(
    registry: &DataRegistry,
    state: &mut BattleState,
    events: &mut Vec<Event>,
) {
    if state.shells.is_empty() {
        return;
    }
    let now = state.absolute_tick(registry);
    let mut landing = Vec::new();
    let mut airborne = Vec::new();
    for shell in std::mem::take(&mut state.shells) {
        if shell.lands <= now {
            landing.push(shell);
        } else {
            airborne.push(shell);
        }
    }
    state.shells = airborne;
    for shell in landing {
        shell_lands(registry, state, &shell, events);
    }
}

/// One shell arriving on one hex.
///
/// There is no hit roll. A shell that comes down on an occupied hex hits what
/// is on it, because the inaccuracy of artillery is already modelled by the
/// thing that makes it artillery: it was aimed at ground, ticks ago, and the
/// target had every one of those ticks to be elsewhere. Rolling a blind-fire
/// accuracy penalty on top would charge the same scatter twice and make a
/// shell that caught a stationary crew flat-footed miss anyway.
/// [`hit_chance`] is untouched — direct fire still rolls to hit exactly as
/// before.
///
/// What lands is then ordinary: the occupant takes the full pipeline (the
/// penetration gate, behind-armor effects, or a bounce and its overpressure),
/// and the neighbouring tiles take blast only, at half — a shell that misses
/// your tile by a hundred metres is a different event from one that arrives
/// on it.
fn shell_lands(
    registry: &DataRegistry,
    state: &mut BattleState,
    shell: &ShellInFlight,
    events: &mut Vec<Event>,
) {
    events.push(Event::ShellLanded {
        attacker: shell.attacker,
        at: shell.at,
        ammo: shell.ammo.clone(),
    });
    // Content can go away underneath a shell — a mod reloaded, a save opened
    // against a different roster. The burst is still announced; it simply has
    // no numbers to resolve with.
    let (Some(ammo), Some(weapon)) = (registry.ammo(&shell.ammo), registry.weapon(&shell.weapon))
    else {
        return;
    };
    let round = Round::loaded(ammo, weapon);

    let direct = state.unit_at(shell.at).map(|u| u.id);
    if let Some(target) = direct {
        resolve_impact(
            registry,
            state,
            shell.attacker,
            weapon,
            &round,
            shell.from,
            target,
            events,
        );
    }

    // Everyone next door, in unit-id order so the rolls land in the same
    // sequence in every replay — `all_neighbors` is fixed, but which of them
    // are occupied is not something the event stream should learn from.
    let mut splashed: Vec<UnitId> = shell
        .at
        .all_neighbors()
        .into_iter()
        .filter_map(|hex| state.unit_at(hex).map(|u| u.id))
        .filter(|id| Some(*id) != direct)
        .collect();
    splashed.sort_unstable();
    for id in splashed {
        let Some(pos) = state.unit(id).map(|u| u.pos) else {
            continue;
        };
        // Half a hex away is half the blast, and being dug in among something
        // halves it again. Reading terrain `cover` for that is crude — it is
        // the same number that hides a tank from a gunner's sights, doing
        // duty as "there is stuff between her and the burst" — but it is data
        // rather than a constant, and it is the first time terrain has
        // mattered against artillery at all. B4/B5 may refine it into a
        // number of its own.
        let mut blast = ammo.blast / 2;
        if terrain_at(registry, state, pos).is_some_and(|t| t.cover >= 30) {
            blast /= 2;
        }
        if blast > 0 {
            overpressure(registry, state, id, blast, shell.at, events);
        }
    }
}

/// Axial hex coordinates on the flat plane, for angle arithmetic. Any
/// consistent hex-metric embedding works because only angles between
/// *differences* are read; this is the standard one.
fn cartesian(h: Hex) -> (f32, f32) {
    (h.x as f32 + h.y as f32 * 0.5, h.y as f32 * 0.866_025_4)
}

/// How much thicker the struck plate stands for a shot arriving off its
/// normal: 1.0 square on, up to ~1.15 at the worst angle a hex face can be
/// hit at.
///
/// The hull is modelled as a hexagonal prism. The struck *face* is the one
/// whose outward normal lies nearest the incoming ray — which is exactly
/// the quantisation [`struck_facing`] already performs — and the armor
/// *value* on that face is the arc's (three faces wear front plate, two
/// side, one rear). Obliquity is then the ray's angle off that face's own
/// normal, which by construction is at most thirty degrees: modest,
/// continuous, and never absurd. An earlier draft measured the angle
/// against the *arc's central* normal instead, and the front arc spans
/// ninety degrees of incoming ray — a shot from its edge read as striking
/// the glacis nearly parallel and clamped to triple armor, making a tank
/// quantised as "frontal" impenetrable from directions the side plate was
/// plainly facing. Angling therefore lives where the hex geometry puts it:
/// turning the hull decides which armor class each threat axis meets, and
/// this factor prices the residual few degrees, not the seam.
pub fn obliquity_scale(target_pos: Hex, attacker_pos: Hex) -> f32 {
    let (tx, ty) = cartesian(target_pos);
    let (ax, ay) = cartesian(attacker_pos);
    let (ix, iy) = (ax - tx, ay - ty);
    let len = (ix * ix + iy * iy).sqrt();
    if len == 0.0 {
        return 1.0;
    }
    let (ix, iy) = (ix / len, iy / len);
    let face = target_pos.main_direction_to(attacker_pos);
    let (nx, ny) = cartesian(Hex::ZERO.neighbor(face));
    let nlen = (nx * nx + ny * ny).sqrt();
    let cos = (ix * nx / nlen + iy * ny / nlen).max(0.5);
    1.0 / cos
}

/// The chance this penetration beats this armor, counting exactly the
/// outcomes [`penetration_roll`] can roll.
///
/// The scatter is an integer percent and the roll a uniform integer in
/// `-s..=s`, so this is a finite count rather than an integral — which is
/// the point: the number the AI plans on and the die the resolver throws
/// can never disagree about what is possible, because they are the same
/// comparison enumerated versus sampled.
pub fn penetration_chance(pen: f32, armor: f32, scatter: i32) -> f32 {
    if armor <= 0.0 {
        return 1.0;
    }
    if pen <= 0.0 {
        return 0.0;
    }
    let s = scatter.max(0);
    let through = (-s..=s)
        .filter(|r| pen * (100 + r) as f32 >= armor * 100.0)
        .count();
    through as f32 / (2 * s + 1) as f32
}

/// One fired round against one plate: the sampled twin of
/// [`penetration_chance`], sharing its comparison verbatim.
fn penetration_roll(state: &mut BattleState, pen: f32, armor: f32, scatter: i32) -> bool {
    if armor <= 0.0 {
        return true;
    }
    if pen <= 0.0 {
        return false;
    }
    let s = scatter.max(0);
    let r = state.rng.random_range(-s..=s);
    pen * (100 + r) as f32 >= armor * 100.0
}

/// Everything about one prospective shot that armor decides: which plate,
/// how thick it effectively stands, the odds of getting through, and what
/// the ledger loses if it does.
pub struct ShotProfile {
    pub facing: ArmorFacing,
    /// The struck plate as the vehicle lists it, before obliquity. Blast
    /// reads this rather than [`Self::effective_armor`], because
    /// [`overpressure`] does: a burst crushing a hull is not defeated by the
    /// angle a solid shot would skip off.
    pub plate: i32,
    /// Plate value after obliquity, in the abstract armor units.
    pub effective_armor: f32,
    /// Probability the chambered round defeats it, 0..=1.
    pub pen_chance: f32,
    /// Ledger damage if it penetrates. Cover and elevation deliberately do
    /// not scale this any more: they already price themselves into the hit
    /// chance, and a round that is through the plate is through — the tree
    /// the shell passed did not make the inside of the tank bigger.
    pub damage: i32,
}

/// Work out what `round` fired from `attacker_pos` does to `target`'s armor.
pub fn shot_profile(
    registry: &DataRegistry,
    state: &BattleState,
    weapon: &WeaponDef,
    round: &Round<'_>,
    attacker_pos: Hex,
    target: UnitId,
) -> Option<ShotProfile> {
    let tgt = state.unit(target)?;
    let vehicle = registry.vehicle(&tgt.vehicle)?;
    let facing = struck_facing(tgt.pos, tgt.facing, attacker_pos);
    let plate = vehicle.armor.value(facing).max(0);
    // Nothing to slope: an unarmored bed is an unarmored bed from any angle.
    let effective_armor = if plate == 0 {
        0.0
    } else {
        plate as f32 * obliquity_scale(tgt.pos, attacker_pos)
    };
    let pen = round.pen_at(attacker_pos.distance_to(tgt.pos), weapon.range);
    Some(ShotProfile {
        facing,
        plate,
        effective_armor,
        pen_chance: penetration_chance(pen, effective_armor, registry.balance.pen_scatter),
        damage: round.damage,
    })
}

/// Expected outcome of a shot — hit chance times what the loader's best
/// round is worth against that plate ([`round_worth`]: the penetration
/// chain plus a modest price on blast) — the currency of AI scoring. Zero
/// for a dry gun. A gun whose best round can neither penetrate nor blast
/// expects nothing and holds its fire; one that can only rattle tracks
/// with high explosive expects a little, and harasses.
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
    let Some(round) = best_round_against(registry, state, attacker, weapon, target) else {
        return 0.0;
    };
    let Some(profile) = shot_profile(registry, state, weapon, &round, from, target) else {
        return 0.0;
    };
    let p = hit_chance(registry, state, attacker, from, weapon, tgt.pos, blind) as f32 / 100.0;
    p * round_worth(registry, state, target, &profile, round.ammo)
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
    /// The target's condition — cadets and modules remaining over her full
    /// complement — as a percent. The health bar's successor: there are no
    /// hit points behind it, only the state of what is aboard.
    pub target_condition: i32,
    pub distance: i32,
    /// `false` when the target sits outside the weapon's range band, in
    /// which case the numbers below are hypothetical.
    pub in_range: bool,
    pub hit: HitBreakdown,
    /// Display name of the chambered round, or `None` for a gun whose racks
    /// are empty — in which case every number below it is zero and the
    /// player is being told exactly why.
    pub ammo: Option<String>,
    /// Chance the round defeats the plate it would strike, as a percent.
    pub pen_chance: i32,
    /// Damage a *penetrating* hit would deal. A hit that does not get
    /// through deals nothing at all.
    pub damage: i32,
    /// Which armour arc the shot lands on.
    pub facing: ArmorFacing,
    /// The plate after obliquity: what the round actually has to beat,
    /// rounded for display.
    pub effective_armor: i32,
    /// Hit chance x penetration chance x damage.
    pub expected_damage: f32,
    /// Whether a penetrating hit would finish the target outright.
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

/// Whether `unit`'s weapon at `index` has finished reloading — and, for
/// the primary mount (index 0), whether the gun module is still a gun. A
/// future data field can map modules to mounts when a vehicle needs finer
/// wiring; today the machine gun keeps chattering after the main gun dies,
/// which is the right picture for every vehicle in the base mod.
pub fn weapon_ready(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    index: usize,
) -> bool {
    let Some(u) = state.unit(unit) else {
        return false;
    };
    if index == 0 && !u.module_ok(registry, crate::data::ModuleEffect::Gun) {
        return false;
    }
    u.cooldowns.get(index).copied().is_some_and(|cd| cd == 0)
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
    // A dry gun previews honestly: the round is named as missing and every
    // consequence of it is zero, which tells the player exactly why the
    // shot she is hovering cannot happen.
    let round = best_round_against(registry, state, attacker, weapon, target);
    let profile = round
        .as_ref()
        .and_then(|r| shot_profile(registry, state, weapon, r, att.pos, target));
    let facing = struck_facing(tgt.pos, tgt.facing, att.pos);
    let (pen_chance, damage, effective_armor) = profile
        .as_ref()
        .map(|p| {
            (
                (p.pen_chance * 100.0).round() as i32,
                p.damage,
                p.effective_armor.round() as i32,
            )
        })
        .unwrap_or((0, 0, tgt_vehicle.armor.value(facing).max(0)));
    let ammo = round.as_ref().map(|r| match r.ammo {
        Some(a) => a.name.clone(),
        None => weapon.name.clone(),
    });

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
                        && weapon_ready(registry, state, target, *i)
                })
                .map(|(_, w)| {
                    let damage = best_round_against(registry, state, target, w, attacker)
                        .and_then(|r| shot_profile(registry, state, w, &r, tgt.pos, attacker))
                        .map(|p| (p.pen_chance * p.damage as f32).round() as i32)
                        .unwrap_or(0);
                    CounterPreview {
                        weapon_name: w.name.clone(),
                        hit_chance: hit_chance(registry, state, target, tgt.pos, w, att.pos, false),
                        damage,
                    }
                })
        } else {
            None
        }
    };

    let pen = pen_chance as f32 / 100.0;
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
        target_condition: (state.condition(registry, tgt) * 100.0).round() as i32,
        distance,
        in_range,
        expected_damage: hit.total as f32 / 100.0 * pen * damage as f32,
        lethal: damage as u32 >= state.substance(registry, tgt).0 && pen > 0.0,
        hit,
        ammo,
        pen_chance,
        damage,
        facing,
        effective_armor,
        counter,
    })
}

/// Resolve one shot from `attacker` at `target`. Damage lands immediately but
/// death does not: see [`reap`]. Reloading, ammunition spend and fog are
/// handled by the callers below.
///
/// Two dice, in order: does it hit, then does it get through. A hit that
/// does not penetrate does **nothing structural** — no chip, no floor —
/// which is the single most consequential rule in the rewrite. What a
/// bounce does do is announce itself ([`Event::ShotBounced`]) and, for
/// anything heavier than small arms, rattle the crew through the morale
/// ladder: the plate held, and they still heard it arrive.
#[allow(clippy::too_many_arguments)]
fn resolve_shot(
    registry: &DataRegistry,
    state: &mut BattleState,
    attacker: UnitId,
    weapon: &WeaponDef,
    round: &Round<'_>,
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

    resolve_impact(
        registry, state, attacker, weapon, round, att_pos, target, events,
    );
}

/// Everything a round does once it is known to have arrived: the plate, the
/// gate, and what it finds behind it.
///
/// Split out of [`resolve_shot`] because a shell has no hit roll to precede
/// it and no attacker whose *current* position means anything — she may have
/// driven off or died in the ticks the shell was in the air. `from` is
/// therefore passed rather than looked up: for direct fire it is where the
/// shooter stands, and for a shell it is where the gun stood when it fired,
/// which is the bearing the round arrives on in this game's geometry.
// Eight parameters, and the dependency list is genuinely eight things: the
// content, the world, the shooter, the gun, the round, the bearing, the
// victim, the log. CLAUDE.md's note about a future `Shot` struct is the
// right cleanup if penetration grows more inputs; inventing one for this
// alone would only move the list.
#[allow(clippy::too_many_arguments)]
fn resolve_impact(
    registry: &DataRegistry,
    state: &mut BattleState,
    attacker: UnitId,
    weapon: &WeaponDef,
    round: &Round<'_>,
    from: Hex,
    target: UnitId,
    events: &mut Vec<Event>,
) {
    let Some(tgt_pos) = state.unit(target).map(|t| t.pos) else {
        return;
    };
    let Some(profile) = shot_profile(registry, state, weapon, round, from, target) else {
        return;
    };
    let pen = round.pen_at(from.distance_to(tgt_pos), weapon.range);
    if !penetration_roll(
        state,
        pen,
        profile.effective_armor,
        registry.balance.pen_scatter,
    ) {
        events.push(Event::ShotBounced {
            attacker,
            target,
            facing: profile.facing,
            rattled: !round.small_arms,
        });
        // The designer's overpressure ruling: a bursting charge that fails
        // the gate still delivers its blast to what lives OUTSIDE the
        // plate. Small arms carry no blast, so nothing special-cases them.
        if let Some(ammo) = round.ammo
            && ammo.blast > 0
        {
            overpressure(registry, state, target, ammo.blast, from, events);
        }
        return;
    }

    let damage_type = round.damage_type().unwrap_or(weapon.damage_type);
    if let Some(tgt) = state.unit_mut(target) {
        // Remembered so that, if this is the hit that kills it, the campaign
        // can ask what actually went through the crew compartment. A kinetic
        // penetration and a machine gun finishing off a burning wreck are
        // very different days for the cadets inside.
        tgt.last_hit_by = Some(damage_type);
    }
    events.push(Event::ShotHit {
        attacker,
        target,
        damage: profile.damage,
        facing: profile.facing,
    });
    behind_armor_effects(registry, state, round, &profile, target, events);
}

/// What one cadet or one module weighs when a penetration rolls what it
/// found inside. Seats before modules, in seat order, then module id
/// (BTreeMap) order — the walk is fixed so replays agree on who was hit.
fn interior(registry: &DataRegistry, unit: &super::Unit) -> Vec<(InteriorChoice, u32)> {
    let mut targets = Vec::new();
    let crew_weight = registry.balance.crew_weight.max(0) as u32;
    for (seat, _) in unit.crew.iter().enumerate() {
        let condition = unit
            .crew_state
            .get(seat)
            .copied()
            .unwrap_or(super::CrewCondition::Fine);
        // Out and absent both weigh nothing, for opposite reasons: one has
        // already been found by a previous roll and the other was never in
        // the vehicle to find.
        if condition.fighting() && crew_weight > 0 {
            targets.push((InteriorChoice::Seat(seat), crew_weight));
        }
    }
    for (id, hits) in &unit.modules {
        if *hits > 0
            && let Some(module) = registry.module(id)
            && module.size > 0
        {
            targets.push((InteriorChoice::Module(id.clone()), module.size));
        }
    }
    targets
}

/// One thing a behind-armor effect roll can land on: a seat (an index
/// into the unit's crew list) or a module by id. Owned, so the picked
/// list outlives the borrow of the unit it was read from.
/// Spend `rolls` weighted picks against what is aboard `target` — the
/// shared engine behind a penetration's interior budget and a shellburst's
/// splash over soft, dispersed things. Stops early on a brew-up or an
/// emptied interior.
fn effect_rolls(
    registry: &DataRegistry,
    state: &mut BattleState,
    target: UnitId,
    rolls: i32,
    savage: bool,
    potency: f32,
    events: &mut Vec<Event>,
) {
    for _ in 0..rolls {
        let Some(unit) = state.unit(target) else {
            return;
        };
        if unit.brewed {
            break;
        }
        // The pool is everything physically inside the hull: the target's
        // own crew and modules, and — the shared-fate ruling — every
        // passenger's cadets and troops too, in passenger id order. A round
        // that comes through a loaded carrier does not check tickets.
        let mut targets: Vec<(UnitId, InteriorChoice, u32)> = interior(registry, unit)
            .into_iter()
            .map(|(choice, w)| (target, choice, w))
            .collect();
        for rider in state.passengers(target) {
            if let Some(r) = state.unit(rider) {
                targets.extend(
                    interior(registry, r)
                        .into_iter()
                        .map(|(choice, w)| (rider, choice, w)),
                );
            }
        }
        let total: u32 = targets.iter().map(|(_, _, w)| w).sum();
        if total == 0 {
            break;
        }
        let mut pick = state.rng.random_range(0..total);
        let mut chosen = None;
        for (who, candidate, weight) in targets {
            if pick < weight {
                chosen = Some((who, candidate));
                break;
            }
            pick -= weight;
        }
        match chosen.expect("total > 0 guarantees a pick") {
            (who, InteriorChoice::Seat(seat)) => crew_hit(state, who, seat, savage, events),
            (who, InteriorChoice::Module(id)) => {
                module_hit(registry, state, who, &id, potency, events);
            }
        }
    }
}

enum InteriorChoice {
    Seat(usize),
    Module(String),
}

/// Spend a penetration's behind-armor budget on what it found inside, then
/// ask the crew whether they are staying.
///
/// The budget is the ledger damage the gate already computed — the weapon's
/// weight times the round's potency — cashed as one effect roll per
/// `points_per_effect`, rounded up. Each roll picks a cadet or a module,
/// weighted by size; a heavily overmatching round (double the budget the
/// knob asks for, per roll) puts a cadet straight out rather than wounding
/// her first. The ammunition rack is the special module: every hit on it
/// rolls brew-up at `brewup_percent` scaled by how full the racks still
/// are, and a rack destroyed without a fire leaves the remaining rounds
/// unusable.
///
/// Then the human question. Every penetration rolls the crew's discipline
/// through the same `holds_together` check that governs refusing orders;
/// a crew that fails abandons the vehicle. That single rule is what keeps
/// time-to-kill honest without hit points — tanks are mostly lost because
/// the crew leaves or dies, not because every box inside is ticked — and
/// it is what makes discipline training visibly be the thing that keeps a
/// damaged tank in the fight.
fn behind_armor_effects(
    registry: &DataRegistry,
    state: &mut BattleState,
    round: &Round<'_>,
    profile: &ShotProfile,
    target: UnitId,
    events: &mut Vec<Event>,
) {
    let per_effect = registry.balance.points_per_effect.max(1);
    let rolls = (profile.damage.max(1) + per_effect - 1) / per_effect;
    // Triple the price of a roll arriving in one round is the overmatch
    // that skips "wounded": an 88 or a 105 in the lap has no light
    // version, a 75 does. The threshold was double, which put every gun on
    // the field over it — B5's crew-cost table read 0.1 wounded to 3.0 out
    // per battle, meaning the dramatic middle state effectively never
    // happened and a cadet's first hit was almost always her last.
    let savage = profile.damage >= per_effect * 3;
    effect_rolls(
        registry,
        state,
        target,
        rolls,
        savage,
        round.post_pen_scale(),
        events,
    );

    // The bail-out check, gated exactly the way disobedience is: the rung
    // decides whether nerve is even in question, and only then do the dice
    // ask whether discipline holds. The rung consulted is *prospective* —
    // where this penetration's pressure will put her once the tick's news
    // is priced — because the pressure system pays after fire resolves and
    // a crew does not wait for the ledger to feel the shell that just came
    // through. In base-mod terms: the first penetration leaves a steady
    // crew wavering and nobody jumps; the second puts her at breaking, and
    // whether she stays is the same three-dice discipline check that
    // decides whether a breaking crew still obeys an order. Training is
    // therefore visibly the thing that keeps a twice-holed tank fighting,
    // and a mod with a one-rung ladder has crews that never bail — the
    // gentle game, with no `if` in Rust to switch.
    let Some(unit) = state.unit(target) else {
        return;
    };
    if unit.brewed || unit.abandoned || state.fighting_crew(unit) == 0 {
        return;
    }
    let rules = &registry.morale;
    let prospective = unit
        .pressure
        .saturating_add(rules.hit)
        .saturating_add(rules.penetrated);
    if rules.rung(prospective).obeys {
        return;
    }
    let level = state.roster.crew_skill(
        registry,
        registry.vehicle(&unit.vehicle),
        &unit.crew,
        &rules.skill,
        state.terrain_at(unit.pos),
    );
    if !crate::data::holds_together(&mut state.rng, level)
        && let Some(unit) = state.unit_mut(target)
    {
        unit.abandoned = true;
        events.push(Event::Abandoned { unit: target });
    }
}

/// One effect roll found a cadet.
fn crew_hit(
    state: &mut BattleState,
    target: UnitId,
    seat: usize,
    savage: bool,
    events: &mut Vec<Event>,
) {
    let Some(unit) = state.unit_mut(target) else {
        return;
    };
    if unit.crew_state.len() < unit.crew.len() {
        unit.crew_state
            .resize(unit.crew.len(), super::CrewCondition::Fine);
    }
    let Some(cadet) = unit.crew.get(seat).copied() else {
        return;
    };
    let Some(condition) = unit.crew_state.get_mut(seat) else {
        return;
    };
    let out = savage || *condition == super::CrewCondition::Wounded;
    *condition = if out {
        super::CrewCondition::Out
    } else {
        super::CrewCondition::Wounded
    };
    events.push(Event::CrewHit {
        unit: target,
        cadet,
        out,
    });
}

/// One effect roll found a module — or blast found one from outside.
fn module_hit(
    registry: &DataRegistry,
    state: &mut BattleState,
    target: UnitId,
    module_id: &str,
    potency: f32,
    events: &mut Vec<Event>,
) {
    let Some(module) = registry.module(module_id).cloned() else {
        return;
    };
    let destroyed = {
        let Some(unit) = state.unit_mut(target) else {
            return;
        };
        let Some(hits) = unit.modules.get_mut(module_id) else {
            return;
        };
        *hits = hits.saturating_sub(1);
        *hits == 0
    };
    events.push(Event::ModuleHit {
        unit: target,
        module: module_id.to_string(),
        destroyed,
    });

    if module.effect == crate::data::ModuleEffect::Ammo {
        // Every rack hit rolls the fire. Three data axes shape the chance,
        // none of them a property of the roll itself:
        //
        // - WHAT is aboard, volatility-weighted: a rack of high explosive
        //   is a bomb waiting for a reason, solid shot is metal ahead of a
        //   charge. The fraction compares what is left against what the
        //   chassis stows, both weighted, so an emptied tank is measurably
        //   harder to torch and an HE-heavy loadout was a choice with a
        //   price.
        // - The round's own behind-armor potency.
        // - The VEHICLE's `safety` — wet stowage in one number, shaving
        //   `brew_safety_percent` points per level, which is what finally
        //   makes "designed around her crew" mean something while the
        //   shooting is still happening rather than only at the fate rolls.
        let (fraction_num, fraction_den, safety) = {
            let Some(unit) = state.unit(target) else {
                return;
            };
            let volatile = |id: &str, count: u32| -> f32 {
                count as f32
                    * registry
                        .ammo(id)
                        .map(|a| a.volatility.max(0.0))
                        .unwrap_or(1.0)
            };
            let aboard: f32 = unit.ammo.iter().map(|(id, n)| volatile(id, *n)).sum();
            let vehicle = registry.vehicle(&unit.vehicle);
            let capacity: f32 = vehicle
                .map(|v| v.stowage.iter().map(|(id, n)| volatile(id, *n)).sum())
                .unwrap_or(0.0);
            (aboard, capacity, vehicle.map(|v| v.safety).unwrap_or(0))
        };
        if fraction_den > 0.0 && fraction_num > 0.0 {
            let stowage = (100 - safety.max(0) * registry.balance.brew_safety_percent.max(0))
                .clamp(0, 100) as f32
                / 100.0;
            let chance = (registry.balance.brewup_percent.max(0) as f32
                * potency
                * stowage
                * (fraction_num / fraction_den)) as u32;
            let roll = state.rng.random_range(0..100u32);
            if roll < chance {
                if let Some(unit) = state.unit_mut(target) {
                    unit.brewed = true;
                }
                events.push(Event::BrewedUp { unit: target });
                return;
            }
        }
        if destroyed {
            // The racks are wrecked but did not light: what is left in them
            // is jammed, scattered, unusable. The guns that fed from them
            // go quiet, and each says so exactly once.
            let dry_weapons: Vec<String> = {
                let Some(unit) = state.unit(target) else {
                    return;
                };
                registry
                    .vehicle(&unit.vehicle)
                    .map(|v| {
                        v.weapons
                            .iter()
                            .filter_map(|w| registry.weapon(w))
                            .filter(|w| {
                                !w.ammo.is_empty()
                                    && w.ammo
                                        .iter()
                                        .any(|id| unit.ammo.get(id).copied().unwrap_or(0) > 0)
                            })
                            .map(|w| w.id.clone())
                            .collect()
                    })
                    .unwrap_or_default()
            };
            if let Some(unit) = state.unit_mut(target) {
                for count in unit.ammo.values_mut() {
                    *count = 0;
                }
            }
            for weapon in dry_weapons {
                events.push(Event::WeaponDry {
                    unit: target,
                    weapon,
                });
            }
        }
    }
}

/// Blast against a plate it could not get through: the exterior — running
/// gear, antennas — at a chance shaped by blast against armor, and blast
/// overmatch wrecking thin-skinned vehicles outright. A recon car under a
/// 105 is not a bounce.
///
/// The plate consulted is the one the burst actually faces, from `from` —
/// the same quantisation every shot uses. The first draft asked the hull's
/// THINNEST plate ("blast does not aim"), and the B5 instrument showed what
/// that means in numbers: a 105's blast of six overmatched even the heavy
/// tank's rear three, so a shell bouncing off her glacis wrecked her
/// through a plate the burst never touched — artillery needed 1.8 shells
/// per heavy tank while its penetration table read zero. Blast does not
/// aim, but it also does not wrap a sixty-ton hull; the bearing rule keeps
/// a frontal burst a frontal problem and makes ground that lets a shell
/// arrive BEHIND a tank worth paying for.
fn overpressure(
    registry: &DataRegistry,
    state: &mut BattleState,
    target: UnitId,
    blast: i32,
    from: Hex,
    events: &mut Vec<Event>,
) {
    let (plate, exterior) = {
        let Some(unit) = state.unit(target) else {
            return;
        };
        let Some(vehicle) = registry.vehicle(&unit.vehicle) else {
            return;
        };
        let plate = vehicle
            .armor
            .value(struck_facing(unit.pos, unit.facing, from))
            .max(0);
        (plate, exterior_modules(registry, unit))
    };
    if plate == 0 {
        // Soft, dispersed things do not have a hull for overpressure to
        // crush: a platoon is thirty people behind folds in the ground,
        // not a box. Splash against plate zero converts to casualty rolls
        // through the same weighted interior machinery a penetration uses
        // — artillery against infantry is attrition, brutal attrition,
        // never a single-event erasure. (Direct hits never reach here:
        // against no armor the gate always passes and the full budget
        // rolls in the ordinary way.)
        let per_effect = registry.balance.points_per_effect.max(1);
        let rolls = (blast.max(1) + per_effect - 1) / per_effect;
        effect_rolls(registry, state, target, rolls, false, 1.0, events);
        return;
    }
    if blast_overmatches(blast, plate) {
        // Overmatch: the shell does not need the gate's permission. Reap
        // folds the flag into `alive` at the end of the tick and announces
        // the destruction, the same simultaneity bargain every other death
        // keeps.
        if let Some(unit) = state.unit_mut(target) {
            unit.wrecked = true;
        }
        return;
    }
    let total: u32 = exterior.iter().map(|(_, w)| w).sum();
    if total == 0 {
        return;
    }
    let chance = overpressure_chance(blast, plate);
    if state.rng.random_range(0..100) >= chance {
        return;
    }
    let mut pick = state.rng.random_range(0..total);
    for (id, weight) in &exterior {
        if pick < *weight {
            module_hit(registry, state, target, id, 1.0, events);
            return;
        }
        pick -= weight;
    }
}

/// Take the wrecks off the board at the end of a tick.
///
/// Nothing is removed while a tick is still resolving, which is what lets two
/// crews that fired at each other in the same instant both get their shot off
/// — and both lose. Deciding it by whoever happened to be processed first
/// would be an artefact of the loop order, not a rule of the game.
///
/// What ends a vehicle, now that there are no hit points: her ammunition
/// went up, blast overmatch crushed her, her crew walked away, or nobody
/// aboard can fight any more. All four leave the board as
/// [`Event::UnitDestroyed`] — the scoring consumers want one word for
/// "she is a loss" — with the *why* already told by the events that set
/// the flags. A vehicle that is merely mission-killed, gun and tracks
/// gone with the crew grimly aboard, stays alive: recovering her is a
/// campaign story, not a contradiction.
pub fn reap(registry: &DataRegistry, state: &mut BattleState, events: &mut Vec<Event>) {
    let done: Vec<(UnitId, Hex)> = state
        .units
        .iter()
        .filter(|u| {
            // The crew clause only applies to a vehicle that HAS a crew
            // list: an empty one means the cadets are abstracted away (test
            // scaffolding, a mod without a roster), and "everyone aboard
            // nobody is out" must read as today's game, not as a ghost
            // ship.
            u.alive
                && (u.brewed
                    || u.wrecked
                    || u.abandoned
                    || (!u.crew.is_empty() && state.fighting_crew(u) == 0))
        })
        .map(|u| (u.id, u.pos))
        .collect();
    for (id, at) in done {
        let brewed = state.units.get(id.index()).is_some_and(|u| u.brewed);
        if let Some(unit) = state.units.get_mut(id.index()) {
            unit.alive = false;
        }
        events.push(Event::UnitDestroyed { unit: id, at });
        // The ride ending the hard way. A brew rolls every passenger
        // through the fire — three savage picks each, the flames not
        // caring who — and whoever is left picks herself up beside the
        // wreck; any other death simply puts them on the ground where it
        // happened. A passenger the fire finishes is reaped the next tick,
        // by the same pass, like every other death.
        for rider in state.passengers(id) {
            if brewed {
                effect_rolls(registry, state, rider, 3, true, 1.0, events);
            }
            let survivor = state
                .unit(rider)
                .is_some_and(|u| !u.crew.is_empty() && state.fighting_crew(u) > 0);
            let ground = state.units.get(id.index()).map(|c| c.pos).unwrap_or(at);
            if let Some(u) = state.units.get_mut(rider.index()) {
                u.aboard = None;
                u.dismounting = false;
                u.pos = ground;
            }
            if survivor {
                events.push(Event::Dismounted {
                    unit: rider,
                    at: ground,
                });
            }
        }
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

/// Take the chambered round out of the racks, announcing the moment a gun
/// runs completely dry. Returns `None` — and fires nothing — for a gun
/// whose whole ammunition list is spent; the legacy path spends nothing
/// and never runs dry, because a mod that counts no rounds has infinite
/// ones, which is exactly the game it shipped with.
fn chamber_and_spend<'r>(
    registry: &'r DataRegistry,
    state: &mut BattleState,
    unit: UnitId,
    weapon: &WeaponDef,
    aim: Option<UnitId>,
    events: &mut Vec<Event>,
) -> Option<Round<'r>> {
    // The loader's choice: a confirmed target gets the round that expects
    // the most against it, ground gets the most blast aboard. Same
    // selectors the AI priced the shot with, so what leaves the muzzle is
    // what the plan was worth.
    let round = match aim {
        Some(target) => best_round_against(registry, state, unit, weapon, target)?,
        None => best_round_for_ground(registry, state, unit, weapon)?,
    };
    if let Some(ammo) = round.ammo {
        if let Some(u) = state.unit_mut(unit)
            && let Some(count) = u.ammo.get_mut(&ammo.id)
        {
            *count = count.saturating_sub(1);
        }
        let dry = state.unit(unit).is_some_and(|u| {
            weapon
                .ammo
                .iter()
                .all(|id| u.ammo.get(id).copied().unwrap_or(0) == 0)
        });
        if dry {
            events.push(Event::WeaponDry {
                unit,
                weapon: weapon.id.clone(),
            });
        }
    }
    Some(round)
}

/// Fire one weapon at a unit, spending its reload and a round from the
/// racks, and giving away the shooter's position.
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
    let Some(round) = chamber_and_spend(registry, state, attacker, &weapon, Some(target), events)
    else {
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
    // An indirect gun ordered onto a *unit* still fires at the ground she is
    // standing on right now, because that is all a gunner behind a ridge can
    // do with a map reference. Whether she is still there when it comes down
    // is the target's problem and the whole point of the rework.
    match shell_in_flight(registry, state, attacker, &weapon, &round, tgt_pos) {
        Some(shell) => {
            events.push(Event::ShotFired {
                attacker,
                from: shell.from,
                at: shell.at,
                weapon: weapon.id.clone(),
                blind: false,
                opportunity,
            });
            state.shells.push(shell);
        }
        None => resolve_shot(
            registry,
            state,
            attacker,
            &weapon,
            &round,
            target,
            false,
            opportunity,
            events,
        ),
    }
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
    // Spent whether or not anybody is standing there: shelling empty ground
    // costs the shell.
    let Some(round) = chamber_and_spend(registry, state, attacker, &weapon, None, events) else {
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
    // Shelling ground is what an indirect gun does natively, so this is the
    // path where flight time is least surprising: the shell goes up, and who
    // is under it is settled when it comes down rather than now.
    if let Some(shell) = shell_in_flight(registry, state, attacker, &weapon, &round, at) {
        events.push(Event::ShotFired {
            attacker,
            from: shell.from,
            at: shell.at,
            weapon: weapon.id.clone(),
            blind: true,
            opportunity: false,
        });
        state.shells.push(shell);
    } else {
        match state.unit_at(at).filter(|t| t.side != side).map(|t| t.id) {
            Some(target) => resolve_shot(
                registry, state, attacker, &weapon, &round, target, true, false, events,
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
    // Opportunity fire is the crew reacting to something nobody told them
    // about, so it costs them their reaction time — per TARGET, from the
    // moment he was first seen, not per round from tick zero. The old gate
    // reset every round, which taxed a crew for watching (a target held in
    // sight across five rounds was paid for five times) and waived the tax
    // exactly when it was owed (an ambush at tick seven answered instantly,
    // because seven beat any delay). The clock is the side's
    // `spotted_since`, and the model is cadets.md's oldest sentence: she
    // notices at tick four and does something about it at tick six. Ordered
    // fire is untouched: they knew what they were shooting at before the
    // round began, and taxing that would model rate of fire twice over.
    let delay =
        super::stats::reaction_delay(registry, &state.roster, att, state.terrain_at(att.pos))
            as u64;
    let now = state.round as u64 * registry.scale.ticks_per_round as u64
        + state.resolving_tick().unwrap_or(0) as u64;
    let vehicle = registry.vehicle(&att.vehicle)?;
    let fog = state.fog.side(att.side);
    let spotted = &fog.spotted;
    // Seen long enough that this crew has caught up with the fact of him.
    let reacted_to = |enemy: UnitId| {
        fog.spotted_since
            .get(&enemy)
            .is_none_or(|since| now >= since + delay)
    };

    let mut best: Option<(usize, UnitId, f32)> = None;
    for (index, weapon_id) in vehicle.weapons.iter().enumerate() {
        let Some(weapon) = registry.weapon(weapon_id) else {
            continue;
        };
        if weapon.indirect || !weapon_ready(registry, state, unit, index) {
            continue;
        }
        // Enemies in id order, so ties resolve the same way in every replay.
        // Passengers are not on the field to be shot at.
        for enemy in state
            .alive_units()
            .filter(|e| e.side != att.side && e.aboard.is_none())
        {
            if !spotted.contains(&enemy.id)
                || !reacted_to(enemy.id)
                || !shot_exists(state, weapon, att.pos, enemy.pos, true)
            {
                continue;
            }
            // Worthless shots are not taken, and this is new discipline
            // rather than an optimisation: with the damage floor gone, a
            // gun whose round cannot beat the plate — or whose racks are
            // empty — expects zero, and a crew that used to plink now
            // holds fire and keeps her position quiet instead.
            let value = expected_damage(registry, state, unit, att.pos, weapon, enemy.id, false);
            if value <= 0.0 {
                continue;
            }
            // Ambush discipline: a crew the enemy has not seen does not
            // spend that advantage on a mediocre shot. While unseen, only
            // a shot worth a real fraction of what the target has left is
            // taken — the column is let close until the shot is decisive,
            // which is when a veteran springs an ambush. Read fog-honestly
            // from the ENEMY side's picture of us; ordered fire is exempt
            // (the commander said shoot), and the moment she is spotted
            // the threshold vanishes — a seen crew fights with whatever
            // she has. No prediction is involved: this is patience, not
            // anticipation, and the target-track memory that would enable
            // true wait-for-the-flank reasoning is deliberately future
            // work.
            const AMBUSH_PATIENCE: f32 = 0.25;
            let unseen = !state.fog.side(enemy.side).spotted.contains(&unit);
            if unseen {
                let decisive = state.substance(registry, enemy).0 as f32 * AMBUSH_PATIENCE;
                if value < decisive {
                    continue;
                }
            }
            if best.is_none_or(|(_, _, v)| value > v) {
                best = Some((index, enemy.id, value));
            }
        }
    }
    best.map(|(w, t, _)| (w, t))
}

/// One crew's shooting decision for a tick, made before anybody's trigger
/// is pulled.
pub enum FireAction {
    AtUnit {
        weapon: usize,
        target: UnitId,
        opportunity: bool,
    },
    AtTile {
        weapon: usize,
        at: Hex,
    },
}

/// What this unit would shoot this tick, judged against the tick's opening
/// state.
///
/// The ordered target comes first. When there is no order, or the order
/// cannot be carried out right now (target destroyed, lost in the fog, out of
/// range, or the gun is still reloading), the crew falls back to opportunity
/// fire. That fallback is what makes return fire happen without a special
/// case for it.
///
/// Deciding is split from firing for the same reason death is reaped at the
/// end of a tick rather than eagerly: a tick is one slice of simultaneous
/// time. When outcomes landed mid-loop, the crew processed first could shoot
/// the gun out of the hands of the crew processed second and cancel a reply
/// that was, in the fiction, already leaving the barrel — mutual destruction
/// became impossible and loop order became a rule of the game. So every
/// decision is taken against the tick's opening state, and only then does
/// anything resolve.
pub fn fire_decision(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
) -> Option<FireAction> {
    let att = state.unit(unit)?;
    // Nobody shoots from inside a carrier: firing ports are a doctrine
    // argument for a later pass, and today the ride is a ride.
    if att.aboard.is_some() {
        return None;
    }
    let (intent, att_pos, side) = (att.intent.fire, att.pos, att.side);

    match intent {
        FireIntent::Target { target, weapon } => {
            let spotted = state.fog.side(side).spotted.contains(&target);
            let ordered_shot = weapon_ready(registry, state, unit, weapon)
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
                return Some(FireAction::AtUnit {
                    weapon,
                    target,
                    opportunity: false,
                });
            }
        }
        FireIntent::Area { at, weapon } => {
            let can_shell = weapon_ready(registry, state, unit, weapon)
                && weapon_at(registry, state, unit, weapon)
                    .is_some_and(|w| shot_exists(state, w, att_pos, at, true));
            if can_shell {
                return Some(FireAction::AtTile { weapon, at });
            }
        }
        FireIntent::Hold => {}
    }

    best_opportunity_shot(registry, state, unit).map(|(weapon, target)| FireAction::AtUnit {
        weapon,
        target,
        opportunity: true,
    })
}

/// Carry out a decision [`fire_decision`] made. The racks are consulted at
/// execution — a rack destroyed earlier in the same tick leaves this shot
/// silently unfired, which is as far as the simultaneity bargain stretches:
/// a decision survives the loop, a shell does not survive its magazine.
pub fn execute_fire(
    registry: &DataRegistry,
    state: &mut BattleState,
    unit: UnitId,
    action: FireAction,
    events: &mut Vec<Event>,
) {
    if state.unit(unit).is_none_or(|u| !u.alive) {
        return;
    }
    match action {
        FireAction::AtUnit {
            weapon,
            target,
            opportunity,
        } => fire_at_unit(registry, state, unit, weapon, target, opportunity, events),
        FireAction::AtTile { weapon, at } => {
            fire_at_tile(registry, state, unit, weapon, at, events)
        }
    }
}
