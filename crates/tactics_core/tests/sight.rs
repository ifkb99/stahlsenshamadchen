//! What a crew can see, and how well the engine agrees with itself about it.
//!
//! The resolved grids exist only to answer line of sight and movement
//! faster than the reference implementation, never differently, so this
//! binary pairs each cache with the reference it must never disagree with.
//! Fog of war, detection and the sight geometry all answer variants of one
//! question — has anybody found her yet, and would a straight line to her
//! actually get there — so they sit together rather than beside the goal
//! chooser that consumes their answer.
//!
//! - the resolved grids: one answer, computed once
//! - fog of war and cached vision
//! - detection: looking is not seeing
//! - line of sight, elevation and terrain

use tactics_core::battle::{
    BattleState, Order, SightGrid, UnitId, destination_blocked, los_clear, reachable,
};
use tactics_core::data::{DataRegistry, MovementClass};
use tactics_core::map::HexMap;

mod common;
use common::{play_round, registry, seen, two_side_battle, unit_at};

// --- the resolved grids: one answer, computed once -------------------------

#[test]
fn the_sight_grid_answers_exactly_what_the_reference_does() {
    // The grid exists only to stop line of sight re-deriving tile heights
    // through a String-keyed registry lookup on every step of every ray. It
    // is allowed to be faster; it is not allowed to see anything different.
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 1).unwrap();
    let mut hexes: Vec<_> = state.map.iter().map(|(h, _)| h).collect();
    hexes.sort_unstable_by_key(|h| (h.x, h.y));

    let mut checked = 0;
    for a in hexes.iter().step_by(29) {
        for b in hexes.iter().step_by(31) {
            assert_eq!(
                state.sight.clear(*a, *b),
                los_clear(&reg, &state.map, *a, *b),
                "sight grid disagrees about {a:?} -> {b:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 1000, "sampled too little of the map: {checked}");
}

#[test]
fn the_move_grid_answers_exactly_what_the_reference_does() {
    // Same bargain as the sight grid: it exists only to stop the searches
    // re-deriving a tile's cost through a String-keyed registry lookup on
    // every edge they touch. It is allowed to be faster; it is not allowed to
    // price a single step differently, for any class, at any climb limit.
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 1).unwrap();
    let mut hexes: Vec<_> = state.map.iter().map(|(h, _)| h).collect();
    hexes.sort_unstable_by_key(|h| (h.x, h.y));

    let mut checked = 0;
    for hex in &hexes {
        for next in hex.all_neighbors() {
            for class in MovementClass::ALL {
                for climb in [0, 1, 2] {
                    assert_eq!(
                        state.moves.cost(class, climb, *hex, next),
                        tactics_core::battle::movement_edge_cost(
                            &reg, &state.map, class, climb, *hex, next
                        ),
                        "move grid disagrees about {hex:?} -> {next:?} for {class:?} at climb \
                         {climb}"
                    );
                    checked += 1;
                }
            }
        }
    }
    assert!(checked > 10_000, "sampled too little of the map: {checked}");
}

/// The occupancy walk gathered once answers exactly what the one-hex
/// reference does.
///
/// The third cache with this bargain, and the one whose reference is a
/// public function the order path still calls: `destination_blocked` walks
/// every unit on the field for one hex, and `reachable` gathers the same
/// facts once for a whole sweep. It is allowed to be faster; it is not
/// allowed to bar her from ground the reference would let her stop on, or
/// let her stop on ground the reference would refuse.
///
/// The two directions are both worth stating. A gather that forgot a
/// passenger, or counted the moving crew as somebody she has to make room
/// for, would over-block — invisible in a battle, because she would simply
/// go somewhere else — so the neighbours of her own hex are checked the
/// other way round.
#[test]
fn the_occupancy_index_answers_exactly_what_the_reference_does() {
    let reg = seen(registry());
    let rows: Vec<&str> = vec!["ggggg"; 3];
    let mut state = two_side_battle(
        &reg,
        &rows,
        vec![
            // A light tank is two footprints and grass holds four, so the
            // enemy's hex below is refused by the enemy rule alone rather than
            // also by the room rule — which is what makes forgetting one of
            // the three visible.
            unit_at([0, 1], 0, "light_tank", "Sweeper"),
            // A friend parked in her way. A medium tank is three footprints
            // and grass holds four, so one of her own is enough to fill a
            // hex — no contrivance needed to get a refusal out of the room
            // rule.
            unit_at([2, 1], 0, "medium_tank", "Parked"),
            // And one who has been ordered somewhere, so a *claim* is in the
            // sample as well as a vehicle.
            unit_at([2, 0], 0, "medium_tank", "Marching"),
            unit_at([4, 1], 1, "light_tank", "Enemy"),
        ],
        13,
    );
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(2),
                to: tactics_core::offset_to_hex(3, 0),
            },
        )
        .expect("a legal march of one hex");
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(3)),
        "the stage needs the enemy found, or her hex is an ambush rather than a wall"
    );
    let marching = state.unit(UnitId(2)).expect("she is on the field");
    assert_ne!(
        marching.planned_destination(),
        marching.pos,
        "the stage needs a claim in it, not just parked vehicles"
    );

    let her = state.unit(UnitId(0)).expect("she is on the field");
    let reach = reachable(&reg, &state, UnitId(0));

    // Nothing she may stop on is ground the reference refuses.
    for hex in reach.keys() {
        assert!(
            !destination_blocked(&reg, &state, her, *hex),
            "the sweep let her stop on {hex:?}, which the reference refuses"
        );
    }

    // Named ground, refused for each of the three reasons the gather has to
    // get right, because "some hexes are refused" would pass with any two of
    // them forgotten.
    for (at, why) in [
        (
            [2, 1],
            "a medium tank is parked on it and three plus two will not fit on grass",
        ),
        ([3, 0], "a friend's orders have already spoken for it"),
        ([4, 1], "an enemy she has found is standing on it"),
    ] {
        let hex = tactics_core::offset_to_hex(at[0], at[1]);
        assert!(
            destination_blocked(&reg, &state, her, hex),
            "she should be refused {at:?}: {why}"
        );
        assert!(
            !reach.contains_key(&hex),
            "the sweep offered her {at:?}, which {why}"
        );
    }
    // And a bare hex next door is not refused, or the three above prove
    // nothing except that everything is refused.
    let bare = tactics_core::offset_to_hex(1, 1);
    assert!(
        !destination_blocked(&reg, &state, her, bare) && reach.contains_key(&bare),
        "empty grass one hex from her tracks is ground she may stand on"
    );

    // The other direction, over hexes a step away, where her budget cannot be
    // what excluded them: anything the reference allows, the sweep offers.
    for next in her.pos.all_neighbors() {
        if state.map.get(next).is_none() || destination_blocked(&reg, &state, her, next) {
            continue;
        }
        assert!(
            reach.contains_key(&next),
            "the sweep kept her off {next:?}, which is next to her and the reference allows"
        );
    }
}

