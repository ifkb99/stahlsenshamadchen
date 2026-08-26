//! Movement: reachability (Dijkstra) and pathing (A*), both aware of
//! terrain costs per movement class, elevation climb limits, and occupancy.

use super::{BattleState, Unit, UnitId};
use crate::data::{DataRegistry, MovementClass};
use crate::map::HexMap;
use crate::roster::Roster;
use hexx::Hex;
use serde::{Deserialize, Serialize};
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

/// What one step costs, given a way of asking a tile for its height and its
/// price to the class doing the stepping.
///
/// The climb rule lives here and nowhere else, so [`edge_cost`] and
/// [`MoveGrid::cost`] cannot drift apart about what "too steep" means. Same
/// arrangement as `sight_line_clear` behind [`super::los_clear`] and
/// `SightGrid::clear`, and for the same reason.
fn step_cost(
    tile: impl Fn(Hex) -> Option<(i32, Option<u32>)>,
    max_climb: i32,
    from: Hex,
    to: Hex,
) -> Option<u32> {
    let (from_elevation, _) = tile(from)?;
    let (to_elevation, cost) = tile(to)?;
    if (to_elevation - from_elevation).abs() > max_climb {
        return None;
    }
    cost
}

/// Cost of stepping from `from` onto `to`, or `None` if that step is
/// impossible (impassable terrain, off-map, or too steep).
///
/// This is the reference implementation and the one to reach for in a test,
/// on the campaign map, or for any one-off query. Anything inside a search
/// should go through [`BattleState::moves`] instead, which answers the same
/// question without hashing a terrain id per step.
pub fn edge_cost(
    registry: &DataRegistry,
    map: &HexMap,
    class: MovementClass,
    max_climb: i32,
    from: Hex,
    to: Hex,
) -> Option<u32> {
    step_cost(
        |hex| {
            map.get(hex).map(|tile| {
                (
                    tile.elevation,
                    registry
                        .terrain(&tile.terrain)
                        .and_then(|t| t.cost_for(class)),
                )
            })
        },
        max_climb,
        from,
        to,
    )
}

/// One tile's movement facts, resolved once.
///
/// **Adding a fact to the grid is meant to be one field here and one line in
/// [`TileMove::of`]**, and nothing else. Everything the grid does — building,
/// streaming a chunk in, answering a step — is written in terms of those two,
/// so the next thing worth resolving once per tile (a terrain's `capacity`,
/// its `cover`) costs a line rather than a redesign.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TileMove {
    elevation: i32,
    /// Cost to enter, indexed by [`MovementClass::index`]. `None` is
    /// impassable to that class, which is what a terrain that names no cost
    /// for it means.
    cost: [Option<u32>; MovementClass::ALL.len()],
}

impl TileMove {
    /// Resolve one tile against the registry. **The one place this module
    /// reads a terrain definition.**
    fn of(tile: &crate::map::Tile, terrain: Option<&crate::data::TerrainDef>) -> Self {
        let mut cost = [None; MovementClass::ALL.len()];
        for class in MovementClass::ALL {
            cost[class.index()] = terrain.and_then(|t| t.cost_for(class));
        }
        Self {
            elevation: tile.elevation,
            cost,
        }
    }
}

/// Every tile's movement cost, resolved once, per movement class.
///
/// The twin of [`super::SightGrid`] and built for the same measured reason.
/// [`edge_cost`] looks terrain up by `String` in the registry, and the
/// searches call it per edge of every tile they touch: one `roads` call over
/// a radius-20 map is about seven and a half thousand of those string hashes,
/// for an answer that cannot change because no battle alters its own terrain.
/// Measured on `river_crossing`, resolving them once took `roads` from 322 to
/// 219 microseconds and `reachable` from 26.5 to 18.0.
///
/// # Built for a world that streams
///
/// This is keyed on tiles, never on a battle or a map's identity, because the
/// design has the overworld and the battlefield converging into **one
/// continuous world at two zoom levels** — crossing what is today a map edge
/// becomes an ordinary drive, with no seam and therefore no exit tiles to
/// declare. Three things follow, and they are why the shape is what it is:
///
/// - **Tiles arrive and leave.** [`Self::extend`] folds a region in and
///   [`Self::insert`] does one tile, so a chunk coming into view costs a pass
///   over its own tiles rather than a rebuild of the world.
///   `HashMap::retain` on `tiles` is how a chunk leaves, when something wants
///   that.
/// - **It is derived data, and cheap.** One pass, no allocation per tile
///   beyond the map itself, so it is always safe to rebuild a region rather
///   than reason about whether it is stale.
/// - **It wants to merge with `SightGrid` when that day comes.** They are the
///   same structure — a per-tile fact resolved once from immutable terrain —
///   and streaming one of them means writing the same logic twice. Tracked in
///   TODO.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MoveGrid {
    /// Not saved: derived entirely from the map and the terrain definitions,
    /// exactly like `SightGrid`, so a loaded game rebuilds it rather than
    /// carrying a copy of something that holds no state a player changed.
    /// [`crate::save::rehydrate`] *must* refill it — an empty grid says every
    /// step is impossible, which is a silent wrong answer rather than a loud
    /// one.
    #[serde(skip)]
    tiles: HashMap<Hex, TileMove>,
}

