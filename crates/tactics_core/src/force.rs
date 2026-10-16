//! Buying an army: what a commander brings to a battle, for a budget.
//!
//! Every battle so far has been fought with the order of battle its map wrote
//! down, which is right for a scenario and wrong for a balance harness — it
//! measures one hand-picked matchup forever, and it cannot answer the question
//! the content most needs answered, which is whether the *points* are honest.
//! Give two doctrines the same budget, let each buy what it likes, and fight
//! them: if one wins reliably, either its shopping list is better than the
//! other's or somebody's `cost` is wrong, and both are things worth knowing.
//!
//! # What decides the buy
//!
//! **Doctrine, which is how this game models a commander's judgment.** A side
//! already carries one — it is what her planner scores tiles with — and the
//! same weights that say how she *fights* are read here to say what she
//! *brings*. Nothing else is consulted, and nothing is random: the same
//! commander with the same budget fields the same army every time, which is
//! what makes a harness result reproducible.
//!
//! **Roles are recognised off the hardware, never declared.** This is the rule
//! the AI already follows for a base of fire (`lays_indirect`) and an infantry
//! element (`goes_on_foot`), and it is why a mod that adds a mortar section or
//! a paratroop platoon gets sensible buying on the day it is written, with no
//! new field and nothing in Rust naming a vehicle. [`Role`] is one chassis
//! fact each, checked in the order that a more specific reading beats a more
//! general one.
//!
//! **The data's `cost` is taken as fair.** This picker does not hunt for
//! value-for-money — dividing appetite by price makes every doctrine buy a
//! swarm of the cheapest thing on the roster, which is a statement about
//! pricing rather than about doctrine. It buys the thing it *wants* and pays
//! the asking price. So when the mustered-forces table shows one doctrine
//! beating another at equal points, that is a real signal about the costs, and
//! it is not hidden behind a shopping algorithm that was already exploiting
//! them.

use crate::data::{
    DataRegistry, DoctrineDef, ForceDef, ForceUnit, MovementClass, PlatoonDef, VehicleDef,
};

/// What a chassis is *for*, read off the chassis.
///
/// One fact each, and the order below is the priority: a self-propelled gun
/// that also carries a section is a gun first, and a battle taxi with a good
/// view is a taxi first. Ties are impossible by construction, which matters
/// because the whole point is that nobody has to write down what a vehicle is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Shoots over things. The base of fire.
    Guns,
    /// Fights on its feet.
    Infantry,
    /// Carries somebody. Wanted in proportion to the infantry bought, never
    /// for its own sake, which is why it has no appetite of its own.
    Lift,
    /// Sees farther than it shoots — the relationship this project states as
    /// a design rule, and the honest definition of a scout. A gun tank is
    /// deliberately the other way round.
    Eyes,
    /// Armour and a gun: what an army is mostly made of.
    Armour,
}

/// Which role this chassis fills.
pub fn role_of(registry: &DataRegistry, vehicle: &VehicleDef) -> Role {
    let indirect = vehicle
        .weapons
        .iter()
        .filter_map(|w| registry.weapon(w))
        .any(|w| w.indirect);
    if indirect {
        return Role::Guns;
    }
    if vehicle.movement.class == MovementClass::Foot {
        return Role::Infantry;
    }
    if vehicle.capacity > 0 {
        return Role::Lift;
    }
    let reach = vehicle
        .weapons
        .iter()
        .filter_map(|w| registry.weapon(w))
        .map(|w| w.range[1])
        .max()
        .unwrap_or(0);
    if vehicle.vision_range > reach {
        return Role::Eyes;
    }
    Role::Armour
}

/// How much this doctrine wants one more of that role.
///
/// Each arm reads the weight that already names the appetite — a doctrine
/// with a high `indirect_appetite` wants guns; one that prizes cover wants
/// the people who live in it; one that scouts wants eyes. Armour is
/// `1.0 + aggression` rather than plain `aggression` because armour and a gun
/// is the *default* an army is made of and every doctrine buys some: the
/// weight says how much further it leans that way, not whether it leans at
/// all. Without the base the aggression scale (0..1) would lose to
/// `cover_value` (up to 2.2) for every doctrine on the roster and nobody
/// would ever buy a tank.
///
/// [`Role::Lift`] inherits the infantry appetite. A taxi is bought because
/// there are troops to carry, never because a commander likes taxis, and
/// pricing it the same as its passengers is what makes a doctrine that wants
/// infantry also want them to arrive.
fn appetite(doctrine: &DoctrineDef, role: Role) -> f32 {
    match role {
        Role::Guns => doctrine.indirect_appetite,
        Role::Infantry | Role::Lift => doctrine.cover_value,
        Role::Eyes => doctrine.scouting,
        Role::Armour => 1.0 + doctrine.aggression,
    }
}

