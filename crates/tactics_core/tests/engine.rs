//! End-to-end tests against the real `assets/mods` content.

use std::path::PathBuf;
use tactics_core::ai::{
    AiConfig, AiDriver, AiPlanner, Evaluator, UtilityPlanner, make_battle_planner,
};
use tactics_core::battle::{
    BattleState, EndReason, Event as BattleEvent, FireIntent, FormationId, Mission, Order,
    STALEMATE_ROUNDS, SideState, UnitId, los_clear, reachable,
};
use tactics_core::data::DataRegistry;
use tactics_core::map::{HexMap, UnitPlacement};
use tactics_core::overworld::{
    Army, ArmyId, ArmyMission, OverworldError, OverworldEvent, OverworldOrder, OverworldState,
    make_overworld_planner,
};

fn mods_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods")
}

fn registry() -> DataRegistry {
    let (registry, report) = DataRegistry::load_dir(&mods_root()).expect("mods load");
    assert!(
        report.is_ok(),
        "base mod must validate: {:?}",
        report.errors
    );
    registry
}

/// The base game with the radio switched off: command rules stripped, so a
/// test about missions themselves — what they store, how they steer units,
/// what the brain issues — is not also a test about latency and radio
/// radius. The wire has its own tests, and its own zero-coefficient pin
/// (`a_command_block_with_zero_coefficients_is_the_game_without_one`).
fn registry_wireless() -> DataRegistry {
    let mut reg = registry();
    reg.command = None;
    reg
}

/// Close every side's planning and play the round out.
fn play_round(reg: &DataRegistry, state: &mut BattleState) -> Vec<BattleEvent> {
    let mut events = Vec::new();
    for side in state.living_sides() {
        if !state.has_committed(side) {
            events.extend(state.apply(reg, &Order::Commit { side }).expect("commit"));
        }
    }
    events.extend(state.resolve_round(reg));
    events
}

#[test]
fn base_mod_loads_and_validates() {
    let reg = registry();
    assert!(reg.vehicles.len() >= 5);
    assert!(reg.characters.len() >= 8);
    assert!(reg.maps.contains_key("river_crossing"));
    assert!(reg.maps.contains_key("frontier"));
    // Doctrines are mod data like everything else.
    for id in ["massed_armor", "elastic_defense", "recon_pull"] {
        assert!(reg.doctrine(id).is_some(), "base mod should ship `{id}`");
    }
    // Every weapon has a reload cadence, whether or not it states one: an
    // absent `reload_ticks` resolves to a full round against the scale.
    assert!(reg.weapons.values().all(|w| w.reload(&reg.scale) > 0));
    // A tick is five seconds, so these are practical aimed rates of fire:
    // an MG burst every ten seconds, an 88 every twenty.
    assert_eq!(reg.weapon("mg").unwrap().reload(&reg.scale), 2);
    assert_eq!(reg.weapon("gun_88").unwrap().reload(&reg.scale), 4);
    assert_eq!(reg.scale.format_duration(4), "20 s");
}

#[test]
fn the_base_mod_declares_the_scale_contract() {
    // The numbers every range, speed and map dimension in `assets/mods` was
    // authored against. They lived only in a documentation table until the
    // scale block existed, which meant nothing could check them.
    let reg = registry();
    assert_eq!(reg.scale.hex_meters, 100.0);
    assert_eq!(reg.scale.round_seconds, 60.0);
    assert_eq!(reg.scale.ticks_per_round, 12);
    assert_eq!(reg.scale.tick_seconds(), 5.0);
    assert_eq!(reg.scale.elevation_meters, 10.0);

    // The relationship the contract actually rests on: an overworld hex is
    // one battle map, so a field battle is a zoom-in and not a new place.
    assert_eq!(reg.scale.battle_hexes_per_overworld_hex(), 40.0);
    assert_eq!(reg.scale.battle_map_radius(), 20);
    assert_eq!(reg.scale.battle_map_tiles(), 1261);

    // Consequences that are easy to violate by accident when adding content.
    let medium = reg.vehicle("medium_tank").unwrap();
    assert_eq!(reg.scale.format_speed(medium.movement.points), "30 km/h");
    assert_eq!(
        reg.scale
            .format_distance(reg.weapon("gun_88").unwrap().range[1] as i32),
        "1.6 km"
    );
    // Gun tanks shoot further than they see on purpose: the tank destroyer
    // reaches 1.6 km and sees 1 km, because needing a spotter is its
    // character. Preserve this when adding vehicles.
    let td = reg.vehicle("tank_destroyer").unwrap();
    let gun = reg.weapon(&td.weapons[0]).unwrap();
    assert!(
        td.vision_range < gun.range[1],
        "the tank destroyer must not be able to see everything it can shoot"
    );
}

#[test]
fn a_battle_map_is_one_overworld_tile() {
    // The overworld draws a tile as a hexagon and a battle is that tile
    // zoomed in, so the battlefield is a hexagon too. As a rectangle this
    // was a slogan the content quietly missed by 20%; as a shape the
    // validator can hold it.
    let reg = registry();
    let file = reg.map("river_crossing").unwrap();
    assert_eq!(file.shape(), tactics_core::map::MapShape::Tile);

    let map = HexMap::from_map_file(file).unwrap();
    assert_eq!(map.len() as u32, reg.scale.battle_map_tiles());
    let centre = map.center();
    assert!(
        map.contains(centre),
        "a centroid that lands off-map would be a broken rotation pivot"
    );
    for hex in centre.range(reg.scale.battle_map_radius()) {
        assert!(map.contains(hex), "hexagon has a hole at {hex:?}");
    }
}

#[test]
fn a_battle_map_that_is_not_a_tile_is_rejected() {
    // The check has teeth, and opting out is one field — otherwise every
    // scenario map and hand-built test fixture would be an error forever.
    let reg = registry();
    let rect = serde_json::json!({
        "id": "oblong",
        "palette": { "g": "grass" },
        "rows": ["ggggg", "ggggg", "ggggg"],
    });
    let file: tactics_core::map::MapFile = serde_json::from_value(rect.clone()).unwrap();
    let mut report = tactics_core::data::ValidationReport::default();
    file.validate_into(&reg, &mut report);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("one overworld tile")),
        "a rectangular battle map must not pass validation: {report:?}"
    );

    let mut freed = rect;
    freed["shape"] = serde_json::json!("free");
    let file: tactics_core::map::MapFile = serde_json::from_value(freed).unwrap();
    let mut report = tactics_core::data::ValidationReport::default();
    file.validate_into(&reg, &mut report);
    assert!(
        report.is_ok(),
        "`shape: free` must opt out cleanly: {report:?}"
    );
}

#[test]
fn a_mod_may_retune_the_whole_games_tempo() {
    // The reason scale is data. A mod that halves the hex and the round
    // together leaves every vehicle's speed intact while doubling the
    // resolution of the battlefield, and nothing in Rust has to know.
    let mut reg = registry();
    let before = reg
        .scale
        .kph(reg.vehicle("medium_tank").unwrap().movement.points);
    reg.scale = tactics_core::data::Scale {
        hex_meters: 50.0,
        round_seconds: 30.0,
        ticks_per_round: 6,
        ..reg.scale
    };
    assert_eq!(
        reg.scale
            .kph(reg.vehicle("medium_tank").unwrap().movement.points),
        before
    );
    assert_eq!(reg.scale.tick_seconds(), 5.0);
    // A weapon that states no reload still means "once per round", which is
    // now six ticks rather than twelve.
    let quiet = tactics_core::data::WeaponDef {
        reload_ticks: None,
        ..reg.weapon("gun_88").unwrap().clone()
    };
    assert_eq!(quiet.reload(&reg.scale), 6);
}

#[test]
fn crew_quality_scales_with_the_vehicle_it_sits_in() {
    // The flat `awareness / 4` and `driving / 5` divisors were tuned against
    // a base vision of 3 and survived the scale change unchanged, which made
    // the best scout in the school worth one hex out of twenty. A percentage
    // of the vehicle's own base cannot be devalued that way again.
    let reg = registry();
    let recon = reg.vehicle("recon_car").unwrap().vision_range;
    let elsa = reg.character("elsa").unwrap().skills["observation"];
    assert_eq!(elsa, 13, "Elsa is the school's eyes");
    assert!(
        reg.balance.vision(recon, elsa) > recon,
        "a trained observer sees further than the vehicle's paper range"
    );
    assert_eq!(
        reg.balance.vision(recon, tactics_core::data::AVERAGE),
        recon,
        "an ordinary crew changes nothing, which is what makes a poor one a penalty"
    );

    let heavy = reg.vehicle("heavy_tank").unwrap().movement.points;
    let juno = reg.character("juno").unwrap().skills["driving"];
    assert!(
        reg.balance.speed(heavy, juno) > heavy,
        "a gifted driver must move a slow tank at all, which `driving / 5` did not"
    );
}

#[test]
fn an_unknown_doctrine_is_a_validation_error() {
    let reg = registry();
    let file: tactics_core::map::MapFile = serde_json::from_str(
        r##"{
            "id": "bad_doctrine",
            "palette": { "g": "grass" },
            "rows": ["gg"],
            "sides": [{ "name": "Them", "ai": { "planner": "utility", "doctrine": "nope" } }]
        }"##,
    )
    .unwrap();
    let mut report = tactics_core::data::ValidationReport::default();
    file.validate_into(&reg, &mut report);
    assert!(
        report.errors.iter().any(|e| e.contains("nope")),
        "a doctrine that does not exist must not silently become the default: {report:?}"
    );
}

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

#[test]
fn battle_resolution_is_deterministic_per_seed() {
    let reg = registry();
    let run = |seed: u64| -> Vec<String> {
        let mut state = BattleState::from_map(&reg, "river_crossing", seed).unwrap();
        let cfg = AiConfig {
            planner: "utility".into(),
            difficulty: 5,
            doctrine: None,
        };
        let mut ai = AiDriver::new();
        ai.insert(0, make_battle_planner(&cfg, seed, &reg));
        ai.insert(1, make_battle_planner(&cfg, seed + 1, &reg));
        let mut log = Vec::new();
        for _ in 0..40 {
            if state.is_over() {
                break;
            }
            // Both sides write orders, then the round plays out at once.
            ai.plan_round(&reg, &mut state);
            log.extend(state.resolve_round(&reg).iter().map(|e| format!("{e:?}")));
        }
        log
    };
    let first = run(1234);
    assert!(!first.is_empty(), "the battle should actually do something");
    assert_eq!(first, run(1234), "same seed, same battle");
}

#[test]
fn the_same_intents_replay_the_same_way() {
    // Determinism at the level the replay system will need: identical orders
    // on an identically seeded battle produce an identical event stream.
    let reg = registry();
    let run = || -> Vec<String> {
        let mut state = duel(&reg, 77);
        let (west, east) = (UnitId(0), UnitId(1));
        state
            .apply(
                &reg,
                &Order::SetMove {
                    unit: west,
                    to: tactics_core::offset_to_hex(1, 1),
                },
            )
            .unwrap();
        state
            .apply(
                &reg,
                &Order::SetFire {
                    unit: east,
                    fire: FireIntent::Target {
                        target: west,
                        weapon: 0,
                    },
                },
            )
            .unwrap();
        play_round(&reg, &mut state)
            .iter()
            .map(|e| format!("{e:?}"))
            .collect()
    };
    assert_eq!(run(), run());
}

#[test]
fn ai_vs_ai_battle_finishes() {
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 99).unwrap();
    let cfg = AiConfig {
        planner: "utility".into(),
        difficulty: 4,
        doctrine: None,
    };
    let mut ai = AiDriver::new();
    ai.insert(0, make_battle_planner(&cfg, 5, &reg));
    ai.insert(1, make_battle_planner(&cfg, 6, &reg));
    for round in 0..200 {
        if state.is_over() {
            println!("battle over after {round} rounds: {:?}", state.over);
            return;
        }
        ai.plan_round(&reg, &mut state);
        state.resolve_round(&reg);
    }
    panic!("battle did not finish; final state: round {}", state.round);
}

#[test]
fn a_battle_with_no_shots_fired_is_called_off() {
    let reg = registry();
    let mut state = standoff(&reg, 3);
    let alive_before = state.alive_units().count();

    let mut ended = None;
    for _ in 0..(STALEMATE_ROUNDS as usize + 2) {
        let events = play_round(&reg, &mut state);
        if let Some(BattleEvent::BattleEnded { winner, reason }) = events
            .iter()
            .find(|e| matches!(e, BattleEvent::BattleEnded { .. }))
        {
            ended = Some((*winner, *reason));
            break;
        }
    }

    assert_eq!(
        ended,
        Some((None, EndReason::Stalemate)),
        "sides that never trade fire should disengage rather than circle forever"
    );
    assert_eq!(
        state.alive_units().count(),
        alive_before,
        "a stalemate costs nobody their tanks"
    );
    assert!(
        state.round <= STALEMATE_ROUNDS + 1,
        "the call should come promptly, not after {} rounds",
        state.round
    );
}

#[test]
fn sides_that_can_see_each_other_are_never_called_off() {
    // The stalemate rule must not cut short a slow approach: as long as
    // somebody has an enemy in sight the fight is live, however quiet.
    let reg = registry();
    let file: tactics_core::map::MapFile = serde_json::from_str(
        r##"{
            "id": "open_field",
            "palette": { "g": "grass" },
            "rows": ["ggggg", "ggggg", "ggggg"]
        }"##,
    )
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let sides = vec![
        SideState {
            name: "West".into(),
            ai: None,
        },
        SideState {
            name: "East".into(),
            ai: None,
        },
    ];
    let placements = vec![
        UnitPlacement {
            at: [0, 1],
            side: 0,
            vehicle: "medium_tank".into(),
            crew: Vec::new(),
            name: Some("Watcher".into()),
            facing: None,
            formation: None,
            leads: false,
        },
        UnitPlacement {
            at: [2, 1],
            side: 1,
            vehicle: "medium_tank".into(),
            crew: Vec::new(),
            name: Some("Watched".into()),
            facing: None,
            formation: None,
            leads: false,
        },
    ];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(&reg, &placements);
    let mut state = BattleState::from_placements(
        &reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        1,
    );
    assert!(
        !state.fog.side(0).spotted.is_empty(),
        "test needs the two units to start in sight of one another"
    );

    for _ in 0..(STALEMATE_ROUNDS as usize + 4) {
        if state.is_over() {
            break;
        }
        play_round(&reg, &mut state);
    }
    assert!(
        // They shoot each other on sight now, so either the fight is still
        // going or somebody won it -- what must never happen is a stalemate.
        !matches!(state.over.map(|r| r.reason), Some(EndReason::Stalemate)),
        "a battle under observation is not a stalemate, but ended as {:?} on round {}",
        state.over,
        state.round
    );
}

#[test]
fn mcts_planner_produces_legal_orders() {
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 11).unwrap();
    let cfg = AiConfig {
        planner: "mcts".into(),
        difficulty: 1,
        doctrine: Some("massed_armor".into()),
    };
    let mut planner = make_battle_planner(&cfg, 3, &reg);
    // Plan a whole round for side 0 and require every order to apply cleanly.
    for _ in 0..64 {
        if state.is_over() || !state.is_planning() || state.has_committed(0) {
            break;
        }
        let order = planner.next_order(&reg, &state, 0);
        let commits = order == Order::Commit { side: 0 };
        state
            .apply(&reg, &order)
            .unwrap_or_else(|e| panic!("mcts produced illegal order {order:?}: {e}"));
        if commits {
            break;
        }
    }
    assert!(
        state.has_committed(0),
        "the planner should finish its round rather than stall"
    );
}

#[test]
fn overworld_income_capture_and_battle_trigger() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    assert_eq!(state.armies.len(), 4);

    // March 1st Company onto the nearby city and check capture.
    let army = state.side_armies(0).next().unwrap().id;
    let city = tactics_core::offset_to_hex(1, 1);
    let events = state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: city })
        .unwrap();
    // The army starts on the city tile's hex in this map, so allow either
    // an immediate capture or a move+capture.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OverworldEvent::ObjectiveCaptured { side: 0, .. })),
        "expected a capture event, got {events:?}"
    );

    // End both turns; side 0's next upkeep should pay out city income.
    let _ = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    let funds_before = state.sides[0].funds;
    let events = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OverworldEvent::Income { side: 0, .. })),
        "expected income, got {events:?}"
    );
    assert!(state.sides[0].funds > funds_before);
}

#[test]
fn overworld_reachability_respects_budget_and_blockers() {
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let army = state.side_armies(0).next().unwrap();

    let reach = state.reachable(&reg, army.id);
    assert!(reach.len() > 1, "an army should be able to go somewhere");
    assert_eq!(reach.get(&army.pos), Some(&0), "its own tile costs nothing");
    assert!(
        reach.values().all(|cost| *cost <= army.movement),
        "no tile should cost more than the movement budget"
    );
    // Tiles holding an army can be neither crossed nor parked on.
    for other in state.armies.iter().filter(|a| a.id != army.id) {
        assert!(
            !reach.contains_key(&other.pos),
            "{} should not be a valid destination",
            other.name
        );
    }
    // Everything reachable must actually be movable-to.
    for hex in reach.keys() {
        assert!(
            state.map.get(*hex).is_some(),
            "reachable tile is on the map"
        );
    }
}

#[test]
fn reinforcements_are_adjacent_and_attackers_must_be_fresh() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let ids: Vec<_> = state.side_armies(0).map(|a| a.id).collect();
    assert!(ids.len() >= 2, "frontier gives side 0 two armies");
    let (principal, neighbour) = (ids[0], ids[1]);

    // Park the second army next to the first and ask who could join a
    // battle fought where the first one stands.
    let at = state.army(principal).unwrap().pos;
    let adjacent = at.all_neighbors()[0];
    state.army_mut(neighbour).unwrap().pos = adjacent;

    let defending = state.reinforcement_candidates(at, 0, principal, false);
    assert_eq!(defending, vec![neighbour]);

    // A spent army can still defend, but cannot join an assault.
    state.army_mut(neighbour).unwrap().moved = true;
    assert_eq!(
        state.reinforcement_candidates(at, 0, principal, false),
        vec![neighbour],
        "defenders answer whatever they did this turn"
    );
    assert!(
        state
            .reinforcement_candidates(at, 0, principal, true)
            .is_empty(),
        "an army that already moved cannot join an attack"
    );

    // Out of reach is out of the fight.
    state.army_mut(neighbour).unwrap().moved = false;
    let far = at + hexx::Hex::new(4, 0);
    state.army_mut(neighbour).unwrap().pos = far;
    assert!(
        state
            .reinforcement_candidates(at, 0, principal, true)
            .is_empty()
    );
}

#[test]
fn battle_results_are_returned_to_each_army() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let attacker = state.side_armies(0).next().unwrap().id;
    let helper = state.side_armies(0).nth(1).unwrap().id;
    let defender = state.side_armies(1).next().unwrap().id;
    let defender_pos = state.army(defender).unwrap().pos;

    // The attacker loses everything but one tank, the helper is untouched,
    // and the defender is wiped out.
    let survivor = state.army(attacker).unwrap().units[..1].to_vec();
    let helper_units = state.army(helper).unwrap().units.clone();
    let events = state.apply_battle_result(
        &reg,
        attacker,
        defender,
        &[
            (attacker, survivor.clone()),
            (helper, helper_units.clone()),
            (defender, Vec::new()),
        ],
        &[],
    );

    assert_eq!(state.army(attacker).unwrap().units.len(), survivor.len());
    assert_eq!(state.army(helper).unwrap().units.len(), helper_units.len());
    assert!(state.army(defender).is_none(), "wiped army is destroyed");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyDestroyed { army } if *army == defender)),
        "expected a destruction event, got {events:?}"
    );
    assert_eq!(
        state.army(attacker).unwrap().pos,
        defender_pos,
        "the victor takes the contested tile"
    );
}

#[test]
fn hit_breakdown_explains_the_same_number_hit_chance_returns() {
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 7).unwrap();
    let attacker = state.side_units(0).next().unwrap();
    let target = state.side_units(1).next().unwrap();
    let weapon = reg
        .weapon(&reg.vehicle(&attacker.vehicle).unwrap().weapons[0])
        .unwrap();

    for blind in [false, true] {
        let chance = tactics_core::battle::hit_chance(
            &reg,
            &state,
            attacker.id,
            attacker.pos,
            weapon,
            target.pos,
            blind,
        );
        let breakdown = tactics_core::battle::hit_breakdown(
            &reg,
            &state,
            attacker.id,
            attacker.pos,
            weapon,
            target.pos,
            blind,
        );
        assert_eq!(
            breakdown.total, chance,
            "breakdown must agree with the roll"
        );
        assert!((5..=95).contains(&breakdown.total));

        // Base plus every listed modifier reproduces the total, unless the
        // clamp stepped in -- in which case it must say so.
        let summed: i32 = breakdown.base + breakdown.modifiers.iter().map(|m| m.delta).sum::<i32>();
        if breakdown.clamped {
            assert_ne!(summed, breakdown.total);
        } else {
            assert_eq!(summed, breakdown.total, "modifiers must add up");
        }
        assert_eq!(
            blind,
            breakdown.modifiers.iter().any(|m| m.label == "Blind fire"),
            "blind fire should be itemised exactly when it applies"
        );
    }
}

#[test]
fn attack_preview_describes_the_target() {
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 11).unwrap();
    let attacker = state.side_units(0).next().unwrap();
    let target = state.side_units(1).next().unwrap();

    let preview =
        tactics_core::battle::preview_attack(&reg, &state, attacker.id, 0, target.id, false)
            .expect("both units exist");

    let vehicle = reg.vehicle(&target.vehicle).unwrap();
    assert_eq!(preview.target_vehicle, vehicle.name);
    assert_eq!(preview.target_side, state.sides[target.side as usize].name);
    assert_eq!(preview.target_max_hp, vehicle.max_hp);
    assert_eq!(preview.distance, attacker.pos.distance_to(target.pos));
    assert!(preview.damage >= 1, "a hit always does something");
    assert_eq!(preview.lethal, preview.damage >= preview.target_hp);
    let expected = preview.hit.total as f32 / 100.0 * preview.damage as f32;
    assert!((preview.expected_damage - expected).abs() < 1e-3);
}

#[test]
fn overworld_ai_moves_armies() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let cfg = AiConfig {
        planner: "simple".into(),
        difficulty: 3,
        doctrine: None,
    };
    let mut planner = make_overworld_planner(&cfg, 42);
    // Skip to side 1 and let the AI issue orders.
    let _ = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    assert_eq!(state.active_side, 1);
    let mut moved = 0;
    for _ in 0..10 {
        let order = planner.next_order(&reg, &state, 1);
        if order == OverworldOrder::EndTurn {
            break;
        }
        match state.apply(&reg, &order) {
            Ok(_) => moved += 1,
            Err(_) => break,
        }
    }
    assert!(moved > 0, "overworld AI should move at least one army");
}