#[test]
fn a_sight_grid_folded_in_a_region_at_a_time_is_the_grid_of_the_whole_map() {
    // The streaming claim for the other grid. Both of them resolve a per-tile
    // fact from immutable terrain, both will have tiles arriving and leaving
    // when the world becomes one continuous map, and both therefore have to
    // land in exactly the state a whole-map build would.
    let reg = registry();
    let whole = BattleState::from_map(&reg, "river_crossing", 1).unwrap();
    let map = &whole.map;

    let mut streamed = tactics_core::battle::SightGrid::default();
    assert!(streamed.is_empty(), "and an unbuilt one knows it");
    streamed.extend(&reg, map);
    streamed.extend(&reg, map);
    assert_eq!(streamed.len(), whole.sight.len());

    let mut hexes: Vec<_> = map.iter().map(|(h, _)| h).collect();
    hexes.sort_unstable_by_key(|h| (h.x, h.y));
    let mut checked = 0;
    for a in hexes.iter().step_by(29) {
        for b in hexes.iter().step_by(31) {
            assert_eq!(
                streamed.clear(*a, *b),
                whole.sight.clear(*a, *b),
                "a streamed grid disagrees about {a:?} -> {b:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 1000, "sampled too little of the map: {checked}");
}

#[test]
fn a_move_grid_folded_in_a_region_at_a_time_is_the_grid_of_the_whole_map() {
    // The streaming claim, which is what the grid is shaped for: the world is
    // meant to become one continuous map at two zoom levels, so tiles arrive
    // and leave and rebuilding everything is not an option. Folding a map in
    // through `extend` must therefore land in exactly the state `build`
    // would, and folding the same region in twice must change nothing —
    // otherwise a chunk that comes back into view is a chunk that prices
    // differently.
    let reg = registry();
    let whole = BattleState::from_map(&reg, "river_crossing", 1).unwrap();
    let map = &whole.map;

    let mut streamed = tactics_core::battle::MoveGrid::default();
    assert!(streamed.is_empty(), "and an unbuilt one knows it");
    streamed.extend(&reg, map);
    streamed.extend(&reg, map);
    assert_eq!(streamed.len(), whole.moves.len());

    let mut checked = 0;
    for (hex, _) in map.iter() {
        for next in hex.all_neighbors() {
            assert_eq!(
                streamed.cost(MovementClass::Tracked, 1, hex, next),
                whole.moves.cost(MovementClass::Tracked, 1, hex, next),
                "a streamed grid disagrees about {hex:?} -> {next:?}"
            );
            checked += 1;
        }
    }
    assert!(checked > 5_000, "sampled too little of the map: {checked}");
}

// --- fog of war and cached vision ------------------------------------------

#[test]
fn cached_vision_is_the_same_answer_as_computing_it_fresh() {
    // Vision is cached per unit against (position, range) because the map
    // cannot change under a unit mid-battle. That makes the cache the real
    // answer rather than an approximation of it — and this is the test that
    // says so, by rebuilding every side's visible set from scratch after
    // several rounds of movement and shooting and demanding it match.
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 42).unwrap();
    for _ in 0..6 {
        if state.is_over() {
            break;
        }
        play_round(&reg, &mut state);
    }

    for side in 0..state.sides.len() as u8 {
        let mut fresh = std::collections::HashSet::new();
        for unit in state.side_units(side) {
            fresh.extend(tactics_core::battle::unit_vision(&reg, &state, unit.id));
        }
        assert_eq!(
            state.fog.side(side).visible,
            fresh,
            "side {side}'s cached visible set drifted from a fresh computation"
        );
        assert!(
            fresh.is_subset(&state.fog.side(side).explored),
            "everything visible must also be remembered as explored"
        );
    }
}

#[test]
fn fog_hides_unseen_enemies() {
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 42).unwrap();
    // At battle start across a river with forests, neither side should have
    // spotted everything, and everyone should see their own deployment.
    let fog0 = state.fog.side(0);
    assert!(!fog0.visible.is_empty());
    assert!(fog0.explored.len() >= fog0.visible.len());
    let enemy_count = state.side_units(1).count();
    assert!(
        fog0.spotted.len() < enemy_count,
        "player should not start with every enemy spotted"
    );
}

// --- detection: looking is not seeing --------------------------------------

/// A registry whose spotting is a search rather than a certainty, with the
/// base mod's own tuning taken out of the way.
///
/// Every one of these tests isolates a single term by making it decisive:
/// what is under test is the *rule*, and numbers chosen so that a die is
/// never actually thrown are what make each of them a statement rather than a
/// bet on a seed. The medium tank is given a four-hex reach and the crew
/// bonus is switched off, so "the far edge of what she can see" is a place a
/// test can put a vehicle by counting hexes.
fn registry_searching(base: i32, certain: i32, at_range: i32, per_hex_moved: i32) -> DataRegistry {
    let mut reg = registry();
    reg.balance.detection_base = base;
    reg.balance.detection_certain_percent = certain;
    reg.balance.detection_at_range_percent = at_range;
    reg.balance.detection_per_hex_moved = per_hex_moved;
    reg.balance.vision_per_observation = 0;
    for terrain in reg.terrain.values_mut() {
        terrain.concealment = 0;
    }
    reg.vehicles
        .get_mut("medium_tank")
        .expect("base mod has a medium tank")
        .vision_range = 4;
    reg
}

/// One watcher at the west end of a five-hex strip and one enemy at the east,
/// four hexes off — which is exactly the watcher's reach, so she is standing
/// on the far edge of it.
fn watched(reg: &DataRegistry, rows: &[&str]) -> BattleState {
    two_side_battle(
        reg,
        rows,
        vec![
            unit_at([0, 0], 0, "medium_tank", "Watcher"),
            unit_at([4, 0], 1, "medium_tank", "Watched"),
        ],
        7,
    )
}

#[test]
fn a_mod_that_asks_for_no_search_spots_exactly_as_it_always_did() {
    // The additivity contract for the whole detection rule, and the strong
    // form of it: with the near band covering a crew's whole reach, being
    // looked at is being seen AND not one die is thrown. The second half is
    // what keeps every other seeded result in this project valid — a rule
    // that consumed randomness to conclude "yes, obviously" would move every
    // battle in the game without changing a single decision.
    //
    // Note the far-range term is set high and still buys nothing: at
    // `detection_certain_percent` 100 there is no far band for it to be
    // spent in, which is what makes the neutral value one number rather than
    // a set of them.
    let reg = registry_searching(100, 100, 70, 0);
    let state = watched(&reg, &["ggggg"]);
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "a crew in the open with nothing to search for is seen at once"
    );
    // And the stream is where the seed left it. Setting a whole battlefield
    // up draws no randomness of its own, so anything the spotting pass spent
    // would show here — worth pinning separately from the determinism
    // snapshot, because the moment the base mod declares detection numbers
    // that snapshot is a record of the rule being *on*.
    let mut untouched = <rand_chacha::ChaCha8Rng as rand::SeedableRng>::seed_from_u64(7);
    assert_eq!(
        rand::RngExt::random_range(&mut state.rng.clone(), 0..u32::MAX),
        rand::RngExt::random_range(&mut untouched, 0..u32::MAX),
        "spotting a whole field with nothing to decide must not draw a die"
    );
}

