//! Movement: reachability (Dijkstra) and pathing (A*), both aware of
//! terrain costs per movement class, elevation climb limits, and occupancy.

use super::{BattleState, Unit, UnitId};
use crate::data::{DataRegistry, MovementClass};
use crate::map::HexMap;
use crate::roster::Roster;
use hexx::Hex;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

/// Movement points for a unit per round: the vehicle's base, scaled by the
/// crew's driving. Resolution spreads these across the scale's
/// `ticks_per_round` ticks.
///
/// One point is one hex of clear terrain per round, so this is a speed —
/// at the shipped scale, 5 points is 30 km/h.
pub fn move_points(
    registry: &DataRegistry,
    roster: &Roster,
    unit: &Unit,
    terrain: Option<&str>,
) -> u32 {
    let base = registry
        .vehicle(&unit.vehicle)
        .map(|v| v.movement.points)
        .unwrap_or(0);
    let skilled = registry
        .balance
        .speed(base, super::stats::driving(registry, roster, unit, terrain));
    // Thrown tracks outrank talented driving: half speed on damaged
    // running gear, none on destroyed. Integer halves so resolution stays
    // bit-for-bit reproducible.
    skilled * unit.mobility_halves(registry) / 2
}

/// Cost of stepping from `from` onto `to`, or `None` if that step is
/// impossible (impassable terrain, off-map, or too steep).
pub fn edge_cost(
    registry: &DataRegistry,
    map: &HexMap,
    class: MovementClass,
    max_climb: i32,
    from: Hex,
    to: Hex,
) -> Option<u32> {
    let from_tile = map.get(from)?;
    let to_tile = map.get(to)?;
    if (to_tile.elevation - from_tile.elevation).abs() > max_climb {
        return None;
    }
    registry.terrain(&to_tile.terrain)?.cost_for(class)
}

fn unit_movement(registry: &DataRegistry, unit: &Unit) -> (MovementClass, i32) {
    registry
        .vehicle(&unit.vehicle)
        .map(|v| (v.movement.class, v.movement.max_climb))
        .unwrap_or((MovementClass::Tracked, 1))
}

/// Cost for `unit` to step from `from` onto `to`, using its own movement
/// class and climb limit. The tick resolver prices each step with this.
pub fn edge_cost_for(
    registry: &DataRegistry,
    state: &BattleState,
    unit: &Unit,
    from: Hex,
    to: Hex,
) -> Option<u32> {
    let (class, max_climb) = unit_movement(registry, unit);
    edge_cost(registry, &state.map, class, max_climb, from, to)
}

/// Whether `unit` may pass through (not stop on) `hex`.
///
/// Friendly units can be passed through; visible enemies block. Enemies the
/// moving side has not spotted do NOT block here — bumping into one mid-path
/// is the ambush case, resolved by [`super::BattleState::apply`].
fn passable(state: &BattleState, unit: &Unit, hex: Hex) -> bool {
    match state.unit_at(hex) {
        None => true,
        Some(other) if other.side == unit.side => true,
        Some(other) => !state.fog.side(unit.side).spotted.contains(&other.id),
    }
}

/// Whether `unit` is barred from *finishing* its move on `hex`.
///
/// Friends and spotted enemies block: the moving side can see both, so
/// refusing the order tells it nothing it did not already know. That includes
/// a hex a friend has merely *planned* to occupy, so two units are never
/// ordered onto the same tile. An unspotted enemy deliberately does not block
/// — the order is accepted and resolves as an ambush. Refusing it instead
/// would announce that someone is standing there, which is the fog leaking
/// through the pathfinder.
pub fn destination_blocked(state: &BattleState, unit: &Unit, hex: Hex) -> bool {
    let occupied = match state.unit_at(hex) {
        None => false,
        Some(other) if other.id == unit.id => false,
        Some(other) if other.side == unit.side => true,
        Some(other) => state.fog.side(unit.side).spotted.contains(&other.id),
    };
    occupied || claimed_by_friend(state, unit, hex)
}

/// Whether a friendly unit's orders already send it to `hex` this round.
fn claimed_by_friend(state: &BattleState, unit: &Unit, hex: Hex) -> bool {
    state
        .units
        .iter()
        .filter(|other| other.alive && other.id != unit.id && other.side == unit.side)
        .any(|other| !other.intent.path.is_empty() && other.planned_destination() == hex)
}

/// All tiles the unit can end its move on, with the cheapest cost to reach
/// each. You may pass through friends but not park on them; see
/// [`destination_blocked`] for why unspotted enemies stay in the set.
/// Includes the unit's own tile at cost 0.
pub fn reachable(registry: &DataRegistry, state: &BattleState, id: UnitId) -> HashMap<Hex, u32> {
    let Some(unit) = state.unit(id) else {
        return HashMap::new();
    };
    let (class, max_climb) = unit_movement(registry, unit);
    let budget = move_points(registry, &state.roster, unit, state.terrain_at(unit.pos));

    let mut best: HashMap<Hex, u32> = HashMap::new();
    let mut heap = BinaryHeap::new();
    best.insert(unit.pos, 0);
    heap.push((Reverse(0u32), unit.pos.x, unit.pos.y));

    while let Some((Reverse(cost), x, y)) = heap.pop() {
        let hex = Hex::new(x, y);
        if best.get(&hex).is_some_and(|&c| c < cost) {
            continue;
        }
        for next in hex.all_neighbors() {
            if !passable(state, unit, next) {
                continue;
            }
            let Some(step) = edge_cost(registry, &state.map, class, max_climb, hex, next) else {
                continue;
            };
            let total = cost + step;
            if total > budget {
                continue;
            }
            if best.get(&next).is_none_or(|&c| total < c) {
                best.insert(next, total);
                heap.push((Reverse(total), next.x, next.y));
            }
        }
    }

    best.retain(|hex, _| !destination_blocked(state, unit, *hex));
    best
}