// --- campaign missions (chunk 8 of chain of command) -----------------------

/// The campaign under a stated radio net. The base mod's own figure is four
/// overworld hexes; these tests state their own so that what they are about is
/// the rule rather than the tuning.
fn registry_with_net(radius: u32, relay: bool) -> DataRegistry {
    let mut reg = registry();
    let mut rules = reg.command.clone().unwrap_or_default();
    rules.overworld_radius = radius;
    rules.relay = relay;
    reg.command = Some(rules);
    reg
}

/// Give a side a third company, so a chain of armies can be strung out across
/// the map. It fields nothing: what these tests weigh is where an army *is*,
/// and a battle is not one of the things that can happen to it.
fn extra_army(state: &mut OverworldState, side: u8, name: &str, at: [i32; 2]) -> ArmyId {
    let id = ArmyId(state.armies.len() as u32);
    state.armies.push(Army {
        id,
        side,
        name: name.into(),
        pos: tactics_core::offset_to_hex(at[0], at[1]),
        movement: 3,
        moved: false,
        units: Vec::new(),
        alive: true,
        mission: None,
    });
    id
}

/// Push the campaign round to the next turn of `side`, so contact is
/// recomputed against wherever everybody now stands.
fn next_turn_of(reg: &DataRegistry, state: &mut OverworldState, side: u8) -> Vec<OverworldEvent> {
    let mut events = Vec::new();
    for _ in 0..8 {
        events.extend(
            state
                .apply(reg, &OverworldOrder::EndTurn)
                .expect("end turn"),
        );
        if state.active_side == side {
            break;
        }
    }
    events
}

#[test]
fn an_army_mission_is_stored_and_said_out_loud() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let army = state.senior_army(0).expect("side 0 has armies");
    let bridge = tactics_core::offset_to_hex(6, 2);

    let events = state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Advance { to: bridge },
            },
        )
        .expect("her own army, on her own turn, within her own net");
    assert!(
        events.iter().any(|e| matches!(
            e,
            OverworldEvent::ArmyMissionAssigned { army: a, mission: ArmyMission::Advance { to } }
                if *a == army && *to == bridge
        )),
        "a decision somebody made is news: {events:?}"
    );
    assert_eq!(
        state.army(army).unwrap().mission,
        Some(ArmyMission::Advance { to: bridge })
    );

    // Countermanding is ordinary business and replaces silently.
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Hold,
            },
        )
        .expect("orders may be changed");
    assert_eq!(state.army(army).unwrap().mission, Some(ArmyMission::Hold));

    // Ground that is not there is refused, and so is somebody else's army.
    assert_eq!(
        state.apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Advance {
                    to: tactics_core::offset_to_hex(400, 400)
                },
            },
        ),
        Err(OverworldError::NotOnMap)
    );
    let enemy = state.senior_army(1).expect("side 1 has armies");
    assert_eq!(
        state.apply(
            &reg,
            &OverworldOrder::SetMission {
                army: enemy,
                mission: ArmyMission::Hold,
            },
        ),
        Err(OverworldError::NotYourTurn)
    );
}

#[test]
fn an_army_mission_out_of_range_waits_and_then_transmits() {
    // frontier's two companies per side start six hexes apart, so a two-hex
    // net with nobody relaying leaves the junior one on its own. An order for
    // her is *not* refused — it waits at headquarters and goes out on the
    // first morning the wire is up, which is the whole of this chunk on the
    // campaign side.
    let reg = registry_with_net(2, false);
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let senior = state.senior_army(0).unwrap();
    let junior = state
        .side_armies(0)
        .map(|a| a.id)
        .find(|id| *id != senior)
        .expect("frontier gives side 0 two companies");
    assert!(!state.in_contact(junior), "she is six hexes from anybody");
    assert!(state.in_contact(senior), "headquarters hears itself");

    let order = OverworldOrder::SetMission {
        army: junior,
        mission: ArmyMission::Hold,
    };
    let queued = state.apply(&reg, &order).expect("accepted, not refused");
    assert!(
        queued
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyOrdersWaiting { army } if *army == junior)),
        "an order parked without a word would be as bad as one dropped: {queued:?}"
    );
    assert_eq!(
        state.army(junior).unwrap().mission,
        None,
        "she has not been told anything yet"
    );
    assert_eq!(
        state.waiting_missions,
        vec![(junior, ArmyMission::Hold)],
        "it is sitting in the tray"
    );

    // A second order replaces the first rather than queueing behind it: only
    // one of them was ever going to be transmitted.
    let to = tactics_core::offset_to_hex(6, 2);
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army: junior,
                mission: ArmyMission::Advance { to },
            },
        )
        .expect("accepted too");
    assert_eq!(
        state.waiting_missions,
        vec![(junior, ArmyMission::Advance { to })],
        "the newer order is the one headquarters means"
    );

    // Closing up is what fixes it, and the campaign says so when it does —
    // then the order transmits, in that order, as an ordinary assignment.
    let beside = state.army(senior).unwrap().pos + hexx::Hex::new(1, 0);
    state.army_mut(junior).unwrap().pos = beside;
    let events = next_turn_of(&reg, &mut state, 0);
    let restored = events
        .iter()
        .position(|e| matches!(e, OverworldEvent::ArmyContactRestored { army } if *army == junior))
        .expect("coming back on the net is news too");
    let assigned = events
        .iter()
        .position(
            |e| matches!(e, OverworldEvent::ArmyMissionAssigned { army, .. } if *army == junior),
        )
        .expect("and the order she could not be given lands with it");
    assert!(
        restored < assigned,
        "the wire comes back before anything goes down it: {events:?}"
    );
    assert_eq!(
        state.army(junior).unwrap().mission,
        Some(ArmyMission::Advance { to }),
        "and it is the order she was actually given"
    );
    assert!(
        state.waiting_missions.is_empty(),
        "nothing is transmitted twice"
    );

    // With no command block there is no net to be outside of: the same order,
    // from the same six hexes away, is simply an order, landing at once and
    // never touching the queue.
    let wireless = registry_wireless();
    let mut open = OverworldState::from_map(&wireless, "frontier", 1).unwrap();
    assert!(open.out_of_contact.is_empty(), "nothing was ever computed");
    open.apply(&wireless, &order)
        .expect("a campaign with no radios has no radio range");
    assert_eq!(open.army(junior).unwrap().mission, Some(ArmyMission::Hold));
    assert!(open.waiting_missions.is_empty());
}

#[test]
fn relay_carries_orders_through_a_chain_of_armies() {
    // Three companies in a line, each three hexes from the next: the far one
    // is six from headquarters and can only be reached through the middle.
    let build = |reg: &DataRegistry| {
        let mut state = OverworldState::from_map(reg, "frontier", 1).unwrap();
        let senior = state.senior_army(0).unwrap();
        state.army_mut(senior).unwrap().pos = tactics_core::offset_to_hex(1, 1);
        let middle = extra_army(&mut state, 0, "3rd Company", [4, 1]);
        let far = extra_army(&mut state, 0, "4th Company", [7, 1]);
        // Recomputed at the top of a turn, so give it one.
        let events = next_turn_of(reg, &mut state, 0);
        (state, middle, far, events)
    };

    let relaying = registry_with_net(3, true);
    let (state, middle, far, _) = build(&relaying);
    assert!(state.in_contact(middle), "she is three hexes out");
    assert!(
        state.in_contact(far),
        "and she is three hexes from her, which is what relaying is for"
    );

    let alone = registry_with_net(3, false);
    let (mut state, middle, far, events) = build(&alone);
    assert!(state.in_contact(middle), "still inside the net herself");
    assert!(
        !state.in_contact(far),
        "with nobody passing the signal on, six hexes is six hexes"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyOutOfContact { army } if *army == far)),
        "an army the player cannot order must be told about: {events:?}"
    );
    // Her orders are taken and held rather than refused; what relaying buys
    // is that they go out today instead of whenever she closes up.
    state
        .apply(
            &alone,
            &OverworldOrder::SetMission {
                army: far,
                mission: ArmyMission::Hold,
            },
        )
        .expect("accepted, and waiting for a wire");
    assert_eq!(state.waiting_missions, vec![(far, ArmyMission::Hold)]);
    assert_eq!(state.army(far).unwrap().mission, None);
}

#[test]
fn a_standing_mission_moves_the_army_when_its_turn_ends() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let army = state.senior_army(0).unwrap();
    // Along the northern highway, well clear of the enemy: this test is about
    // orders being carried out, not about what happens when they meet
    // somebody.
    let target = tactics_core::offset_to_hex(6, 2);
    let start = state.army(army).unwrap().pos;
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Advance { to: target },
            },
        )
        .unwrap();

    let events = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyMoved { army: a, .. } if *a == army)),
        "nobody ordered her anywhere this turn and she went anyway: {events:?}"
    );
    let after_one = state.army(army).unwrap().pos;
    assert_ne!(after_one, start, "she set off");

    // ...and keeps going, day after day, until she is standing on it.
    let mut days = 1;
    while state.army(army).unwrap().pos != target && days < 12 {
        next_turn_of(&reg, &mut state, 0);
        state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
        days += 1;
    }
    assert_eq!(
        state.army(army).unwrap().pos,
        target,
        "she should have arrived within {days} days"
    );
    assert!(days > 1, "or this test proves nothing about the days after");

    // Arrived is arrived: the order stands, and standing on it is obeying it.
    next_turn_of(&reg, &mut state, 0);
    let events = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyMoved { army: a, .. } if *a == army)),
        "she is already there: {events:?}"
    );
    assert!(
        state.army(army).unwrap().mission.is_some(),
        "and she is still under orders, not released from them"
    );
}

#[test]
fn a_hand_moved_army_is_not_second_guessed_by_its_mission() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let army = state.senior_army(0).unwrap();
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Advance {
                    to: tactics_core::offset_to_hex(6, 2),
                },
            },
        )
        .unwrap();

    // The player has changed her mind today, and hers is the newer decision.
    let elsewhere = tactics_core::offset_to_hex(1, 3);
    state
        .apply(
            &reg,
            &OverworldOrder::MoveArmy {
                army,
                to: elsewhere,
            },
        )
        .unwrap();
    let by_hand = state.army(army).unwrap().pos;

    let events = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyMoved { army: a, .. } if *a == army)),
        "her turn was already spent: {events:?}"
    );
    assert_eq!(state.army(army).unwrap().pos, by_hand);
    assert!(
        state.army(army).unwrap().mission.is_some(),
        "the standing order survives the day it was overruled"
    );
}

#[test]
fn a_withdrawing_army_fights_its_battle_toward_the_exit() {
    let reg = registry_wireless();
    let file = reg.map("river_crossing").expect("shipped battle map");
    let map = HexMap::from_map_file(file).expect("map parses");

    // Two companies facing each other along the trunk road, each one a
    // formation, exactly as the campaign's `deploy` assembles them.
    let placement = |col: i32, side: u8, formation: &str, leads: bool| UnitPlacement {
        at: [col, 20],
        side,
        vehicle: "medium_tank".into(),
        crew: Vec::new(),
        name: Some(format!("{formation}-{col}")),
        facing: None,
        formation: Some(formation.into()),
        leads,
    };
    let placements = vec![
        placement(10, 0, "kuhlmann_armor", true),
        placement(11, 0, "kuhlmann_armor", false),
        placement(30, 1, "valkyrie_line", true),
        placement(31, 1, "valkyrie_line", false),
    ];
    let sides = vec![
        SideState {
            name: "Kuhlmann".into(),
            ai: None,
        },
        SideState {
            name: "Valkyries".into(),
            ai: None,
        },
    ];
    let mut state = BattleState::from_placements(
        &reg,
        map,
        sides,
        &placements,
        &[Vec::new(), Vec::new(), Vec::new(), Vec::new()],
        std::sync::Arc::new(tactics_core::roster::Roster::new()),
        11,
    );

    // The helper first, on its own terms: each side is sent down its own
    // road, and a side the map offers no lane to is sent nowhere.
    let west = state.side_units(0).next().unwrap().pos;
    let east = state.side_units(1).next().unwrap().pos;
    assert_eq!(
        tactics_core::battle::nearest_exit(&state, 0, west).as_deref(),
        Some("west_road")
    );
    assert_eq!(
        tactics_core::battle::nearest_exit(&state, 1, east).as_deref(),
        Some("east_road"),
        "a lane belongs to the side that was given it, however close the other is"
    );
    assert_eq!(
        tactics_core::battle::nearest_exit(&state, 2, west),
        None,
        "a side with no road home holds where it stands"
    );

    // Now the campaign's order, as the field-battle setup issues it: every
    // formation of the withdrawing side, out by its nearest lane.
    let index = state
        .formations()
        .iter()
        .position(|f| f.side == 1)
        .expect("side 1 fields a formation");
    let via = tactics_core::battle::nearest_exit(&state, 1, east).unwrap();
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: FormationId(index as u32),
                mission: Mission::Withdraw { via: via.clone() },
            },
        )
        .expect("her own lane");

    let lane: Vec<tactics_core::Hex> = state
        .map
        .objectives()
        .iter()
        .find(|o| o.id == via)
        .unwrap()
        .hexes
        .clone();
    let mut ai = AiDriver::new();
    ai.insert(1, sharp_planner(&reg, 11, "massed_armor"));
    ai.plan_round(&reg, &mut state);

    let toward = |hex: tactics_core::Hex| lane.iter().map(|h| h.distance_to(hex)).min().unwrap();
    for id in state.formations()[index].members.clone() {
        let unit = state.unit(id).expect("planning harms nobody");
        assert!(
            toward(unit.planned_destination()) < toward(unit.pos),
            "{} was told on the campaign map to break off, so she drives for the road",
            unit.name
        );
    }
}

fn two_side_battle(
    reg: &DataRegistry,
    rows: &[&str],
    placements: Vec<UnitPlacement>,
    seed: u64,
) -> BattleState {
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "test_map",
        "palette": { "g": "grass", "f": "forest" },
        "rows": rows,
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let sides = vec![
        SideState {
            name: "West".into(),
            ai: None,
        },
        SideState {
            name: "East".into(),
            ai: None,
        },
    ];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
}

fn unit_at(at: [i32; 2], side: u8, vehicle: &str, name: &str) -> UnitPlacement {
    UnitPlacement {
        at,
        side,
        vehicle: vehicle.into(),
        crew: Vec::new(),
        name: Some(name.into()),
        facing: None,
        formation: None,
        leads: false,
    }
}

/// Two medium tanks three hexes apart in the open, in plain sight of each
/// other. Unit 0 is West, unit 1 is East.
fn duel(reg: &DataRegistry, seed: u64) -> BattleState {
    let state = two_side_battle(
        reg,
        &["ggggg", "ggggg", "ggggg"],
        vec![
            unit_at([0, 1], 0, "medium_tank", "West"),
            unit_at([3, 1], 1, "medium_tank", "East"),
        ],
        seed,
    );
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1))
            && state.fog.side(1).spotted.contains(&UnitId(0)),
        "the duel needs both crews to see each other"
    );
    state
}

/// Two tanks that cannot see each other, with a forest curtain between them.
///
/// A hex is 100 m and a tank commander sees over a kilometre, so a small test
/// map can no longer put units out of contact by standing them far apart —
/// terrain has to do it. Tests about the round structure rather than about
/// shooting start here, so a chance encounter cannot end the battle early.
/// A battle on a map that declares ground worth taking.
///
/// Every objective test uses the same forest curtain as [`standoff`]: the
/// rules being checked are about who is standing where, and two crews trading
/// fire would end the battle before the bookkeeping could be observed.
fn objective_battle(
    reg: &DataRegistry,
    objectives: serde_json::Value,
    victory_score: Option<u32>,
    placements: Vec<UnitPlacement>,
) -> BattleState {
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "objective_map",
        "palette": { "g": "grass", "f": "forest" },
        "rows": ["gggggfggggg"],
        "objectives": objectives,
        "victory_score": victory_score,
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let sides = vec![
        SideState {
            name: "West".into(),
            ai: None,
        },
        SideState {
            name: "East".into(),
            ai: None,
        },
    ];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        1,
    )
}

/// The two crews of an objective test, out of contact behind the curtain.
fn curtained_pair() -> Vec<UnitPlacement> {
    vec![
        unit_at([0, 0], 0, "medium_tank", "West"),
        unit_at([10, 0], 1, "medium_tank", "East"),
    ]
}

#[test]
fn ground_is_taken_by_standing_on_it_and_stays_taken_after_leaving() {
    // Control persists on purpose: ground you have taken has to be taken back
    // rather than merely vacated, or an objective would be worth nothing to
    // anyone who has somewhere else to be.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{ "id": "crossroads", "at": [[2, 0]], "value": 1 }]),
        None,
        curtained_pair(),
    );
    assert_eq!(state.objective_held, vec![None], "nobody starts holding it");

    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: tactics_core::offset_to_hex(2, 0),
            },
        )
        .expect("west can drive to the crossroads");
    let events = play_round(&reg, &mut state);
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::ObjectiveTaken { objective, side: Some(0), .. } if objective == "crossroads"
        )),
        "taking ground is announced, or a battle decided on points reads as arbitrary"
    );
    assert_eq!(state.objective_held, vec![Some(0)]);

    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: tactics_core::offset_to_hex(0, 0),
            },
        )
        .expect("west can drive home again");
    let events = play_round(&reg, &mut state);
    assert_eq!(
        state.objective_held,
        vec![Some(0)],
        "walking away does not hand the ground back"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::ObjectiveTaken { .. })),
        "ground that did not change hands says nothing"
    );
}

#[test]
fn ground_two_sides_stand_on_belongs_to_neither() {
    // The objective is deliberately two hexes at opposite ends of the map, so
    // that one crew from each side can stand on it without being in a
    // position to shoot the other. What is under test is the contest rule,
    // not gunnery.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{ "id": "the_valley", "at": [[0, 0], [10, 0]], "value": 4 }]),
        None,
        curtained_pair(),
    );
    play_round(&reg, &mut state);
    assert_eq!(
        state.objective_held,
        vec![None],
        "ground both sides are standing on is nobody's"
    );
    assert_eq!(
        (state.score(0), state.score(1)),
        (0, 0),
        "and a contested objective pays nobody, or a defender could collect \
         points while being overrun"
    );
}

#[test]
fn holding_ground_pays_once_a_round_however_many_ticks_a_round_has() {
    // Paying per tick would make the size of every score an accident of
    // `ticks_per_round`, which is mod data and may be anything.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{ "id": "crossroads", "at": [[0, 0]], "value": 3 }]),
        None,
        curtained_pair(),
    );
    for round in 1..=3 {
        play_round(&reg, &mut state);
        assert_eq!(
            state.score(0),
            3 * round,
            "three points a round, not three a tick"
        );
        assert_eq!(state.score(1), 0);
    }
}

#[test]
fn a_side_that_holds_the_ground_wins_a_battle_that_loses_contact() {
    // The rule this whole feature exists for. Two crews who never find each
    // other used to produce a draw, which made sitting still unbeatable; now
    // the one that walked to the objective has something to show for it.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{ "id": "crossroads", "at": [[2, 0]], "value": 1 }]),
        None,
        curtained_pair(),
    );
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: tactics_core::offset_to_hex(2, 0),
            },
        )
        .expect("west can drive to the crossroads");

    let mut ended = None;
    for _ in 0..(STALEMATE_ROUNDS as usize + 2) {
        let events = play_round(&reg, &mut state);
        if let Some(BattleEvent::BattleEnded { winner, reason }) = events
            .iter()
            .find(|e| matches!(e, BattleEvent::BattleEnded { .. }))
        {
            ended = Some((*winner, *reason));
            break;
        }
    }
    assert_eq!(
        ended,
        Some((Some(0), EndReason::Stalemate)),
        "breaking contact ends the shooting; the points say who won"
    );
    assert_eq!(
        state.alive_units().count(),
        2,
        "and it still costs nobody their tanks"
    );
}

#[test]
fn reaching_the_victory_score_ends_the_battle_outright() {
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{ "id": "the_hill", "at": [[0, 0]], "value": 5 }]),
        Some(10),
        curtained_pair(),
    );
    let mut ended = None;
    for _ in 0..4 {
        let events = play_round(&reg, &mut state);
        if let Some(BattleEvent::BattleEnded { winner, reason }) = events
            .iter()
            .find(|e| matches!(e, BattleEvent::BattleEnded { .. }))
        {
            ended = Some((*winner, *reason));
            break;
        }
    }
    assert_eq!(ended, Some((Some(0), EndReason::Objectives)));
    assert_eq!(
        state.round, 2,
        "two rounds at five points a round, and not a round later"
    );
}

#[test]
fn driving_off_an_exit_takes_the_crew_home_rather_than_killing_them() {
    // The distinction the whole exit mechanism rests on. `alive` is "on the
    // battlefield" and answers targeting and fog; it is not "came home", and
    // the campaign reads the second.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{
            "id": "west_road", "at": [[0, 0]], "value": 5,
            "kind": "exit", "side": 0
        }]),
        None,
        vec![
            unit_at([1, 0], 0, "medium_tank", "Leaver"),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
    );
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: tactics_core::offset_to_hex(0, 0),
            },
        )
        .expect("west can reach the road");
    let events = play_round(&reg, &mut state);

    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::UnitExited { unit, objective, .. }
                if *unit == UnitId(0) && objective == "west_road"
        )),
        "leaving is announced in its own right, never as a destruction"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::UnitDestroyed { .. })),
        "nobody was destroyed"
    );

    let leaver = &state.units[0];
    assert!(!leaver.alive, "she is off the board");
    assert!(leaver.exited, "but she left under her own power");
    assert_eq!(state.score(0), 5, "and the exit paid its value once");

    assert!(
        state.surviving_units().any(|u| u.id == UnitId(0)),
        "the campaign must count her among the survivors"
    );
    assert!(
        !state.lost_units().any(|u| u.id == UnitId(0)),
        "and must not count her among the losses"
    );
}

#[test]
fn an_exit_belongs_to_the_side_it_names() {
    // An exit anyone may use is a lane both armies leave by on round one.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{
            "id": "west_road", "at": [[0, 0], [10, 0]], "value": 5,
            "kind": "exit", "side": 0
        }]),
        None,
        // Side 1 starts standing on a hex of side 0's exit. Both stay behind
        // the curtain: the rule under test is eligibility, and a firefight
        // would settle it by killing somebody instead.
        vec![
            unit_at([1, 0], 0, "medium_tank", "West"),
            unit_at([10, 0], 1, "medium_tank", "Squatter"),
        ],
    );
    play_round(&reg, &mut state);
    assert!(
        state.units[1].alive && !state.units[1].exited,
        "side 1 may not leave by side 0's road"
    );
    assert_eq!(state.score(1), 0);
}