#[test]
fn a_crew_in_timber_has_to_be_found_and_one_in_the_open_does_not() {
    // The ground's own concealment, made decisive: a hundred points of it,
    // fully faded in at the edge of a watcher's reach, is a crew who is never
    // picked out of that wood by looking, however long anybody looks. She is
    // still given away by driving or firing, which is what the tests below
    // are about.
    let mut reg = registry_searching(100, 0, 0, 0);
    reg.terrain
        .get_mut("forest")
        .expect("base mod has forest")
        .concealment = 100;
    let open = watched(&reg, &["ggggg"]);
    assert!(
        open.fog.side(0).spotted.contains(&UnitId(1)),
        "the crew on open grass is seen at once"
    );
    let timber = watched(&reg, &["ggggf"]);
    assert!(
        timber
            .fog
            .side(0)
            .visible
            .contains(&tactics_core::offset_to_hex(4, 0)),
        "the wood itself is in view — this is a test about detection, not sight"
    );
    assert!(
        !timber.fog.side(0).spotted.contains(&UnitId(1)),
        "the crew in the wood is standing on ground the watcher can see, and is \
         not thereby seen"
    );
}

#[test]
fn a_crew_who_drives_gives_herself_away() {
    // The motion term on its own: nobody is worth looking for at all
    // (`detection_base` zero), and one hex of driving is worth the whole
    // search. The two targets are identical and stand side by side, so the
    // only thing between them is the counter the gunner reads too.
    let reg = registry_searching(0, 100, 0, 100);
    let mut state = two_side_battle(
        &reg,
        &["ggggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Watcher"),
            unit_at([3, 0], 1, "medium_tank", "Halted"),
            unit_at([4, 0], 1, "medium_tank", "Under way"),
        ],
        7,
    );
    assert!(
        state.fog.side(0).spotted.is_empty(),
        "with no search worth making, nobody is found"
    );
    // Nobody has orders and nobody is spotted, so the ticks below are empty:
    // what they do is look again. Two of them, because a look is spent once
    // per tick whether or not it had any chance of succeeding — the battle's
    // own opening pass already used up tick zero — and `moved` is zeroed at
    // the *end* of a round rather than at the top of one, so a count set
    // between ticks is the count the next spotting pass reads.
    for side in state.living_sides() {
        state.apply(&reg, &Order::Commit { side }).unwrap();
    }
    state.step_tick(&reg);
    state.unit_mut(UnitId(2)).unwrap().moved = 1;
    state.step_tick(&reg);
    let fog = state.fog.side(0);
    assert!(
        fog.spotted.contains(&UnitId(2)),
        "a crew who crossed a hex this round has been noticed"
    );
    assert!(
        !fog.spotted.contains(&UnitId(1)),
        "the one who sat still has not"
    );
}