impl MoveGrid {
    /// Resolve a whole map.
    pub fn build(registry: &DataRegistry, map: &HexMap) -> Self {
        let mut grid = Self::default();
        grid.extend(registry, map);
        grid
    }

    /// Fold a region's tiles in, replacing anything already known about them.
    ///
    /// `build` is this on an empty grid, which is deliberate: the streaming
    /// path and the whole-map path are one piece of code, so the one that is
    /// used every day is the one that keeps the other honest.
    pub fn extend(&mut self, registry: &DataRegistry, map: &HexMap) {
        for (hex, tile) in map.iter() {
            self.insert(registry, hex, tile);
        }
    }

    /// Resolve one tile.
    pub fn insert(&mut self, registry: &DataRegistry, hex: Hex, tile: &crate::map::Tile) {
        self.tiles
            .insert(hex, TileMove::of(tile, registry.terrain(&tile.terrain)));
    }

    /// Whether this grid has been built. A deserialized battle carries an
    /// empty one until [`crate::save`] refills it.
    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// How many tiles are resolved. Uninteresting today and the thing a
    /// streamed world watches.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    /// Cost of stepping from `from` onto `to`. Identical in result to
    /// [`edge_cost`] — they share [`step_cost`] precisely so the fast path
    /// cannot drift away from the reference one.
    pub fn cost(&self, class: MovementClass, max_climb: i32, from: Hex, to: Hex) -> Option<u32> {
        step_cost(
            |hex| {
                self.tiles
                    .get(&hex)
                    .map(|tile| (tile.elevation, tile.cost[class.index()]))
            },
            max_climb,
            from,
            to,
        )
    }
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
    state.moves.cost(class, max_climb, from, to)
}