#[test]
fn an_exit_is_not_ground_anybody_holds() {
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{
            "id": "west_road", "at": [[0, 0]], "value": 5,
            "kind": "exit", "side": 0
        }]),
        None,
        curtained_pair(),
    );
    play_round(&reg, &mut state);
    assert_eq!(
        state.objective_held,
        vec![None],
        "an exit is passed through, not held, so it never pays per round"
    );
}

#[test]
fn a_withdrawal_that_reaches_its_target_wins_on_the_tick_it_completes() {
    // Elimination is checked *after* the score for exactly this case: the
    // last vehicle of a withdrawing force leaves the board and reaches the
    // target in the same tick. Checking the board first would award the
    // battle to an enemy holding a field nobody wanted.
    let reg = registry();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([{
            "id": "west_road", "at": [[0, 0]], "value": 10,
            "kind": "exit", "side": 0
        }]),
        Some(10),
        vec![
            unit_at([1, 0], 0, "medium_tank", "Last Out"),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
    );
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: tactics_core::offset_to_hex(0, 0),
            },
        )
        .expect("west can reach the road");
    let events = play_round(&reg, &mut state);

    let ended = events.iter().find_map(|e| match e {
        BattleEvent::BattleEnded { winner, reason } => Some((*winner, *reason)),
        _ => None,
    });
    assert_eq!(
        ended,
        Some((Some(0), EndReason::Objectives)),
        "the force that got away won, though it has nothing left on the field"
    );
}

#[test]
fn an_intact_crew_will_not_run_for_the_exit_but_a_broken_one_will() {
    // Withdrawal has to be conditional or the lane is a free win: every unit
    // would drive off on round one. The gate is the doctrine's
    // `withdraw_threshold` against the vehicle's own damage.
    let reg = registry();
    let state = objective_battle(
        &reg,
        serde_json::json!([{
            "id": "west_road", "at": [[0, 0]], "value": 5,
            "kind": "exit", "side": 0
        }]),
        None,
        curtained_pair(),
    );
    let eval = Evaluator::new(reg.doctrine("elastic_defense").cloned().unwrap());
    let exit = tactics_core::offset_to_hex(0, 0);
    let away = tactics_core::offset_to_hex(4, 0);

    let healthy = state.units[0].hp;
    assert!(
        eval.score_tile(&reg, &state, UnitId(0), exit).score
            <= eval.score_tile(&reg, &state, UnitId(0), away).score,
        "an undamaged crew cannot see the exit at all"
    );

    let mut hurt = state;
    hurt.units[0].hp = 1;
    assert!(hurt.units[0].hp < healthy);
    assert!(
        eval.score_tile(&reg, &hurt, UnitId(0), exit).score
            > eval.score_tile(&reg, &hurt, UnitId(0), away).score,
        "a crew that is nearly finished should run for the road"
    );
}

#[test]
fn a_map_that_names_no_objectives_is_fought_exactly_as_it_was_before() {
    // Objectives have to be an additive rule whose absence is the old game —
    // the same constraint difficulty-as-a-mod puts on every harsh system. The
    // check that bites is the evaluator's: on a map with no objectives, how
    // much a doctrine cares about objectives must not change a single score.
    let reg = registry();
    let state = standoff(&reg, 1);
    assert!(state.map.objectives().is_empty());
    assert!(
        state.leader().is_none(),
        "nobody leads a battle with nothing to lead on"
    );

    let mut indifferent = reg.doctrine("massed_armor").cloned().unwrap();
    indifferent.objective_value = 0.0;
    let mut greedy = indifferent.clone();
    greedy.objective_value = 25.0;

    let (a, b) = (Evaluator::new(indifferent), Evaluator::new(greedy));
    for (tile, _) in state.map.iter() {
        assert_eq!(
            a.score_tile(&reg, &state, UnitId(0), tile).score,
            b.score_tile(&reg, &state, UnitId(0), tile).score,
            "a map with no objectives cannot be scored differently by a \
             doctrine that wants them"
        );
    }
}

#[test]
fn an_objective_the_map_does_not_contain_is_a_validation_error() {
    let reg = registry();
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "bad_objectives",
        "palette": { "g": "grass" },
        "rows": ["ggg"],
        "shape": "free",
        "objectives": [
            { "id": "nowhere", "at": [[99, 99]], "value": 1 },
            { "id": "nowhere", "at": [], "value": 1 },
        ],
    }))
    .unwrap();
    let mut report = tactics_core::data::ValidationReport::default();
    file.validate_into(&reg, &mut report);

    let errors = report.errors.join("\n");
    assert!(
        errors.contains("outside the map"),
        "an objective nobody can stand on must not load quietly: {errors}"
    );
    assert!(
        errors.contains("names no hexes"),
        "nor one with no ground at all: {errors}"
    );
    assert!(
        errors.contains("share the id"),
        "nor two that cannot be told apart: {errors}"
    );
}

/// A map file whose chain of command is deliberately broken in every way
/// there is, so one call to `validate_into` can be asked about all of them.
fn tangled_command_map() -> tactics_core::map::MapFile {
    serde_json::from_value(serde_json::json!({
        "id": "tangled_command",
        "palette": { "g": "grass" },
        "rows": ["gggggg"],
        "shape": "free",
        "sides": [{ "name": "West" }, { "name": "East" }],
        "formations": [
            { "id": "twins", "side": 0 },
            { "id": "twins", "side": 0 },
            { "id": "nobody", "side": 1 },
            { "id": "mixed", "side": 0 },
        ],
        "units": [
            { "at": [0, 0], "side": 0, "vehicle": "medium_tank",
              "formation": "twins", "leads": true },
            { "at": [1, 0], "side": 0, "vehicle": "medium_tank",
              "formation": "twins", "leads": true },
            { "at": [2, 0], "side": 1, "vehicle": "medium_tank",
              "formation": "mixed" },
            { "at": [3, 0], "side": 0, "vehicle": "medium_tank",
              "formation": "ghost_platoon" },
            { "at": [4, 0], "side": 0, "vehicle": "medium_tank", "leads": true },
        ],
    }))
    .unwrap()
}

#[test]
fn a_chain_of_command_that_does_not_join_up_is_a_validation_error() {
    // Formations say who obeys whom, so a typo does not merely look wrong: it
    // leaves a vehicle outside the chain, which once missions exist is
    // indistinguishable from a crew that was told to sit still. Every one of
    // these is refused rather than warned about for that reason.
    let reg = registry();
    let mut report = tactics_core::data::ValidationReport::default();
    tangled_command_map().validate_into(&reg, &mut report);
    let errors = report.errors.join("\n");

    assert!(
        errors.contains("does not declare"),
        "a unit in a formation nobody declared must be refused: {errors}"
    );
    assert!(
        errors.contains("has no members"),
        "nor a formation nobody is in: {errors}"
    );
    assert!(
        errors.contains("only one girl can be in command"),
        "nor two crews both claiming to lead: {errors}"
    );
    assert!(
        errors.contains("which belongs to side"),
        "nor a formation spanning two armies: {errors}"
    );
    assert!(
        errors.contains("marked `leads` but is in no formation"),
        "nor a leader of nothing: {errors}"
    );
    assert!(
        errors.contains("two formations share the id"),
        "nor two formations that cannot be told apart: {errors}"
    );
}

#[test]
fn a_formation_asking_for_an_unknown_doctrine_falls_back_rather_than_failing() {
    // Doctrine degrades everywhere else it is named — an unknown one on a side
    // becomes the balanced default rather than refusing to field the side —
    // and a formation's own doctrine is the same bargain one level down.
    let reg = registry();
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "odd_doctrine",
        "palette": { "g": "grass" },
        "rows": ["gg"],
        "shape": "free",
        "sides": [{ "name": "West" }],
        "formations": [
            { "id": "first", "side": 0, "doctrine": "napoleonic_squares" },
        ],
        "units": [
            { "at": [0, 0], "side": 0, "vehicle": "medium_tank", "formation": "first" },
        ],
    }))
    .unwrap();
    let mut report = tactics_core::data::ValidationReport::default();
    file.validate_into(&reg, &mut report);

    assert!(
        report.is_ok(),
        "an unknown doctrine must not stop the map loading: {:?}",
        report.errors
    );
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("napoleonic_squares")),
        "but it must be said out loud: {:?}",
        report.warnings
    );
}

#[test]
fn a_map_that_declares_formations_puts_them_on_the_battle() {
    // The shipped scenario is the fixture on purpose: it is what the
    // determinism baseline is fought on, so populating command state here is
    // what makes "nothing reads it yet" a checkable claim rather than a hope.
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 1).expect("battle");

    let ids: Vec<&str> = state.formations().iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "kuhlmann_armor",
            "kuhlmann_recon",
            "valkyrie_line",
            "valkyrie_screen"
        ],
        "formations must arrive in the order the map declared them"
    );

    let armor = state
        .formations()
        .iter()
        .find(|f| f.id == "kuhlmann_armor")
        .expect("the map declares it");
    assert_eq!(armor.side, 0);
    // Anka's medium tank is placed first and says `leads`; Mina's light tank
    // follows her. Members are unit ids, which is placement order.
    assert_eq!(armor.members, vec![UnitId(0), UnitId(2)]);
    assert_eq!(armor.leader, Some(UnitId(0)));

    // A formation that names no leader falls back to its first member, which
    // is the seniority a map author controls by declaration order.
    let screen = state
        .formations()
        .iter()
        .find(|f| f.id == "valkyrie_screen")
        .expect("the map declares it");
    assert_eq!(screen.side, 1);
    assert_eq!(screen.leader, Some(UnitId(7)), "Greta's car says `leads`");

    // Every unit answers to exactly one formation, and to the right one.
    for unit in state.units.iter() {
        let formation = state
            .formation_of(unit.id)
            .unwrap_or_else(|| panic!("{} is in no formation", unit.name));
        assert_eq!(
            formation.side, unit.side,
            "{} answers to the other army",
            unit.name
        );
    }
}

#[test]
fn a_map_that_declares_no_formations_has_no_chain_of_command() {
    // The additivity rule: saying nothing is one flat pool per side, which is
    // every battle this engine fought before formations existed.
    let reg = registry();
    let state = standoff(&reg, 1);
    assert!(state.formations().is_empty());
    assert!(state.formation_of(UnitId(0)).is_none());
}

/// The formation a test orders about: `river_crossing`'s first, which is
/// Kuhlmann's armored platoon on side 0. Named through the map rather than by
/// a bare index so that a map edit that reorders the declarations fails here
/// loudly instead of quietly testing a different platoon.
fn formation_named(state: &BattleState, id: &str) -> FormationId {
    let index = state
        .formations()
        .iter()
        .position(|f| f.id == id)
        .unwrap_or_else(|| panic!("river_crossing declares no formation `{id}`"));
    FormationId(index as u32)
}

#[test]
fn setting_a_mission_stores_it_on_the_formation_and_says_so_out_loud() {
    // The whole point of routing missions through `apply` is that the human,
    // the AI and a replay all speak one vocabulary — so the order has to land
    // in state *and* produce the event a log can carry. Silence would be the
    // failure mode: a mission nobody can see is indistinguishable from one
    // that was dropped.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 5).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let bridge = state.map.objectives()[0].anchor();

    let events = state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: bridge },
            },
        )
        .expect("the bridge is on the map and the platoon exists");
    assert_eq!(
        events,
        vec![BattleEvent::MissionAssigned {
            formation: "kuhlmann_armor".into(),
            mission: Mission::Advance { to: bridge },
        }],
        "the event names the formation a reader would recognise, not its index"
    );
    assert_eq!(
        state.formations()[armor.index()].mission,
        Some(Mission::Advance { to: bridge })
    );

    // A second mission replaces the first rather than queueing behind it:
    // countermanding an order is ordinary business, and a formation holding
    // two missions at once means nothing anyone could act on.
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Hold { at: None },
            },
        )
        .expect("countermanding is legal");
    assert_eq!(
        state.formations()[armor.index()].mission,
        Some(Mission::Hold { at: None })
    );
}

#[test]
fn a_mission_is_a_standing_order_and_outlives_the_round_it_was_given_in() {
    // The distinction the whole command layer rests on: a unit's intent is
    // this round's instructions and is wiped when the next one opens, while a
    // formation told to take the bridge is still taking it tomorrow. If
    // `begin_round` ever cleared this, missions would silently become
    // per-round orders and every executor built on top would be wrong.
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 6).expect("battle");
    let recon = formation_named(&state, "kuhlmann_recon");
    let ford = state.map.objectives()[1].anchor();
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: recon,
                mission: Mission::Recon { toward: ford },
            },
        )
        .expect("the upper ford is on the map");

    let round = state.round;
    play_round(&reg, &mut state);
    assert!(state.round > round, "a whole round must have gone by");
    assert_eq!(
        state.formations()[recon.index()].mission,
        Some(Mission::Recon { toward: ford }),
        "a standing order stands"
    );
}

#[test]
fn a_mission_for_a_formation_that_does_not_exist_is_refused() {
    // The handle arrives from outside — a save, a replay, a brain that is not
    // this process — so a stale one is an ordinary refusal, never a panic.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 7).expect("battle");
    let past_the_end = FormationId(state.formations().len() as u32);
    assert_eq!(
        state.apply(
            &reg,
            &Order::SetMission {
                formation: past_the_end,
                mission: Mission::Hold { at: None },
            },
        ),
        Err(tactics_core::battle::OrderError::NoSuchFormation),
    );
}

#[test]
fn a_mission_set_after_the_side_has_committed_is_refused() {
    // A mission is an order, and orders close when a side hands its planning
    // in — the same bargain `SetFire` and `SetMove` already make. Ordering a
    // platoon about after the round has been sealed would let a side plan
    // twice.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 8).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let line = formation_named(&state, "valkyrie_line");
    state
        .apply(&reg, &Order::Commit { side: 0 })
        .expect("commit");

    assert_eq!(
        state.apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Hold { at: None },
            },
        ),
        Err(tactics_core::battle::OrderError::AlreadyCommitted),
    );
    // The other army has not committed and is unaffected: the refusal is
    // about whose side spoke, not about the phase.
    assert!(
        state
            .apply(
                &reg,
                &Order::SetMission {
                    formation: line,
                    mission: Mission::Hold { at: None },
                },
            )
            .is_ok(),
        "side 1 is still planning"
    );
}

#[test]
fn a_withdrawal_must_name_an_exit_this_side_may_use() {
    // Three ways to get this wrong, all of which would otherwise send a
    // formation to the map edge to wait for a way out that is not there: a
    // name nobody declared, ground that is held rather than left by, and the
    // enemy's lane. `river_crossing` gives west_road to side 0 and east_road
    // to side 1, which is what makes the last one checkable at all.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 9).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let withdraw = |via: &str| Order::SetMission {
        formation: armor,
        mission: Mission::Withdraw { via: via.into() },
    };
    let refused = Err(tactics_core::battle::OrderError::NoSuchExit);

    assert_eq!(
        state.apply(&reg, &withdraw("the_scenic_route")),
        refused,
        "no objective by that name"
    );
    assert_eq!(
        state.apply(&reg, &withdraw("bridge")),
        refused,
        "the bridge is ground to hold, not a way off the map"
    );
    assert_eq!(
        state.apply(&reg, &withdraw("east_road")),
        refused,
        "the eastern road is the Valkyries' lane"
    );
    assert!(
        state.apply(&reg, &withdraw("west_road")).is_ok(),
        "but her own road is hers to leave by"
    );
    assert_eq!(
        state.formations()[armor.index()].mission,
        Some(Mission::Withdraw {
            via: "west_road".into()
        }),
        "and only the accepted one is remembered"
    );
}

#[test]
fn a_mission_that_names_ground_off_the_map_is_refused() {
    // Only what cannot change as the round plays out is checked — a hex being
    // on the map is that; the ground being reachable or wise is the
    // executor's problem. Every variant that carries a hex is covered,
    // because `Hold`'s optional one is exactly the sort of field a validator
    // forgets.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 10).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let nowhere = tactics_core::offset_to_hex(500, 500);
    assert!(!state.map.contains(nowhere));
    let not_on_map = Err(tactics_core::battle::OrderError::NotOnMap);
    for mission in [
        Mission::Advance { to: nowhere },
        Mission::Recon { toward: nowhere },
        Mission::Hold { at: Some(nowhere) },
    ] {
        assert_eq!(
            state.apply(
                &reg,
                &Order::SetMission {
                    formation: armor,
                    mission: mission.clone(),
                },
            ),
            not_on_map,
            "{mission:?} names a tile that is not there"
        );
    }
    assert!(
        state.formations()[armor.index()].mission.is_none(),
        "a refused mission leaves the formation as it was"
    );
}

fn standoff(reg: &DataRegistry, seed: u64) -> BattleState {
    let state = two_side_battle(
        reg,
        &["gggggfggggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "West"),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
        seed,
    );
    assert!(
        state.fog.side(0).spotted.is_empty() && state.fog.side(1).spotted.is_empty(),
        "the standoff needs the forest curtain to hide both crews"
    );
    state
}

#[test]
fn a_column_advances_without_ambushing_itself() {
    // One hex holds one unit, so a friend in the way is traffic rather than an
    // enemy: the unit behind waits a tick and follows, and nothing about it
    // resembles an ambush.
    let reg = registry();
    // The bystander sits behind a forest curtain: in the open it would be
    // spotted and shot at, and the battle could end before the column moves.
    let mut state = two_side_battle(
        &reg,
        &["ggggggfgggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Rear"),
            unit_at([1, 0], 0, "medium_tank", "Lead"),
            unit_at([10, 0], 1, "medium_tank", "Bystander"),
        ],
        1,
    );
    let (rear, lead) = (UnitId(0), UnitId(1));
    let start = state.unit(rear).unwrap().pos;
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: lead,
                to: tactics_core::offset_to_hex(3, 0),
            },
        )
        .expect("the lead tank has open ground");
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: rear,
                to: tactics_core::offset_to_hex(2, 0),
            },
        )
        .expect("routing behind a friend is legal");

    let events = play_round(&reg, &mut state);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::UnitTrapped { .. })),
        "a friend is never an ambush: {events:?}"
    );
    assert_ne!(
        state.unit(rear).unwrap().pos,
        start,
        "the rear tank should have followed the lead one up the road"
    );
}

#[test]
fn friendlies_are_never_ordered_onto_the_same_hex() {
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["ggggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "First"),
            unit_at([1, 0], 0, "medium_tank", "Second"),
            unit_at([4, 0], 1, "medium_tank", "Bystander"),
        ],
        1,
    );
    let (first, second) = (UnitId(0), UnitId(1));
    let contested = tactics_core::offset_to_hex(3, 0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: first,
                to: contested,
            },
        )
        .expect("an empty hex is a fine destination");
    assert!(
        !reachable(&reg, &state, second).contains_key(&contested),
        "a hex a friend is already driving to is taken"
    );
    assert_eq!(
        state.apply(
            &reg,
            &Order::SetMove {
                unit: second,
                to: contested
            }
        ),
        Err(tactics_core::battle::OrderError::NoPath),
        "two units must not be ordered into the same hex"
    );
}

#[test]
fn unspotted_enemies_still_ambush() {
    // Crews look between ticks, not between hexes. A unit crossing several
    // hexes in one tick outruns its own eyes and can drive into somebody it
    // never saw; a unit ambling along a hex at a time normally spots the
    // enemy the tick before contact and simply stops.
    let reg = registry();
    // A forest curtain hides the ambusher, so the mover plans a route
    // through ground it cannot see into.
    let mut state = two_side_battle(
        &reg,
        &["ggfgggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Mover"),
            unit_at([4, 0], 1, "medium_tank", "Ambusher"),
        ],
        2,
    );
    let mover = UnitId(0);
    let ambusher = UnitId(1);
    assert!(
        !state.fog.side(0).spotted.contains(&ambusher),
        "ambusher must start unseen for this test"
    );
    // The ambusher's own tile: an unspotted enemy never blocks a
    // destination, which is what makes this order an ambush rather than a
    // refusal. Grass, forest, grass, grass costs exactly one round's fuel.
    let dest = tactics_core::offset_to_hex(4, 0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: mover,
                to: dest,
            },
        )
        .expect("pathing through fog should be attempted");
    for side in state.living_sides() {
        state.apply(&reg, &Order::Commit { side }).unwrap();
    }
    // Enough banked movement to run the whole leg inside the first tick,
    // before any fog recompute can warn the driver.
    state.unit_mut(mover).unwrap().move_credit = 64 * reg.scale.ticks_per_round;

    let events = state.step_tick(&reg);
    let trapped = events.iter().find_map(|e| match e {
        BattleEvent::UnitTrapped { unit, at } if *unit == mover => Some(*at),
        _ => None,
    });
    assert_eq!(
        trapped,
        Some(tactics_core::offset_to_hex(3, 0)),
        "the advance should stop on the tile before the ambusher, short of {dest:?}: {events:?}"
    );
    let mover = state.unit(mover).expect("the mover survives one tick");
    assert_eq!(mover.pos, tactics_core::offset_to_hex(3, 0));
    assert!(
        mover.intent.path.is_empty(),
        "the rest of the route is abandoned"
    );
}

#[test]
fn an_enemy_you_can_see_halts_the_advance_without_surprising_anyone() {
    // The other half of the same rule: contact with a spotted enemy ends the
    // route, but nobody was ambushed, so no `UnitTrapped`.
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["gggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Mover"),
            unit_at([2, 0], 1, "medium_tank", "Seen"),
        ],
        3,
    );
    let mover = UnitId(0);
    assert!(state.fog.side(0).spotted.contains(&UnitId(1)));
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: mover,
                to: tactics_core::offset_to_hex(1, 0),
            },
        )
        .unwrap();
    for side in state.living_sides() {
        state.apply(&reg, &Order::Commit { side }).unwrap();
    }
    // Point the route at the enemy the way a resolved tick would find it,
    // then hand the mover the fuel to try to drive through.
    state.unit_mut(mover).unwrap().intent.path = vec![
        tactics_core::offset_to_hex(1, 0),
        tactics_core::offset_to_hex(2, 0),
    ];
    state.unit_mut(mover).unwrap().move_credit = 64 * reg.scale.ticks_per_round;

    let events = state.step_tick(&reg);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::UnitTrapped { .. })),
        "an enemy in plain sight is a roadblock, not an ambush: {events:?}"
    );
    assert_eq!(
        state.unit(mover).map(|u| u.pos),
        Some(tactics_core::offset_to_hex(1, 0)),
        "the mover stops on the tile before the enemy"
    );
}

