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
    // Which skill answers for getting her moving is a fact about the chassis,
    // not about the crew: a platoon has no driver's seat, and asking her for
    // `driving` charged her the stand-in penalty for a seat she has never
    // had. `athletics` is her own, and `speed_per_athletics` is the rate.
    let afoot = matches!(
        registry.vehicle(&unit.vehicle).map(|v| v.movement.class),
        Some(crate::data::MovementClass::Foot)
    );
    let skilled = if afoot {
        // Her chassis's own, and nobody's skill. A percentage of a one-point
        // allowance cannot say anything — one hex a round already is walking
        // pace — and she is deliberately still not asked for `driving`, which
        // charged a platoon a stand-in penalty for a seat her chassis has
        // never had. `athletics` buys steep ground instead; see
        // `balance.athletics_per_climb_level`.
        base
    } else {
        registry
            .balance
            .speed(base, super::stats::driving(registry, roster, unit, terrain))
    };
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
/// should go through the battle's [`crate::world::World::moves`] instead, which answers the same
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
                        .terrain(tile.terrain)
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
    fn of(tile: crate::map::Tile<'_>, terrain: Option<&crate::data::TerrainDef>) -> Self {
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
/// - **`SightGrid` is the same structure and has the same shape**, deliberately:
///   a per-tile fact resolved once from immutable terrain, folded in a region
///   at a time. They are both cheap to derive — 85 and 25 microseconds for a
///   whole 1261-tile battle map — which is why they are built on load rather
///   than shipped inside map assets, where they would be a second copy of
///   `move_cost` and `vision_block` that a retuned mod would silently
///   contradict. Whether the two should become one object owned by the map is
///   in TODO under hex streaming.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MoveGrid {
    /// Not saved: derived entirely from the map and the terrain definitions,
    /// exactly like `SightGrid`, so a loaded game rebuilds it rather than
    /// carrying a copy of something that holds no state a player changed.
    /// [`crate::battle::SavedBattle::rehydrate`] refills it, and since that
    /// is the only way a deserialised battle becomes a [`BattleState`] the
    /// refill cannot be forgotten — an empty grid says every step is
    /// impossible, which is a silent wrong answer rather than a loud one.
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
    pub fn insert(&mut self, registry: &DataRegistry, hex: Hex, tile: crate::map::Tile<'_>) {
        self.tiles
            .insert(hex, TileMove::of(tile, registry.terrain(tile.terrain)));
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

fn unit_movement(
    registry: &DataRegistry,
    state: &BattleState,
    unit: &Unit,
) -> (MovementClass, i32) {
    let (class, listed) = registry
        .vehicle(&unit.vehicle)
        .map(|v| (v.movement.class, v.movement.max_climb))
        .unwrap_or((MovementClass::Tracked, 1));
    // What a tank can climb is a fact about her suspension; what a platoon
    // can climb is a fact about the platoon, so `athletics` reaches this and
    // only on foot.
    if class != MovementClass::Foot {
        return (class, listed);
    }
    let climbing =
        super::stats::athletics(registry, &state.roster, unit, state.terrain_at(unit.pos));
    (class, registry.balance.climb(listed, climbing))
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
    let (class, max_climb) = unit_movement(registry, state, unit);
    state.world.moves().cost(class, max_climb, from, to)
}

/// Whether `unit` may pass through (not stop on) `hex`.
///
/// Friendly units can be passed through; visible enemies block. Enemies the
/// moving side has not spotted do NOT block here — bumping into one mid-path
/// is the ambush case, resolved by [`super::BattleState::apply`].
/// Whether a crew of `mine` footprints fits on ground of this `capacity`
/// beside `count` others taking up `taken` between them.
///
/// The room rule itself, with no opinion about how the numbers were counted.
/// [`BattleState::room_for`] walks the field to get them and [`Occupancy`]
/// looks them up in an index it gathered once; sharing the rule rather than
/// the gathering is the arrangement [`edge_cost`] and [`MoveGrid::cost`]
/// already have through `step_cost`, and for the same reason — two readings
/// of "is there room" would drift, and the drift would look like a
/// pathfinding bug rather than a bookkeeping one.
///
/// Terrain that declares no capacity keeps the rule this game had before
/// stacking: one crew to a hex, whatever size she is. That is why the count
/// is passed beside the footprint — a crew whose chassis the mod does not
/// define still takes the hex.
pub(crate) fn fits(count: u32, taken: u32, capacity: Option<u32>, mine: u32) -> bool {
    let Some(capacity) = capacity else {
        return count == 0;
    };
    taken + mine <= capacity.max(mine)
}

/// Whether friends' orders have already spoken for this ground.
///
/// [`fits`]'s twin, and the reason a claim is counted rather than refused on
/// sight: counting is what lets a section be ordered into a wood together,
/// where the old rule sent the second crew somewhere else however much room
/// was left. `standing` is everybody on the hex, seen or not, because the
/// crews already parked there are a fact rather than a report.
pub(crate) fn claim_blocks(claimed: u32, standing: u32, capacity: Option<u32>, mine: u32) -> bool {
    if claimed == 0 {
        return false;
    }
    let Some(capacity) = capacity else {
        // No declared capacity is the old rule: one crew to a hex, so any
        // claim at all is somebody else's ground.
        return true;
    };
    standing + claimed + mine > capacity.max(mine)
}

/// Who is standing and heading where, gathered once for one crew's sweep.
///
/// [`reachable`] asks the same questions — may she drive through this hex,
/// may she stop on it — of every tile within her reach, and each answer used
/// to walk every unit on the field. That is O(hexes × units) twice over, and
/// it is **5.0 of `reachable`'s 16.4 µs** on `river_crossing` with eight
/// units, measured by deleting both checks outright, which is the ceiling on
/// what any index can buy. Gathering the same facts once is O(units + hexes).
///
/// The single-hex callers gather one hex's worth through the same function
/// and ask the same methods, so this is not a second reading of the rules —
/// the rules are [`fits`] and [`claim_blocks`], and everything here only
/// counts.
#[derive(Default)]
struct Occupancy {
    /// Where an enemy she has *found* is standing: a wall whatever the
    /// capacity says, since two sides do not share a hex. An unspotted one is
    /// deliberately absent — that order is accepted and resolves as an
    /// ambush, because refusing it would announce her.
    enemies: Vec<Hex>,
    /// Everybody standing on the field with the footprint they take, which is
    /// what [`BattleState::crowding`] counts. Her own crew is in it, because a
    /// crew does not have to make room for herself.
    standing: Vec<(Hex, u32)>,
    /// The others she can see — her own side, and the enemies she has found —
    /// with their footprints.
    seen: Vec<(Hex, u32)>,
    /// Where her friends' orders already send them this round, with what they
    /// will take up when they arrive.
    claimed: Vec<(Hex, u32)>,
    /// What she takes up herself, resolved once.
    ///
    /// It is a string-keyed registry lookup, and those are about a fifth of a
    /// round; asking for it per tile of her reach was most of what the old
    /// per-hex check actually cost.
    mine: u32,
}

impl Occupancy {
    /// Walk the field once.
    ///
    /// `only`, when given, keeps just that hex, which is what the single-hex
    /// callers want. It cannot change an answer — every read below matches on
    /// the hex again — so it is a bound on what gets stored and nothing else:
    /// the order path should not allocate a list the size of the army to ask
    /// about one tile.
    fn gather(
        registry: &DataRegistry,
        state: &BattleState,
        unit: &Unit,
        only: Option<Hex>,
    ) -> Self {
        let spotted = &state.fog.side(unit.side).spotted;
        let mut occ = Self {
            mine: registry
                .vehicle(&unit.vehicle)
                .map(|v| v.footprint())
                .unwrap_or(1),
            ..Self::default()
        };
        for other in &state.units {
            if !other.alive() {
                continue;
            }
            // A chassis the mod does not define takes up no room and still
            // takes the hex, which is why a crew is an *entry* here and her
            // footprint is only the number beside it.
            let footprint = registry
                .vehicle(&other.vehicle)
                .map(|v| v.footprint())
                .unwrap_or(0);
            // A passenger is aboard rather than on the field, so she is on
            // nobody's hex to stand on.
            if other.aboard.is_none() && only.is_none_or(|hex| hex == other.pos) {
                occ.standing.push((other.pos, footprint));
                if other.id != unit.id && (other.side == unit.side || spotted.contains(&other.id)) {
                    occ.seen.push((other.pos, footprint));
                }
                if other.side != unit.side && spotted.contains(&other.id) {
                    occ.enemies.push(other.pos);
                }
            }
            if other.id != unit.id && other.side == unit.side && !other.intent.path.is_empty() {
                let heading = other.planned_destination();
                if only.is_none_or(|hex| hex == heading) {
                    occ.claimed.push((heading, footprint));
                }
            }
        }
        occ
    }

    /// How many of `list` are on `hex`, and what they take up between them.
    ///
    /// A linear scan, deliberately, and the measurement is the argument: a
    /// `HashMap<Hex, _>` index of the same facts made `reachable` *slower*
    /// (17.7 µs against 16.4), because hashing a coordinate per tile costs
    /// more than walking a handful of contiguous entries. These lists are one
    /// per crew on the field — eight on `river_crossing`, a few dozen in the
    /// worst army this game fields — and the win was never the lookup. It is
    /// that the vehicle is resolved out of the registry once per crew instead
    /// of once per crew *per tile*, and string-keyed registry lookups are
    /// about a fifth of a round.
    fn count_on(list: &[(Hex, u32)], hex: Hex) -> (u32, u32) {
        let mut count = 0;
        let mut taken = 0;
        for (at, footprint) in list {
            if *at == hex {
                count += 1;
                taken += footprint;
            }
        }
        (count, taken)
    }

    /// Whether she may drive *through* `hex`.
    ///
    /// Crowding is deliberately not consulted: a hex being full is a reason
    /// not to *stop* there, not a reason a tank cannot drive across it. That
    /// distinction is the whole difference between this and [`Self::blocked`],
    /// and collapsing them would make a wood holding three platoons into a
    /// wall.
    fn passable(&self, hex: Hex) -> bool {
        !self.enemies.contains(&hex)
    }

    /// Whether she is barred from *finishing* her move on `hex`. See
    /// [`destination_blocked`].
    fn blocked(&self, registry: &DataRegistry, state: &BattleState, hex: Hex) -> bool {
        if self.enemies.contains(&hex) {
            return true;
        }
        let capacity = state
            .terrain_at(hex)
            .and_then(|id| registry.terrain(id))
            .and_then(|t| t.capacity);
        let (count, taken) = Self::count_on(&self.seen, hex);
        if !fits(count, taken, capacity, self.mine) {
            return true;
        }
        let (_, claimed) = Self::count_on(&self.claimed, hex);
        let (_, standing) = Self::count_on(&self.standing, hex);
        claim_blocks(claimed, standing, capacity, self.mine)
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
///
/// One hex's worth of [`Occupancy`], which is the same walk over the units
/// this function always did.
pub fn destination_blocked(
    registry: &DataRegistry,
    state: &BattleState,
    unit: &Unit,
    hex: Hex,
) -> bool {
    Occupancy::gather(registry, state, unit, Some(hex)).blocked(registry, state, hex)
}

/// The pinning gate: a crew on a `pinned` morale rung will not step onto
/// ground where more fire reaches her than reaches her where she stands.
///
/// **The one reading of the rule**, asked by everything that moves a crew —
/// [`reachable`] (so the planner, the player's move range and
/// [`step_toward`] never offer the ground), [`path_to`] (so a route to
/// allowed ground does not cross forbidden ground) and the tick's own drive
/// (so a crew pinned half-way along a route stops where she is). "Fire" is
/// [`super::incoming`], her own knowledge priced in the currency, so she is
/// held by the guns she knows of and by nothing she has not seen.
///
/// The ceiling is the fire on her hex when she is asked. Quieter ground is
/// always open, which is what makes a pinned crew crawl for cover rather
/// than freeze: the drill's destination is by construction no hotter.
pub struct Pinning {
    unit: UnitId,
    ceiling: f32,
    heat: std::cell::RefCell<HashMap<Hex, f32>>,
}

impl Pinning {
    /// The gate for `unit` now, or `None` if her rung does not pin her.
    pub fn of(registry: &DataRegistry, state: &BattleState, unit: &Unit) -> Option<Self> {
        if !registry.morale.rung(unit.pressure).pinned {
            return None;
        }
        Some(Self {
            unit: unit.id,
            ceiling: super::incoming(registry, state, unit.id, unit.pos).worth,
            heat: std::cell::RefCell::new(HashMap::new()),
        })
    }

    /// Whether she will step onto `hex`.
    pub fn allows(&self, registry: &DataRegistry, state: &BattleState, hex: Hex) -> bool {
        let heat = *self
            .heat
            .borrow_mut()
            .entry(hex)
            .or_insert_with(|| super::incoming(registry, state, self.unit, hex).worth);
        heat <= self.ceiling + 1e-3
    }
}

/// All tiles the unit can end its move on, with the cheapest cost to reach
/// each. You may pass through friends but not park on them; see
/// [`destination_blocked`] for why unspotted enemies stay in the set.
/// Includes the unit's own tile at cost 0.
pub fn reachable(registry: &DataRegistry, state: &BattleState, id: UnitId) -> HashMap<Hex, u32> {
    let Some(unit) = state.unit(id) else {
        return HashMap::new();
    };
    let (class, max_climb) = unit_movement(registry, state, unit);
    let budget = move_points(registry, &state.roster, unit, state.terrain_at(unit.pos));

    // One walk over the units, then every question about every tile in her
    // reach is a lookup. See [`Occupancy`] for what that is worth.
    let occupancy = Occupancy::gather(registry, state, unit, None);
    let pinning = Pinning::of(registry, state, unit);

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
            if !occupancy.passable(next) {
                continue;
            }
            if pinning
                .as_ref()
                .is_some_and(|p| !p.allows(registry, state, next))
            {
                continue;
            }
            let Some(step) = state.world.moves().cost(class, max_climb, hex, next) else {
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

    best.retain(|hex, _| !occupancy.blocked(registry, state, *hex));
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
    let unit = state.unit(id)?;
    let pos = unit.pos;
    let bearing = destination - pos;
    // How far each hex still is from the destination, as her order prices
    // the way: the crow flight when nothing is watching or her order does
    // not care, and otherwise the covered way round (`to_go`). The two agree
    // exactly at a price of zero on a whole battle map, which is why the
    // cheaper one stands in for the other.
    let left = match Watch::of(registry, state, unit) {
        Some(watch) => {
            let way = to_go(state, &watch, destination);
            Some((watch, way))
        }
        None => None,
    };
    let remaining = |hex: Hex| match &left {
        Some((watch, way)) => way
            .get(&hex)
            .copied()
            .unwrap_or(u32::MAX / 2)
            .saturating_add(watch.price(state, hex)),
        None => destination.distance_to(hex) as u32,
    };
    let friends = friends_of(state, unit);
    reachable(registry, state, id)
        .into_iter()
        .min_by_key(|(hex, cost)| {
            (
                remaining(*hex),
                *cost,
                -along_the_bearing(*hex - pos, bearing),
                // Two hexes the same distance on, at the same cost and the
                // same bearing, are each other's reflection about her line of
                // advance, and the coordinate below would pick the same side
                // of it for every crew in the game — the "harmless left or
                // right" this key was once said to be. On a ridge it is not
                // harmless: it sent one end's crews over the crest and the
                // other's along the foot (`examples/mirror --map
                // battle_hills`). Keeping with her own is the same from
                // either end.
                apart(&friends, *hex),
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

/// What is left of the way to `destination` from every hex of the map,
/// counted in hexes with every watched hex entered on the way costing
/// [`Watch`]'s price more — `step_toward`'s notion of "closer" for a crew
/// whose order wants dead ground. A reverse search from the destination, so
/// one call answers every hex she might halt on this round. Crow steps
/// rather than terrain cost, because the key it replaces was a crow
/// distance and terrain already speaks through `reachable`'s own cost.
fn to_go(state: &BattleState, watch: &Watch, destination: Hex) -> HashMap<Hex, u32> {
    let mut left: HashMap<Hex, u32> = HashMap::new();
    let mut heap = BinaryHeap::new();
    left.insert(destination, 0);
    heap.push(Reverse((0u32, destination.x, destination.y)));
    while let Some(Reverse((d, x, y))) = heap.pop() {
        let hex = Hex::new(x, y);
        if left.get(&hex).is_some_and(|&b| b < d) {
            continue;
        }
        // Stepping from `next` onto `hex` enters `hex`, so that is the hex
        // whose watcher is charged.
        let enter = 1 + watch.price(state, hex);
        for next in hex.all_neighbors() {
            if !state.world.contains(next) {
                continue;
            }
            let total = d + enter;
            if left.get(&next).is_none_or(|&b| total < b) {
                left.insert(next, total);
                heap.push(Reverse((total, next.x, next.y)));
            }
        }
    }
    left
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
    let (class, max_climb) = unit_movement(registry, state, unit);
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
            let Some(step) = state.world.moves().cost(class, max_climb, hex, next) else {
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

/// What the order she is driving under makes a hex in the enemy's sight
/// cost her, and who is watching: `balance.route_exposure` for her order's
/// verb, against every enemy she knows of (`Knower::Crew`) as an observer
/// reaching his own vision range.
///
/// `None` when the price is zero or nobody is watching, which is the whole
/// of the rule's absence: every route is then the plain cheapest one.
pub struct Watch {
    price: u32,
    observers: Vec<(Hex, u32)>,
    seen: std::cell::RefCell<HashMap<Hex, bool>>,
}

impl Watch {
    pub fn of(registry: &DataRegistry, state: &BattleState, unit: &Unit) -> Option<Self> {
        let price = registry
            .balance
            .route_exposure
            .for_verb(driving_under(state, unit));
        if price == 0 {
            return None;
        }
        let observers: Vec<(Hex, u32)> = state
            .known_enemies(registry, super::Knower::Crew(unit.id))
            .into_iter()
            .map(|e| {
                (
                    e.pos,
                    registry
                        .vehicle(&e.vehicle)
                        .map(|v| v.vision_range)
                        .unwrap_or(0),
                )
            })
            .collect();
        if observers.is_empty() {
            return None;
        }
        Some(Self {
            price,
            observers,
            seen: std::cell::RefCell::new(HashMap::new()),
        })
    }

    /// What entering `hex` costs her on top of the ground: the price if a
    /// watcher can see it, else nothing. One definition of "seen" with the
    /// planner's covered routes (`ground::watched`).
    pub fn price(&self, state: &BattleState, hex: Hex) -> u32 {
        let seen = *self
            .seen
            .borrow_mut()
            .entry(hex)
            .or_insert_with(|| crate::ground::watched(state, &self.observers, hex));
        if seen { self.price } else { 0 }
    }
}

/// The verb of the order `unit` is driving under, as `route_exposure` keys
/// it: `march` for a personal march, her formation's mission as she heard
/// it, or nothing.
fn driving_under(state: &BattleState, unit: &Unit) -> &'static str {
    if unit.march().is_some() {
        return "march";
    }
    state
        .formation_of(unit.id)
        .and_then(|f| f.mission_for(unit.id))
        .map(|m| m.verb())
        .unwrap_or("")
}

/// How far `hex` stands from the rest of her side: the sum of distances to
/// every friend on the field. The path finder's tiebreak between routes of
/// equal price, and chosen because it is **the same number from either end
/// of a mirrored field** — a reflection carries her and all her friends
/// across together — where the neighbour order `hexx::a_star` settled ties
/// by is a compass, and handed `battle_town` to its eastern end
/// (`examples/mirror --map`). Staying with one's own is also what a driver
/// with no other reason to choose does.
pub(crate) fn apart(friends: &[Hex], hex: Hex) -> u32 {
    friends.iter().map(|f| f.distance_to(hex) as u32).sum()
}

/// Where the rest of her side is standing: every friend on the field, not
/// her and not a passenger. What [`apart`] measures against.
pub(crate) fn friends_of(state: &BattleState, unit: &Unit) -> Vec<Hex> {
    state
        .units
        .iter()
        .filter(|u| u.side == unit.side && u.id != unit.id && u.alive() && u.aboard.is_none())
        .map(|u| u.pos)
        .collect()
}

/// Cheapest path for `unit` to `to`, if it exists within this turn's budget.
/// Returns the path including the start tile, plus its total cost in
/// movement points.
///
/// "Cheapest" is the ground **plus** what her order makes a watched hex cost
/// ([`Watch`]), so a scout goes round and an assault goes straight; the
/// budget is still spent in movement points alone, since being seen does not
/// slow a tank. Ties fall to the route that keeps her nearest her own
/// ([`apart`]) and only then to the coordinate.
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
    let occupancy = Occupancy::gather(registry, state, unit, None);
    if occupancy.blocked(registry, state, to) {
        return None;
    }
    let (class, max_climb) = unit_movement(registry, state, unit);
    let budget = move_points(registry, &state.roster, unit, state.terrain_at(unit.pos));
    let pinning = Pinning::of(registry, state, unit);
    let watch = Watch::of(registry, state, unit);
    let friends = friends_of(state, unit);

    // The search runs over (hex, movement spent), not hex alone. The price a
    // route is chosen by is the ground *plus* exposure, but the budget is the
    // ground alone, and a search keyed on the hex keeps only the cheapest
    // price to each — which may be the covered way round that has already
    // spent too much, having thrown away the exposed way that would have
    // fitted. That refused orders `reachable` had just offered
    // (`two_commanders_fight_each_other_without_an_illegal_order_or_a_wedged_round`
    // caught it). A round's budget is a handful of points, so the extra key
    // costs a handful of labels per hex.
    type Label = (Hex, u32);
    let start: Label = (unit.pos, 0);
    let mut best: HashMap<Label, (u32, u32)> = HashMap::new();
    let mut came: HashMap<Label, Label> = HashMap::new();
    let mut heap = BinaryHeap::new();
    best.insert(start, (0, 0));
    heap.push(Reverse((0u32, 0u32, 0u32, unit.pos.x, unit.pos.y)));
    let mut arrived: Option<Label> = None;
    while let Some(Reverse((price, spread, spent, x, y))) = heap.pop() {
        let hex = Hex::new(x, y);
        let here = (hex, spent);
        if best.get(&here).is_some_and(|&b| b < (price, spread)) {
            continue;
        }
        if hex == to {
            arrived = Some(here);
            break;
        }
        for next in hex.all_neighbors() {
            if !occupancy.passable(next) {
                continue;
            }
            if pinning
                .as_ref()
                .is_some_and(|p| !p.allows(registry, state, next))
            {
                continue;
            }
            let Some(step) = state.world.moves().cost(class, max_climb, hex, next) else {
                continue;
            };
            let total = spent + step;
            if total > budget {
                continue;
            }
            let key = (
                price + step + watch.as_ref().map_or(0, |w| w.price(state, next)),
                spread + apart(&friends, next),
            );
            let there = (next, total);
            if best.get(&there).is_none_or(|&b| key < b) {
                best.insert(there, key);
                came.insert(there, here);
                heap.push(Reverse((key.0, key.1, total, next.x, next.y)));
            }
        }
    }
    let arrived = arrived?;
    let mut labels = vec![arrived];
    while let Some(prev) = came.get(labels.last()?) {
        labels.push(*prev);
    }
    labels.reverse();
    let path: Vec<Hex> = labels.iter().map(|(h, _)| *h).collect();
    (path.first() == Some(&unit.pos)).then_some((path, arrived.1))
}