/// One round's step toward ground that may be many rounds away: the hex she
/// can reach this round that stands closest to `destination`.
///
/// The engine refuses a [`super::Order::SetMove`] naming ground beyond this
/// round's movement — [`path_to`] returns `None` past the budget — which is
/// the right rule for an order and is why a long march has to be walked a leg
/// at a time. Both things that walk one are this function's callers: a
/// commander's personal `tasking`, and a crew's own [`super::Goal`]. They
/// share it rather than each computing "closest reachable", because two
/// implementations of that would be two answers to *where is she going*, and
/// the replay would only agree with one of them.
///
/// Three keys, in order, and **every one of them is a distance or a cost**.
/// That is not tidiness, it is the whole correctness argument, and it was
/// bought expensively — see [`along_the_bearing`].
///
/// 1. Get as close to the destination as this round's movement allows.
/// 2. Among those, spend the least getting there. A driver would, and it
///    leaves her the most credit for whatever the rest of the round asks.
/// 3. Among *those*, stay on the bearing she is driving.
pub fn step_toward(
    registry: &DataRegistry,
    state: &BattleState,
    id: UnitId,
    destination: Hex,
) -> Option<Hex> {
    let pos = state.unit(id)?.pos;
    let bearing = destination - pos;
    reachable(registry, state, id)
        .into_iter()
        .min_by_key(|(hex, cost)| {
            (
                destination.distance_to(*hex),
                *cost,
                -along_the_bearing(*hex - pos, bearing),
                // Last, and only ever a coin flip off the line of advance:
                // `reachable` is a `HashMap`, so the three keys above being
                // non-total made the winner depend on hash order. That is the
                // one bug this engine has already shipped once, and
                // `two_runs_in_one_process_agree_with_each_other` caught it
                // within a minute of it being reintroduced here.
                hex.x,
                hex.y,
            )
        })
        .map(|(hex, _)| hex)
        .filter(|step| *step != pos)
}

/// How much of `step` goes the way `bearing` points: the cube dot product.
///
/// This exists because the tiebreak it serves used to read `(h.x, h.y)`, and
/// that is a **compass** direction. Smallest x is west, so on every tie every
/// crew in the game drifted west — which pulls a side attacking east backwards
/// and a side attacking west forwards. It was worth about three points of win
/// rate to whichever side was advancing westward, and it hid for months inside
/// a "mirrored" arena because the arena was not mirrored either and the two
/// faults pointed the same way. `balance --only skill` measures what is left.
///
/// The rule that replaced it: **a tiebreak may only read quantities a
/// reflection preserves.** Distances and terrain costs qualify. A dot product
/// of two differences qualifies, because a point reflection negates both and
/// the product is unchanged. A coordinate does not, and no total order on
/// coordinates can — a reflection maps the least element to the greatest, so
/// asking for "the smallest" is asking which way is west. Anything added to
/// the key in `step_toward` has to pass that test.
///
/// A residue is unavoidable and is deliberately left where it does no harm.
/// Two hexes at equal distance, equal cost and equal bearing are mirror images
/// of each other *within* one crew's own choice, and the coordinate key still
/// sitting at the bottom of the sort picks between them. It has to: `reachable`
/// returns a `HashMap`, so a key that is not a total order makes the answer
/// depend on hash iteration order, which is the one bug this engine has
/// already shipped. What matters is that the compass now decides only a
/// left-or-right choice *off* the line of advance rather than a
/// forward-or-backward one, so it cannot accumulate into an edge for an end
/// of the map.
pub fn along_the_bearing(step: Hex, bearing: Hex) -> i32 {
    let z = |h: Hex| -h.x - h.y;
    step.x * bearing.x + step.y * bearing.y + z(step) * z(bearing)
}

/// Cheapest path for `unit` to `to`, if it exists within this turn's budget.
/// Returns the path including the start tile, plus its total cost.
pub fn path_to(
    registry: &DataRegistry,
    state: &BattleState,
    id: UnitId,
    to: Hex,
) -> Option<(Vec<Hex>, u32)> {
    let unit = state.unit(id)?;
    if to == unit.pos {
        return Some((vec![unit.pos], 0));
    }
    if destination_blocked(state, unit, to) {
        return None;
    }
    let (class, max_climb) = unit_movement(registry, unit);
    let budget = move_points(registry, &state.roster, unit, state.terrain_at(unit.pos));

    let path = hexx::algorithms::a_star(unit.pos, to, |from, next| {
        if from == next {
            return Some(0);
        }
        if !passable(state, unit, next) {
            return None;
        }
        edge_cost(registry, &state.map, class, max_climb, from, next)
    })?;

    let mut cost = 0;
    for pair in path.windows(2) {
        cost += edge_cost(registry, &state.map, class, max_climb, pair[0], pair[1])?;
    }
    (cost <= budget).then_some((path, cost))
}