#[test]
fn hidden_enemies_do_not_show_up_as_holes_in_the_move_range() {
    // Refusing a move because an unseen enemy stands there would announce
    // its position, so the tile stays offered and the order becomes an
    // ambush instead.
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["ggfgggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Mover"),
            unit_at([4, 0], 1, "medium_tank", "Hidden"),
        ],
        4,
    );
    let mover = UnitId(0);
    let hidden = UnitId(1);
    let hidden_pos = state.unit(hidden).unwrap().pos;
    assert!(
        !state.fog.side(0).spotted.contains(&hidden),
        "precondition: the enemy is unseen"
    );
    assert!(
        reachable(&reg, &state, mover).contains_key(&hidden_pos),
        "an unseen enemy must not punch a hole in the move overlay"
    );

    // Ordering the move onto that tile is legal; the advance simply stops
    // when it runs into whoever is standing there.
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: mover,
                to: hidden_pos,
            },
        )
        .expect("the order must be accepted, not refused with NoPath");
    let events = play_round(&reg, &mut state);
    assert!(
        state.unit(mover).is_none_or(|u| u.pos != hidden_pos),
        "nobody drives through an occupied hex: {events:?}"
    );
}

#[test]
fn spotted_enemies_still_block_a_destination() {
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["ggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Mover"),
            unit_at([1, 0], 1, "medium_tank", "Seen"),
        ],
        5,
    );
    let mover = UnitId(0);
    let seen = UnitId(1);
    let seen_pos = state.unit(seen).unwrap().pos;
    assert!(
        state.fog.side(0).spotted.contains(&seen),
        "precondition: the enemy is in plain sight"
    );
    assert_eq!(
        state.apply(
            &reg,
            &Order::SetMove {
                unit: mover,
                to: seen_pos
            }
        ),
        Err(tactics_core::battle::OrderError::NoPath),
        "you cannot drive onto an enemy you can see; that is an attack"
    );
}

#[test]
fn faster_units_arrive_earlier_in_the_same_round() {
    // Movement points buy time, not just distance: both tanks cover three
    // hexes this round, but the light one is there long before the heavy one.
    // The enemy sits behind a forest curtain, so the round runs its full
    // length instead of ending in a shootout.
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["gggggfgggggg", "gggggfgggggg", "gggggfgggggg"],
        vec![
            unit_at([0, 0], 0, "light_tank", "Quick"),
            unit_at([0, 2], 0, "heavy_tank", "Slow"),
            unit_at([11, 1], 1, "medium_tank", "Bystander"),
        ],
        8,
    );
    assert!(
        state.fog.side(0).spotted.is_empty() && state.fog.side(1).spotted.is_empty(),
        "nobody should be in contact; this test is about the clock"
    );
    let (quick, slow) = (UnitId(0), UnitId(1));
    let targets = [
        (quick, tactics_core::offset_to_hex(3, 0)),
        (slow, tactics_core::offset_to_hex(3, 2)),
    ];
    for (unit, to) in targets {
        state.apply(&reg, &Order::SetMove { unit, to }).unwrap();
    }
    for side in state.living_sides() {
        state.apply(&reg, &Order::Commit { side }).unwrap();
    }

    let mut arrived: Vec<(UnitId, u32)> = Vec::new();
    for tick in 0..reg.scale.ticks_per_round {
        state.step_tick(&reg);
        for (unit, to) in targets {
            if state.unit(unit).is_some_and(|u| u.pos == to)
                && !arrived.iter().any(|(u, _)| *u == unit)
            {
                arrived.push((unit, tick));
            }
        }
    }
    let at = |unit: UnitId| arrived.iter().find(|(u, _)| *u == unit).map(|(_, t)| *t);
    let (quick_tick, slow_tick) = (at(quick), at(slow));
    assert!(
        quick_tick.is_some() && slow_tick.is_some(),
        "both should complete a three-hex move inside one round: {arrived:?}"
    );
    assert!(
        quick_tick < slow_tick,
        "the faster tank should get there first, but arrived at {quick_tick:?} against {slow_tick:?}"
    );
}

#[test]
fn reload_time_sets_the_rate_of_fire() {
    // An MG chatters through a round; an 88 gets a couple of shots off. The
    // targets are artillery, which never answers: indirect guns do not
    // snap-fire, so the cadence is measured undisturbed.
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["gggggggg"],
        vec![
            unit_at([0, 0], 0, "recon_car", "Gunner"),
            unit_at([1, 0], 1, "artillery", "Near"),
            unit_at([4, 0], 0, "tank_destroyer", "Sniper"),
            unit_at([7, 0], 1, "artillery", "Far"),
        ],
        12,
    );
    let (mg_carrier, sniper) = (UnitId(0), UnitId(2));
    let (near, far) = (UnitId(1), UnitId(3));
    // Cadence, not lethality: an 88 kills a self-propelled gun in two hits,
    // which would end the battle mid-round and cut the count short.
    for target in [near, far] {
        state.unit_mut(target).unwrap().hp = 500;
    }
    for (unit, target) in [(mg_carrier, near), (sniper, far)] {
        state
            .apply(
                &reg,
                &Order::SetFire {
                    unit,
                    fire: FireIntent::Target { target, weapon: 0 },
                },
            )
            .expect("both targets are spotted and in range");
    }

    let events = play_round(&reg, &mut state);
    let shots = |weapon: &str| {
        events
            .iter()
            .filter(|e| matches!(e, BattleEvent::ShotFired { weapon: w, .. } if w == weapon))
            .count()
    };
    // 12 ticks a round at five seconds each: an MG reloads in 2 ticks, the
    // 88 in 4, so a minute of fighting is six bursts against three shells.
    assert_eq!(shots("mg"), 6, "an MG should fire every second tick");
    assert_eq!(shots("gun_88"), 3, "an 88 gets three shots a round");
}

#[test]
fn two_crews_can_kill_each_other_in_the_same_tick() {
    // Shots inside one tick happen together, so being processed first is not
    // an advantage. Both wrecks burn and the battle is a draw.
    let reg = registry();
    let mut drawn = false;
    for seed in 0..40 {
        let mut state = duel(&reg, seed);
        let (west, east) = (UnitId(0), UnitId(1));
        for unit in [west, east] {
            state.unit_mut(unit).unwrap().hp = 1;
        }
        for (unit, target) in [(west, east), (east, west)] {
            state
                .apply(
                    &reg,
                    &Order::SetFire {
                        unit,
                        fire: FireIntent::Target { target, weapon: 0 },
                    },
                )
                .unwrap();
        }
        for side in state.living_sides() {
            state.apply(&reg, &Order::Commit { side }).unwrap();
        }
        let events = state.step_tick(&reg);
        let dead = events
            .iter()
            .filter(|e| matches!(e, BattleEvent::UnitDestroyed { .. }))
            .count();
        if dead == 2 {
            assert_eq!(
                state.over.map(|r| (r.winner, r.reason)),
                Some((None, EndReason::Eliminated)),
                "if everyone dies at once nobody won"
            );
            drawn = true;
            break;
        }
    }
    assert!(
        drawn,
        "in forty tries, two tanks shooting each other point blank never both died"
    );
}

#[test]
fn a_unit_that_spent_the_round_driving_still_shoots_back() {
    // What used to be a hard-coded counterattack is now ordinary opportunity
    // fire, and it costs nothing to have been busy: the crew answers whoever
    // shoots at them, even mid-move.
    let reg = registry();
    let mut state = duel(&reg, 21);
    let (west, east) = (UnitId(0), UnitId(1));
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: west,
                to: tactics_core::offset_to_hex(0, 0),
            },
        )
        .expect("west has room to reposition");
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: east,
                fire: FireIntent::Target {
                    target: west,
                    weapon: 0,
                },
            },
        )
        .unwrap();

    let events = play_round(&reg, &mut state);
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::ShotFired {
                attacker,
                opportunity: true,
                ..
            } if *attacker == west
        )),
        "a unit under orders to move should still answer fire: {events:?}"
    );
}

#[test]
fn holding_fire_means_watching_not_idling() {
    // `Hold` is overwatch, not passivity: the crew shoots at whatever their
    // fog turns up without being told to.
    let reg = registry();
    let mut state = duel(&reg, 5);
    let west = UnitId(0);
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: west,
                fire: FireIntent::Hold,
            },
        )
        .unwrap();
    let events = play_round(&reg, &mut state);
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::ShotFired { attacker, opportunity: true, .. } if *attacker == west
        )),
        "a unit holding fire should engage a visible enemy: {events:?}"
    );
}

#[test]
fn orders_are_closed_once_the_round_is_resolving() {
    let reg = registry();
    let mut state = duel(&reg, 9);
    let west = UnitId(0);
    assert_eq!(
        state.apply(&reg, &Order::Commit { side: 0 }),
        Ok(Vec::new()),
        "one side committing is not enough to start the round"
    );
    assert!(state.is_planning(), "still waiting on the other side");
    assert_eq!(
        state.apply(
            &reg,
            &Order::SetFire {
                unit: west,
                fire: FireIntent::Hold
            }
        ),
        Err(tactics_core::battle::OrderError::AlreadyCommitted),
        "a side cannot rewrite orders it has already handed in"
    );
    state.apply(&reg, &Order::Commit { side: 1 }).unwrap();
    assert_eq!(state.resolving_tick(), Some(0), "now the round runs");
    assert_eq!(
        state.apply(
            &reg,
            &Order::SetMove {
                unit: west,
                to: tactics_core::offset_to_hex(1, 1)
            }
        ),
        Err(tactics_core::battle::OrderError::NotPlanningPhase),
    );
}

#[test]
fn a_round_clears_last_round_orders() {
    let reg = registry();
    // Out of contact, so the round runs to its end instead of the battle
    // being decided inside it.
    let mut state = standoff(&reg, 13);
    let west = UnitId(0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: west,
                to: tactics_core::offset_to_hex(0, 0),
            },
        )
        .unwrap();
    assert!(state.unit(west).unwrap().planned);
    let round_before = state.round;

    play_round(&reg, &mut state);
    if let Some(unit) = state.unit(west) {
        assert!(
            state.round > round_before,
            "the round should have turned over"
        );
        assert!(!unit.planned, "orders do not carry into the next round");
        assert!(unit.intent.path.is_empty());
        assert_eq!(unit.move_credit, 0, "unspent movement does not bank");
    }
}

/// How far side 0's plan leaves it from the nearest enemy, averaged over
/// seeds. Lower means it closed the distance; higher means it kept its
/// distance. Used to show that doctrine changes behaviour.
///
/// The ground is chosen to pose the question: two tanks in the open with a
/// visible enemy ahead of them and a belt of woods behind. Closing and
/// digging in are both available, and the doctrine decides which.
fn mean_approach(reg: &DataRegistry, doctrine: &str, difficulty: u8) -> f32 {
    let mut total = 0.0;
    let seeds = 0u64..8;
    let count = (seeds.end - seeds.start) as f32;
    for seed in seeds {
        let mut state = two_side_battle(
            reg,
            &["ffgggggg", "ffgggggg", "ffgggggg"],
            vec![
                unit_at([2, 0], 0, "medium_tank", "Ours"),
                unit_at([2, 2], 0, "medium_tank", "Theirs"),
                unit_at([4, 1], 1, "medium_tank", "Enemy"),
            ],
            seed,
        );
        assert_eq!(
            state.fog.side(0).spotted.len(),
            1,
            "the doctrines are being asked what to do about an enemy they can see"
        );
        let cfg = AiConfig {
            planner: "utility".into(),
            difficulty,
            doctrine: Some(doctrine.into()),
        };
        let mut planner = make_battle_planner(&cfg, seed, reg);
        for _ in 0..32 {
            if state.has_committed(0) {
                break;
            }
            let order = planner.next_order(reg, &state, 0);
            let _ = state.apply(reg, &order);
        }
        let enemies: Vec<_> = state.side_units(1).map(|u| u.pos).collect();
        for unit in state.side_units(0) {
            let dest = unit.planned_destination();
            total += enemies
                .iter()
                .map(|e| dest.distance_to(*e))
                .min()
                .unwrap_or(0) as f32;
        }
    }
    total / count
}

#[test]
fn doctrine_changes_how_a_side_fights() {
    let reg = registry();
    let massed = mean_approach(&reg, "massed_armor", 5);
    let elastic = mean_approach(&reg, "elastic_defense", 5);
    assert!(
        massed < elastic,
        "massed armour should close ({massed}) where elastic defence holds back ({elastic})"
    );
}

#[test]
fn doctrine_survives_a_bad_commander() {
    // Difficulty is competence, doctrine is character. A clumsy massed-armour
    // opponent still comes at you; it just does it badly.
    let reg = registry();
    let massed = mean_approach(&reg, "massed_armor", 1);
    let elastic = mean_approach(&reg, "elastic_defense", 1);
    assert!(
        massed < elastic,
        "dropping difficulty must not turn one doctrine into the other: {massed} against {elastic}"
    );
}

#[test]
fn the_evaluator_reads_doctrine_rather_than_hard_coded_weights() {
    let reg = registry();
    let state = duel(&reg, 4);
    let tile = state.unit(UnitId(0)).unwrap().pos;
    let score = |doctrine: &str| {
        let eval = Evaluator::new(reg.doctrine(doctrine).unwrap().clone());
        eval.score_tile(&reg, &state, UnitId(0), tile).score
    };
    assert_ne!(
        score("massed_armor"),
        score("elastic_defense"),
        "two doctrines should not value the same ground identically"
    );
}

#[test]
fn an_unknown_planner_falls_back_instead_of_crashing() {
    let reg = registry();
    let state = duel(&reg, 6);
    let cfg = AiConfig {
        planner: "does_not_exist".into(),
        difficulty: 3,
        doctrine: None,
    };
    let mut planner = make_battle_planner(&cfg, 1, &reg);
    let order = planner.next_order(&reg, &state, 0);
    assert!(
        matches!(
            order,
            Order::SetMove { .. } | Order::SetFire { .. } | Order::Commit { .. }
        ),
        "a typo in a mod should degrade to a working planner, got {order:?}"
    );
}

#[test]
fn a_searching_planner_cannot_read_the_enemys_orders() {
    // Planning is simultaneous, so nobody's orders are knowable while they
    // are being written. Search runs on a determinized copy of the battle:
    // unspotted enemies are gone, and the other side's plan is blank even
    // when it has already been written down and committed.
    let reg = registry();
    let mut state = duel(&reg, 21);
    let east = UnitId(1);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: east,
                to: tactics_core::offset_to_hex(4, 1),
            },
        )
        .expect("the east tank has open ground behind it");
    state.apply(&reg, &Order::Commit { side: 1 }).unwrap();
    assert!(
        state.unit(east).unwrap().planned,
        "precondition: side 1 has a plan"
    );

    let known = tactics_core::ai::determinize(&state, 0, 7);
    let seen = known
        .unit(east)
        .expect("a spotted enemy is still on the board");
    assert!(
        !seen.planned && seen.intent.path.is_empty(),
        "side 0 must not see what side 1 was ordered to do"
    );
    assert!(
        !known.has_committed(1),
        "and must not treat the enemy as done planning, or it would expect them to stand still"
    );
    assert!(
        known.unit(UnitId(0)).is_some_and(|u| u.side == 0),
        "its own units are untouched"
    );
}

#[test]
fn the_policy_planner_is_usable_on_its_own() {
    // MCTS leans on a plain utility planner to stand in for the enemy, so
    // that planner has to be constructible without any mod data at all.
    let reg = registry();
    let state = duel(&reg, 15);
    let mut planner = UtilityPlanner::with_difficulty(3, 99);
    let order = planner.next_order(&reg, &state, 1);
    assert!(!matches!(order, Order::ClearIntent { .. }));
}

/// Units used to spawn facing due east no matter where the enemy was, which on
/// a map where the sides deploy east and west handed the eastern side's rear
/// armour to its opponent until it happened to move or turn. A Panther is
/// armour 5 from the front and 2 from behind and the damage formula divides by
/// that number, so the bias was real and it fell on one side only.
#[test]
fn units_spawn_facing_the_enemy_rather_than_due_east() {
    let reg = registry();
    let state = duel(&reg, 7);
    let west = state.unit(UnitId(0)).expect("west alive");
    let east = state.unit(UnitId(1)).expect("east alive");

    assert_eq!(
        west.facing,
        west.pos.main_direction_to(east.pos),
        "the western unit should be looking at its enemy"
    );
    assert_eq!(
        east.facing,
        east.pos.main_direction_to(west.pos),
        "the eastern unit should be looking at its enemy, not away from it"
    );

    // The specific regression: the two must not be pointing the same way.
    assert_ne!(
        west.facing, east.facing,
        "two units facing each other cannot share a facing"
    );

    // And what actually matters: a head-on shot lands on front armour.
    assert_eq!(
        tactics_core::battle::struck_facing(east.pos, east.facing, west.pos),
        tactics_core::data::ArmorFacing::Front,
        "a head-on shot should strike the front, not the rear"
    );
}

/// A scenario may still say which way someone is looking — that is what makes
/// an ambush placeable rather than something the engine decides for you.
#[test]
fn a_map_can_place_a_unit_looking_the_wrong_way() {
    use tactics_core::map::Facing;
    let reg = registry();
    let mut placements = vec![
        unit_at([0, 1], 0, "medium_tank", "West"),
        unit_at([3, 1], 1, "medium_tank", "East"),
    ];
    placements[1].facing = Some(Facing::East);
    let state = two_side_battle(&reg, &["ggggg", "ggggg", "ggggg"], placements, 7);

    let east = state.unit(UnitId(1)).expect("east alive");
    assert_eq!(
        east.facing,
        hexx::EdgeDirection::POINTY_EAST,
        "an explicit facing must survive the point-at-the-enemy pass"
    );
    let west = state.unit(UnitId(0)).expect("west alive");
    assert_eq!(
        tactics_core::battle::struck_facing(east.pos, east.facing, west.pos),
        tactics_core::data::ArmorFacing::Rear,
        "a unit told to look away presents its rear, which is the point"
    );
}

/// The point of the roster: a girl is the same person on either side of a
/// battle. Before this she was a lookup into static mod data, so nothing that
/// happened to her could be recorded anywhere.
#[test]
fn girls_persist_across_battles_and_recover_over_days() {
    use tactics_core::roster::{CasualtyRules, CrewFate, GirlStatus};

    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 9).expect("overworld");

    // The map named crews by definition id; the world turned them into people.
    assert!(
        state.roster.len() >= 3,
        "frontier's armies should have enlisted their crews"
    );
    let army = state.side_armies(0).next().unwrap();
    let girl = army.units[0].crew[0];
    assert_eq!(
        state.roster.get(girl).unwrap().owner,
        0,
        "a girl belongs to the academy whose army she rides with"
    );
    assert_eq!(state.roster.get(girl).unwrap().battles, 0);

    // Surviving a battle is recorded on her, not on the vehicle.
    let attacker = state.side_armies(0).next().unwrap().id;
    let defender = state.side_armies(1).next().unwrap().id;
    let survivors = vec![(attacker, state.army(attacker).unwrap().units.clone())];
    state.apply_battle_result(&reg, attacker, defender, &survivors, &[]);
    assert_eq!(state.roster.get(girl).unwrap().battles, 1);

    // And so is being shot out of it. With permadeath off, the worst case is
    // a long recovery rather than a funeral.
    state.rules = CasualtyRules { permadeath: false };
    let loss = tactics_core::overworld::CrewLoss {
        girl,
        vehicle: state.army(attacker).unwrap().units[0].vehicle.clone(),
        killed_by: Some(tactics_core::data::DamageType::Kinetic),
    };
    let events = state.apply_battle_result(&reg, attacker, defender, &[], &[loss]);
    assert!(
        events.iter().any(|e| matches!(
            e,
            OverworldEvent::CrewCasualty { fate, .. } if !matches!(fate, CrewFate::Killed)
        )),
        "permadeath is off, so nobody should die: {events:?}"
    );

    // Whatever befell her, it is temporary, and the campaign clock resolves it.
    let status = state.roster.get(girl).unwrap().status;
    assert!(
        !status.is_permanent(),
        "no permanent losses with the rule off"
    );
    if let Some(days) = status.days_out().filter(|d| *d > 0) {
        for _ in 0..days {
            state.roster.advance_day();
        }
        assert_eq!(
            state.roster.get(girl).unwrap().status,
            GirlStatus::Ready,
            "she should come back after her days are served"
        );
    }
}

/// `Lost` is not a euphemism: she bailed out, could not reach her own side
/// before the shooting stopped, and is walking home.
#[test]
fn a_lost_girl_walks_back_rather_than_being_gone() {
    use tactics_core::roster::{GirlStatus, Roster};
    let mut roster = Roster::new();
    let reg = registry();
    let girl = roster
        .enlist_from_registry(&reg, 0, "anka")
        .expect("anka exists");
    roster.get_mut(girl).unwrap().status = GirlStatus::Lost { days: 2 };

    assert!(!roster.get(girl).unwrap().status.is_permanent());
    roster.advance_day();
    assert_eq!(
        roster.get(girl).unwrap().status,
        GirlStatus::Lost { days: 1 },
        "still walking"
    );
    roster.advance_day();
    assert!(
        roster.get(girl).unwrap().status.is_ready(),
        "she made it back"
    );
}