/// Whether `unit` may pass through (not stop on) `hex`.
///
/// Friendly units can be passed through; visible enemies block. Enemies the
/// moving side has not spotted do NOT block here — bumping into one mid-path
/// is the ambush case, resolved by [`super::BattleState::apply`].
fn passable(state: &BattleState, unit: &Unit, hex: Hex) -> bool {
    // Crowding is deliberately not consulted: a hex being full is a reason
    // not to *stop* there, not a reason a tank cannot drive across it. That
    // distinction is the whole difference between this and
    // [`destination_blocked`], and collapsing them would make a wood holding
    // three platoons into a wall.
    !state.occupants(hex).any(|other| {
        other.side != unit.side && state.fog.side(unit.side).spotted.contains(&other.id)
    })
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
pub fn destination_blocked(
    registry: &DataRegistry,
    state: &BattleState,
    unit: &Unit,
    hex: Hex,
) -> bool {
    // A spotted enemy is a wall whatever the capacity says: two sides do not
    // share a hex, and refusing here tells the moving side nothing it cannot
    // already see. An unspotted one still does not block — that order is
    // accepted and resolves as an ambush.
    if state.occupants(hex).any(|other| {
        other.side != unit.side && state.fog.side(unit.side).spotted.contains(&other.id)
    }) {
        return true;
    }
    // Friends are a question of room rather than of presence now. On terrain
    // that declares no capacity `room_for` says "one crew, whatever size",
    // which is the rule this line used to spell out itself.
    !state.room_for(registry, unit, hex) || claimed_by_friend(registry, state, unit, hex)
}

/// Whether a friendly unit's orders already send it to `hex` this round.
fn claimed_by_friend(registry: &DataRegistry, state: &BattleState, unit: &Unit, hex: Hex) -> bool {
    // A claim takes up room exactly as a vehicle already parked there does,
    // and for the same reason: by the time she arrives the other crew will be
    // standing on it. Counting *claims* rather than refusing on the first one
    // is what lets a section be ordered into a wood together — the old rule
    // sent the second crew somewhere else no matter how much space was left.
    let claimed: u32 = state
        .units
        .iter()
        .filter(|other| other.alive && other.id != unit.id && other.side == unit.side)
        .filter(|other| !other.intent.path.is_empty() && other.planned_destination() == hex)
        .filter_map(|other| registry.vehicle(&other.vehicle))
        .map(|v| v.footprint())
        .sum();
    if claimed == 0 {
        return false;
    }
    let capacity = state
        .terrain_at(hex)
        .and_then(|id| registry.terrain(id))
        .and_then(|t| t.capacity);
    let Some(capacity) = capacity else {
        // No declared capacity is the old rule: one crew to a hex, so any
        // claim at all is somebody else's ground.
        return true;
    };
    let mine = registry
        .vehicle(&unit.vehicle)
        .map(|v| v.footprint())
        .unwrap_or(1);
    let standing = state.crowding(registry, hex);
    standing + claimed + mine > capacity.max(mine)
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
            let Some(step) = state.moves.cost(class, max_climb, hex, next) else {
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

    best.retain(|hex, _| !destination_blocked(registry, state, unit, *hex));
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

/// Every road out of where a crew is standing, to as far as she cares to
/// look: the cheapest terrain cost to each hex, and the hex she would have
/// come from.
///
/// The many-destinations twin of [`path_to`], and different from it in the
/// three ways a plan is different from an order.
///
/// - **Many destinations at once.** A goal chooser asks about a handful of
///   places, and a Dijkstra that answers all of them costs less than one A*
///   per candidate — which is what this replaced, at five to ten times the
///   price of a planned order.
/// - **No occupancy.** Neither friends nor spotted enemies block. Where
///   everybody is standing *now* will not hold for the several rounds this
///   march takes, so treating a tank parked on the bridge as a wall would
///   make the bridge unreachable and the ground a crew most wants invisible.
///   The opposition reaches the decision as danger along the way instead,
///   which is a cost rather than a refusal, and the leg-by-leg
///   [`step_toward`] still respects every one of them when she actually
///   drives. A happy consequence: the inner loop touches terrain only, so
///   unlike [`reachable`] this is not O(hexes x units).
/// - **A horizon rather than a budget.** An order naming ground beyond this
///   round's movement is refused, which is right for an order; a goal three
///   rounds off is an ordinary thing to intend. The caller says how many
///   rounds of driving are worth pricing, and ground beyond that is simply
///   absent — a road nobody would take is not worth the tiles it costs to
///   find.
#[derive(Debug, Clone, Default)]
pub struct Roads {
    cost: HashMap<Hex, u32>,
    from: HashMap<Hex, Hex>,
    start: Hex,
}

impl Roads {
    /// Terrain cost of the cheapest road to `hex`, if one was found inside
    /// the horizon.
    pub fn cost(&self, hex: Hex) -> Option<u32> {
        self.cost.get(&hex).copied()
    }

    /// The road itself, start tile first.
    ///
    /// Walked back through the predecessors, so it is the same road the cost
    /// was measured along. Which of two equally cheap roads that is comes
    /// from the heap order in [`roads`], whose last key is a coordinate for
    /// the reason [`step_toward`]'s is: the map is a `HashMap` and the answer
    /// has to be total. See [`along_the_bearing`] for why that is a residue
    /// worth watching rather than a bug — `balance --only skill` on the
    /// mirrored arena is the instrument that would catch it growing teeth.
    pub fn path(&self, hex: Hex) -> Option<Vec<Hex>> {
        if !self.cost.contains_key(&hex) {
            return None;
        }
        let mut out = vec![hex];
        let mut at = hex;
        while at != self.start {
            at = *self.from.get(&at)?;
            out.push(at);
        }
        out.reverse();
        Some(out)
    }
}

/// Price every road out of `id`'s hex, out to `rounds` rounds of driving.
pub fn roads(registry: &DataRegistry, state: &BattleState, id: UnitId, rounds: u32) -> Roads {
    let Some(unit) = state.unit(id) else {
        return Roads::default();
    };
    let (class, max_climb) = unit_movement(registry, unit);
    let horizon =
        move_points(registry, &state.roster, unit, state.terrain_at(unit.pos)).max(1) * rounds;

    let mut out = Roads {
        start: unit.pos,
        ..Roads::default()
    };
    out.cost.insert(unit.pos, 0);
    let mut heap = BinaryHeap::new();
    heap.push((Reverse(0u32), unit.pos.x, unit.pos.y));

    while let Some((Reverse(cost), x, y)) = heap.pop() {
        let hex = Hex::new(x, y);
        if out.cost.get(&hex).is_some_and(|&c| c < cost) {
            continue;
        }
        for next in hex.all_neighbors() {
            let Some(step) = state.moves.cost(class, max_climb, hex, next) else {
                continue;
            };
            let total = cost + step;
            if total > horizon {
                continue;
            }
            if out.cost.get(&next).is_none_or(|&c| total < c) {
                out.cost.insert(next, total);
                out.from.insert(next, hex);
                heap.push((Reverse(total), next.x, next.y));
            }
        }
    }
    out
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
    if destination_blocked(registry, state, unit, to) {
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
        state.moves.cost(class, max_climb, from, next)
    })?;

    let mut cost = 0;
    for pair in path.windows(2) {
        cost += state.moves.cost(class, max_climb, pair[0], pair[1])?;
    }
    (cost <= budget).then_some((path, cost))
}