#[test]
fn the_far_edge_of_a_crews_reach_is_where_she_has_to_search() {
    // The range term, made decisive: at a hundred percent, with no near band
    // to protect anybody, the chance runs linearly to nothing at the limit of
    // what a crew can see — so a target standing exactly there is never
    // picked out, and the identical battle with the term at zero finds her at
    // once. That is the wall this whole item exists to take down: at 100 m to
    // the hex a commander who can see four hexes can see 400 m, and certainty
    // at 399 m with nothing at 401 m is not eyesight, it is a cutoff.
    for (at_range, found) in [(0, true), (100, false)] {
        let reg = registry_searching(100, 0, at_range, 0);
        let state = watched(&reg, &["ggggg"]);
        assert_eq!(
            state.fog.side(0).spotted.contains(&UnitId(1)),
            found,
            "at detection_at_range_percent {at_range}, a crew four hexes off a \
             four-hex reach should{} be spotted",
            if found { "" } else { " not" }
        );
    }
}

#[test]
fn nobody_searches_for_what_is_plainly_in_front_of_her() {
    // The near band, which the first draft of these rules did not have and
    // was wrong without: with the far term at its maximum, a crew at half of
    // a four-hex reach is inside the certain band and simply seen, while the
    // same crew at the edge of it is never found at all. Two hundred metres
    // of open field is not something anybody has to search.
    let reg = registry_searching(100, 50, 100, 0);
    let close = two_side_battle(
        &reg,
        &["ggggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Watcher"),
            unit_at([2, 0], 1, "medium_tank", "Two hexes off"),
        ],
        7,
    );
    assert!(
        close.fog.side(0).spotted.contains(&UnitId(1)),
        "half a reach away in the open is not a thing anybody searches for"
    );
    let far = watched(&reg, &["ggggg"]);
    assert!(
        !far.fog.side(0).spotted.contains(&UnitId(1)),
        "the same crew at the limit of the same reach has to be found"
    );
}