/// A trait changes *whether or when* a rule applies, which is what separates
/// it from a skill. Juno's lead foot is the clearest case: the same girl in the
/// same tank drives differently depending on what is under her tracks.
#[test]
fn a_trait_can_depend_on_where_the_check_is_happening() {
    let reg = registry();
    let mut roster = tactics_core::roster::Roster::new();
    let juno = roster
        .enlist_from_registry(&reg, 0, "juno")
        .expect("juno exists");
    assert!(
        reg.character("juno")
            .unwrap()
            .traits
            .contains(&"lead_foot".into()),
        "this test is about her lead foot"
    );

    let on_road = roster
        .skill_level(
            &reg,
            juno,
            "driving",
            &tactics_core::data::CheckContext {
                terrain: Some("road"),
                ..Default::default()
            },
        )
        .unwrap();
    let off_road = roster
        .skill_level(
            &reg,
            juno,
            "driving",
            &tactics_core::data::CheckContext {
                terrain: Some("mud"),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        on_road > off_road,
        "a lead foot should be quick on a road and worse off it: {on_road} vs {off_road}"
    );

    // And the gift and the cost are both real, measured against the girl she
    // would have been without it.
    let plain = reg.skill("driving").unwrap().level_for(
        &reg.core_index,
        &roster.get(juno).unwrap().cores,
        roster.get(juno).unwrap().skills.get("driving").copied(),
    );
    assert!(on_road > plain, "the gift");
    assert!(off_road < plain, "and the cost");
}

/// Traits that are always on still have to cut both ways, or they are just a
/// skill with a name.
#[test]
fn a_paired_trait_costs_something() {
    let reg = registry();
    let mut roster = tactics_core::roster::Roster::new();
    let nadja = roster
        .enlist_from_registry(&reg, 0, "nadja")
        .expect("nadja exists");
    let ctx = tactics_core::data::CheckContext::default();

    let cores = roster.get(nadja).unwrap().cores.clone();
    let plain = |skill: &str| {
        reg.skill(skill).unwrap().level_for(
            &reg.core_index,
            &cores,
            roster.get(nadja).unwrap().skills.get(skill).copied(),
        )
    };
    assert!(
        roster.skill_level(&reg, nadja, "gunnery", &ctx).unwrap() > plain("gunnery"),
        "deliberate makes her a better shot"
    );
    assert!(
        roster
            .skill_level(&reg, nadja, "observation", &ctx)
            .unwrap()
            < plain("observation"),
        "and she stops watching anything else while she does it"
    );
}

/// The reaction rules answer "how long before she acts", which is the number
/// slice 5 will spend when a girl has to respond to something she was not
/// told about. Nothing consumes it yet — see the note in
/// `assets/wiki/reference/girls.md` on why gating *planned* execution was the
/// wrong place for it.
#[test]
fn reaction_delay_reads_the_crew_that_is_aboard() {
    let reg = registry();
    let mut roster = tactics_core::roster::Roster::new();

    let make = |roster: &mut tactics_core::roster::Roster, id: &str, speed: i32, trained: i32| {
        let def: tactics_core::data::CharacterDef = serde_json::from_value(serde_json::json!({
            "id": id, "name": id,
            "cores": { "speed": speed, "will": 10 },
            "skills": { "reactions": trained },
        }))
        .unwrap();
        roster.enlist(0, &def, &reg)
    };
    let quick = make(&mut roster, "quick", 16, 16);
    let slow = make(&mut roster, "slow", 5, 5);

    let delay = |girl| {
        reg.reaction.delay(
            roster
                .skill_level(&reg, girl, "reactions", &Default::default())
                .unwrap(),
        )
    };
    assert!(
        delay(quick) < delay(slow),
        "a quick crew should be ready sooner: {} vs {}",
        delay(quick),
        delay(slow)
    );
    // Reactions 16 against an average of 10 shaves one tick off the base of
    // two at four points a tick; it takes a genuinely exceptional crew to act
    // the instant they are told.
    assert_eq!(delay(quick), 1);
    assert_eq!(
        reg.reaction.delay(30),
        0,
        "and there is a ceiling: nobody acts before they are told"
    );
}

/// Difficulty is a mod, so the whole system has to switch off in data. Nothing
/// in Rust may need changing to get orders that simply happen.
#[test]
fn a_gentle_mod_removes_reaction_delay_entirely() {
    let mut reg = registry();
    let ordinary = reg.reaction.delay(tactics_core::data::AVERAGE);
    assert!(ordinary > 0, "the shipped rules make crews take a moment");

    reg.reaction = tactics_core::data::ReactionRules {
        base_ticks: 0,
        max_ticks: 0,
        ..reg.reaction.clone()
    };
    for level in [0, tactics_core::data::AVERAGE, 20] {
        assert_eq!(reg.reaction.delay(level), 0, "level {level}");
    }
}

/// A crew that has taken enough will not drive into more of it. They are not
/// out of the fight — they still shoot — they simply stop advancing, which is
/// what frightened people do.
#[test]
fn a_breaking_crew_refuses_to_advance_and_says_so() {
    let reg = registry();
    let mut state = duel(&reg, 21);

    // Put one crew past the last rung of the ladder.
    let breaking = reg
        .morale
        .rungs
        .last()
        .expect("the shipped ladder has rungs")
        .at_pressure;
    state.units[0].pressure = breaking;
    assert!(
        !state.obeys(&reg, state.unit(UnitId(0)).unwrap()),
        "this test needs a crew that has stopped obeying"
    );

    // Open ground toward the enemy, not the enemy's own tile, which is
    // occupied and therefore unpathable.
    let forward = tactics_core::offset_to_hex(1, 1);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: forward,
            },
        )
        .expect("the order is accepted; it is the crew who decline");
    let events = play_round(&reg, &mut state);

    // Asserted as behaviour rather than as a position: a duel can kill her
    // during the round, and a dead unit has no position to compare.
    assert!(
        !events.iter().any(|e| matches!(
            e,
            BattleEvent::UnitMoved { unit, .. } if *unit == UnitId(0)
        )),
        "a breaking crew should not have advanced a hex: {events:?}"
    );
    // And it must be attributable. An order that quietly fails is
    // indistinguishable from a bug.
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::OrderRefused { unit, .. } if *unit == UnitId(0)
        )),
        "the refusal has to be said out loud: {events:?}"
    );
}

/// The player has to be able to see a crew wavering *before* it costs them
/// something, or licence to disobey reads as the game cheating.
#[test]
fn crews_report_moving_up_the_ladder() {
    let reg = registry();
    let mut state = duel(&reg, 22);
    for side in state.living_sides() {
        state.apply(&reg, &Order::Commit { side }).unwrap();
    }

    // Fight until somebody has been hurt enough to move a rung.
    let mut said = false;
    for _ in 0..6 {
        if state.is_over() {
            break;
        }
        let events = play_round(&reg, &mut state);
        if events
            .iter()
            .any(|e| matches!(e, BattleEvent::MoraleChanged { .. }))
        {
            said = true;
            break;
        }
        for side in state.living_sides() {
            let _ = state.apply(&reg, &Order::Commit { side });
        }
    }
    assert!(said, "taking fire should eventually be reported as morale");
}

/// Difficulty is a mod. A one-rung ladder has to produce girls who always do
/// as they are told, with nothing in Rust switched off to achieve it.
#[test]
fn a_gentle_mod_has_girls_who_never_refuse() {
    let mut reg = registry();
    reg.morale = tactics_core::data::MoraleRules {
        rungs: vec![tactics_core::data::MoraleRung {
            id: "steady".into(),
            name: "Steady".into(),
            at_pressure: 0,
            obeys: true,
        }],
        ..reg.morale.clone()
    };

    let mut state = duel(&reg, 23);
    state.units[0].pressure = 10_000;
    assert!(
        state.obeys(&reg, state.unit(UnitId(0)).unwrap()),
        "under a one-rung ladder no amount of pressure stops her"
    );

    // Open ground toward the enemy, not the enemy's own tile, which is
    // occupied and therefore unpathable.
    let forward = tactics_core::offset_to_hex(1, 1);
    let start = state.unit(UnitId(0)).unwrap().pos;
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: forward,
            },
        )
        .expect("ordered forward");
    let events = play_round(&reg, &mut state);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::OrderRefused { .. })),
        "nobody refuses in the gentle game: {events:?}"
    );
    assert_ne!(
        state.unit(UnitId(0)).map(|u| u.pos),
        Some(start),
        "and she actually advances"
    );
}

// --- missions steering units (chunk 2 of chain of command) -----------------

/// A sharp-eyed utility planner for one side, for tests that assert where
/// units choose to go: difficulty 5 is zero scoring noise, so the assertion
/// is about the evaluator rather than the dice.
fn sharp_planner(
    reg: &DataRegistry,
    seed: u64,
    doctrine: &str,
) -> Box<dyn AiPlanner<BattleState, Order>> {
    make_battle_planner(
        &AiConfig {
            planner: "utility".into(),
            difficulty: 5,
            doctrine: Some(doctrine.into()),
        },
        seed,
        reg,
    )
}

#[test]
fn a_map_with_formations_but_no_missions_fights_exactly_as_the_flat_pool_did() {
    // The additivity hinge for the whole command system: formations that
    // have been given nothing to do must change nothing. Same seed, same
    // planners, one battle with its command state stripped — the event
    // streams must match to the byte, or the mission machinery is leaking
    // into battles that never asked for it.
    //
    // Succession narrowed this from "identical" to "identical in deeds", the
    // same way the command block did in chunk 5. A formation whose commander
    // burns hands over to the next girl whether or not anybody priced a
    // radio, and says so — so `CommandPassed` is set aside here as words.
    // What it *costs* is `morale.leader_lost`, and at zero, which is what a
    // mod that never mentions the field gets, it costs nothing: the rest of
    // the stream has to match byte for byte.
    let reg = {
        let mut reg = registry_wireless();
        reg.morale.leader_lost = 0;
        reg
    };
    let run = |strip: bool| -> Vec<String> {
        let mut state = BattleState::from_map(&reg, "river_crossing", 21).unwrap();
        if strip {
            state.command = Default::default();
        }
        let mut ai = AiDriver::new();
        ai.insert(0, sharp_planner(&reg, 21, "massed_armor"));
        ai.insert(1, sharp_planner(&reg, 22, "elastic_defense"));
        let mut log = Vec::new();
        for _ in 0..8 {
            if state.is_over() {
                break;
            }
            ai.plan_round(&reg, &mut state);
            log.extend(state.resolve_round(&reg).iter().map(|e| format!("{e:?}")));
        }
        log
    };
    let with_formations = run(false);
    assert!(
        !with_formations.is_empty(),
        "the battle should do something"
    );
    let (spoken, deeds): (Vec<String>, Vec<String>) = with_formations
        .into_iter()
        .partition(|e| e.starts_with("CommandPassed"));
    assert!(
        !spoken.is_empty(),
        "and somebody's commander should be lost in it, or this proves nothing"
    );
    assert_eq!(
        deeds,
        run(true),
        "unmissioned formations must fight exactly as the flat pool did"
    );
}

#[test]
fn a_formation_advances_on_the_ground_its_mission_names() {
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 5).unwrap();
    let bridge = state
        .map
        .objectives()
        .iter()
        .find(|o| o.id == "bridge")
        .expect("river_crossing has a bridge")
        .anchor();
    let formation = formation_named(&state, "kuhlmann_armor");
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation,
                mission: Mission::Advance { to: bridge },
            },
        )
        .unwrap();

    let mut ai = AiDriver::new();
    ai.insert(0, sharp_planner(&reg, 5, "massed_armor"));
    ai.plan_round(&reg, &mut state);

    let members = state.formations()[formation.index()].members.clone();
    for id in members {
        let unit = state.unit(id).expect("nobody has died in planning");
        let before = unit.pos.distance_to(bridge);
        let after = unit.planned_destination().distance_to(bridge);
        assert!(
            after < before,
            "{} was ordered to the bridge and planned from {} to {} hexes away",
            unit.name,
            before,
            after
        );
    }
}

#[test]
fn an_ordered_withdrawal_needs_no_wounds() {
    // The evaluator's own exit pull is gated on damage, because an exit
    // nobody was ordered to take must not tempt an intact crew. A withdraw
    // *mission* is that order, so it pulls at full strength on full health —
    // which is what makes withdrawal a command decision rather than a
    // symptom.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 13).unwrap();
    let lane = state
        .map
        .objectives()
        .iter()
        .find(|o| o.id == "west_road")
        .expect("river_crossing has a western retreat lane")
        .hexes
        .clone();
    let formation = formation_named(&state, "kuhlmann_armor");
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation,
                mission: Mission::Withdraw {
                    via: "west_road".into(),
                },
            },
        )
        .unwrap();

    let mut ai = AiDriver::new();
    ai.insert(0, sharp_planner(&reg, 13, "massed_armor"));
    ai.plan_round(&reg, &mut state);

    let toward = |hex: tactics_core::Hex| lane.iter().map(|h| h.distance_to(hex)).min().unwrap();
    let members = state.formations()[formation.index()].members.clone();
    for id in members {
        let unit = state.unit(id).expect("planning harms nobody");
        assert_eq!(
            unit.hp,
            reg.vehicle(&unit.vehicle).unwrap().max_hp,
            "intact"
        );
        assert!(
            toward(unit.planned_destination()) < toward(unit.pos),
            "{} is unhurt and was still ordered out, so she heads for the lane",
            unit.name
        );
    }
}

#[test]
fn the_command_planner_assigns_missions_once_and_units_follow_them() {
    // A centralized doctrine (low delegation), because a devolved one
    // deliberately assigns no ground at all — see
    // a_devolved_commander_issues_no_ground_missions.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 9).unwrap();
    let mut ai = AiDriver::new();
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: Some("massed_armor".into()),
            },
            9,
            &reg,
        ),
    );

    // Round one: the commander divides the ground among her formations and
    // the driver carries the announcements out of the planning phase.
    let mut assigned = 0;
    ai.plan_round_with(&reg, &mut state, |d| {
        assigned += d
            .events
            .iter()
            .filter(|e| matches!(e, BattleEvent::MissionAssigned { .. }))
            .count();
    });
    let valkyrie_formations: Vec<_> = state.formations().iter().filter(|f| f.side == 1).collect();
    assert_eq!(
        assigned,
        valkyrie_formations.len(),
        "every formation gets a mission and each is said once"
    );
    assert!(
        valkyrie_formations.iter().all(|f| f.mission.is_some()),
        "the missions are standing on the formations"
    );
    assert!(state.has_committed(1), "and the side finishes its planning");

    // Round two: standing orders stand. The brain reviews and finds nothing
    // to change, so the log hears nothing.
    let _ = state.apply(&reg, &Order::Commit { side: 0 });
    state.resolve_round(&reg);
    assert!(state.is_planning(), "a new round has opened");
    let mut reassigned = 0;
    ai.plan_round_with(&reg, &mut state, |d| {
        reassigned += d
            .events
            .iter()
            .filter(|e| matches!(e, BattleEvent::MissionAssigned { .. }))
            .count();
    });
    assert_eq!(
        reassigned, 0,
        "an unchanged mission is not news, and re-announcing it every round would be"
    );
}

#[test]
fn a_beaten_formation_is_ordered_out_by_its_commander() {
    // Withdrawal as a command decision: nobody in this formation consults
    // her own damage — the commander weighs the formation against her
    // doctrine's threshold and orders it out by the nearest lane.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 31).unwrap();
    let formation = formation_named(&state, "valkyrie_line");
    for id in state.formations()[formation.index()].members.clone() {
        state.units[id.index()].hp = 1;
    }

    let mut ai = AiDriver::new();
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: Some("elastic_defense".into()),
            },
            31,
            &reg,
        ),
    );
    ai.plan_round(&reg, &mut state);

    match &state.formations()[formation.index()].mission {
        Some(Mission::Withdraw { via }) => assert_eq!(
            via, "east_road",
            "the lane is the nearest exit her side may use"
        ),
        other => panic!("a formation at a tenth strength should be ordered out, got {other:?}"),
    }
    // The intact formation hears nothing: elastic defence devolves command,
    // so its formations fight their own ground and are only ever *ordered*
    // to leave it.
    let screen = formation_named(&state, "valkyrie_screen");
    assert!(
        state.formations()[screen.index()].mission.is_none(),
        "a devolved commander does not micro-assign ground to a formation that is fighting well"
    );
}

#[test]
fn an_executor_only_command_fills_gaps_without_issuing_missions() {
    // The human hybrid, driven headlessly. The player's side has no brain —
    // she is the brain — so the object standing behind it must issue no
    // missions at all, plan the units of a formation she has given orders to,
    // and leave everyone else alone. That last part is the one worth pinning:
    // an AI side hands an unformationed unit to its fallback planner, which
    // sends her off to fight on her own judgment. Doing that on the player's
    // behalf would be inventing an order she never gave, so a unit outside
    // any mission gets today's "planned, watching" default instead.
    let reg = registry_wireless();
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "delegation_field",
        "palette": { "g": "grass" },
        "rows": ["gggggggggggggg", "gggggggggggggg", "gggggggggggggg"],
        "shape": "free",
        "sides": [{ "name": "Kuhlmann" }],
        "formations": [{ "id": "first", "name": "1st Platoon", "side": 0 }],
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let placement =
        |col: i32, row: i32, name: &str, formation: Option<&str>, leads: bool| UnitPlacement {
            at: [col, row],
            side: 0,
            vehicle: "medium_tank".into(),
            crew: Vec::new(),
            name: Some(name.into()),
            facing: None,
            formation: formation.map(str::to_string),
            leads,
        };
    let placements = vec![
        placement(0, 0, "Leader", Some("first"), true),
        placement(0, 1, "Follower", Some("first"), false),
        placement(0, 2, "Nobody's", None, false),
    ];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(&reg, &placements);
    let mut state = BattleState::from_placements(
        &reg,
        map,
        vec![SideState {
            name: "Kuhlmann".into(),
            ai: None,
        }],
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        17,
    );

    // The human's order, issued exactly as the UI issues it.
    let target = tactics_core::offset_to_hex(12, 1);
    let formation = FormationId(0);
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation,
                mission: Mission::Advance { to: target },
            },
        )
        .expect("the player may order her own formation");

    let mut ai = AiDriver::new();
    ai.insert(
        0,
        Box::new(tactics_core::ai::SideCommand::executor_only(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            17,
            &reg,
        )),
    );
    let mut spoken = 0;
    ai.plan_round_with(&reg, &mut state, |d| {
        assert!(d.rejected.is_none(), "the executors issued {:?}", d.order);
        spoken += d
            .events
            .iter()
            .filter(|e| matches!(e, BattleEvent::MissionAssigned { .. }))
            .count();
    });

    assert_eq!(
        spoken, 0,
        "an executor-only side has no commander and must never issue a mission"
    );
    assert!(state.has_committed(0), "and it closes the side's planning");

    for id in state.formations()[formation.index()].members.clone() {
        let unit = state.unit(id).expect("planning harms nobody");
        assert!(
            unit.planned_destination().distance_to(target) < unit.pos.distance_to(target),
            "{} is under a mission she can hear, so her executor drives her at it",
            unit.name
        );
    }

    let loose = state
        .side_units(0)
        .find(|u| state.formation_of(u.id).is_none())
        .expect("one unit answers to nobody");
    assert!(
        loose.planned,
        "she is accounted for, so the round can start"
    );
    assert!(
        loose.intent.path.is_empty() && loose.intent.fire == FireIntent::Hold,
        "but nobody ordered her anywhere, so she holds and watches: {:?}",
        loose.intent
    );
}

#[test]
fn initiative_moves_a_commander_on_and_obedience_does_not() {
    let reg = registry_wireless();

    // The balanced doctrine carries initiative 0.5: with the bridge already
    // hers, her formations are re-aimed at ground she does not hold.
    let mut state = BattleState::from_map(&reg, "river_crossing", 33).unwrap();
    state.objective_held[0] = Some(1);
    let mut ai = AiDriver::new();
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            33,
            &reg,
        ),
    );
    ai.plan_round(&reg, &mut state);
    let line = formation_named(&state, "valkyrie_line");
    let ford = state
        .map
        .objectives()
        .iter()
        .find(|o| o.id == "north_ford")
        .unwrap()
        .anchor();
    assert_eq!(
        state.formations()[line.index()].mission,
        Some(Mission::Advance { to: ford }),
        "initiative moves her off ground already taken"
    );

    // Massed armour carries initiative 0.3: the plan said the bridge, so
    // the bridge it is, held or not.
    let mut state = BattleState::from_map(&reg, "river_crossing", 33).unwrap();
    state.objective_held[0] = Some(0);
    let mut ai = AiDriver::new();
    ai.insert(
        0,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: Some("massed_armor".into()),
            },
            33,
            &reg,
        ),
    );
    ai.plan_round(&reg, &mut state);
    let armor = formation_named(&state, "kuhlmann_armor");
    let bridge = state.map.objectives()[0].anchor();
    assert_eq!(
        state.formations()[armor.index()].mission,
        Some(Mission::Advance { to: bridge }),
        "an obedient doctrine follows the letter of the plan"
    );
}

#[test]
fn a_devolved_commander_issues_no_ground_missions() {
    // Elastic defence devolves command (delegation 0.7): its formations
    // keep the whole-map judgment that is the doctrine's strength, and the
    // commander's only order is the one that is never devolved — leaving.
    // Measured before believed: pinning this doctrine to anchor hexes cost
    // it 16 of 24 wins against an unchanged opponent.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 17).unwrap();
    let mut ai = AiDriver::new();
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: Some("elastic_defense".into()),
            },
            17,
            &reg,
        ),
    );
    ai.plan_round(&reg, &mut state);
    assert!(
        state
            .formations()
            .iter()
            .filter(|f| f.side == 1)
            .all(|f| f.mission.is_none()),
        "her formations fight their own ground"
    );
    assert!(
        state.has_committed(1),
        "and the side still finishes planning"
    );
}

// --- contact and order latency (chunk 4 of chain of command) ----------------

/// Take the radio sets out of every vehicle, so a test that engineers a net
/// with `command_rules(radius, ..)` is testing the radius it wrote rather
/// than the 8-hex hardware the base vehicles carry.
fn strip_radios(reg: &mut DataRegistry) {
    for vehicle in reg.vehicles.values_mut() {
        vehicle.radio = None;
    }
}

/// Command rules built white-box, so these tests can state the rules at any
/// coefficient — including the zero coefficients that must give back today's
/// game exactly — without moving the determinism baseline an inch.
fn command_rules(radius: u32, relay: bool, base_ticks: u32) -> tactics_core::data::CommandRules {
    tactics_core::data::CommandRules {
        radius,
        // No flags in these tests unless a test says otherwise: they are
        // about the radio, and the visual medium has its own.
        visual_range: 0,
        // Zero per point, so these tests are about the rules rather than about
        // which girl happens to be sitting in the radio seat.
        radius_per_signals: 0,
        relay,
        // These are battle tests; the campaign's own radius has its own.
        overworld_radius: 999,
        review: tactics_core::data::ReactionRules {
            skill: "command".into(),
            base_ticks: 0,
            levels_per_tick: 0,
            max_ticks: 0,
        },
        latency: tactics_core::data::ReactionRules {
            skill: "command".into(),
            base_ticks,
            levels_per_tick: 0,
            max_ticks: 5,
        },
    }
}

/// Close every side's planning without giving anybody anything to do.
fn commit_all(reg: &DataRegistry, state: &mut BattleState) {
    for side in state.living_sides() {
        if !state.has_committed(side) {
            state.apply(reg, &Order::Commit { side }).expect("commit");
        }
    }
}

