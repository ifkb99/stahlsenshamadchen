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

use crate::data::{DataRegistry, DoctrineDef, MovementClass, VehicleDef};

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