// --- line of sight, elevation and terrain ----------------------------------

/// Sight is the same relation seen from either end of the battlefield.
///
/// The invariants say a tiebreak may only read quantities a reflection
/// preserves. That rule was written about the AI and broken in the
/// **geometry**: resolving a ray to a hex is a nearest-centre question, a ray
/// running exactly along a hex boundary has no nearest centre, and
/// `hexx::Hex::round` settles it consistently in absolute terms. Reflecting a
/// query reverses the ray, its arithmetic differs by an ulp, and the coin
/// lands the other way up.
///
/// Measured on this very map before the fix: **1,382 of 109,230 ordered hex
/// pairs — 1.27% — disagreed with their own mirror image**, and side B took 46
/// of 72 equal-skill battles at difficulty 5, where the blur is zero and
/// nothing else can break a tie. It stayed invisible for as long as the arena
/// was open grass, because a ray that clips the corner of nothing blocks
/// nothing.
///
/// The arenas are the fixtures because they are the maps in the tree built to
/// be symmetric, and they are symmetric under two reflections rather than one,
/// so this pins both at once. **Every** arena is walked, not only the one the
/// numbers were measured on: the rule is about the geometry, and a second
/// battlefield with a second set of ridges is a second sample of it. Note what
/// is deliberately **not** asserted: `clear(a, b) == clear(b, a)`. Sight is not
/// reciprocal here and should not be — the ray runs from the observer's eye to
/// the target's hull, and a crew on a ridge can see a hull that cannot see her
/// back.
#[test]
fn a_reflection_leaves_a_sight_line_alone() {
    use tactics_core::harness::arena::{ARENAS, REFLECTIONS};

    let reg = registry();
    for arena in ARENAS {
        let map = arena.map().expect("the arena builds");
        let sight = SightGrid::build(&reg, &map);
        let hexes: Vec<_> = map.iter().map(|(hex, _)| hex).collect();
        assert!(
            hexes.len() > 300,
            "{} should be a proper battlefield, got {} tiles",
            arena.id,
            hexes.len()
        );

        let mut broken = Vec::new();
        for &a in &hexes {
            for &b in &hexes {
                if a == b {
                    continue;
                }
                let here = sight.clear(a, b);
                for (name, image) in REFLECTIONS {
                    if here != sight.clear(image(arena, a), image(arena, b)) {
                        broken.push(format!("{name}: {a:?} -> {b:?}"));
                    }
                }
            }
        }
        assert!(
            broken.is_empty(),
            "{}: {} of {} ordered pairs disagree with their own image; first few: {:?}",
            arena.id,
            broken.len(),
            hexes.len() * (hexes.len() - 1),
            &broken[..broken.len().min(5)]
        );
    }
}