#[test]
fn orders_take_time_to_arrive_when_the_radio_says_so() {
    // The heart of the chunk: an order is sent when it is issued and arrives
    // later. Two ticks of latency means the formation spends the opening of
    // the round doing the last thing it heard, which is the whole point —
    // and, per the reaction-latency post-mortem, the delay is on the *new
    // information* reaching them, never on executing a plan they already had.
    let mut reg = registry();
    reg.command = Some(command_rules(999, true, 2));
    let mut state = BattleState::from_map(&reg, "river_crossing", 5).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let bridge = state.map.objectives()[0].anchor();

    let events = state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: bridge },
            },
        )
        .expect("the bridge is on the map");
    assert_eq!(
        events,
        vec![BattleEvent::MissionAssigned {
            formation: "kuhlmann_armor".into(),
            mission: Mission::Advance { to: bridge },
        }],
        "the order is sent the moment it is given, and said out loud"
    );
    assert!(
        state.formations()[armor.index()].mission.is_none(),
        "but the platoon plans this round without having heard it"
    );

    commit_all(&reg, &mut state);
    let first = state.step_tick(&reg);
    assert!(
        !first
            .iter()
            .any(|e| matches!(e, BattleEvent::MissionReceived { .. })),
        "still in the air after one tick"
    );
    assert!(state.formations()[armor.index()].mission.is_none());

    let second = state.step_tick(&reg);
    assert!(
        second.iter().any(|e| matches!(
            e,
            BattleEvent::MissionReceived { formation, mission }
                if formation == "kuhlmann_armor" && *mission == Mission::Advance { to: bridge }
        )),
        "it lands on the second tick, and the log says so: {second:?}"
    );
    assert_eq!(
        state.formations()[armor.index()].mission,
        Some(Mission::Advance { to: bridge }),
        "and only then is it what the platoon is doing"
    );
    assert!(
        state.formations()[armor.index()].incoming.is_none(),
        "nothing is left travelling"
    );
}

#[test]
fn a_command_block_with_zero_coefficients_is_the_game_without_one() {
    // The additivity pin, and the one this chunk most needs: the first
    // reaction-latency attempt died because it broke tests at *any* setting of
    // its knob, which is the tell that a model is wrong rather than mistuned.
    // So the rules are declared at their most generous — everyone in radio
    // contact of everyone, orders that arrive instantly — and the battle they
    // produce must be the same battle, decision for decision, as one whose
    // registry has no `command` block at all. A command-planner side, so
    // missions are actually being issued and could actually go astray.
    //
    // Leaders die in this window — on `river_crossing` a platoon leader dies
    // in the first round of nearly every seed — and the pin holds anyway,
    // because a cut-off crew soldiers on the orders she was carrying rather
    // than dropping them. What a block is *allowed* to add at zero
    // coefficients is words, not deeds: the wire events (out of contact,
    // restored, reports reaching the commander) are the system's information
    // surface and exist whenever it does. So the comparison is exact after
    // setting those three aside, and then requires that nothing else was set
    // aside — a behaviour drift hiding among the wire events fails the
    // second assertion instead of slipping through the first.
    let wire = |line: &String| {
        line.starts_with("OutOfContact")
            || line.starts_with("ContactRestored")
            || line.starts_with("ContactReported")
    };
    let run = |rules: Option<tactics_core::data::CommandRules>| -> Vec<String> {
        let mut reg = registry();
        reg.command = rules;
        // The pin pins the RULES coefficients, so the vehicles' own radio
        // sets are stripped: hardware at 8 hexes would cap the "everyone in
        // contact" net the zeroed block declares, and hardware is content,
        // not a coefficient. Stripped identically in both runs.
        for vehicle in reg.vehicles.values_mut() {
            vehicle.radio = None;
        }
        let mut state = BattleState::from_map(&reg, "river_crossing", 21).unwrap();
        let mut ai = AiDriver::new();
        ai.insert(
            0,
            make_battle_planner(
                &AiConfig {
                    planner: "command".into(),
                    difficulty: 5,
                    doctrine: Some("massed_armor".into()),
                },
                21,
                &reg,
            ),
        );
        ai.insert(1, sharp_planner(&reg, 22, "elastic_defense"));
        let mut log = Vec::new();
        for _ in 0..6 {
            if state.is_over() {
                break;
            }
            // Planning-phase events too: a mission going astray in transit
            // would show up here first.
            ai.plan_round_with(&reg, &mut state, |d| {
                log.extend(d.events.iter().map(|e| format!("{e:?}")));
            });
            log.extend(state.resolve_round(&reg).iter().map(|e| format!("{e:?}")));
        }
        log
    };
    let without = run(None);
    assert!(!without.is_empty(), "the battle should do something");
    assert!(
        without.iter().any(|line| line.contains("MissionAssigned")),
        "and it should be issuing missions, or this proves nothing"
    );
    assert!(
        without.iter().any(|line| line.contains("ShotHit")),
        "and fighting, rather than driving about out of contact"
    );
    assert!(
        !without.iter().any(wire),
        "no block, no wires: the None run must contain no wire events at all"
    );
    let zeroed = run(Some(tactics_core::data::CommandRules {
        radius: 999,
        radius_per_signals: 0,
        relay: true,
        visual_range: 0,
        overworld_radius: 999,
        review: tactics_core::data::ReactionRules {
            skill: "command".into(),
            base_ticks: 0,
            levels_per_tick: 0,
            max_ticks: 0,
        },
        latency: tactics_core::data::ReactionRules {
            skill: "command".into(),
            base_ticks: 0,
            levels_per_tick: 0,
            max_ticks: 0,
        },
    }));
    let (spoken, deeds): (Vec<String>, Vec<String>) = zeroed.into_iter().partition(wire);
    assert_eq!(
        deeds, without,
        "a command block at zero coefficients must change words, never deeds"
    );
    assert!(
        spoken.iter().all(wire),
        "and everything set aside really was wire traffic"
    );
}

/// Play the opening round of `river_crossing` under these command rules with
/// nobody ordered to do anything, so that contact is computed against the
/// deployment as declared. Returns the battle in its second planning phase and
/// everything the round said.
fn quiet_round(
    reg: &DataRegistry,
    mission: Option<(FormationId, Mission)>,
) -> (BattleState, Vec<BattleEvent>) {
    let mut state = BattleState::from_map(reg, "river_crossing", 5).expect("battle");
    if let Some((formation, mission)) = mission {
        state
            .apply(reg, &Order::SetMission { formation, mission })
            .expect("a legal mission");
    }
    commit_all(reg, &mut state);
    let events = state.resolve_round(reg);
    (state, events)
}

/// Plan exactly one unit, by handing every other unit on her side a
/// hold-fire order first so the planner has a single decision left to make.
///
/// That isolation is the point: the evaluator's mass term reads what her
/// neighbours are *planning*, so planning a whole side would let a difference
/// in somebody else's orders leak into hers and make a comparison meaningless.
fn plan_one(
    reg: &DataRegistry,
    state: &mut BattleState,
    unit: UnitId,
    seed: u64,
) -> tactics_core::Hex {
    let side = state.unit(unit).expect("she is alive").side;
    let others: Vec<UnitId> = state
        .side_units(side)
        .map(|u| u.id)
        .filter(|id| *id != unit)
        .collect();
    for id in others {
        state
            .apply(
                reg,
                &Order::SetFire {
                    unit: id,
                    fire: FireIntent::Hold,
                },
            )
            .expect("holding fire is always legal");
    }
    let mut planner = UtilityPlanner::new(
        Evaluator::new(reg.doctrine("massed_armor").expect("base doctrine").clone()),
        0.0,
        seed,
    );
    loop {
        let order = planner.next_order(reg, state, side);
        if matches!(order, Order::Commit { .. }) {
            break;
        }
        let _ = state.apply(reg, &order);
    }
    state
        .unit(unit)
        .expect("planning harms nobody")
        .planned_destination()
}

#[test]
fn a_cut_off_unit_keeps_the_orders_she_had() {
    // Out of contact is not amnesia and it is not license: a girl who loses
    // the wire soldiers on the standing orders she was carrying when it went
    // dead. What she cannot do is hear anything new. This inverts the first
    // model this test pinned — "cut off means unmissioned" — which measured
    // badly the moment leaders started dying on first contact: a command
    // block silently deleted half the map's missions by round two, and a
    // system whose presence deletes orders is a tax, not a texture.
    let reg = registry();
    let (armor, follower, start) = {
        let probe = BattleState::from_map(&reg, "river_crossing", 5).unwrap();
        let armor = formation_named(&probe, "kuhlmann_armor");
        let formation = &probe.formations()[armor.index()];
        let leader = formation.leader.expect("the platoon has a commander");
        let follower = *formation
            .members
            .iter()
            .find(|id| **id != leader)
            .expect("and somebody to command");
        (armor, follower, probe.unit(follower).unwrap().pos)
    };
    // Ground to hold, rather than ground to take: the map's own objectives
    // already pull everyone toward the bridge, so a mission to advance on it
    // would be indistinguishable from her own judgment and this test would
    // pass while proving nothing.
    let mission = Mission::Hold { at: Some(start) };

    let cut_off_reg = {
        let mut r = registry();
        // Two hexes and no relay: the platoon deploys strung out, so the
        // second tank cannot hear her commander.
        r.command = Some(command_rules(2, false, 0));
        strip_radios(&mut r);
        r
    };
    let (cut_off, events) = quiet_round(&cut_off_reg, Some((armor, mission.clone())));
    let formation = &cut_off.formations()[armor.index()];
    assert_eq!(
        formation.mission,
        Some(mission.clone()),
        "the platoon is under orders"
    );
    let carried = formation
        .out_of_contact
        .iter()
        .map(|c| (c.unit, c.orders.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        carried,
        vec![(follower, Some(mission.clone()))],
        "she is the one out of contact, and she took the order with her"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, BattleEvent::OutOfContact { unit } if *unit == follower))
            .count(),
        1,
        "said once, not once a tick"
    );

    // The same battle under rules that reach the whole map, so the two states
    // differ in nothing but who can hear the wire.
    let heard_reg = {
        let mut r = registry();
        r.command = Some(command_rules(999, false, 0));
        strip_radios(&mut r);
        r
    };
    let (in_contact, _) = quiet_round(&heard_reg, Some((armor, mission)));
    assert!(
        in_contact.formations()[armor.index()]
            .out_of_contact
            .is_empty(),
        "nobody is cut off when the radius covers the map"
    );

    // And the twin with no chain of command at all: what an unmissioned girl
    // would do, which the deaf one must NOT match — she has orders.
    let mut twin = cut_off.clone();
    twin.command = Default::default();

    let mut a = cut_off.clone();
    let mut b = twin;
    let mut c = in_contact;
    let deaf = plan_one(&cut_off_reg, &mut a, follower, 77);
    let unmissioned = plan_one(&cut_off_reg, &mut b, follower, 77);
    let obedient = plan_one(&heard_reg, &mut c, follower, 77);
    assert_eq!(
        deaf, obedient,
        "cut off or not, she is executing the same standing order"
    );
    assert_ne!(
        deaf, unmissioned,
        "and it is the order steering her, not her own judgment"
    );
    assert!(
        start.distance_to(deaf) <= start.distance_to(unmissioned),
        "holding means staying: {deaf:?} against {unmissioned:?} from {start:?}"
    );
}

#[test]
fn an_order_never_heard_does_not_steer_her() {
    // The counterpart: a girl already out of contact when the order is given
    // never receives it. The formation's standing mission changes behind her
    // back; she fights on what she knew — which was nothing.
    let reg = registry();
    let (armor, follower) = {
        let probe = BattleState::from_map(&reg, "river_crossing", 5).unwrap();
        let armor = formation_named(&probe, "kuhlmann_armor");
        let formation = &probe.formations()[armor.index()];
        let leader = formation.leader.expect("a commander");
        let follower = *formation
            .members
            .iter()
            .find(|id| **id != leader)
            .expect("a subordinate");
        (armor, follower)
    };
    let cut_off_reg = {
        let mut r = registry();
        r.command = Some(command_rules(2, false, 0));
        strip_radios(&mut r);
        r
    };
    // Round one, no mission: she goes out of contact carrying nothing.
    let (mut state, _) = quiet_round(&cut_off_reg, None);
    let start = state.unit(follower).unwrap().pos;
    // Now the order goes out. It reaches the formation — the leader can hear
    // herself — but not her.
    state
        .apply(
            &cut_off_reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Hold { at: Some(start) },
            },
        )
        .expect("a legal mission");
    let formation = &state.formations()[armor.index()];
    assert!(
        formation.latest_mission().is_some(),
        "the platoon has its orders"
    );
    assert_eq!(
        formation
            .out_of_contact
            .iter()
            .find(|c| c.unit == follower)
            .map(|c| c.orders.clone()),
        Some(None),
        "but she is carrying the nothing she was cut off with"
    );

    let mut twin = state.clone();
    twin.command = Default::default();
    let deaf = plan_one(&cut_off_reg, &mut state, follower, 91);
    let unmissioned = plan_one(&cut_off_reg, &mut twin, follower, 91);
    assert_eq!(
        deaf, unmissioned,
        "an order she never heard cannot steer her"
    );
}

#[test]
fn contact_lost_is_said_once_and_restored_out_loud() {
    let mut reg = registry();
    reg.command = Some(command_rules(3, false, 0));
    strip_radios(&mut reg);
    let mut state = BattleState::from_map(&reg, "river_crossing", 8).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let formation = &state.formations()[armor.index()];
    let leader = formation.leader.expect("a commander");
    let follower = *formation
        .members
        .iter()
        .find(|id| **id != leader)
        .expect("somebody to command");
    let beside = state.unit(leader).unwrap().pos + tactics_core::Hex::new(1, 0);
    let away = state.unit(leader).unwrap().pos + tactics_core::Hex::new(0, 8);
    assert!(state.map.contains(beside) && state.map.contains(away));
    assert!(state.unit_at(beside).is_none() && state.unit_at(away).is_none());

    // She starts alongside her commander, so the opening tick has nothing to
    // report about her.
    state.unit_mut(follower).unwrap().pos = beside;
    commit_all(&reg, &mut state);
    let quiet = state.step_tick(&reg);
    assert!(
        !quiet.iter().any(|e| matches!(
            e,
            BattleEvent::OutOfContact { unit } | BattleEvent::ContactRestored { unit } if *unit == follower
        )),
        "a platoon driving together says nothing: {quiet:?}"
    );

    // Then she drives out of earshot. Once.
    state.unit_mut(follower).unwrap().pos = away;
    let lost = state.step_tick(&reg);
    assert_eq!(
        lost.iter()
            .filter(|e| matches!(e, BattleEvent::OutOfContact { unit } if *unit == follower))
            .count(),
        1,
        "losing contact is news exactly once: {lost:?}"
    );
    let still = state.step_tick(&reg);
    assert!(
        !still
            .iter()
            .any(|e| matches!(e, BattleEvent::OutOfContact { .. })),
        "staying out of contact is not news again every tick: {still:?}"
    );

    // And back, which the player must also hear: a unit silently starting to
    // obey again is as confusing as one silently ignoring orders.
    state.unit_mut(follower).unwrap().pos = beside;
    let back = state.step_tick(&reg);
    assert_eq!(
        back.iter()
            .filter(|e| matches!(e, BattleEvent::ContactRestored { unit } if *unit == follower))
            .count(),
        1,
        "restored contact is said out loud: {back:?}"
    );
    assert!(
        state.formations()[armor.index()].out_of_contact.is_empty(),
        "and the state agrees with the log"
    );
}

// --- the command picture (chunk 5) -----------------------------------------

/// A long open road: a leader in the west, her scout far to the east with an
/// enemy recon car beyond — inside the scout's eyes, outside everybody's
/// guns, and far outside the leader's radio unless a test says otherwise.
fn picture_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    let row = "g".repeat(60);
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "picture_stage",
        "palette": { "g": "grass" },
        "rows": [row],
        "formations": [ { "id": "net", "name": "The Net", "side": 0 } ],
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let sides = vec![
        SideState {
            name: "West".into(),
            ai: None,
        },
        SideState {
            name: "East".into(),
            ai: None,
        },
    ];
    let mut leader = unit_at([0, 0], 0, "recon_car", "Leader");
    leader.formation = Some("net".into());
    leader.leads = true;
    let mut scout = unit_at([25, 0], 0, "recon_car", "Scout");
    scout.formation = Some("net".into());
    let enemy = unit_at([32, 0], 1, "recon_car", "Prowler");
    let placements = vec![leader, scout, enemy];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
}

#[test]
fn a_scout_out_of_contact_reports_nothing() {
    // Seeing is not reporting. The side's fog spots through any unit's eyes;
    // the commander's picture learns only what somebody on the net can tell
    // her. A scout beyond the radio finds the enemy and nobody knows —
    // which is recon wasted, and the whole reason the wires matter.
    let mut reg = registry();
    reg.command = Some(command_rules(8, false, 0));
    strip_radios(&mut reg);
    let mut state = picture_stage(&reg, 3);
    let (scout, enemy) = (UnitId(1), UnitId(2));

    commit_all(&reg, &mut state);
    let events = state.step_tick(&reg);
    assert!(
        state.fog.side(0).spotted.contains(&enemy),
        "the side's fog does see him, through her"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, BattleEvent::OutOfContact { unit } if *unit == scout)),
        "and she is announced as off the net"
    );
    assert!(
        !state.picture(0).iter().any(|c| c.unit == enemy),
        "but the commander has heard nothing: a cut-off scout files no report"
    );

    // March the leader east until the scout is back on the net; the report
    // goes through the moment somebody in contact can vouch for the sighting.
    if let Some(unit) = state.unit_mut(UnitId(0)) {
        unit.pos = tactics_core::offset_to_hex(20, 0);
    }
    let events = state.step_tick(&reg);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, BattleEvent::ContactRestored { unit } if *unit == scout)),
        "she is back on the net"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, BattleEvent::ContactReported { unit, .. } if *unit == enemy)),
        "and the sighting finally reaches the commander, said out loud"
    );
    let contact = state
        .picture(0)
        .iter()
        .find(|c| c.unit == enemy)
        .expect("the picture now carries him");
    assert!(contact.fresh, "freshly, because somebody can see him now");
}

#[test]
fn a_contact_no_longer_seen_goes_stale_not_absent() {
    // "We lost sight of it" is information; "it was never there" is a lie.
    // A contact nobody can re-report stays on the picture as a ghost at the
    // last reported position, marked stale rather than deleted.
    let mut reg = registry();
    reg.command = Some(command_rules(999, false, 0));
    strip_radios(&mut reg);
    let mut state = picture_stage(&reg, 4);
    let enemy = UnitId(2);
    let seen_at = state.unit(enemy).unwrap().pos;

    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    let contact = state
        .picture(0)
        .iter()
        .find(|c| c.unit == enemy)
        .expect("reported while seen");
    assert!(contact.fresh);
    assert_eq!(contact.at, seen_at);

    // He slips away beyond every eye on the field.
    if let Some(unit) = state.unit_mut(enemy) {
        unit.pos = tactics_core::offset_to_hex(55, 0);
    }
    state.step_tick(&reg);
    assert!(
        !state.fog.side(0).spotted.contains(&enemy),
        "nobody can see him any more"
    );
    let ghost = state
        .picture(0)
        .iter()
        .find(|c| c.unit == enemy)
        .expect("but the commander still has him on the map");
    assert!(!ghost.fresh, "as a ghost");
    assert_eq!(
        ghost.at, seen_at,
        "standing where he was last reported, not where he is"
    );
}

// --- commander loss (chunk 6) ----------------------------------------------

/// A battle on a map written out in the test, so a chain of command and the
/// stakes a scenario places on it can be declared in one place and read in
/// one place. `file` is the map file minus its units, which come in as
/// placements the way every other battle helper here takes them.
fn scripted_battle(
    reg: &DataRegistry,
    file: serde_json::Value,
    placements: Vec<UnitPlacement>,
) -> BattleState {
    let file: tactics_core::map::MapFile = serde_json::from_value(file).unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let sides = vec![
        SideState {
            name: "West".into(),
            ai: None,
        },
        SideState {
            name: "East".into(),
            ai: None,
        },
    ];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        1,
    )
}

/// A placement that answers to a formation, and optionally commands it.
fn in_formation(mut placement: UnitPlacement, formation: &str, leads: bool) -> UnitPlacement {
    placement.formation = Some(formation.into());
    placement.leads = leads;
    placement
}

/// Take a unit off the board the way a shell would, but silently: no
/// `UnitDestroyed` event, so nothing this produces can be confused with the
/// pressure of watching a friend burn.
fn strike_down(state: &mut BattleState, unit: UnitId) {
    let victim = state.unit_mut(unit).expect("she was alive");
    victim.hp = 0;
    victim.alive = false;
}

#[test]
fn command_passes_to_the_next_girl_in_the_order_of_battle() {
    // Succession is formation machinery, not wire machinery, so this runs on
    // a registry with no `command` block at all: who is in charge of a platoon
    // is a fact about the platoon, and a mod that never priced a radio still
    // has one girl senior to another. Seniority is the order the map author
    // wrote her formation down in — lowest living unit id — which is the same
    // authorable rule `leads` follows for the first leader.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 3).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let (leader, heir) = {
        let formation = &state.formations()[armor.index()];
        let leader = formation.leader.expect("a commander");
        let heir = *formation
            .members
            .iter()
            .find(|id| **id != leader)
            .expect("and somebody to inherit");
        (leader, heir)
    };
    assert_eq!(
        state.formations()[armor.index()].founding_leader,
        Some(leader),
        "the map's commander is on record from the first tick"
    );

    commit_all(&reg, &mut state);
    let quiet = state.step_tick(&reg);
    assert!(
        !quiet
            .iter()
            .any(|e| matches!(e, BattleEvent::CommandPassed { .. })),
        "nobody is promoted while she is alive"
    );

    strike_down(&mut state, leader);
    let events = state.step_tick(&reg);

    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::CommandPassed { formation, from, to }
                if formation == "kuhlmann_armor" && *from == leader && *to == heir
        )),
        "command passes, and the log names both ends of it: {events:?}"
    );
    let formation = &state.formations()[armor.index()];
    assert_eq!(formation.leader, Some(heir), "she has the platoon now");
    assert_eq!(
        formation.founding_leader,
        Some(leader),
        "but the girl the map put in charge is not rewritten by her own death \
         — a scenario's loss condition asks about her, not her successor"
    );
    assert!(
        !state
            .step_tick(&reg)
            .iter()
            .any(|e| matches!(e, BattleEvent::CommandPassed { .. })),
        "and it is said once, not once a tick"
    );
}

/// A formation strung out along a road: the commander at the west end, three
/// more in a huddle twenty hexes east of her, and an enemy far beyond
/// everybody's guns. With a short radius nobody but the commander is on the
/// net, which is what makes the succession visible in the contact graph.
fn strung_out_platoon(reg: &DataRegistry) -> BattleState {
    let row = "g".repeat(60);
    let placements = vec![
        in_formation(unit_at([0, 0], 0, "recon_car", "Commander"), "column", true),
        in_formation(unit_at([20, 0], 0, "recon_car", "Heir"), "column", false),
        in_formation(
            unit_at([22, 0], 0, "recon_car", "Neighbour"),
            "column",
            false,
        ),
        in_formation(
            unit_at([40, 0], 0, "recon_car", "Straggler"),
            "column",
            false,
        ),
        unit_at([59, 0], 1, "recon_car", "Prowler"),
    ];
    scripted_battle(
        reg,
        serde_json::json!({
            "id": "strung_out",
            "palette": { "g": "grass" },
            "rows": [row],
            "formations": [ { "id": "column", "name": "The Column", "side": 0 } ],
        }),
        placements,
    )
}