/// Buy an army for `budget` points, the way `doctrine` would.
///
/// Greedy, and deliberately so: a commander picks the thing she most wants
/// that she can still afford, then asks again. Two rules shape the result
/// into a force rather than a heap:
///
/// - **Wanting one is not wanting six.** Each copy already owned divides the
///   appetite for the next, at a rate `concentration` sets — the doctrine
///   weight that says whether this commander masses or disperses, and one of
///   the two the design doc noted as unused. Massed armour at 1.8 barely
///   decays and fields a fist; recon pull at 0.4 decays hard and fields a
///   spread of everything.
/// - **Lift follows troops.** A transport is only considered while there is
///   infantry aboard nothing, so a force never buys a taxi it has nobody to
///   put in, and never buys a second for the same platoon.
///
/// Ties inside a role go to the more expensive chassis: given the points, you
/// bring the better example of the thing you decided you wanted. Everything
/// here is total and ordered, so the same doctrine and budget always produce
/// the same list, in the same order — a harness that shuffled its armies
/// between runs could not attribute a result to anything.
pub fn muster(registry: &DataRegistry, doctrine: &DoctrineDef, budget: i32) -> Vec<String> {
    // Sorted by id so the roster's own map order — which is a `HashMap`'s —
    // can never reach the shopping list. This is the determinism rule applied
    // to a decision, and a force that differed by hash seed would poison every
    // number the harness printed downstream.
    let mut roster: Vec<&VehicleDef> = registry.vehicles.values().collect();
    roster.sort_by(|a, b| a.id.cmp(&b.id));

    // How fast a second copy of the same thing loses its shine. Clamped at
    // both ends: a doctrine that massed without limit would field one chassis
    // and nothing else, and one that dispersed without limit would refuse a
    // second tank while a first taxi was still on offer.
    let decay = (1.5 - doctrine.concentration).clamp(0.15, 1.25);

    let mut bought: Vec<String> = Vec::new();
    let mut owned: std::collections::BTreeMap<&str, u32> = Default::default();
    let mut spent = 0;
    let (mut foot, mut lift) = (0u32, 0u32);

    loop {
        let mut best: Option<(f32, i32, &VehicleDef)> = None;
        for vehicle in &roster {
            if vehicle.cost <= 0 || spent + vehicle.cost > budget {
                continue;
            }
            let role = role_of(registry, vehicle);
            // Nobody to put in it, or everybody already has a ride.
            if role == Role::Lift && lift >= foot {
                continue;
            }
            let have = owned.get(vehicle.id.as_str()).copied().unwrap_or(0);
            let want = appetite(doctrine, role) / (1.0 + have as f32 * decay);
            if want <= 0.0 {
                continue;
            }
            // Strictly greater on the appetite, then on price: the first
            // comparison decides the role, the second decides which example
            // of it, and the sorted roster decides a genuine dead heat.
            let better = match best {
                None => true,
                Some((w, c, _)) => want > w || (want == w && vehicle.cost > c),
            };
            if better {
                best = Some((want, vehicle.cost, vehicle));
            }
        }
        let Some((_, cost, vehicle)) = best else {
            break;
        };
        spent += cost;
        *owned.entry(vehicle.id.as_str()).or_default() += 1;
        match role_of(registry, vehicle) {
            Role::Infantry => foot += 1,
            Role::Lift => lift += 1,
            _ => {}
        }
        bought.push(vehicle.id.clone());
    }
    bought
}

/// What a force costs, for printing beside a budget. Unknown chassis are
/// free rather than fatal, the same forgiveness the rest of the loader has.
pub fn cost_of(registry: &DataRegistry, force: &[String]) -> i32 {
    force
        .iter()
        .filter_map(|id| registry.vehicle(id))
        .map(|v| v.cost)
        .sum()
}

/// What one platoon on sale costs: its vehicles' `cost`, and its
/// passengers'.
pub fn platoon_cost(registry: &DataRegistry, platoon: &PlatoonDef) -> i32 {
    let each = |id: &str| registry.vehicle(id).map_or(0, |v| v.cost);
    let rider = platoon.unit.carries.as_deref().map_or(0, each);
    platoon.unit.count as i32 * (each(&platoon.unit.vehicle) + rider)
}

/// What a bought force may spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Purchase {
    /// Points, spent at the chassis' asking price.
    pub points: i32,
    /// The most vehicles it may field (passengers not counted): a budget
    /// spent on the cheapest platoons would otherwise field a crowd the
    /// deployment has no room for.
    pub max_vehicles: u32,
}