#[test]
fn elevation_blocks_and_grants_line_of_sight() {
    let reg = registry();
    let file: tactics_core::map::MapFile = serde_json::from_str(
        r##"{
            "id": "los_test",
            "palette": { "g": "grass", "f": "forest" },
            "rows":      ["ggggg", "ggggg", "ggggg"],
            "elevation": ["00000", "00300", "00000"]
        }"##,
    )
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let a = tactics_core::offset_to_hex(0, 1);
    let b = tactics_core::offset_to_hex(4, 1);
    let peak = tactics_core::offset_to_hex(2, 1);
    // The ridge blocks sight across, but the peak sees both sides.
    assert!(!los_clear(&reg, &map, a, b), "ridge should block flat LoS");
    assert!(los_clear(&reg, &map, peak, a), "high ground sees down");
    assert!(los_clear(&reg, &map, a, peak), "the peak itself is visible");
}

/// The hit clamp is data, so a mod can decide how much luck the game has.
///
/// `MIN_HIT`/`MAX_HIT` were Rust constants: the one pair of numbers deciding
/// whether a certainty or an impossibility can exist on a battlefield were the
/// only gunnery numbers a mod could not touch.
#[test]
fn how_much_luck_a_battlefield_has_is_a_mod_decision() {
    let mut reg = seen(registry());
    assert_eq!((reg.balance.min_hit, reg.balance.max_hit), (5, 95));
    reg.balance.max_hit = 40;
    reg.balance.min_hit = 30;

    let state = two_side_battle(
        &reg,
        &["ggggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "West"),
            unit_at([1, 0], 1, "medium_tank", "East"),
        ],
        7,
    );
    let (west, east) = (state.units[0].id, state.units[1].id);
    let weapon = reg
        .vehicle(&state.units[0].vehicle)
        .and_then(|v| v.weapons.first())
        .and_then(|w| reg.weapon(w))
        .expect("a medium tank has a gun");
    let breakdown = tactics_core::battle::hit_breakdown(
        &reg,
        &state,
        west,
        state.units[0].pos,
        weapon,
        east,
        state.units[1].pos,
        false,
    );
    assert!(
        (30..=40).contains(&breakdown.total),
        "a point-blank shot should be clamped to the mod's ceiling, got {}",
        breakdown.total
    );
}

/// A taller cupola sees over a rise a shorter one does not.
///
/// `EYE_HEIGHT` and `TARGET_HEIGHT` were Rust constants too. They are
/// centimetres in the `balance` block because that block is deliberately
/// all-integer, so the sight arithmetic stays exact.
#[test]
fn a_mod_that_raises_the_cupola_sees_over_the_rise() {
    let mut reg = registry();
    let file: tactics_core::map::MapFile = serde_json::from_str(
        r##"{
            "id": "cupola",
            "palette": { "g": "grass" },
            "rows":      ["ggggg"],
            "elevation": ["00100"]
        }"##,
    )
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let a = tactics_core::offset_to_hex(0, 0);
    let b = tactics_core::offset_to_hex(4, 0);

    assert_eq!(reg.balance.eye_height_cm, 250);
    assert!(
        !los_clear(&reg, &map, a, b),
        "a 10 m rise blocks a 2.5 m cupola"
    );

    // Twenty-five metres up, the sight line passes over it. Absurd for a tank
    // and exactly the point: the number is the mod's to choose.
    reg.balance.eye_height_cm = 2500;
    assert!(
        los_clear(&reg, &map, a, b),
        "a 25 m cupola should see over a 10 m rise, or the field is not read"
    );
    assert!(
        SightGrid::build(&reg, &map).clear(a, b),
        "and the cached path must agree, or it is holding stale heights"
    );
}