#[test]
fn a_successor_leads_a_formation_back_into_contact() {
    // This inverts a rule an earlier chunk pinned: a dead leader used to
    // strand her whole formation out of contact for the rest of the battle,
    // because the net was anchored on a girl who was no longer there. She is
    // replaced within the tick now, and the net re-forms around wherever her
    // successor is standing — which is not where the commander was, so who is
    // in contact genuinely changes hands with the command.
    let mut reg = registry();
    // Five hexes and no relay: the column is too long for one voice, so the
    // three easterners are cut off while the commander is alive.
    reg.command = Some(command_rules(5, false, 0));
    strip_radios(&mut reg);
    let mut state = strung_out_platoon(&reg);
    let column = formation_named(&state, "column");
    let (commander, heir, neighbour, straggler) = (UnitId(0), UnitId(1), UnitId(2), UnitId(3));

    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    assert_eq!(
        state.formations()[column.index()]
            .out_of_contact
            .iter()
            .map(|c| c.unit)
            .collect::<Vec<_>>(),
        vec![heir, neighbour, straggler],
        "nobody down the road can hear her"
    );

    strike_down(&mut state, commander);
    let events = state.step_tick(&reg);

    let formation = &state.formations()[column.index()];
    assert_eq!(formation.leader, Some(heir), "the next girl has it");
    assert!(
        formation.in_contact(heir) && formation.in_contact(neighbour),
        "and the net re-forms around her: {:?}",
        formation.out_of_contact
    );
    assert!(
        !formation.in_contact(straggler),
        "twenty hexes further on is still twenty hexes further on"
    );
    for unit in [heir, neighbour] {
        assert!(
            events
                .iter()
                .any(|e| matches!(e, BattleEvent::ContactRestored { unit: u } if *u == unit)),
            "coming back onto the net is said out loud: {events:?}"
        );
    }
}

#[test]
fn losing_a_commander_shakes_her_formation() {
    // The formation takes it hard, and the whole formation does — unlike
    // watching a friend burn, which only reaches the crews who could see it,
    // this is news that travels the chain of command. It rides the same
    // ladder and the same one place that turns a tick's events into fear.
    let mut reg = registry_wireless();
    // A distinctive number, so what arrives can only have come from here.
    reg.morale.leader_lost = 5;
    let mut state = strung_out_platoon(&reg);
    let column = formation_named(&state, "column");
    let commander = UnitId(0);

    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    assert!(
        state.alive_units().all(|u| u.pressure == 0),
        "nothing has happened to anybody yet"
    );

    strike_down(&mut state, commander);
    state.step_tick(&reg);

    let members = state.formations()[column.index()].members.clone();
    for id in members.iter().filter(|id| **id != commander) {
        assert_eq!(
            state.unit(*id).expect("still on the field").pressure,
            5,
            "every girl in the column felt it, however far down the road she is"
        );
    }
    assert_eq!(
        state.unit(UnitId(4)).expect("the enemy is fine").pressure,
        0,
        "and nobody outside the formation felt anything at all"
    );
}

/// A decapitation stage: two crews of side 0 in one formation behind a forest
/// curtain, one enemy on the far side of it, and a lane home in the west. The
/// curtain is the same one the objective tests use — these rules are about
/// who is left, and a firefight would decide the battle before the
/// bookkeeping could be watched.
fn decapitation_battle(reg: &DataRegistry, loss_conditions: serde_json::Value) -> BattleState {
    scripted_battle(
        reg,
        serde_json::json!({
            "id": "decapitation_map",
            "palette": { "g": "grass", "f": "forest" },
            "rows": ["gggggfggggg"],
            "objectives": [{
                "id": "west_road", "at": [[0, 0], [1, 0]], "value": 1,
                "kind": "exit", "side": 0
            }],
            "formations": [ { "id": "staff", "name": "Staff Group", "side": 0 } ],
            "loss_conditions": loss_conditions,
        }),
        vec![
            in_formation(unit_at([3, 0], 0, "medium_tank", "Kuhlmann"), "staff", true),
            in_formation(
                unit_at([4, 0], 0, "medium_tank", "Adjutant"),
                "staff",
                false,
            ),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
    )
}

#[test]
fn a_map_may_declare_that_losing_the_command_formation_loses_the_battle() {
    // Decapitation is map data. The engine always degrades a formation that
    // loses its commander; whether the *battle* is over because of it is a
    // question about what this battle was for, and only the scenario knows.
    let reg = registry();
    let mut state = decapitation_battle(
        &reg,
        serde_json::json!([{ "side": 0, "formation": "staff", "when": "leader_lost" }]),
    );
    let commander = state.formations()[0]
        .founding_leader
        .expect("the map named one");

    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    assert!(!state.is_over(), "the battle is ordinary until she is hit");

    strike_down(&mut state, commander);
    let events = state.step_tick(&reg);
    assert_eq!(
        state.over.map(|r| (r.winner, r.reason)),
        Some((Some(1), EndReason::Decapitated)),
        "her side has lost, whatever is still on the field: {events:?}"
    );
    assert!(
        state.side_units(0).next().is_some(),
        "and it really is a decapitation rather than an elimination — side 0 \
         still has a tank"
    );

    // The negative, and the additivity rule in one line: the same battle, the
    // same dead commander, with the declaration taken out.
    let reg = registry();
    let mut plain = decapitation_battle(&reg, serde_json::json!([]));
    let commander = plain.formations()[0].founding_leader.expect("a commander");
    commit_all(&reg, &mut plain);
    plain.step_tick(&reg);
    strike_down(&mut plain, commander);
    plain.step_tick(&reg);
    assert!(
        !plain.is_over(),
        "a map that says nothing fights on with a new commander"
    );
}

#[test]
fn a_formation_that_withdrew_intact_is_not_a_decapitation() {
    // `wiped` asks whether a formation was destroyed, and driving off the map
    // by a lane your own map wrote down is not being destroyed. Reading
    // `!alive` here — the mistake exits exist to prevent — would end the
    // battle against the side that carried out its withdrawal perfectly.
    let reg = registry();
    let wiped = serde_json::json!([{ "side": 0, "formation": "staff", "when": "wiped" }]);
    let mut state = decapitation_battle(&reg, wiped.clone());
    for (unit, to) in [(UnitId(0), [0, 0]), (UnitId(1), [1, 0])] {
        state
            .apply(
                &reg,
                &Order::SetMove {
                    unit,
                    to: tactics_core::offset_to_hex(to[0], to[1]),
                },
            )
            .expect("the road home is walkable");
    }
    play_round(&reg, &mut state);

    assert!(
        state.units[0].exited && state.units[1].exited,
        "the whole staff group got away"
    );
    assert_ne!(
        state.over.map(|r| r.reason),
        Some(EndReason::Decapitated),
        "and leaving is not losing"
    );

    // The other half of the same rule: a formation that leaves a vehicle
    // burning behind it *has* been wiped out, and the condition fires.
    let mut caught = decapitation_battle(&reg, wiped);
    strike_down(&mut caught, UnitId(1));
    caught
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: tactics_core::offset_to_hex(0, 0),
            },
        )
        .expect("the survivor runs for the road");
    play_round(&reg, &mut caught);
    assert_eq!(
        caught.over.map(|r| (r.winner, r.reason)),
        Some((Some(1), EndReason::Decapitated)),
        "one of them died, so the formation was destroyed rather than withdrawn"
    );
}

#[test]
fn a_loss_condition_must_name_a_formation_of_its_own_side() {
    // A loss condition decides a battle, so a typo in one does not look
    // wrong — it quietly makes a scenario unwinnable, or unlosable. Both of
    // these are errors for that reason.
    let reg = registry();
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "misplaced_stakes",
        "palette": { "g": "grass" },
        "rows": ["gg"],
        "shape": "free",
        "sides": [{ "name": "West" }, { "name": "East" }],
        "formations": [ { "id": "staff", "side": 0 } ],
        "units": [
            { "at": [0, 0], "side": 0, "vehicle": "medium_tank", "formation": "staff" },
        ],
        "loss_conditions": [
            { "side": 0, "formation": "ghost_staff", "when": "leader_lost" },
            { "side": 1, "formation": "staff", "when": "wiped" },
        ],
    }))
    .unwrap();
    let mut report = tactics_core::data::ValidationReport::default();
    file.validate_into(&reg, &mut report);
    let errors = report.errors.join("\n");

    assert!(
        errors.contains("`ghost_staff`, which the map does not declare"),
        "a stake on a formation that does not exist can never be settled: {errors}"
    );
    assert!(
        errors.contains("but that formation belongs to side 0"),
        "and a side cannot stake the battle on somebody else's girls: {errors}"
    );
}

// --- the net is two media (chunk 9a) ---------------------------------------

/// Two side-0 formations on a road: Alpha's leader far west, her one member
/// far east beyond any radio — but two hexes from Bravo's leader, who is on
/// the net by definition. With `forest`, a wall of trees stands between that
/// member and Bravo, so nobody can see a flag.
fn signal_stage(reg: &DataRegistry, forest: bool, seed: u64) -> BattleState {
    let mut row: Vec<char> = std::iter::repeat_n('g', 40).collect();
    if forest {
        row[11] = 'f';
    }
    let row: String = row.into_iter().collect();
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "signal_stage",
        "palette": { "g": "grass", "f": "forest" },
        "rows": [row],
        "formations": [
            { "id": "alpha", "name": "Alpha", "side": 0 },
            { "id": "bravo", "name": "Bravo", "side": 0 },
        ],
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let sides = vec![
        SideState {
            name: "West".into(),
            ai: None,
        },
        SideState {
            name: "East".into(),
            ai: None,
        },
    ];
    let mut alpha_lead = unit_at([0, 0], 0, "recon_car", "Alpha Lead");
    alpha_lead.formation = Some("alpha".into());
    alpha_lead.leads = true;
    let mut stray = unit_at([12, 0], 0, "recon_car", "Stray");
    stray.formation = Some("alpha".into());
    let mut bravo_lead = unit_at([10, 0], 0, "recon_car", "Bravo Lead");
    bravo_lead.formation = Some("bravo".into());
    bravo_lead.leads = true;
    let enemy = unit_at([38, 0], 1, "recon_car", "Far Foe");
    let placements = vec![alpha_lead, stray, bravo_lead, enemy];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
}

#[test]
fn a_flag_carries_between_formations_where_no_radio_does() {
    // Radio follows the chain of command, but formation membership is
    // irrelevant to seeing a signal flag: a stray twelve hexes from her own
    // leader is on the net through the neighbouring platoon's commander two
    // hexes away — unless a forest stands where the flag would have to be
    // seen, or the mod declares no visual signalling at all.
    let rules = |visual: u32| {
        let mut r = registry();
        let mut rules = command_rules(4, true, 0);
        rules.visual_range = visual;
        r.command = Some(rules);
        strip_radios(&mut r);
        r
    };
    let stray = UnitId(1);
    let contact_of = |reg: &DataRegistry, forest: bool| -> bool {
        let mut state = signal_stage(reg, forest, 6);
        commit_all(reg, &mut state);
        state.step_tick(reg);
        state
            .formations()
            .iter()
            .find(|f| f.id == "alpha")
            .expect("alpha exists")
            .in_contact(stray)
    };

    let flags = rules(2);
    assert!(
        contact_of(&flags, false),
        "the flag reaches her through Bravo's commander"
    );
    assert!(
        !contact_of(&flags, true),
        "but not through a forest: a signal has to be seen"
    );
    let silent = rules(0);
    assert!(
        !contact_of(&silent, false),
        "and a mod that declares no visual medium has none"
    );
}

// --- orders wait instead of dying (chunk 9b) --------------------------------

/// A leader and one crew on an open road, with an enemy parked far enough
/// east to be nobody's business. Under a two-hex radio with nobody relaying,
/// the crew's ten hexes leave her stone deaf, and a test can drive her back
/// onto the net by hand.
fn radio_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "radio_stage",
        "palette": { "g": "grass" },
        "rows": ["g".repeat(30)],
        "formations": [ { "id": "net", "name": "The Net", "side": 0 } ],
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let sides = vec![
        SideState {
            name: "West".into(),
            ai: None,
        },
        SideState {
            name: "East".into(),
            ai: None,
        },
    ];
    let mut leader = unit_at([0, 0], 0, "recon_car", "Leader");
    leader.formation = Some("net".into());
    leader.leads = true;
    let mut crew = unit_at([10, 0], 0, "recon_car", "Stray");
    crew.formation = Some("net".into());
    let enemy = unit_at([29, 0], 1, "recon_car", "Far Foe");
    let placements = vec![leader, crew, enemy];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
}

/// A two-hex radio, nobody relaying, no flags: the narrowest net there is, so
/// a girl ten hexes out is out for a reason a test can state in one line.
fn radio_rules() -> DataRegistry {
    let mut reg = registry();
    reg.command = Some(command_rules(2, false, 0));
    strip_radios(&mut reg);
    reg
}

/// Play a round out so contact is computed and the next planning phase opens.
fn settle(reg: &DataRegistry, state: &mut BattleState) -> Vec<BattleEvent> {
    commit_all(reg, state);
    state.resolve_round(reg)
}

#[test]
fn an_order_to_a_cut_off_unit_waits_at_the_radio() {
    // The heart of the chunk: an order to a girl who cannot hear it is
    // *accepted* and held, not refused. Refusing was the old model, and it
    // made the player's only recourse "remember to click again", which is
    // bookkeeping rather than command.
    let reg = radio_rules();
    let mut state = radio_stage(&reg, 4);
    let crew = UnitId(1);
    settle(&reg, &mut state);
    assert!(!state.hears_orders(crew), "ten hexes on a two-hex radio");

    let first = tactics_core::offset_to_hex(13, 0);
    let events = state
        .apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: Some(first),
                fire: None,
            },
        )
        .expect("accepted, not refused");
    assert_eq!(
        events,
        vec![BattleEvent::OrdersWaiting { unit: crew }],
        "and said out loud once: an order silently parked is as illegible as \
         one silently dropped"
    );
    let hers = state.unit(crew).expect("she is alive");
    assert!(
        hers.intent.path.is_empty() && !hers.planned,
        "not a step of it reached her"
    );
    assert_eq!(
        state
            .command
            .waiting_for(crew)
            .expect("it is at the radio")
            .destination,
        Some(first),
        "the destination is what is held — never a path, which she will \
         recompute from wherever she actually is"
    );

    // Countermanding something that never went out replaces it rather than
    // queueing behind it. Two orders in the same tray is not a state anybody
    // could act on, exactly as it is not for a formation's mission.
    let second = tactics_core::offset_to_hex(7, 0);
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: Some(second),
                fire: None,
            },
        )
        .expect("accepted too");
    assert_eq!(
        state.command.waiting_for(crew).unwrap().destination,
        Some(second)
    );
    assert_eq!(state.command.waiting().len(), 1, "one slot, one girl");
}

#[test]
fn waiting_orders_arrive_with_contact_and_are_repathed() {
    // Delivery is at the planning phase — WEGO's bargain is that resolution
    // plays out what was planned — and what is delivered is the destination,
    // re-pathed. She has driven eight hexes since it was given; a route
    // computed back then would walk her through hexes she is nowhere near.
    let reg = radio_rules();
    let mut state = radio_stage(&reg, 4);
    let crew = UnitId(1);
    settle(&reg, &mut state);
    let was = state.unit(crew).expect("alive").pos;

    // Three hexes of grass is a wheeled car's round, and the destination has
    // to be affordable from where she will *be*: nothing about the order was
    // pathable from where she was when it was given, which is the point.
    let to = tactics_core::offset_to_hex(4, 0);
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: Some(to),
                fire: None,
            },
        )
        .expect("accepted");

    // She closes up on her commander overnight, which is what the whole
    // system is waiting for.
    let beside = tactics_core::offset_to_hex(1, 0);
    state.unit_mut(crew).unwrap().pos = beside;
    let events = settle(&reg, &mut state);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, BattleEvent::OrdersDelivered { unit } if *unit == crew))
            .count(),
        1,
        "the order lands, once, and says so: {events:?}"
    );
    assert!(
        state.command.waiting().is_empty(),
        "and is not delivered again tomorrow"
    );

    let path = &state.unit(crew).expect("alive").intent.path;
    assert_eq!(
        path.last().copied(),
        Some(to),
        "she is going where she was told"
    );
    assert_eq!(
        path.first().expect("a route").distance_to(beside),
        1,
        "and the first step is from where she is standing now"
    );
    assert!(
        path.first().expect("a route").distance_to(was) > 1,
        "which is nowhere near where she was when it was given ({was:?})"
    );
}

#[test]
fn clearing_reaches_the_radio_but_not_the_girl() {
    // Not sending is free, so taking back an order that never went out needs
    // no contact. Stopping *her* is a different thing entirely: she is
    // driving on her last orders down a wire that is dead, and clearing her
    // intent from here would be the commander countermanding into silence.
    let reg = radio_rules();
    let mut state = radio_stage(&reg, 4);
    let crew = UnitId(1);
    settle(&reg, &mut state);
    assert!(!state.hears_orders(crew));

    // Her own judgment about her own tank, which is what a planner issues and
    // what needs no radio at all.
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: crew,
                to: tactics_core::offset_to_hex(13, 0),
            },
        )
        .expect("a crew decides her own route");
    let hers = state.unit(crew).expect("alive").intent.clone();
    assert!(!hers.path.is_empty(), "she is going somewhere");

    state
        .apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: Some(tactics_core::offset_to_hex(4, 0)),
                fire: None,
            },
        )
        .expect("accepted, and waiting");
    assert!(state.command.waiting_for(crew).is_some());

    state
        .apply(&reg, &Order::ClearIntent { unit: crew })
        .expect("clearing needs no wire");
    assert!(
        state.command.waiting_for(crew).is_none(),
        "the message never leaves the radio"
    );
    assert_eq!(
        state.unit(crew).expect("alive").intent,
        hers,
        "and she carries on: you cannot reach her to stop her"
    );
}

#[test]
fn a_dead_girl_takes_no_delivery() {
    // An order for somebody who is not coming back is not news, it is an
    // epitaph. The slot goes quietly.
    let reg = radio_rules();
    let mut state = radio_stage(&reg, 4);
    let crew = UnitId(1);
    settle(&reg, &mut state);
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: Some(tactics_core::offset_to_hex(13, 0)),
                fire: None,
            },
        )
        .expect("accepted");

    strike_down(&mut state, crew);
    let events = settle(&reg, &mut state);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::OrdersDelivered { .. })),
        "nothing is delivered to a wreck: {events:?}"
    );
    assert!(
        state.command.waiting().is_empty(),
        "and the queue does not carry her for the rest of the battle"
    );
}

#[test]
fn a_radioed_order_to_a_girl_on_the_net_is_just_an_order() {
    // The ordinary case, which has to stay the ordinary code: a commander
    // talking to somebody who can hear her produces exactly what `SetMove`
    // and `SetFire` produce, with nothing held and nothing announced.
    let reg = radio_rules();
    let mut radioed = radio_stage(&reg, 4);
    let mut plain = radio_stage(&reg, 4);
    let leader = UnitId(0);
    let to = tactics_core::offset_to_hex(3, 0);
    let at = tactics_core::offset_to_hex(6, 0);

    let events = radioed
        .apply(
            &reg,
            &Order::Radio {
                unit: leader,
                to: Some(to),
                fire: Some(FireIntent::Area { at, weapon: 0 }),
            },
        )
        .expect("her own commander, on the net");
    assert!(
        events.is_empty(),
        "nothing waits and nothing is announced: {events:?}"
    );
    assert!(radioed.command.waiting().is_empty());

    plain
        .apply(&reg, &Order::SetMove { unit: leader, to })
        .expect("move");
    plain
        .apply(
            &reg,
            &Order::SetFire {
                unit: leader,
                fire: FireIntent::Area { at, weapon: 0 },
            },
        )
        .expect("fire");
    assert_eq!(
        radioed.unit(leader).unwrap().intent,
        plain.unit(leader).unwrap().intent,
        "the wire changes when an order lands, never what it says"
    );
    assert!(radioed.unit(leader).unwrap().planned);

    // And an order that was never legal is refused to the commander's face
    // whether or not anybody could hear it — the queue must never become a
    // way to smuggle a shot at a friendly past the rules.
    let mut deaf = radio_stage(&reg, 4);
    let crew = UnitId(1);
    settle(&reg, &mut deaf);
    assert!(!deaf.hears_orders(crew));
    assert_eq!(
        deaf.apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: None,
                fire: Some(FireIntent::Target {
                    target: UnitId(0),
                    weapon: 0
                }),
            },
        ),
        Err(tactics_core::battle::OrderError::FriendlyTarget)
    );
    assert!(deaf.command.waiting().is_empty(), "and nothing was queued");
}

// --- mission sequences (chunk 9c) ------------------------------------------

#[test]
fn a_plan_advances_when_its_first_leg_is_done() {
    // "Advance to the ford, then hold it." The plan is transmitted once and
    // promoted locally: when a member stands on the first leg's ground, the
    // next leg becomes the standing mission with no wire and no latency —
    // the leader has known the whole plan since it arrived.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 45).unwrap();
    let armor = formation_named(&state, "kuhlmann_armor");
    let leader = state.formations()[armor.index()].leader.unwrap();
    // A first leg two hexes from where the leader already stands, so one
    // round of driving completes it.
    let start = state.unit(leader).unwrap().pos;
    let near = start + tactics_core::Hex::new(2, 0);
    assert!(state.map.contains(near));
    let hold_at = near;
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: near },
            },
        )
        .unwrap();
    state
        .apply(
            &reg,
            &Order::QueueMission {
                formation: armor,
                mission: Mission::Hold { at: Some(hold_at) },
            },
        )
        .unwrap();
    assert_eq!(state.formations()[armor.index()].plan.len(), 1);

    let mut ai = AiDriver::new();
    ai.insert(0, sharp_planner(&reg, 45, "massed_armor"));
    let mut log: Vec<String> = Vec::new();
    for _ in 0..4 {
        if state.is_over() {
            break;
        }
        ai.plan_round(&reg, &mut state);
        let _ = state.apply(&reg, &Order::Commit { side: 1 });
        log.extend(state.resolve_round(&reg).iter().map(|e| format!("{e:?}")));
        if state.formations()[armor.index()].mission == Some(Mission::Hold { at: Some(hold_at) }) {
            break;
        }
    }
    assert_eq!(
        state.formations()[armor.index()].mission,
        Some(Mission::Hold { at: Some(hold_at) }),
        "the second leg is standing once the first is done"
    );
    assert!(
        state.formations()[armor.index()].plan.is_empty(),
        "and the plan has been consumed"
    );
    assert!(
        log.iter().any(|l| l.starts_with("MissionCompleted")),
        "the completion was announced: {log:?}"
    );
}