/// A force bought for `purchase` from the mod's platoon catalogue
/// (`platoons/`), the draws seeded by `seed`: a platoon at a time, each
/// drawn evenly from those still affordable, until none is. `None` when
/// nothing on sale is affordable at all.
///
/// The draw is even rather than by a doctrine's appetite on purpose. What a
/// doctrine would buy is [`muster`]'s question, and it gives one army per
/// doctrine and budget: a handful of matchups, which is what the learner is
/// to be kept from memorising. Here the army comes first and the doctrine
/// follows from it ([`doctrine_for`], the designer's ruling that a doctrine
/// comes from what a side has).
///
/// The platoons deploy eyes first, then armour, then the carriers and
/// whoever walks, then the guns: the order a force advances in, and tanks
/// before carriers was most of what the motor rifle company was missing
/// (2026-10-15). Within a role, in the order they were bought.
pub fn buy(registry: &DataRegistry, purchase: Purchase, seed: u64) -> Option<ForceDef> {
    let catalogue = registry.platoon_catalogue();
    let mut state = seed ^ 0xB0_06_47;
    let mut bought: Vec<&PlatoonDef> = Vec::new();
    let (mut spent, mut vehicles) = (0, 0);
    loop {
        let affordable: Vec<&PlatoonDef> = catalogue
            .iter()
            .copied()
            .filter(|p| {
                spent + platoon_cost(registry, p) <= purchase.points
                    && vehicles + p.unit.count <= purchase.max_vehicles
            })
            .collect();
        if affordable.is_empty() {
            break;
        }
        state = mix(state);
        let pick = affordable[(state % affordable.len() as u64) as usize];
        spent += platoon_cost(registry, pick);
        vehicles += pick.unit.count;
        bought.push(pick);
    }
    if bought.is_empty() {
        return None;
    }
    let rank = |p: &PlatoonDef| match registry
        .vehicle(&p.unit.vehicle)
        .map(|v| role_of(registry, v))
    {
        Some(Role::Eyes) => 0,
        Some(Role::Armour) => 1,
        Some(Role::Lift | Role::Infantry) | None => 2,
        Some(Role::Guns) => 3,
    };
    bought.sort_by_key(|p| rank(p));
    let units: Vec<ForceUnit> = bought.iter().map(|p| p.unit.clone()).collect();
    let doctrine = doctrine_for(registry, &units)?;
    Some(ForceDef {
        id: format!("bought_{doctrine}"),
        name: "Bought force".into(),
        description: format!("{spent} points of platoons drawn from the catalogue"),
        doctrine,
        units,
    })
}

/// The doctrine a force fights by, read from what it has: of the doctrines
/// the mod's own forces fight by (the ones written to be played), the one
/// whose appetite for each role ([`muster`]'s weights) is the nearest match
/// for the share of its vehicles in that role, a tie going to the id.
///
/// Compared over the roles the platoon catalogue sells, so a doctrine is
/// not marked down for wanting guns nobody could buy; walking troops and
/// their carriers are one role here, since a carrier is bought for its
/// passengers. A passenger is not counted again beside her carrier.
///
/// Not [`muster`]'s own shopping list: that shops the whole roster, chassis
/// the catalogue does not sell included, and read that way 69% of bought
/// forces fought by `recon_pull` and none by `bounding_overwatch`.
pub fn doctrine_for(registry: &DataRegistry, units: &[ForceUnit]) -> Option<String> {
    // Walking troops and lift share a bucket, as they share an appetite.
    let bucket = |role: Role| match role {
        Role::Guns => 0,
        Role::Infantry | Role::Lift => 1,
        Role::Eyes => 2,
        Role::Armour => 3,
    };
    let role = |id: &str| registry.vehicle(id).map(|v| bucket(role_of(registry, v)));
    let mut sold = [false; 4];
    for platoon in registry.platoon_catalogue() {
        if let Some(b) = role(&platoon.unit.vehicle) {
            sold[b] = true;
        }
    }
    let mut have = [0f32; 4];
    for unit in units {
        if let Some(b) = role(&unit.vehicle) {
            have[b] += unit.count as f32;
        }
    }
    let share = |v: [f32; 4]| {
        let total: f32 = (0..4)
            .filter(|&b| sold[b])
            .map(|b| v[b])
            .sum::<f32>()
            .max(f32::EPSILON);
        v.map(|x| x / total)
    };
    let have = share(have);
    let mut played: Vec<&str> = registry
        .forces
        .values()
        .map(|f| f.doctrine.as_str())
        .collect();
    played.sort();
    played.dedup();
    played
        .into_iter()
        .filter_map(|id| Some((id, registry.doctrine(id)?)))
        .map(|(id, doctrine)| {
            let wants = share([
                appetite(doctrine, Role::Guns),
                appetite(doctrine, Role::Infantry),
                appetite(doctrine, Role::Eyes),
                appetite(doctrine, Role::Armour),
            ]);
            let distance: f32 = (0..4)
                .filter(|&b| sold[b])
                .map(|b| (have[b] - wants[b]).abs())
                .sum();
            (distance, id)
        })
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(b.1)))
        .map(|(_, id)| id.to_string())
}

/// SplitMix64, written out: a bought force must not move with the platform.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