/// How tall an elevation digit is belongs to the scale contract, so a mod that
/// changes it must change what a ridge hides.
///
/// This is the check that `Scale::elevation_meters` is *read*. It used to be
/// declared in `mod.json`, printed by the validator, and consulted by nothing:
/// `fog.rs` carried its own `const ELEVATION_STEP = 10.0` and the two agreed
/// only because both said ten. A mod that raised the field got a steeper climb
/// — `max_climb` did read it — and a skyline that had not moved, which is the
/// scale contract quietly meaning two different things in two places.
///
/// Both sight paths are asserted because they resolve heights separately:
/// [`los_clear`] walks the registry per step and [`SightGrid`] resolves every
/// tile once. They share `Heights::of` so they cannot disagree, and this is
/// what says so.
#[test]
fn a_mod_that_flattens_a_level_flattens_the_skyline() {
    let mut reg = registry();
    let file: tactics_core::map::MapFile = serde_json::from_str(
        r##"{
            "id": "elevation_scale",
            "palette": { "g": "grass" },
            "rows":      ["ggggg", "ggggg", "ggggg"],
            "elevation": ["00000", "00300", "00000"]
        }"##,
    )
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let a = tactics_core::offset_to_hex(0, 1);
    let b = tactics_core::offset_to_hex(4, 1);

    // The base mod's 10 m a level: three levels is a 30 m ridge across a sight
    // line drawn between a 2.5 m cupola and a 2.0 m target, so it blocks.
    assert_eq!(reg.scale.elevation_meters, 10.0);
    assert!(!los_clear(&reg, &map, a, b), "a 30 m ridge blocks");
    assert!(
        !SightGrid::build(&reg, &map).clear(a, b),
        "and the cached path agrees"
    );

    // Half a metre a level makes the same three digits a 1.5 m hummock, which
    // the sight line clears at 2.25 m over the ridge tile. Same map, same
    // elevation digits, same terrain: only the scale moved.
    reg.scale.elevation_meters = 0.5;
    assert!(
        los_clear(&reg, &map, a, b),
        "a 1.5 m hummock does not, or the geometry is not reading the field"
    );
    assert!(
        SightGrid::build(&reg, &map).clear(a, b),
        "and the cached path agrees here too"
    );
}

#[test]
fn forests_block_sight_at_range() {
    let reg = registry();
    let file: tactics_core::map::MapFile = serde_json::from_str(
        r##"{
            "id": "forest_los",
            "palette": { "g": "grass", "f": "forest" },
            "rows": ["ggfgg"]
        }"##,
    )
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let a = tactics_core::offset_to_hex(0, 0);
    let b = tactics_core::offset_to_hex(4, 0);
    assert!(!los_clear(&reg, &map, a, b), "forest curtain blocks sight");
    let edge = tactics_core::offset_to_hex(2, 0);
    assert!(
        los_clear(&reg, &map, a, edge),
        "the forest tile itself is visible"
    );
}

#[test]
fn movement_respects_water_and_reaches_bridge() {
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 7).unwrap();
    // Unit 0 is Anka's medium tank at offset [2,4] on the road.
    let unit = state.side_units(0).next().unwrap().id;
    let tiles = reachable(&reg, &state, unit);
    assert!(!tiles.is_empty());
    for hex in tiles.keys() {
        let tile = state.map.get(*hex).unwrap();
        assert_ne!(tile.terrain, "water", "tracked vehicles cannot enter water");
    }
}