#[test]
fn nothing_follows_a_stand_fast_or_a_retreat() {
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 46).unwrap();
    let armor = formation_named(&state, "kuhlmann_armor");
    let anywhere = state.map.objectives()[0].anchor();

    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Hold { at: None },
            },
        )
        .unwrap();
    assert_eq!(
        state.apply(
            &reg,
            &Order::QueueMission {
                formation: armor,
                mission: Mission::Advance { to: anywhere },
            },
        ),
        Err(tactics_core::battle::OrderError::MissionIsTerminal),
        "a stand-fast has no afterwards"
    );

    // A withdrawal may end a plan — "take the bridge, then get out" is a
    // legitimate raid — but nothing may follow it.
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: anywhere },
            },
        )
        .unwrap();
    state
        .apply(
            &reg,
            &Order::QueueMission {
                formation: armor,
                mission: Mission::Withdraw {
                    via: "west_road".into(),
                },
            },
        )
        .unwrap();
    assert_eq!(
        state.apply(
            &reg,
            &Order::QueueMission {
                formation: armor,
                mission: Mission::Advance { to: anywhere },
            },
        ),
        Err(tactics_core::battle::OrderError::MissionIsTerminal),
        "and neither has a retreat"
    );
}

#[test]
fn an_amendment_travels_the_wire_like_any_order() {
    // A queued leg is still an order: with a command block it spends its
    // ticks in the air, and a countermand issued while it travels replaces
    // the whole plan — the wire does not care what the envelope says.
    let mut reg = registry();
    reg.command = Some(command_rules(999, true, 2));
    strip_radios(&mut reg);
    let mut state = BattleState::from_map(&reg, "river_crossing", 47).unwrap();
    let armor = formation_named(&state, "kuhlmann_armor");
    let bridge = state.map.objectives()[0].anchor();

    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: bridge },
            },
        )
        .unwrap();
    // First order lands (2 ticks), then the amendment goes into the air.
    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    state.step_tick(&reg);
    assert!(state.formations()[armor.index()].mission.is_some());
    state.resolve_round(&reg);

    state
        .apply(
            &reg,
            &Order::QueueMission {
                formation: armor,
                mission: Mission::Recon { toward: bridge },
            },
        )
        .unwrap();
    let f = &state.formations()[armor.index()];
    assert!(
        matches!(
            f.incoming,
            Some((tactics_core::battle::MissionChange::Append(_), _))
        ),
        "the amendment is in the air, not in the plan"
    );
    assert!(f.plan.is_empty());

    // Countermanded before it lands: the replacement wins and the amendment
    // never existed.
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Hold { at: None },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    state.step_tick(&reg);
    let f = &state.formations()[armor.index()];
    assert_eq!(f.mission, Some(Mission::Hold { at: None }));
    assert!(f.plan.is_empty(), "the countermand replaced the whole plan");
}

// --- seeing the net (chunk 9d) ----------------------------------------------

#[test]
fn the_ring_the_screen_draws_is_the_edge_the_engine_walks() {
    // `radio_reach` exists so the battle screen can draw a leader's range
    // ring without keeping its own copy of the formula. What makes it worth
    // having is that the contact graph reads the same function: set a radius
    // the ring can be counted against, and the girl one hex inside it is on
    // the net while the one a hex outside is not.
    let mut reg = registry();
    reg.command = Some(command_rules(6, false, 0));
    strip_radios(&mut reg);
    let mut state = radio_stage(&reg, 3);

    let leader = UnitId(0);
    let stray = UnitId(1);
    assert_eq!(
        state.radio_reach(&reg, leader),
        Some(6),
        "no radio hardware and no signals coefficient: the block's own radius"
    );

    // The stray sits ten hexes out in `radio_stage`; walk her to the ring and
    // then one hex past it, and contact follows the number the ring is drawn
    // at rather than any second opinion.
    let on_the_ring = state.unit(leader).expect("leader").pos + tactics_core::Hex::new(6, 0);
    state.units[stray.index()].pos = on_the_ring;
    settle(&reg, &mut state);
    assert!(
        state.formations()[0].in_contact(stray),
        "a girl standing on the ring hears her leader"
    );

    state.units[stray.index()].pos = on_the_ring + tactics_core::Hex::new(1, 0);
    settle(&reg, &mut state);
    assert!(
        !state.formations()[0].in_contact(stray),
        "and one hex beyond it she does not"
    );

    // Nothing to draw where nothing is priced: the same window answers `None`
    // for a mod with no chain of command, which is what keeps the display's
    // additivity story the same as the engine's.
    reg.command = None;
    assert_eq!(state.radio_reach(&reg, leader), None);
}

// --- the battle drill (chunk 10 opening move) ------------------------------

/// Open grass with a forest stand to the west: her, unordered and outside
/// any formation, and a gun tank well inside range to the east.
fn drill_stage(reg: &DataRegistry, enemy_at: i32, seed: u64) -> BattleState {
    let row = format!("gggf{}", "g".repeat(26));
    two_side_battle(
        reg,
        &[&row, &row, &row],
        vec![
            unit_at([5, 1], 0, "recon_car", "Unordered"),
            unit_at([enemy_at, 1], 1, "medium_tank", "Gun Tank"),
        ],
        seed,
    )
}

#[test]
fn a_crew_under_fire_takes_cover_instead_of_waiting_for_orders() {
    // The battle drill: nobody under fire waits for permission to survive.
    // An unordered unit the delegation layer used to park with a bare
    // hold-fire now returns fire and seeks cover when something that can
    // hit her is in sight — and only then; explicit orders still outrank
    // the drill because they mark her planned before it is consulted.
    let reg = registry_wireless();
    let mut state = drill_stage(&reg, 9, 51);
    let (crew, enemy) = (UnitId(0), UnitId(1));
    assert!(
        state.fog.side(0).spotted.contains(&enemy),
        "the stage needs her to see the danger"
    );

    let mut ai = AiDriver::new();
    ai.insert(
        0,
        Box::new(tactics_core::ai::SideCommand::executor_only(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            51,
            &reg,
        )),
    );
    ai.plan_round(&reg, &mut state);

    let unit = state.unit(crew).unwrap();
    assert!(unit.planned, "the drill plans her");
    let dest = unit.planned_destination();
    assert_ne!(dest, unit.pos, "she does not sit in the open");
    assert_eq!(
        state.terrain_at(dest),
        Some("forest"),
        "she makes for the cover, not merely anywhere"
    );
}

#[test]
fn an_idle_crew_out_of_danger_stays_put() {
    // The other half of the bargain: the drill is survival, not initiative.
    // Nothing spotted that can reach her means the parking lot stays parked,
    // exactly as it did before the drill existed.
    let reg = registry_wireless();
    let mut state = drill_stage(&reg, 28, 52);
    let (crew, enemy) = (UnitId(0), UnitId(1));
    assert!(
        !state.fog.side(0).spotted.contains(&enemy),
        "the stage needs the danger out of sight"
    );
    let parked = state.unit(crew).unwrap().pos;

    let mut ai = AiDriver::new();
    ai.insert(
        0,
        Box::new(tactics_core::ai::SideCommand::executor_only(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            52,
            &reg,
        )),
    );
    ai.plan_round(&reg, &mut state);

    let unit = state.unit(crew).unwrap();
    assert!(unit.planned && unit.intent.path.is_empty());
    assert_eq!(unit.pos, parked);
}

// --- fighting as one (chunk 10d) -------------------------------------------

#[test]
fn a_formation_keeps_its_interval_and_its_sight_lines() {
    // The spacing band, measured directly: with every other term zeroed,
    // the mass term should prefer the supported interval over hugging
    // (one shell, one vehicle), over straggling (out of support), and over
    // standing near a friend who cannot see you (near but masked is not
    // mutual support).
    let reg = registry_wireless();
    let row = {
        let mut r: Vec<char> = std::iter::repeat_n('g', 46).collect();
        r[4] = 'f';
        r.into_iter().collect::<String>()
    };
    let state = {
        let mut s = two_side_battle(
            &reg,
            &[&row, &row, &row],
            vec![
                unit_at([15, 1], 0, "medium_tank", "Scored"),
                unit_at([6, 1], 0, "medium_tank", "Friend"),
                unit_at([45, 1], 1, "medium_tank", "Far Foe"),
            ],
            9,
        );
        assert!(
            s.fog.side(0).spotted.is_empty(),
            "the stage needs no enemy in sight"
        );
        // The friend is already under orders to stand where she stands, so
        // the band reads her planned destination.
        s.unit_mut(UnitId(1)).unwrap().intent.path = vec![tactics_core::offset_to_hex(6, 1)];
        s
    };
    let evaluator = Evaluator::new(tactics_core::data::DoctrineDef {
        id: "band_probe".into(),
        name: String::new(),
        description: String::new(),
        aggression: 0.0,
        cover_value: 0.0,
        elevation_value: 0.0,
        concentration: 1.0,
        scouting: 0.0,
        objective_value: 0.0,
        indirect_appetite: 1.0,
        withdraw_threshold: 0.5,
        initiative: 0.5,
        delegation: 0.5,
    });
    let score = |col: i32| {
        evaluator
            .score_tile(&reg, &state, UnitId(0), tactics_core::offset_to_hex(col, 1))
            .score
    };

    let hugging = score(7);
    let interval = score(9);
    let straggling = score(14);
    let masked = score(3);
    assert!(
        interval > hugging,
        "the interval beats hugging: {interval} vs {hugging}"
    );
    assert!(
        interval > straggling,
        "the interval beats straggling: {interval} vs {straggling}"
    );
    assert!(
        interval > masked,
        "a friend who cannot see you is not support: {interval} vs {masked}"
    );
}

/// A two-car section under one leader on a long road, ordered east, with a
/// gun tank visible far beyond — inside their eyes, outside everyone's guns,
/// so contact exists and nobody dies while the section moves.
fn bounding_stage(
    reg: &DataRegistry,
    enemy_at: i32,
    doctrine: Option<&str>,
    seed: u64,
) -> BattleState {
    let row = "g".repeat(40);
    let mut formation = serde_json::json!({ "id": "section", "name": "The Section", "side": 0 });
    if let Some(doctrine) = doctrine {
        formation["doctrine"] = serde_json::json!(doctrine);
    }
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "bounding_stage",
        "palette": { "g": "grass" },
        "rows": [row.clone(), row.clone(), row],
        "formations": [ formation ],
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let sides = vec![
        SideState {
            name: "West".into(),
            ai: None,
        },
        SideState {
            name: "East".into(),
            ai: None,
        },
    ];
    let mut lead = unit_at([2, 1], 0, "recon_car", "Lead");
    lead.formation = Some("section".into());
    lead.leads = true;
    let mut wing = unit_at([2, 2], 0, "recon_car", "Wing");
    wing.formation = Some("section".into());
    let enemy = unit_at([enemy_at, 1], 1, "medium_tank", "Gun Tank");
    let placements = vec![lead, wing, enemy];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
}

/// Plan one executor-only round for side 0 and say who moved.
fn bound_once(reg: &DataRegistry, state: &mut BattleState, seed: u64) -> Vec<bool> {
    let mut ai = AiDriver::new();
    ai.insert(
        0,
        Box::new(tactics_core::ai::SideCommand::executor_only(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            seed,
            reg,
        )),
    );
    ai.plan_round(reg, state);
    [UnitId(0), UnitId(1)]
        .iter()
        .map(|id| !state.unit(*id).unwrap().intent.path.is_empty())
        .collect()
}

#[test]
fn a_section_in_contact_bounds_by_element() {
    // Fire and movement, with WEGO rounds as the bounds: in contact and
    // under a movement mission, one element advances while the other stands
    // with guns up, and the elements swap every round.
    let reg = registry_wireless();
    let mut state = bounding_stage(&reg, 16, None, 61);
    assert!(
        !state.fog.side(0).spotted.is_empty(),
        "the stage needs contact"
    );
    let section = formation_named(&state, "section");
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: section,
                mission: Mission::Advance {
                    to: tactics_core::offset_to_hex(30, 1),
                },
            },
        )
        .unwrap();

    let first = bound_once(&reg, &mut state, 61);
    assert_eq!(
        first.iter().filter(|moved| **moved).count(),
        1,
        "one element bounds while the other overwatches: {first:?}"
    );
    let _ = state.apply(&reg, &Order::Commit { side: 1 });
    state.resolve_round(&reg);

    let second = bound_once(&reg, &mut state, 61);
    assert_eq!(
        second.iter().filter(|moved| **moved).count(),
        1,
        "and again next round: {second:?}"
    );
    assert_ne!(first, second, "with the elements swapped");
}

#[test]
fn a_section_out_of_contact_travels() {
    // No contact, no ceremony: everyone moves, which is exactly the game
    // before bounding existed.
    let reg = registry_wireless();
    let mut state = bounding_stage(&reg, 39, None, 62);
    assert!(
        state.fog.side(0).spotted.is_empty(),
        "the stage needs the danger out of sight"
    );
    let section = formation_named(&state, "section");
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: section,
                mission: Mission::Advance {
                    to: tactics_core::offset_to_hex(30, 1),
                },
            },
        )
        .unwrap();
    let moved = bound_once(&reg, &mut state, 62);
    assert_eq!(moved, vec![true, true], "traveling, not bounding");
}

#[test]
fn an_aggressive_doctrine_travels_in_overwatch() {
    // Traveling versus bounding is doctrine's call: massed armour trades
    // security for tempo and keeps everyone moving even in contact — which
    // is both the textbook and what the harness demanded, since universal
    // bounding cost the aggressive doctrine half its wins.
    let reg = registry_wireless();
    let mut state = bounding_stage(&reg, 16, Some("massed_armor"), 63);
    assert!(!state.fog.side(0).spotted.is_empty());
    let section = formation_named(&state, "section");
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: section,
                mission: Mission::Advance {
                    to: tactics_core::offset_to_hex(30, 1),
                },
            },
        )
        .unwrap();
    let moved = bound_once(&reg, &mut state, 63);
    assert_eq!(moved, vec![true, true], "tempo over ceremony");
}

// --- radios as hardware (chunk 10a) ----------------------------------------

/// A leader with a transceiver and a wing with a receive-only set, ten hexes
/// out — far beyond any flag, well inside the set — with an enemy scout
/// visible only to the wing. `ridge` raises a wall of ground between them;
/// `forest` plants trees there instead.
fn hardware_stage(reg: &DataRegistry, ridge: bool, forest: bool, seed: u64) -> BattleState {
    let mut row: Vec<char> = std::iter::repeat_n('g', 30).collect();
    if forest {
        row[5] = 'f';
    }
    let row: String = row.into_iter().collect();
    let elev = if ridge {
        format!("000003{}", "0".repeat(24))
    } else {
        "0".repeat(30)
    };
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "hardware_stage",
        "palette": { "g": "grass", "f": "forest" },
        "rows": [row.clone(), row.clone(), row],
        "elevation": [elev.clone(), elev.clone(), elev],
        "formations": [ { "id": "net", "name": "The Net", "side": 0 } ],
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let sides = vec![
        SideState {
            name: "West".into(),
            ai: None,
        },
        SideState {
            name: "East".into(),
            ai: None,
        },
    ];
    let mut lead = unit_at([0, 1], 0, "medium_tank", "Lead");
    lead.formation = Some("net".into());
    lead.leads = true;
    let mut wing = unit_at([10, 1], 0, "light_tank", "Wing");
    wing.formation = Some("net".into());
    let enemy = unit_at([21, 1], 1, "recon_car", "Prowler");
    let placements = vec![lead, wing, enemy];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
}

#[test]
fn a_receive_only_tank_hears_orders_and_files_no_reports() {
    // The early-war fit, working as it did in 1941: the line tank's
    // receiver keeps her on her leader's net — orders reach her — but her
    // sightings die in her silence until a flag can carry them to somebody
    // with a set.
    let reg = registry();
    let mut state = hardware_stage(&reg, false, false, 71);
    let (lead, wing, enemy) = (UnitId(0), UnitId(1), UnitId(2));

    commit_all(&reg, &mut state);
    state.step_tick(&reg);

    let net = formation_named(&state, "net");
    assert!(
        state.formations()[net.index()].in_contact(wing),
        "ten hexes is far beyond any flag, and her receiver hears the set"
    );
    assert!(
        state.fog.side(0).spotted.contains(&enemy),
        "she sees the prowler her leader cannot"
    );
    assert!(
        !state.picture(0).iter().any(|c| c.unit == enemy),
        "and the commander learns nothing: a receiver files no reports"
    );

    // Fall back beside the leader: the flag reaches a transmitter, and the
    // sighting she is still holding goes through.
    if let Some(unit) = state.unit_mut(wing) {
        unit.pos = tactics_core::offset_to_hex(2, 1);
    }
    let events = state.step_tick(&reg);
    let still_seen = state.fog.side(0).spotted.contains(&enemy);
    assert_eq!(
        state.picture(0).iter().any(|c| c.unit == enemy),
        still_seen,
        "beside the set, whatever she still sees is reported"
    );
    if still_seen {
        assert!(
            events
                .iter()
                .any(|e| matches!(e, BattleEvent::ContactReported { unit, .. } if *unit == enemy)),
            "and said out loud"
        );
    }
    let _ = lead;
}

#[test]
fn a_hill_masks_the_radio_and_a_forest_does_not() {
    // VHF is line-of-sight-ish: the ground stands in the way, the canopy
    // does not. The same two vehicles at the same ten hexes are on the net
    // through a forest and off it behind a ridge.
    let reg = registry();
    let wing = UnitId(1);

    let mut behind_trees = hardware_stage(&reg, false, true, 72);
    commit_all(&reg, &mut behind_trees);
    behind_trees.step_tick(&reg);
    let net = formation_named(&behind_trees, "net");
    assert!(
        behind_trees.formations()[net.index()].in_contact(wing),
        "a forest does not mask a radio"
    );

    let mut behind_ridge = hardware_stage(&reg, true, false, 73);
    commit_all(&reg, &mut behind_ridge);
    behind_ridge.step_tick(&reg);
    let net = formation_named(&behind_ridge, "net");
    assert!(
        !behind_ridge.formations()[net.index()].in_contact(wing),
        "a ridge does: she is in a radio shadow"
    );
}

// --- the commander's pulse (chunk 10b) -------------------------------------

/// Drive one planning round for a command side and count what it assigned.
fn pulse_round(reg: &DataRegistry, state: &mut BattleState, ai: &mut AiDriver) -> usize {
    let mut assigned = 0;
    ai.plan_round_with(reg, state, |d| {
        assigned += d
            .events
            .iter()
            .filter(|e| matches!(e, BattleEvent::MissionAssigned { .. }))
            .count();
    });
    let _ = state.apply(reg, &Order::Commit { side: 0 });
    state.resolve_round(reg);
    assigned
}

fn pulsed_rules() -> tactics_core::data::CommandRules {
    let mut rules = command_rules(999, true, 0);
    // One round between reviews at every skill: she thinks every other
    // round, and what happens in between must wake her or wait. The cap
    // must rise with the base — delay() clamps to it, and a zero cap is
    // the every-round pulse regardless of base.
    rules.review.base_ticks = 1;
    rules.review.max_ticks = 3;
    rules
}

/// A quiet battlefield for watching the pulse itself: one commanded
/// formation, two pieces of ground worth holding, and the only enemy far
/// beyond anyone's eyes — so no contact can ever interrupt the clock.
fn pulse_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    let row = "g".repeat(40);
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "pulse_stage",
        "palette": { "g": "grass" },
        "rows": [row.clone(), row.clone(), row],
        "objectives": [
            { "id": "bridge", "name": "Bridge", "at": [[5, 1]], "value": 3 },
            { "id": "ford", "name": "Ford", "at": [[35, 1]], "value": 2 },
        ],
        "formations": [ { "id": "line", "name": "The Line", "side": 1 } ],
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let sides = vec![
        SideState {
            name: "West".into(),
            ai: None,
        },
        SideState {
            name: "East".into(),
            ai: None,
        },
    ];
    let mut lead = unit_at([25, 1], 1, "medium_tank", "Lead");
    lead.formation = Some("line".into());
    lead.leads = true;
    let mut wing = unit_at([26, 2], 1, "medium_tank", "Wing");
    wing.formation = Some("line".into());
    let hermit = unit_at([39, 2], 0, "medium_tank", "Hermit");
    let placements = vec![lead, wing, hermit];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
}

#[test]
fn a_commander_reviews_on_her_own_pulse() {
    // Between pulses the plan stands. The balanced doctrine retargets off
    // ground already taken — but only when she is actually reviewing, so
    // the ford assignment waits for her clock even though the bridge fell
    // in the first minute.
    let mut reg = registry();
    reg.command = Some(pulsed_rules());
    strip_radios(&mut reg);
    let mut state = pulse_stage(&reg, 81);
    assert!(state.fog.side(1).spotted.is_empty(), "a quiet field");
    let mut ai = AiDriver::new();
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            81,
            &reg,
        ),
    );

    assert!(
        pulse_round(&reg, &mut state, &mut ai) > 0,
        "round 1 assigns"
    );
    // The bridge falls to her side between pulses.
    state.objective_held[0] = Some(1);
    assert_eq!(
        pulse_round(&reg, &mut state, &mut ai),
        0,
        "round 2 is between pulses: the plan stands"
    );
    assert!(
        pulse_round(&reg, &mut state, &mut ai) > 0,
        "round 3 is her pulse, and she moves her people on"
    );
}

#[test]
fn a_breaking_formation_wakes_her_between_pulses() {
    // No commander sleeps through a formation breaking: the interrupt runs
    // the review early and the withdrawal goes out on the round the damage
    // is known, not on the next scheduled pulse.
    let mut reg = registry();
    reg.command = Some(pulsed_rules());
    strip_radios(&mut reg);
    let mut state = BattleState::from_map(&reg, "river_crossing", 82).unwrap();
    let mut ai = AiDriver::new();
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: Some("elastic_defense".into()),
            },
            82,
            &reg,
        ),
    );

    let _ = pulse_round(&reg, &mut state, &mut ai);
    // Between pulses, the line is shot to pieces.
    let line = formation_named(&state, "valkyrie_line");
    for id in state.formations()[line.index()].members.clone() {
        state.units[id.index()].hp = 1;
    }
    let mut withdrew = false;
    ai.plan_round_with(&reg, &mut state, |d| {
        withdrew |= d.events.iter().any(|e| {
            matches!(
                e,
                BattleEvent::MissionAssigned {
                    mission: Mission::Withdraw { .. },
                    ..
                }
            )
        });
    });
    assert!(
        withdrew,
        "the shock wakes her and the order goes out at once"
    );
}
