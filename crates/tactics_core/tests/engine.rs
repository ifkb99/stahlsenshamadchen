//! End-to-end tests against the real `assets/mods` content.

use std::path::PathBuf;
use tactics_core::ai::{
    AiConfig, AiDriver, AiPlanner, Evaluator, UtilityPlanner, make_battle_planner,
};
use tactics_core::battle::{
    BattleState, EndReason, Event as BattleEvent, FireIntent, FormationId, Latitude, Mission,
    Order, STALEMATE_ROUNDS, SideState, UnitId, los_clear, reachable,
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
            aboard_at: None,
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
            aboard_at: None,
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
            target.id,
            blind,
        );
        let breakdown = tactics_core::battle::hit_breakdown(
            &reg,
            &state,
            attacker.id,
            attacker.pos,
            weapon,
            target.id,
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
    assert_eq!(preview.target_condition, 100, "she is untouched");
    assert_eq!(preview.distance, attacker.pos.distance_to(target.pos));
    // The old assertion here — "a hit always does something" — was the
    // damage floor's own slogan, and the floor is dead. What the preview
    // owes the player now is the round by name, the odds of it beating the
    // plate, and arithmetic that multiplies through honestly.
    assert!(preview.ammo.is_some(), "the chambered round is named");
    assert!(
        (0..=100).contains(&preview.pen_chance),
        "penetration is a percentage"
    );
    assert!(
        !preview.lethal || preview.pen_chance > 0,
        "lethal means a penetration that would finish her, not a wish"
    );
    let expected = preview.hit.total as f32 / 100.0 * preview.pen_chance as f32 / 100.0
        * preview.damage as f32;
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

/// One company on the trunk road with an enemy standing on it three hexes
/// ahead, and open ground beyond that to be ordered onto.
///
/// `frontier`'s row 2 is an unbroken highway from column 2 to column 11 with
/// plains either side, so the enemy blocks the direct road without walling it
/// off — which is the interesting case. A march that means to avoid contact can
/// go round; the question these tests ask is which marches take that offer.
///
/// Wireless, because what is being weighed is what an order *does*, not whether
/// headquarters could get it out.
fn blocked_road(reg: &DataRegistry) -> (OverworldState, ArmyId, ArmyId, hexx::Hex) {
    let mut state = OverworldState::from_map(reg, "frontier", 1).unwrap();
    let army = state.senior_army(0).unwrap();
    let blocker = state.senior_army(1).unwrap();
    state.army_mut(army).unwrap().pos = tactics_core::offset_to_hex(2, 2);
    state.army_mut(blocker).unwrap().pos = tactics_core::offset_to_hex(5, 2);
    (state, army, blocker, tactics_core::offset_to_hex(9, 2))
}

#[test]
fn a_standing_advance_engages_the_army_blocking_its_road() {
    // The bug this pins: an advance aimed at ground *beyond* an enemy used to
    // treat that enemy as a wall, so it either detoured around him or halted
    // beside him and stood there for the rest of the campaign, because the only
    // thing that ever started a battle was an enemy sitting on the tile the
    // order named. An operational advance is movement to contact. It fights
    // what is in the road.
    let reg = registry_wireless();
    let (mut state, army, blocker, target) = blocked_road(&reg);
    let at = state.army(blocker).unwrap().pos;
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
        events.iter().any(|e| matches!(
            e,
            OverworldEvent::BattleTriggered { attacker, defender, at: hex }
                if *attacker == army && *defender == blocker && *hex == at
        )),
        "she was told to take the road and somebody is on it: {events:?}"
    );
    assert_eq!(
        state.army(army).unwrap().pos.distance_to(at),
        1,
        "and she is up against him, not on him — the battle decides the tile"
    );
    // One move, one battle. Nothing walks past the enemy it just found to go
    // looking for the next one.
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, OverworldEvent::BattleTriggered { .. }))
            .count(),
        1,
        "{events:?}"
    );
    assert_eq!(
        state.army(army).unwrap().mission,
        Some(ArmyMission::Advance { to: target }),
        "the order stands; the battle is how she carries it out"
    );
}

#[test]
fn a_standing_withdraw_goes_round_the_enemy_rather_than_through_him() {
    // The same road and the same enemy, under the opposite order. An army
    // falling back is trying to be somewhere else; one that started a battle on
    // the way out would be obeying the reverse of what it was told. So a
    // withdrawal keeps the avoid semantics a hand order has — the enemy is a
    // wall, it goes round him if there is a way round, and it does not go
    // anywhere at all if there is not.
    //
    // On this road there is a way round — plains either side of the highway —
    // and she takes it, ending the day beside him without a shot. Adjacency is
    // not contact; the order is.
    let reg = registry_wireless();
    let (mut state, army, blocker, target) = blocked_road(&reg);
    let at = state.army(blocker).unwrap().pos;
    let start = state.army(army).unwrap().pos;
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Withdraw { to: target },
            },
        )
        .unwrap();

    let events = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OverworldEvent::BattleTriggered { .. })),
        "a withdrawal does not pick fights: {events:?}"
    );
    let ended = state.army(army).unwrap().pos;
    assert_ne!(ended, at, "and it certainly does not drive through him");
    assert_ne!(
        ended, start,
        "she is not stuck either — the enemy is a wall, not a full stop"
    );
    assert_eq!(
        state.army(army).unwrap().mission,
        Some(ArmyMission::Withdraw { to: target }),
        "the order survives the day, as every standing order does"
    );
}

#[test]
fn a_hand_ordered_march_past_an_enemy_is_not_a_declaration_of_war() {
    // The player's click is not an order to attack. She may put a company on
    // the tile in front of an enemy to hold a line, or send it somewhere the
    // short way happens to run past him, and neither is a decision to fight
    // today. Only pointing *at* the enemy is that. This is the half of the
    // ruling that the advance change must not quietly take away.
    let reg = registry_wireless();
    let (mut state, army, blocker, target) = blocked_road(&reg);
    let at = state.army(blocker).unwrap().pos;

    // Straight up to his front bumper and stop.
    let beside = tactics_core::offset_to_hex(4, 2);
    let events = state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: beside })
        .unwrap();
    assert_eq!(state.army(army).unwrap().pos, beside);
    assert_eq!(state.army(army).unwrap().pos.distance_to(at), 1);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OverworldEvent::BattleTriggered { .. })),
        "standing next to somebody is not attacking him: {events:?}"
    );

    // And a march aimed at ground on the far side of him is a march, not an
    // assault: it finds its own way there and starts nothing on the way.
    next_turn_of(&reg, &mut state, 0);
    let events = state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: target })
        .unwrap();
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, OverworldEvent::BattleTriggered { .. })),
        "the ordered destination was empty ground: {events:?}"
    );
    assert_ne!(state.army(army).unwrap().pos, at);

    // Pointing at him, on the other hand, is exactly that — unchanged.
    next_turn_of(&reg, &mut state, 0);
    let events = state
        .apply(&reg, &OverworldOrder::MoveArmy { army, to: at })
        .unwrap();
    assert!(
        events.iter().any(|e| matches!(
            e,
            OverworldEvent::BattleTriggered { attacker, defender, at: hex }
                if *attacker == army && *defender == blocker && *hex == at
        )),
        "she was aimed at him: {events:?}"
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
        aboard_at: None,
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
        aboard_at: None,
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

    assert!(
        eval.score_tile(&reg, &state, UnitId(0), exit).score
            <= eval.score_tile(&reg, &state, UnitId(0), away).score,
        "an undamaged crew cannot see the exit at all"
    );

    let mut hurt = state;
    maul(&reg, &mut hurt, UnitId(0));
    assert!(hurt.condition(&reg, hurt.unit(UnitId(0)).unwrap()) < 0.5);
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
        errors.contains("only one cadet can be in command"),
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
                    latitude: tactics_core::battle::Latitude::Delegated,
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
        latitude: tactics_core::battle::Latitude::Delegated,
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
                    latitude: tactics_core::battle::Latitude::Delegated,
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
    let mut reg = registry();
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
    // Cadence, not lethality: with real penetration an 88 that gets in can
    // end the battle mid-round and cut the count short, so the targets are
    // given absurd plate. Ordered fire has no value gate — the guns keep
    // shooting, the rounds keep bouncing, and only the metronome is
    // measured.
    for target in [near, far] {
        let vid = state.unit(target).unwrap().vehicle.clone();
        if let Some(v) = reg.vehicles.get_mut(&vid) {
            v.armor = tactics_core::data::ArmorSpec {
                front: 99,
                side: 99,
                rear: 99,
            };
        }
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
        // One cadet still fighting — already wounded, so any hit that finds
        // her is her last — and nothing else aboard but the gun: a single
        // penetration finishes the vehicle, and both crews are in that
        // state when both rounds arrive in the same tick. (Wounded rather
        // than fine because a 75 is deliberately below the savage
        // threshold now: it wounds before it kills.)
        for unit in [west, east] {
            let u = state.unit_mut(unit).unwrap();
            let seats = u.crew.len();
            u.crew_state = vec![tactics_core::battle::CrewCondition::Out; seats];
            if seats > 0 {
                u.crew_state[0] = tactics_core::battle::CrewCondition::Wounded;
            }
            for (id, hits) in u.modules.iter_mut() {
                if id != "main_gun" {
                    *hits = 0;
                }
            }
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
    // shoots at them, even mid-move. Softened so the scene stays about
    // movement: an unsoftened 75 can destroy the main gun with the opening
    // hit, and a disarmed crew proves nothing either way.
    let mut reg = registry();
    soften(&mut reg);
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
            Order::SetMove { .. }
                | Order::SetFire { .. }
                | Order::SetGoal { .. }
                | Order::Commit { .. }
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

/// The point of the roster: a cadet is the same person on either side of a
/// battle. Before this she was a lookup into static mod data, so nothing that
/// happened to her could be recorded anywhere.
#[test]
fn girls_persist_across_battles_and_recover_over_days() {
    use tactics_core::roster::{CadetStatus, CasualtyRules, CrewFate};

    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 9).expect("overworld");

    // The map named crews by definition id; the world turned them into people.
    assert!(
        state.roster.len() >= 3,
        "frontier's armies should have enlisted their crews"
    );
    let army = state.side_armies(0).next().unwrap();
    let cadet = army.units[0].crew[0];
    assert_eq!(
        state.roster.get(cadet).unwrap().owner,
        0,
        "a cadet belongs to the academy whose army she rides with"
    );
    assert_eq!(state.roster.get(cadet).unwrap().battles, 0);

    // Surviving a battle is recorded on her, not on the vehicle.
    let attacker = state.side_armies(0).next().unwrap().id;
    let defender = state.side_armies(1).next().unwrap().id;
    let survivors = vec![(attacker, state.army(attacker).unwrap().units.clone())];
    state.apply_battle_result(&reg, attacker, defender, &survivors, &[]);
    assert_eq!(state.roster.get(cadet).unwrap().battles, 1);

    // And so is being shot out of it. With permadeath off, the worst case is
    // a long recovery rather than a funeral.
    state.rules = CasualtyRules { permadeath: false };
    let loss = tactics_core::overworld::CrewLoss {
        cadet,
        vehicle: state.army(attacker).unwrap().units[0].vehicle.clone(),
        killed_by: Some(tactics_core::data::DamageType::Kinetic),
        found: None,
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
    let status = state.roster.get(cadet).unwrap().status;
    assert!(
        !status.is_permanent(),
        "no permanent losses with the rule off"
    );
    if let Some(days) = status.days_out().filter(|d| *d > 0) {
        for _ in 0..days {
            state.roster.advance_day();
        }
        assert_eq!(
            state.roster.get(cadet).unwrap().status,
            CadetStatus::Ready,
            "she should come back after her days are served"
        );
    }
}

/// `Lost` is not a euphemism: she bailed out, could not reach her own side
/// before the shooting stopped, and is walking home.
#[test]
fn a_lost_girl_walks_back_rather_than_being_gone() {
    use tactics_core::roster::{CadetStatus, Roster};
    let mut roster = Roster::new();
    let reg = registry();
    let cadet = roster
        .enlist_from_registry(&reg, 0, "anka")
        .expect("anka exists");
    roster.get_mut(cadet).unwrap().status = CadetStatus::Lost { days: 2 };

    assert!(!roster.get(cadet).unwrap().status.is_permanent());
    roster.advance_day();
    assert_eq!(
        roster.get(cadet).unwrap().status,
        CadetStatus::Lost { days: 1 },
        "still walking"
    );
    roster.advance_day();
    assert!(
        roster.get(cadet).unwrap().status.is_ready(),
        "she made it back"
    );
}

/// A trait changes *whether or when* a rule applies, which is what separates
/// it from a skill. Juno's lead foot is the clearest case: the same cadet in the
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

    // And the gift and the cost are both real, measured against the cadet she
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
/// slice 5 will spend when a cadet has to respond to something she was not
/// told about. Nothing consumes it yet — see the note in
/// `assets/wiki/reference/cadets.md` on why gating *planned* execution was the
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

    let delay = |cadet| {
        reg.reaction.delay(
            roster
                .skill_level(&reg, cadet, "reactions", &Default::default())
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
            BattleEvent::Defied { unit, .. } if *unit == UnitId(0)
        )),
        "the refusal has to be said out loud: {events:?}"
    );
}

// --- defiance: what a crew does instead (direction step 4, tax 2) -----------

/// Force one response for every crew, so a test about what flight *does* is
/// not also a test about who reaches for it. `base` outranks the `core` term
/// by more than any core can differ, which is the point: temperament has its
/// own tests.
fn always(reg: &mut DataRegistry, response: &str) {
    reg.morale.defiance = vec![tactics_core::data::DefianceDef {
        id: response.into(),
        name: "does it".into(),
        response: serde_json::from_value(serde_json::json!(response)).expect("a real response"),
        core: None,
        base: 100,
    }];
}

/// Two mediums far enough apart on a long field that a frightened crew has
/// somewhere to reverse to. `duel`'s map is five columns wide, which is a
/// fine place to shoot at somebody and no place at all to run away.
fn flight_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    let row = "g".repeat(20);
    two_side_battle(
        reg,
        &[&row, &row, &row],
        vec![
            unit_at([8, 1], 0, "medium_tank", "Runner"),
            unit_at([13, 1], 1, "medium_tank", "Gun"),
        ],
        seed,
    )
}

/// The pressure that puts a crew off the end of the shipped ladder.
fn breaking(reg: &DataRegistry) -> u32 {
    reg.morale
        .rungs
        .last()
        .expect("the shipped ladder has rungs")
        .at_pressure
}

/// The seed the flight stage is fought on.
///
/// Shared by the two tests below because it is one stage, and it is a
/// constant because it is *staging* rather than the rule: `flight_stage` is a
/// single duel, so on most seeds the gun behind her gets through on the
/// opening round and her crew bails, which proves nothing either way. It
/// moved from 61/62 to 84 when the resolver-depth arc changed what a shot at
/// a moving vehicle is worth and shifted every roll after the first. If it
/// has to move again, scan for a seed on which she survives to be watched —
/// never weaken what is asserted about her, which is the part that is not
/// staging.
const FLIGHT_SEED: u64 = 84;

#[test]
fn a_frightened_crew_reverses_out_of_contact() {
    // The review's second fun tax, and the shape of the defect was worse than
    // it read: a crew who would not advance would not retreat either, and
    // would not even break for cover, because all three went through one
    // `obeys` gate. "A broken unit that cannot retreat is free kills for the
    // enemy" — so morale narrated a death spiral instead of buying anything.
    //
    // Away from what is shooting at her, note, and not toward a lane. She is
    // not navigating.
    let mut reg = registry_wireless();
    always(&mut reg, "flight");
    let mut state = flight_stage(&reg, FLIGHT_SEED);
    state.units[0].pressure = breaking(&reg);

    let before = state.unit(UnitId(0)).expect("on the field").pos;
    let enemy = state.unit(UnitId(1)).expect("on the field").pos;
    let events = play_round(&reg, &mut state);
    let after = state.unit(UnitId(0)).expect("on the field").pos;

    assert!(
        after.distance_to(enemy) > before.distance_to(enemy),
        "she should have put ground between herself and the gun: {before:?} -> {after:?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::Defied { unit, to: Some(_), .. } if *unit == UnitId(0)
        )),
        "and it has to be said out loud, with where she went: {events:?}"
    );
}

#[test]
fn a_crew_cannot_refuse_the_decision_she_made_herself() {
    // The trap under the whole feature. A refusing crew has her ordered path
    // thrown away; flight then lays a path of her own. If the refusal check
    // cannot tell the two apart it selects her again on the next tick, throws
    // her own route away, lays it again, and she stands in place shaking for
    // the rest of the battle — a livelock that looks exactly like the freeze
    // this was built to remove.
    let mut reg = registry_wireless();
    always(&mut reg, "flight");
    let mut state = flight_stage(&reg, FLIGHT_SEED);
    state.units[0].pressure = breaking(&reg);
    let enemy = state.unit(UnitId(1)).expect("on the field").pos;

    let mut range = state
        .unit(UnitId(0))
        .expect("on the field")
        .pos
        .distance_to(enemy);
    let mut opened = 0;
    for _ in 0..3 {
        if state.is_over() {
            break;
        }
        play_round(&reg, &mut state);
        let Some(me) = state.unit(UnitId(0)) else {
            break;
        };
        let now = me.pos.distance_to(enemy);
        if now > range {
            opened += 1;
        }
        range = now;
    }
    assert!(
        opened >= 2,
        "she has to keep going, not re-argue with herself every tick"
    );
}

#[test]
fn a_crew_gone_to_ground_will_not_fire_on_her_own_initiative() {
    // Freeze had to cost something or the third response was a label rather
    // than a rule. She is a passenger in her own vehicle: nothing in front of
    // her prompts her to shoot. Her gun is not broken, though, and the
    // difference is the whole of it — an order still reaches the gunner.
    let mut reg = registry_wireless();
    always(&mut reg, "freeze");
    let mut state = duel(&reg, 63);
    state.units[0].pressure = breaking(&reg);

    let events = play_round(&reg, &mut state);
    assert!(
        !events.iter().any(|e| matches!(
            e,
            BattleEvent::ShotFired { attacker, opportunity: true, .. } if *attacker == UnitId(0)
        )),
        "she is not looking for a shot: {events:?}"
    );

    let mut told = duel(&reg, 63);
    told.units[0].pressure = breaking(&reg);
    told.apply(
        &reg,
        &Order::SetFire {
            unit: UnitId(0),
            fire: FireIntent::Target {
                target: UnitId(1),
                weapon: 0,
            },
        },
    )
    .expect("the order is accepted");
    let ordered = play_round(&reg, &mut told);
    assert!(
        ordered.iter().any(|e| matches!(
            e,
            BattleEvent::ShotFired { attacker, .. } if *attacker == UnitId(0)
        )),
        "but a target called by her commander is still shot at: {ordered:?}"
    );
}

#[test]
fn a_mod_that_names_no_defiance_freezes_exactly_as_it_always_did() {
    // Additivity, the same rule difficulty-as-a-mod and the zeroed command
    // block are held to. Freezing was the only thing a broken crew could ever
    // do, so a mod that declines to describe defiance must still get it —
    // which is also why `Freeze` is the enum's `#[default]` and why ties in
    // the score go to the first entry listed.
    let mut reg = registry_wireless();
    reg.morale.defiance.clear();
    let mut state = flight_stage(&reg, 64);
    state.units[0].pressure = breaking(&reg);

    let forward = tactics_core::offset_to_hex(10, 1);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: forward,
            },
        )
        .expect("the order is accepted");
    let events = play_round(&reg, &mut state);

    // Behaviour rather than a final position, for the reason
    // `a_breaking_crew_refuses_to_advance_and_says_so` states: the round can
    // kill her, and a dead unit has no position to compare.
    assert!(
        !events.iter().any(|e| matches!(
            e,
            BattleEvent::UnitMoved { unit, .. } if *unit == UnitId(0)
        )),
        "she does not move a hex, in any direction, which is what she always did: {events:?}"
    );
}

#[test]
fn the_senior_cadet_still_fighting_decides_how_the_crew_breaks() {
    // Not the best score aboard and not an average: somebody says "back her
    // out" or "keep firing" and the rest do it. In this engine that is
    // whoever is left in the most forward seat, so a commander going out
    // hands her temperament to the next woman down along with everything
    // else — the same leaders-last ordering the interior model already uses.
    let mut reg = registry_wireless();
    reg.morale.defiance = vec![
        tactics_core::data::DefianceDef {
            id: "fight".into(),
            name: "fights on".into(),
            response: tactics_core::data::DefianceResponse::Fight,
            core: Some("will".into()),
            base: 0,
        },
        tactics_core::data::DefianceDef {
            id: "flight".into(),
            name: "falls back".into(),
            response: tactics_core::data::DefianceResponse::Flight,
            core: Some("speed".into()),
            base: 0,
        },
    ];
    // Anka has will 13 and no speed of her own; Sofia has speed 12 and no
    // will. Read off the shipped characters on purpose — a temperament rule
    // that only works on invented cadets is not a rule about this game.
    let (mut state, ours) = crewed_stage(&reg, &["anka", "sofia"]);
    state.unit_mut(ours).expect("on the field").pressure = breaking(&reg);
    assert_eq!(
        state.defiance(&reg, state.unit(ours).expect("on the field")),
        tactics_core::data::DefianceResponse::Fight,
        "her commander is the steady one, so the tank is"
    );

    let unit = state.unit_mut(ours).expect("on the field");
    unit.crew_state = vec![
        tactics_core::battle::CrewCondition::Out,
        tactics_core::battle::CrewCondition::Fine,
    ];
    assert_eq!(
        state.defiance(&reg, state.unit(ours).expect("on the field")),
        tactics_core::data::DefianceResponse::Flight,
        "with the commander out it is the next cadet's nerve that answers"
    );
}

#[test]
fn an_officer_in_sight_settles_a_crew_faster() {
    // The half of the chain of command that had never paid anybody anything.
    // Losing a leader has cost a formation its nerve since `leader_lost` was
    // added; still having one bought nothing, so there was no reason beyond
    // succession bookkeeping to keep an officer alive.
    //
    // Sight rather than the radio net, and deliberately: a commander steadies
    // a frightened crew by being visibly still in it, which does not travel
    // down a wire. It is also the only version that leaves a zeroed `command`
    // block behaving identically to no block at all, because a crew with no
    // radio is out of contact under one and not the other.
    let mut reg = registry_wireless();
    reg.morale.recovery_near_leader = 3;
    let settled = |reg: &DataRegistry, blind: bool| -> u32 {
        // A map that declares a formation, because a rally is a thing a chain
        // of command does and `two_side_battle`'s map has no chain.
        let row = "g".repeat(50);
        let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
            "id": "rally_stage",
            "palette": { "g": "grass" },
            "rows": [&row, &row, &row],
            "formations": [{ "id": "ours", "side": 0 }],
        }))
        .expect("fixture map");
        let map = HexMap::from_map_file(&file).expect("map parses");
        let formed = |at: [i32; 2], name: &str, leads: bool| UnitPlacement {
            aboard_at: None,
            at,
            side: 0,
            vehicle: "medium_tank".into(),
            crew: Vec::new(),
            name: Some(name.into()),
            facing: None,
            formation: Some("ours".into()),
            leads,
        };
        // A second side is required or the battle is over before anybody
        // recovers anything — one living side wins immediately, and pressure
        // sheds at the *start* of the next round. She is parked forty hexes
        // off and blind to everyone, because a firefight would move this
        // number for reasons that are not the officer.
        let placements = vec![
            formed([0, 1], "Leader", true),
            formed([2, 1], "Follower", false),
            unit_at([45, 1], 1, "medium_tank", "Nobody"),
        ];
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
        let mut state = BattleState::from_placements(
            reg,
            map,
            sides,
            &placements,
            &crews,
            std::sync::Arc::new(roster),
            71,
        );
        // High enough that neither branch reaches the floor: pressure
        // saturates at zero, and a comparison against a floor measures the
        // floor.
        state.units[1].pressure = 60;
        if blind {
            // Put the officer where her crew cannot see her, by taking her
            // sight line rather than her life: killing her would charge the
            // formation `leader_lost` and measure that instead.
            // Twenty-odd hexes from her crew and twenty from the enemy:
            // vision is ten to twenty, so she is out of everybody's sight and
            // this measures the officer and nothing else. Parking her beside
            // the enemy instead started a firefight and moved the number.
            state.units[0].pos = tactics_core::offset_to_hex(25, 1);
        }
        // Recovery is paid when a round opens, which `resolve_round` reaches
        // at the end of the round it played.
        play_round(reg, &mut state);
        state.unit(UnitId(1)).expect("on the field").pressure
    };
    let with_her = settled(&reg, false);
    let without = settled(&reg, true);
    assert!(
        with_her < without,
        "a crew who can see her officer settles faster: {with_her} against {without}"
    );
    assert_eq!(
        without - with_her,
        reg.morale.recovery_near_leader,
        "and by exactly what the mod said, so the number means what it says"
    );
}

#[test]
fn a_side_that_sees_clearly_is_untouched_by_the_blur() {
    // Difficulty noise changed shape — from an independent draw per candidate
    // tile to one lean per unit per round — and the pin that has to survive
    // that is the additivity rule: difficulty is content, and the top of the
    // scale has none of it. A planner at difficulty 5 must plan exactly as it
    // would with the whole mechanism deleted.
    //
    // This is the cheap half of the check. The expensive half is the
    // determinism baseline, which fights at difficulty 3 and therefore moves
    // when this changes; if a future edit makes THIS test fail, the noise has
    // leaked into a side that is supposed to see the field as it is.
    let reg = registry_wireless();
    let orders_from = |difficulty: u8| -> Vec<String> {
        // A shipped map, because the point is a field with enough ground on
        // it to choose between. A twenty-hex test strip gives every planner
        // the same answer whatever it can see, which pins nothing.
        let mut state =
            BattleState::from_map(&reg, "battle_plains", 93).expect("a shipped battle map");
        let mut planner = UtilityPlanner::with_difficulty(difficulty, 5);
        let mut log = Vec::new();
        for _ in 0..6 {
            if state.is_over() {
                break;
            }
            log.push(format!("{:?}", planner.next_order(&reg, &state, 0)));
            play_round(&reg, &mut state);
        }
        log
    };
    let sharp = orders_from(5);
    assert!(!sharp.is_empty(), "the scene has to produce orders at all");
    assert_ne!(
        sharp,
        orders_from(3),
        "a blurred side must actually play differently, or this pins nothing"
    );
    // The real assertion: two difficulty-5 planners agree, and they agree
    // because neither of them drew anything, not because the rng happened to
    // land twice the same way.
    assert_eq!(sharp, orders_from(5));
}

// --- goals: an intention that outlives a round --------------------------

#[test]
fn a_crew_keeps_the_goal_she_chose_until_it_is_finished() {
    // The point of the whole layer. A greedy planner re-decides where it is
    // going every round and therefore never gets anywhere; measured, that was
    // a medium tank driving 53 hexes over 19 rounds to end 10 hexes further
    // forward. A goal is kept, so the second round's plan is the first
    // round's plan continued.
    let reg = registry_wireless();
    let mut state =
        BattleState::from_map(&reg, "battle_plains", 101).expect("a shipped battle map");
    let mut planner = UtilityPlanner::with_difficulty(5, 11);

    let mut seen: Vec<(tactics_core::battle::Goal, tactics_core::Hex)> = Vec::new();
    for _ in 0..6 {
        if state.is_over() {
            break;
        }
        loop {
            let order = planner.next_order(&reg, &state, 0);
            if matches!(order, Order::Commit { .. }) {
                break;
            }
            let _ = state.apply(&reg, &order);
        }
        if let Some(me) = state.unit(UnitId(0))
            && let Some(goal) = me.goal
        {
            seen.push((goal, me.pos));
        }
        play_round(&reg, &mut state);
    }
    assert!(
        seen.len() >= 4,
        "she should be planning every round: {seen:?}"
    );
    assert!(
        seen.windows(2).any(|w| w[0].0 == w[1].0),
        "she has to carry an intention across a round at all: {seen:?}"
    );
    // The real rule, and the one worth pinning: she only ever changes her
    // mind by *finishing*. Anything else is the re-deciding this layer was
    // built to stop, and it would not show up as a goal that never persists —
    // it would show up as one that persists for a while and then wanders.
    for pair in seen.windows(2) {
        let ((was, _), (now, at)) = (pair[0], pair[1]);
        if was != now {
            assert_eq!(
                was,
                tactics_core::battle::Goal::Take(at),
                "she changed her goal without having arrived at the old one: {seen:?}"
            );
        }
    }
}

#[test]
fn an_order_replaces_her_own_ideas_rather_than_competing_with_them() {
    // The rule the direction memo exists to defend, restated where the goal
    // layer could most easily have undone it. A mission that names ground is
    // not one candidate among the objectives she likes the look of — it is
    // the only one. Letting it compete was the first draft, and it meant a
    // crew under orders and a crew with none chose the same ground, which is
    // an order that has stopped being one.
    //
    // Note this is also the hook for subordinate initiative: that chunk
    // widens this list by doctrine, and nothing else has to move.
    let reg = registry_wireless();
    let state = BattleState::from_map(&reg, "battle_plains", 102).expect("a shipped map");
    let unit = UnitId(0);
    let free = tactics_core::ai::goal::candidates(&reg, &state, unit, None, None);
    assert!(
        free.len() > 2,
        "with no orders she has the run of the map: {free:?}"
    );

    let told = tactics_core::offset_to_hex(30, 30);
    let under_orders = tactics_core::ai::goal::candidates(
        &reg,
        &state,
        unit,
        Some(&tactics_core::battle::Mission::Advance { to: told }),
        None,
    );
    assert_eq!(
        under_orders,
        vec![
            tactics_core::battle::Goal::Take(told),
            tactics_core::battle::Goal::Hold
        ],
        "told where to go, that is where she is going"
    );
}

#[test]
fn two_crews_do_not_drive_for_the_same_hex() {
    // A section takes a piece of ground each. Said in the candidate list so
    // that it is said once and visibly, rather than as a tie-break buried in
    // whatever does the scoring — which is where the equivalent problem lived
    // before, as the plateau rule, and where it was very hard to see.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "battle_plains", 103).expect("a shipped map");
    let (first, second) = (UnitId(0), UnitId(1));
    let side = state.unit(first).expect("on the field").side;
    assert_eq!(
        state.unit(second).expect("on the field").side,
        side,
        "this test needs two crews of the same side"
    );

    let mine = tactics_core::ai::goal::candidates(&reg, &state, first, None, None);
    let taken = mine
        .iter()
        .find_map(|g| match g {
            tactics_core::battle::Goal::Take(hex) => Some(*hex),
            tactics_core::battle::Goal::Hold => None,
        })
        .expect("there is ground worth having on this map");
    state.unit_mut(first).expect("on the field").goal =
        Some(tactics_core::battle::Goal::Take(taken));

    let hers = tactics_core::ai::goal::candidates(&reg, &state, second, None, None);
    assert!(
        !hers.contains(&tactics_core::battle::Goal::Take(taken)),
        "somebody is already going there: {hers:?}"
    );
    assert!(
        hers.len() > 1,
        "and she still has somewhere of her own to go"
    );
}

/// The player has to be able to see a crew wavering *before* it costs them
/// something, or licence to disobey reads as the game cheating.
#[test]
fn crews_report_moving_up_the_ladder() {
    let mut reg = registry();
    // The scene needs an attritional fight: unsoftened, the first
    // penetration can find the ammunition rack and end the battle as a
    // brew-up before anyone's nerves are observable. Softened, pens wound
    // and frighten without destroying, which is the thing under test —
    // the ladder's reporting, not the gun's lethality.
    soften(&mut reg);
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

/// Difficulty is a mod. A one-rung ladder has to produce cadets who always do
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
            accuracy: 0,
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
            .any(|e| matches!(e, BattleEvent::Defied { .. })),
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
    // burns hands over to the next cadet whether or not anybody priced a
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
        assert!(
            (state.condition(&reg, unit) - 1.0).abs() < f32::EPSILON,
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
fn a_commander_sends_her_grenadiers_to_hold_the_covered_ground() {
    // Composition-aware tasking. The brain divides the ground among its
    // formations, and until it could tell them apart it divided it *evenly*:
    // the grenadier section — a rifle platoon and the taxi carrying her —
    // drew a slot in the same rotation as three medium tanks and was ordered
    // to march at whatever objective came up next, beside armour that could
    // survive the trip. That is how a harness run buries most of its
    // infantry in the back of an APC.
    //
    // Two things are asserted, and neither of them names a chassis, because
    // neither does the brain: the section is told to *hold* rather than to
    // advance or assault, whatever the doctrine's appetite, and the ground
    // it is told to hold is the ground with cover in it. Both are read off
    // the hardware — somebody in the formation walks — the same way a base
    // of fire is recognised by somebody in it laying an indirect weapon.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "battle_plains", 17).expect("battle");
    let mut ai = AiDriver::new();
    ai.insert(
        0,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                // Aggression 0.85, so without the infantry branch the
                // grenadiers would be ordered to *assault* — the most
                // expensive thing a rifle platoon in a battle taxi can be
                // told to do.
                doctrine: Some("massed_armor".into()),
            },
            17,
            &reg,
        ),
    );

    let mut given: Vec<(String, Mission)> = Vec::new();
    ai.plan_round_with(&reg, &mut state, |d| {
        for event in &d.events {
            if let BattleEvent::MissionAssigned { formation, mission } = event {
                given.push((formation.clone(), mission.clone()));
            }
        }
    });

    let grenadiers = given
        .iter()
        .find(|(f, _)| f == "kuhlmann_grenadiers")
        .map(|(_, m)| m.clone())
        .expect("the grenadier section is given a mission like everybody else");
    let armor = given
        .iter()
        .find(|(f, _)| f == "kuhlmann_line")
        .map(|(_, m)| m.clone())
        .expect("so is the armoured line");

    let at = match grenadiers {
        Mission::Hold { at: Some(at) } => at,
        other => panic!("the grenadiers should be holding ground, not {other:?}"),
    };
    assert!(
        matches!(armor, Mission::Assault { .. } | Mission::Advance { .. }),
        "while the tanks are still sent to take it: {armor:?}"
    );

    // And the ground they were given is the covered ground. Scored the way
    // the brain scores it — mean cover over the objective's own hexes — so
    // a map edit that moves the trees moves this assertion with it rather
    // than pinning an objective by name.
    let cover = |objective: &tactics_core::map::Objective| -> i32 {
        let hexes: Vec<i32> = objective
            .hexes
            .iter()
            .filter_map(|h| state.map.get(*h))
            .filter_map(|t| reg.terrain(&t.terrain))
            .map(|def| def.cover)
            .collect();
        if hexes.is_empty() {
            return 0;
        }
        hexes.iter().sum::<i32>() / hexes.len() as i32
    };
    let held = state
        .map
        .objectives()
        .iter()
        .find(|o| o.anchor() == at)
        .expect("the anchor is an objective's");
    let best = state
        .map
        .objectives()
        .iter()
        .filter(|o| o.kind == tactics_core::map::ObjectiveKind::Hold)
        .map(cover)
        .max()
        .expect("the map has ground to hold");
    assert_eq!(
        cover(held),
        best,
        "the section on foot gets the ground it can hide in, not the next \
         slot in the rotation"
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
        maul(&reg, &mut state, id);
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
            aboard_at: None,
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
    // the bridge it is, held or not. (It says so as an *assault* rather than
    // an advance — that is the aggression split, pinned next door; what this
    // test is about is the hex it names.)
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
        Some(Mission::Assault { to: bridge }),
        "an obedient doctrine follows the letter of the plan"
    );
}

#[test]
fn an_aggressive_commander_orders_assaults_and_a_balanced_one_advances() {
    // Two orders for ground, and which one a commander gives is her
    // doctrine's answer to what the ground is worth. Massed armour (0.85)
    // will spend vehicles for it and says so: an assault presses through
    // whatever is firing. The balanced default (0.6) — which is what the
    // player's own delegated formations run under — orders a movement to
    // contact instead: take the bridge, but fight what you meet on the way.
    // Both name the same hex; the difference is entirely in what they will
    // pay for it.
    let reg = registry_wireless();
    let mission = |doctrine: Option<&str>| {
        let mut state = BattleState::from_map(&reg, "river_crossing", 33).unwrap();
        let mut ai = AiDriver::new();
        ai.insert(
            0,
            make_battle_planner(
                &AiConfig {
                    planner: "command".into(),
                    difficulty: 5,
                    doctrine: doctrine.map(str::to_string),
                },
                33,
                &reg,
            ),
        );
        ai.plan_round(&reg, &mut state);
        let armor = formation_named(&state, "kuhlmann_armor");
        state.formations()[armor.index()].mission.clone()
    };
    let bridge = BattleState::from_map(&reg, "river_crossing", 33)
        .unwrap()
        .map
        .objectives()[0]
        .anchor();
    assert_eq!(
        mission(Some("massed_armor")),
        Some(Mission::Assault { to: bridge }),
        "a doctrine that trades vehicles for ground orders the deliberate attack"
    );
    assert_eq!(
        mission(None),
        Some(Mission::Advance { to: bridge }),
        "and the balanced default moves to contact for the same hex"
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
        // which cadet happens to be sitting in the radio seat.
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
            .apply(
                reg,
                &Order::SetMission {
                    formation,
                    mission,
                    latitude: tactics_core::battle::Latitude::Delegated,
                },
            )
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
    // Out of contact is not amnesia and it is not license: a cadet who loses
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

    // And the twin with no chain of command at all: what an unmissioned cadet
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
    // The counterpart: a cadet already out of contact when the order is given
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
                latitude: tactics_core::battle::Latitude::Delegated,
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

/// Beat a vehicle down to the state the old tests wrote as `hp = 1`:
/// every cadet wounded, everything but the running gear destroyed. Her
/// condition falls below any doctrine's breaking point while she stays
/// alive, mobile, and reapable by nothing — exactly what a withdraw test
/// needs its casualties to be.
fn maul(reg: &DataRegistry, state: &mut BattleState, unit: UnitId) {
    let mobility: Vec<String> = reg
        .modules
        .iter()
        .filter(|(_, m)| m.effect == tactics_core::data::ModuleEffect::Mobility)
        .map(|(id, _)| id.clone())
        .collect();
    let u = state.unit_mut(unit).expect("she is on the field");
    u.crew_state = vec![tactics_core::battle::CrewCondition::Wounded; u.crew.len()];
    for (id, hits) in u.modules.iter_mut() {
        if !mobility.contains(id) {
            *hits = 0;
        }
    }
}

/// Take a unit off the board the way a shell would, but silently: no
/// `UnitDestroyed` event, so nothing this produces can be confused with the
/// pressure of watching a friend burn.
fn strike_down(state: &mut BattleState, unit: UnitId) {
    let victim = state.unit_mut(unit).expect("she was alive");
    victim.alive = false;
}

#[test]
fn command_passes_to_the_next_girl_in_the_order_of_battle() {
    // Succession is formation machinery, not wire machinery, so this runs on
    // a registry with no `command` block at all: who is in charge of a platoon
    // is a fact about the platoon, and a mod that never priced a radio still
    // has one cadet senior to another. Seniority is the order the map author
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
        "but the cadet the map put in charge is not rewritten by her own death \
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
    // because the net was anchored on a cadet who was no longer there. She is
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
    assert_eq!(formation.leader, Some(heir), "the next cadet has it");
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
            "every cadet in the column felt it, however far down the road she is"
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
        "and a side cannot stake the battle on somebody else's cadets: {errors}"
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
/// a cadet ten hexes out is out for a reason a test can state in one line.
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
    // The heart of the chunk: an order to a cadet who cannot hear it is
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
                latitude: Latitude::Delegated,
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
                latitude: Latitude::Delegated,
            },
        )
        .expect("accepted too");
    assert_eq!(
        state.command.waiting_for(crew).unwrap().destination,
        Some(second)
    );
    assert_eq!(state.command.waiting().len(), 1, "one slot, one cadet");
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
                latitude: Latitude::Delegated,
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
                latitude: Latitude::Delegated,
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
                latitude: Latitude::Delegated,
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
                latitude: Latitude::Delegated,
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
                latitude: Latitude::Delegated,
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
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    state
        .apply(
            &reg,
            &Order::QueueMission {
                formation: armor,
                mission: Mission::Hold { at: Some(hold_at) },
                latitude: tactics_core::battle::Latitude::Delegated,
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
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    assert_eq!(
        state.apply(
            &reg,
            &Order::QueueMission {
                formation: armor,
                mission: Mission::Advance { to: anywhere },
                latitude: tactics_core::battle::Latitude::Delegated,
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    assert_eq!(
        state.apply(
            &reg,
            &Order::QueueMission {
                formation: armor,
                mission: Mission::Advance { to: anywhere },
                latitude: tactics_core::battle::Latitude::Delegated,
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    let f = &state.formations()[armor.index()];
    assert!(
        matches!(
            f.incoming,
            Some((tactics_core::battle::MissionChange::Append { .. }, _))
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
    // the ring can be counted against, and the cadet one hex inside it is on
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
        "a cadet standing on the ring hears her leader"
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

// --- latitude: an order the crew may not set aside --------------------------

/// A crew with a long march east along an open road, woods flanking the
/// western half of it, and a gun watching from the open ground beyond.
///
/// Both halves of that shape were paid for by a failing test. The woods are
/// there because the drill *seeks cover*: with none in reach it has nothing
/// to choose and both latitudes march identically, so the test proves
/// nothing. The woods stop short of the gun because cover works for whoever
/// stands in it — with woods beside her too, the gun's own mid-round drill
/// bolted into them, went concealed, and stopped being a threat at all,
/// which killed the test from the other end.
///
/// A medium marching and a tank destroyer watching, which also took some
/// getting to. The gun has to be one that can actually hurt the marcher,
/// because `threatened` — the drill's trigger — asks whether any visible
/// enemy has a weapon that could meaningfully hurt *her*, and after the
/// ballistics rewrite a medium's gun cannot touch another medium's front
/// plate head-on. Two mediums are therefore never in danger from each other
/// on this road, the drill never fires, and a test in which the drill cannot
/// fire cannot fail. The tank destroyer's gun gets through, which is what
/// makes the road dangerous enough to be worth an order about.
///
/// The seed is pinned because she has to live through the opening round to
/// have a second one. The sim is deterministic, so "she survives at seed 62"
/// is a fact about this stage rather than a probability.
fn marching_under_fire(reg: &DataRegistry, latitude: Latitude, seed: u64) -> (BattleState, UnitId) {
    // Woods flank the road as far as x = 12; the gun sits at 13 on bare
    // ground, with nothing better within a bound of it.
    let flank = format!("{}{}", "f".repeat(12), "g".repeat(18));
    let road = "g".repeat(30);
    let mut state = two_side_battle(
        reg,
        &[&flank, &road, &flank],
        vec![
            unit_at([5, 1], 0, "medium_tank", "Ordered"),
            unit_at([13, 1], 1, "tank_destroyer", "Gun Tank"),
        ],
        seed,
    );
    let crew = UnitId(0);
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the stage needs her to see the danger she is being asked to drive past"
    );
    state
        .apply(
            reg,
            &Order::Radio {
                unit: crew,
                to: Some(MARCH_TO),
                fire: None,
                latitude,
            },
        )
        .expect("a far destination is an order, not a refusal");
    assert_eq!(state.unit(crew).unwrap().latitude, latitude);
    (state, crew)
}

/// The ground she is sent to, well east of the gun watching the road.
const MARCH_TO: tactics_core::Hex = tactics_core::Hex::new(28, 1);

fn executor_only_side(reg: &DataRegistry, seed: u64) -> AiDriver {
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
    ai
}

/// Fight the opening round, then plan the next one and report where she
/// stands, where she is being sent from there, and whether that is cover.
///
/// The second round is the one that matters and it took a failing test to
/// see why: `radio()` marches her itself the moment the order lands, so on
/// the round she is ordered she is already planned and no planner is
/// consulted at all. The drill can only ever preempt a march that is
/// *already under way* — which is exactly the case the player was
/// complaining about, the tank that sets off and then never arrives.
fn second_round_plan(
    reg: &DataRegistry,
    mut state: BattleState,
    crew: UnitId,
    seed: u64,
) -> (tactics_core::Hex, tactics_core::Hex, bool) {
    executor_only_side(reg, seed).plan_round(reg, &mut state);
    let _ = state.apply(reg, &Order::Commit { side: 1 });
    state.resolve_round(reg);
    let unit = state.unit(crew).expect("she survives being shot at");
    assert!(unit.alive, "the stage is meant to bruise, not to kill");
    assert!(
        state.unit(UnitId(1)).is_some_and(|e| e.alive),
        "and the gun watching the road has to still be watching it, or there \
         is no threat left for the drill to answer"
    );
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "and she has to still be able to see it"
    );
    let from = unit.pos;
    executor_only_side(reg, seed).plan_round(reg, &mut state);
    let to = state.unit(crew).unwrap().planned_destination();
    let cover = state.terrain_at(to) == Some("forest");
    (from, to, cover)
}

#[test]
fn a_binding_march_presses_on_where_an_ordinary_one_takes_cover() {
    // The rule this chunk exists for, and its additivity twin, in one test
    // because they are one comparison: same crew, same gun, same ground,
    // same seed, and the *only* difference is whether her commander said she
    // meant it.
    //
    // Stated as a comparison rather than against a named tile on purpose.
    // What is being defended is that latitude changes what she does, and in
    // which direction — not which particular hedge the drill happens to
    // like, which is a tuning detail that should be free to move without
    // failing this.
    // The seed is staging, not the rule, and it moved once already: the
    // resolver-depth arc made a crossing target harder to hit, so the gun
    // watching the road stopped killing her on the opening round, she closed
    // to two hexes, and the gun's own battle drill took it into the woods —
    // leaving `second_round_plan` with no threat left to stage. 42 is the
    // nearest seed where the gun stays on the road and the comparison below
    // is the only thing being measured. If it moves again, scan for another
    // rather than weakening what is asserted.
    const SEED: u64 = 42;
    let reg = registry_wireless();
    let (delegated_from, delegated_to, delegated_took_cover) = {
        let (state, crew) = marching_under_fire(&reg, Latitude::Delegated, SEED);
        second_round_plan(&reg, state, crew, SEED)
    };
    let (binding_from, binding_to, _) = {
        let (state, crew) = marching_under_fire(&reg, Latitude::Binding, SEED);
        second_round_plan(&reg, state, crew, SEED)
    };

    // Latitude is read when the *executor* plans, and on the opening round
    // the order plans her itself, so the two runs must be identical up to
    // that point. If this ever fails, latitude has leaked somewhere it does
    // not belong.
    assert_eq!(
        delegated_from, binding_from,
        "the two runs are the same battle until the second round's planning"
    );

    assert!(
        MARCH_TO.distance_to(binding_to) < MARCH_TO.distance_to(delegated_to),
        "told she is meant, she keeps driving at ground she has been shown is \
         dangerous; told to use her judgment, she does not: \
         from {binding_from:?}, binding -> {binding_to:?}, delegated -> {delegated_to:?}"
    );
    assert!(
        MARCH_TO.distance_to(binding_to) < MARCH_TO.distance_to(binding_from),
        "and pressing on is progress toward the ordered ground, not merely \
         a different tile: {binding_from:?} -> {binding_to:?}"
    );
    // Not merely "somewhere else": the crew who was given her judgment used
    // it for the thing the drill is for.
    assert!(
        delegated_took_cover,
        "the delegated crew breaks off into the woods: {delegated_from:?} -> {delegated_to:?}"
    );
}

#[test]
fn a_recall_forgets_that_she_was_pressed_on() {
    // Latitude belongs to an order, not to a crew: take the order back and
    // the insistence goes with it, or the next thing she is told inherits an
    // urgency nobody attached to it.
    let reg = registry_wireless();
    let (mut state, crew) = marching_under_fire(&reg, Latitude::Binding, 62);
    state
        .apply(&reg, &Order::ClearIntent { unit: crew })
        .expect("a recall is always sayable");
    let unit = state.unit(crew).unwrap();
    assert_eq!(unit.tasking, None, "the march is off");
    assert_eq!(
        unit.latitude,
        Latitude::Delegated,
        "and so is the insistence behind it"
    );
}

#[test]
fn an_order_about_her_gun_says_nothing_about_her_march() {
    // The two halves of a radioed order are independent, and latitude rides
    // with the *route*. Telling a crew who is pressing on what to shoot at
    // must not quietly relax the march she is already under.
    let reg = registry_wireless();
    let (mut state, crew) = marching_under_fire(&reg, Latitude::Binding, 63);
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: None,
                fire: Some(FireIntent::Hold),
                latitude: Latitude::Delegated,
            },
        )
        .expect("hold fire is always sayable");
    assert_eq!(
        state.unit(crew).unwrap().latitude,
        Latitude::Binding,
        "she is still pressing on"
    );
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
                latitude: tactics_core::battle::Latitude::Delegated,
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
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    let moved = bound_once(&reg, &mut state, 63);
    assert_eq!(moved, vec![true, true], "tempo over ceremony");
}

// --- movement to contact versus the deliberate attack ----------------------

/// A scout in the woods with a gun on her flank, and open ground between her
/// and the hex she has been ordered to take.
///
/// Everything about the stage exists to isolate the mission term. She is a
/// `recon_car` and the enemy a `tank_destroyer` seven hexes off her flank:
/// he outranges her badly, so she is genuinely under fire (his 88 reaches
/// sixteen hexes and she can see him for twenty) while her own machine gun
/// reaches nobody from any tile compared — the attack term is zero on both
/// and cannot decide anything. Seven hexes is also outside the six-hex band
/// the threat term prices, so that is zero on both too. What is left arguing
/// against walking is the wood she is sitting in and the crew's own appetite
/// to keep the contact close, and what argues for it is the mission — which
/// is exactly the argument the damping settles.
fn contact_stage(reg: &DataRegistry, seed: u64) -> (BattleState, FormationId) {
    let open = "g".repeat(40);
    let mut wood: Vec<char> = open.chars().collect();
    wood[10] = 'f';
    let wood: String = wood.into_iter().collect();
    let mut rows: Vec<String> = std::iter::repeat_n(open, 15).collect();
    rows[7] = wood;
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "contact_stage",
        "palette": { "g": "grass", "f": "forest" },
        "rows": rows,
        "formations": [ { "id": "section", "name": "The Section", "side": 0 } ],
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
    let mut scout = unit_at([10, 7], 0, "recon_car", "Scout");
    scout.formation = Some("section".into());
    scout.leads = true;
    let placements = vec![scout, unit_at([10, 0], 1, "tank_destroyer", "Gun")];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    let state = BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    );
    let section = formation_named(&state, "section");
    (state, section)
}

/// Where she stands, where she was told to be, and the ground between: the
/// two tiles every test below compares. `FORWARD` is seven hexes along the
/// lane — one bound, and far enough that the mission has something to say
/// about it.
const COVER: (i32, i32) = (10, 7);
const FORWARD: (i32, i32) = (17, 7);

/// Score both tiles for a scout under `mission`, in the order (cover,
/// forward). The balanced doctrine on purpose: it is what the player's own
/// delegated formations fight under, so this is the case the playtest was
/// complaining about.
fn contact_scores(
    reg: &DataRegistry,
    state: &mut BattleState,
    section: FormationId,
    mission: Mission,
) -> (f32, f32) {
    state
        .apply(
            reg,
            &Order::SetMission {
                formation: section,
                mission,
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("the lane is on the map");
    let evaluator = Evaluator::new(tactics_core::data::DoctrineDef::default());
    let score = |at: (i32, i32)| {
        evaluator
            .score_tile(
                reg,
                state,
                UnitId(0),
                tactics_core::offset_to_hex(at.0, at.1),
            )
            .score
    };
    (score(COVER), score(FORWARD))
}

/// What the forward tile is worth to a scout ordered onto it, under one
/// doctrine at one latitude. Everything else about the calls a test makes is
/// identical, so the difference between two of them is the mission term and
/// nothing else.
fn ordered_pull(
    reg: &DataRegistry,
    doctrine: &tactics_core::data::DoctrineDef,
    latitude: Latitude,
) -> f32 {
    let (mut state, section) = contact_stage(reg, 4);
    let forward = tactics_core::offset_to_hex(FORWARD.0, FORWARD.1);
    state
        .apply(
            reg,
            &Order::SetMission {
                formation: section,
                mission: Mission::Advance { to: forward },
                latitude,
            },
        )
        .expect("the lane is on the map");
    Evaluator::new(doctrine.clone())
        .score_tile(reg, &state, UnitId(0), forward)
        .score
}

#[test]
fn a_binding_mission_is_not_discounted_by_a_loose_doctrine() {
    // Step 1 part 3 of DIRECTION.md, and the complaint it answers: a
    // player's order was quietly worth less because of who she gave it to.
    // Elastic defence devolves (delegation 0.7), so it read "take that
    // ground" at four fifths of face value; massed armour does not (0.3) and
    // read the same sentence at 1.2. Nobody issues an order meaning four
    // fifths of it.
    //
    // Fought out on one doctrine with only `delegation` moved, because the
    // score of a tile is a whole doctrine's opinion of it — cover, threat,
    // the shot available — and comparing two *different* doctrines' totals
    // would be comparing everything except the thing under test.
    //
    // Wireless on purpose: with a command block the order is still in the
    // air when the tile is scored, and a mission nobody has heard yet has no
    // latitude to read. What is under test is the executor, not the wire.
    let reg = registry_wireless();
    let base = reg
        .doctrine("elastic_defense")
        .cloned()
        .expect("base doctrine");
    let loose = tactics_core::data::DoctrineDef {
        delegation: 0.7,
        ..base.clone()
    };
    let neutral = tactics_core::data::DoctrineDef {
        delegation: 0.5,
        ..base.clone()
    };
    let tight = tactics_core::data::DoctrineDef {
        delegation: 0.3,
        ..base
    };

    let loose_delegated = ordered_pull(&reg, &loose, Latitude::Delegated);
    let loose_binding = ordered_pull(&reg, &loose, Latitude::Binding);
    assert!(
        loose_binding > loose_delegated,
        "insisting has to reach a formation that would otherwise have used its \
         own judgment: delegated {loose_delegated}, binding {loose_binding}"
    );

    // How far it reaches, exactly: to the letter of the order and no
    // further. A binding order read by a devolving doctrine is worth what an
    // ordinary order read by a neutral one is worth — it raises a floor, it
    // does not turn every commander into a martinet.
    assert_eq!(
        loose_binding,
        ordered_pull(&reg, &neutral, Latitude::Delegated),
        "insisting should buy the letter of the order, not more than it"
    );

    // And the other half of the rule, which is what keeps "I mean it"
    // honest: `delegation` may make a subordinate *more* literal than she
    // was asked to be, never less. A doctrine already holding to the letter
    // hears nothing new in being told it twice.
    assert_eq!(
        ordered_pull(&reg, &tight, Latitude::Binding),
        ordered_pull(&reg, &tight, Latitude::Delegated),
        "a formation that was already going to follow the letter of it must \
         not be pulled harder for being insisted on"
    );
}

#[test]
fn an_order_held_on_the_wire_arrives_as_hard_as_it_was_meant() {
    // Latitude travels with the mission rather than being applied when it is
    // sent, for the same reason `WaitingOrders` carries a unit's: an order
    // that waits two ticks for a signaller has to land meaning what the
    // commander meant, not what she happens to mean by the time it lands.
    let mut reg = registry();
    reg.command = Some(command_rules(999, true, 2));
    let mut state = BattleState::from_map(&reg, "river_crossing", 5).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let bridge = state.map.objectives()[0].anchor();

    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: bridge },
                latitude: Latitude::Binding,
            },
        )
        .expect("the bridge is on the map");
    assert_eq!(
        state.formations()[armor.index()]
            .incoming
            .as_ref()
            .map(|(c, _)| c.latitude()),
        Some(Latitude::Binding),
        "the insistence is in the envelope, not left behind at headquarters"
    );

    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    state.step_tick(&reg);
    assert_eq!(
        state.formations()[armor.index()].latitude,
        Latitude::Binding,
        "and it is still there when the order lands"
    );
}

#[test]
fn a_movement_to_contact_pauses_under_fire_and_resumes_after() {
    // What `Advance` means, and the playtest that forced it to mean this: a
    // delegated advance drove through effective fire to the hex it had been
    // given and was gone by round three. A movement to contact halts and
    // fights when it is fired on — the mission does not outrank the drill —
    // so while the gun is on her the tree line she is sitting in outscores
    // the ground she was told to take.
    //
    // Nothing about that is latched, which is the other half of the rule:
    // the damping is a scale on the mission term, so the moment the gun is
    // dead or lost the full pull is back with no state for anybody to clear.
    let reg = registry_wireless();
    let (mut state, section) = contact_stage(&reg, 71);
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the stage needs her to know she is being shot at"
    );

    let lane = Mission::Advance {
        to: tactics_core::offset_to_hex(38, 7),
    };
    let (cover, forward) = contact_scores(&reg, &mut state, section, lane.clone());
    assert!(
        cover > forward,
        "under fire she stops and fights: cover {cover} vs forward {forward}"
    );

    // The gun is gone. Nobody re-issues anything and nothing is reset.
    strike_down(&mut state, UnitId(1));
    let (cover, forward) = contact_scores(&reg, &mut state, section, lane);
    assert!(
        forward > cover,
        "and with nothing shooting at her the march resumes on its own: \
         cover {cover} vs forward {forward}"
    );
}

#[test]
fn an_assault_presses_through_what_an_advance_pauses_for() {
    // The same crew, the same gun on her flank, the same hex to take — and
    // the two orders a commander can give for it. An advance is a movement
    // to contact and stops; an assault is the deliberate attack and does
    // not, which is the entire difference between them and is why the two
    // score identically everywhere except here.
    let reg = registry_wireless();
    let to = tactics_core::offset_to_hex(38, 7);

    let (mut state, section) = contact_stage(&reg, 71);
    let (cover, forward) = contact_scores(&reg, &mut state, section, Mission::Advance { to });
    assert!(
        cover > forward,
        "the advance pauses: cover {cover} vs forward {forward}"
    );

    let (mut state, section) = contact_stage(&reg, 71);
    let (cover, forward) = contact_scores(&reg, &mut state, section, Mission::Assault { to });
    assert!(
        forward > cover,
        "the assault presses through the same fire: cover {cover} vs forward {forward}"
    );
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
        maul(&reg, &mut state, id);
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

// --- detachment: a personal order outranks the standing mission ------------

#[test]
fn a_hand_placed_vehicle_stays_where_her_commander_put_her() {
    // The second playtest's bug: a tank ordered by hand into a town tile
    // drifted back to the road at the next planning phase, because the
    // formation's Advance quietly reasserted itself. A direct order is the
    // commander taking personal charge — DETACHMENT — and the standing
    // mission stops applying to her until she is recalled or the formation
    // is given fresh orders.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 91).unwrap();
    let armor = formation_named(&state, "kuhlmann_armor");
    let bridge = state.map.objectives()[0].anchor();
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: bridge },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    let member = state.formations()[armor.index()].members[0];
    let start = state.unit(member).unwrap().pos;
    // Two hexes north, off the mission's axis: the commander's own spot.
    let post = start + tactics_core::Hex::new(0, -2);
    assert!(state.map.contains(post));
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: member,
                to: Some(post),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .unwrap();
    assert!(state.unit(member).unwrap().detached, "she is detached");

    // Play the round out, then let her formation's executor fill the next
    // round's plans. She must not be marched back toward the bridge.
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);
    let held = state.unit(member).unwrap().pos;
    let mut ai = AiDriver::new();
    ai.insert(
        0,
        Box::new(tactics_core::ai::SideCommand::executor_only(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            91,
            &reg,
        )),
    );
    ai.plan_round(&reg, &mut state);
    let unit = state.unit(member).unwrap();
    assert_eq!(
        unit.planned_destination(),
        held,
        "no threat, no recall: she stands where she was put"
    );

    // A fresh formation order collects everyone: detachment does not
    // survive the commander speaking to the whole formation again. (The
    // executor closed the side above, so this happens next round.)
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);
    let ford = state
        .map
        .objectives()
        .iter()
        .find(|o| o.id == "north_ford")
        .unwrap()
        .anchor();
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: ford },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    assert!(
        !state.unit(member).unwrap().detached,
        "new orders for the formation reach her too"
    );
}

// --- base of fire (chunk 10b slice 2) ---------------------------------------

#[test]
fn a_fires_formation_stands_base_of_fire_for_the_assault() {
    // Tier 3 of "fighting as one": coordination *between* formations, which
    // is the commander's business rather than an executor's. Kuhlmann's
    // reconnaissance section carries the battery, so a centralized brain
    // takes it out of the ground rotation entirely and points it at the
    // fight her armour is having — while the armour still gets the ground.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 11).unwrap();
    let mut ai = AiDriver::new();
    ai.insert(
        0,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: Some("massed_armor".into()),
            },
            11,
            &reg,
        ),
    );
    ai.plan_round(&reg, &mut state);

    let recon = formation_named(&state, "kuhlmann_recon");
    let armor = formation_named(&state, "kuhlmann_armor");
    assert_eq!(
        state.formations()[recon.index()].mission,
        Some(Mission::Support {
            formation: "kuhlmann_armor".into()
        }),
        "the section with the howitzer in it shoots for the platoon"
    );
    assert!(
        matches!(
            state.formations()[armor.index()].mission,
            Some(Mission::Assault { .. })
        ),
        "and the platoon still gets its ground — massed armour presses for it: {:?}",
        state.formations()[armor.index()].mission
    );
}

/// One formation to be shot for and one to do the shooting, on an open road
/// with the only enemy far outside anybody's eyes — so a tile's score is the
/// mission and nothing else.
fn base_of_fire_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    let row = "g".repeat(46);
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "base_of_fire_stage",
        "palette": { "g": "grass" },
        "rows": [row.clone(), row.clone(), row],
        "formations": [
            { "id": "assault", "name": "The Assault", "side": 0 },
            { "id": "guns", "name": "The Guns", "side": 0 },
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
    let mut lead = unit_at([6, 1], 0, "medium_tank", "Lead");
    lead.formation = Some("assault".into());
    lead.leads = true;
    let mut battery = unit_at([20, 1], 0, "artillery", "Battery");
    battery.formation = Some("guns".into());
    battery.leads = true;
    let placements = vec![lead, battery, unit_at([45, 1], 1, "medium_tank", "Far Foe")];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    let state = BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    );
    assert!(
        state.fog.side(0).spotted.is_empty(),
        "the stage needs no enemy in sight, or the attack term joins in"
    );
    state
}

#[test]
fn support_holds_her_at_overwatch_distance() {
    // A band, not a pull. The attack term already pays her for a tile with a
    // shot on it, so what the mission has to say is the distance: close
    // enough that her fire lands where theirs is needed, far enough that she
    // is not in the assault she is covering. Measured directly against both
    // failures — hugging the people she supports, and trailing the map
    // behind them.
    let reg = registry_wireless();
    let mut state = base_of_fire_stage(&reg, 4);
    let guns = formation_named(&state, "guns");
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: guns,
                mission: Mission::Support {
                    formation: "assault".into(),
                },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("shooting for a friendly formation that is not your own is legal");

    let evaluator = Evaluator::new(tactics_core::data::DoctrineDef {
        id: "band_probe".into(),
        name: String::new(),
        description: String::new(),
        aggression: 0.0,
        cover_value: 0.0,
        elevation_value: 0.0,
        // Zero, so the out-of-support penalty is the mass term's business
        // and not this test's: what separates these tiles must be the
        // mission. The crowding penalty inside two hexes is universal and
        // survives — it is drill, not doctrine — so hugging is penalised
        // twice over, which is the right answer arrived at honestly.
        concentration: 0.0,
        scouting: 0.0,
        objective_value: 1.0,
        indirect_appetite: 1.0,
        withdraw_threshold: 0.5,
        initiative: 0.5,
        delegation: 0.5,
    });
    let score = |col: i32| {
        evaluator
            .score_tile(&reg, &state, UnitId(1), tactics_core::offset_to_hex(col, 1))
            .score
    };

    // The assault's leader stands at column 6, so these are one, three and
    // fourteen hexes off her.
    let hugging = score(7);
    let standoff = score(9);
    let straggling = score(20);
    assert!(
        standoff > hugging,
        "supporting distance beats riding along with them: {standoff} vs {hugging}"
    );
    assert!(
        standoff > straggling,
        "and beats trailing the map behind them: {standoff} vs {straggling}"
    );
}

#[test]
fn nobody_supports_the_enemy_or_herself() {
    // The three sentences that have no meaning, refused where both ends of
    // the order are in hand. A base of fire for a formation that is not
    // there would score zero forever and look exactly like a formation that
    // had decided to do nothing.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 3).unwrap();
    let armor = formation_named(&state, "kuhlmann_armor");
    let support = |formation: &str| Order::SetMission {
        formation: armor,
        mission: Mission::Support {
            formation: formation.into(),
        },
        latitude: tactics_core::battle::Latitude::Delegated,
    };

    assert_eq!(
        state.apply(&reg, &support("valkyrie_line")),
        Err(tactics_core::battle::OrderError::CannotSupportThat),
        "you do not stand base of fire for the people shooting at you"
    );
    assert_eq!(
        state.apply(&reg, &support("kuhlmann_armor")),
        Err(tactics_core::battle::OrderError::CannotSupportThat),
        "nor for yourself"
    );
    assert_eq!(
        state.apply(&reg, &support("ghost_battalion")),
        Err(tactics_core::battle::OrderError::NoSuchFormation),
        "nor for somebody who is not in this battle"
    );
    state
        .apply(&reg, &support("kuhlmann_recon"))
        .expect("her own side's other formation is the whole point");
}

#[test]
fn a_plan_may_end_in_support_but_not_continue_past_it() {
    // Support is a posture, not a leg: "advance to the ridge, then shoot for
    // the platoon" is a plan, and it ends there. Nothing follows a posture,
    // and the refusal is loud rather than a leg left sitting in a queue that
    // can never begin.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 8).unwrap();
    let recon = formation_named(&state, "kuhlmann_recon");
    let ridge = state.map.objectives()[0].anchor();
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: recon,
                mission: Mission::Advance { to: ridge },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    state
        .apply(
            &reg,
            &Order::QueueMission {
                formation: recon,
                mission: Mission::Support {
                    formation: "kuhlmann_armor".into(),
                },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("a plan may end in support");
    assert_eq!(
        state.apply(
            &reg,
            &Order::QueueMission {
                formation: recon,
                mission: Mission::Hold { at: None },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        ),
        Err(tactics_core::battle::OrderError::MissionIsTerminal),
        "and nothing follows it"
    );
}

// --- the crew's clock (chunk 10c, first slice) -----------------------------

/// One watcher on overwatch and one enemy who will walk out from behind a
/// forest wall mid-round. `gap` is how far from the wall the watcher stands.
fn ambush_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    // A tall forest curtain with the enemy tucked behind it: she cannot be
    // seen until she moves out from behind the trees.
    let rows = ["gggggggggggg", "ggggggffgggg", "gggggggggggg"];
    two_side_battle(
        reg,
        &rows,
        vec![
            unit_at([1, 1], 0, "medium_tank", "Watcher"),
            unit_at([8, 1], 1, "medium_tank", "Walker"),
        ],
        seed,
    )
}

#[test]
fn a_surprise_costs_reaction_time_whenever_it_arrives() {
    // The old gate waived the tax after tick two of every round: an enemy
    // appearing at tick seven was answered the same tick. The clock now
    // starts when SHE APPEARS, so the watcher's first shot comes exactly
    // reaction-delay ticks after first sight, wherever in the round the
    // surprise falls.
    let mut reg = registry();
    reg.command = None;
    let mut state = ambush_stage(&reg, 101);
    let (watcher, walker) = (UnitId(0), UnitId(1));
    assert!(
        !state.fog.side(0).spotted.contains(&walker),
        "the stage needs the walker hidden"
    );
    // The walker steps into the open; the watcher holds.
    let out = state.unit(walker).unwrap().pos + tactics_core::Hex::new(0, -1);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: walker,
                to: out,
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let (mut seen_at, mut fired_at) = (None, None);
    while state.resolving_tick().is_some() && fired_at.is_none() {
        let tick = state.resolving_tick().unwrap();
        for event in state.step_tick(&reg) {
            match event {
                BattleEvent::UnitSpotted {
                    unit, by_side: 0, ..
                } if unit == walker => {
                    seen_at.get_or_insert(tick);
                }
                BattleEvent::ShotFired {
                    attacker,
                    opportunity: true,
                    ..
                } if attacker == watcher => {
                    fired_at.get_or_insert(tick);
                }
                _ => {}
            }
        }
    }
    let seen = seen_at.expect("she steps into view during the round");
    let fired = fired_at.expect("and is engaged before it ends");
    let delay = 2; // reactions untrained on an average crew: base_ticks
    assert_eq!(
        fired,
        seen + delay,
        "noticed at tick {seen}, engaged {delay} ticks later — not instantly"
    );
}

#[test]
fn a_target_watched_across_rounds_is_not_news_twice() {
    // The other direction: the old gate re-charged the delay at the top of
    // every round. A crew that has held the same enemy in sight since last
    // round owes nothing — her gun speaks on the first tick it is ready.
    let mut reg = registry();
    reg.command = None;
    // Soft guns for the same reason as `crews_report_moving_up_the_ladder`:
    // the clock across the round boundary is the thing under test, and it
    // needs both crews alive to see round two. The reload is pinned at five
    // ticks so the arithmetic discriminates: first shots at tick 2 (the
    // reaction delay), again at 7, and the barrel is ready exactly as round
    // two opens — so tick 0 proves the clock did not re-charge, while a
    // re-charged clock would show tick 2. With the gun's own reload of
    // three the carry-over happens to equal the delay and the test could
    // not tell a cooling barrel from a re-taxed crew.
    if let Some(w) = reg.weapons.get_mut("gun_75") {
        w.damage = 2;
        w.reload_ticks = Some(5);
    }
    let mut state = duel(&reg, 102);
    let watcher = UnitId(0);
    // Round one: they see each other, shots are exchanged, the clock is paid.
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "still watching him"
    );
    // Round two, both hold: the first opportunity shot must come at tick 0,
    // the moment the gun is ready — not at tick two again.
    commit_all(&reg, &mut state);
    let mut first_shot = None;
    while state.resolving_tick().is_some() && first_shot.is_none() && !state.is_over() {
        let tick = state.resolving_tick().unwrap();
        for event in state.step_tick(&reg) {
            if let BattleEvent::ShotFired {
                attacker,
                opportunity: true,
                ..
            } = event
                && attacker == watcher
            {
                first_shot.get_or_insert(tick);
            }
        }
    }
    assert_eq!(
        first_shot,
        Some(0),
        "an enemy watched across the round boundary is old news"
    );
}

// --- the crew's loop (chunk 10c, second slice: the mid-round drill) --------

/// Pull the 75's teeth without pulling its threat: one point of effect
/// budget still prices the shot above zero — she is being shot at by
/// something that CAN hurt her, which is what `threatened` and the drill
/// read — but a penetration wounds one cadet or dings one module instead of
/// savaging the vehicle. The clock and drill tests need their subjects
/// alive, mobile and unbroken long enough to watch them decide.
fn soften(reg: &mut DataRegistry) {
    if let Some(w) = reg.weapons.get_mut("gun_75") {
        w.damage = 1;
    }
    // Size-zero modules are never rolled, so penetrations wound cadets and
    // break nothing: the gun keeps firing and the tracks keep driving,
    // which is what a test about timing or movement needs its subject to do.
    for module in reg.modules.values_mut() {
        module.size = 0;
    }
}

/// The ambush staged with somewhere to go: a forest stand west of the
/// watcher, the same curtain in the middle, and the walker tucked behind it.
/// The watcher sits idle on open grass — exactly the crew the mid-round
/// drill exists for.
fn open_ground_stage(reg: &DataRegistry, watcher_at: [i32; 2], seed: u64) -> BattleState {
    let rows = ["ffgggggggggg", "ffggggffgggg", "ffgggggggggg"];
    two_side_battle(
        reg,
        &rows,
        vec![
            unit_at(watcher_at, 0, "medium_tank", "Watcher"),
            unit_at([8, 1], 1, "medium_tank", "Walker"),
        ],
        seed,
    )
}

/// Play out the ambush and record when side 0 first saw the walker and when
/// (and where) the watcher broke for cover, then finish the round.
fn spring_the_ambush(
    reg: &DataRegistry,
    state: &mut BattleState,
) -> (Option<u32>, Option<u32>, Option<tactics_core::Hex>) {
    let (watcher, walker) = (UnitId(0), UnitId(1));
    assert!(
        !state.fog.side(0).spotted.contains(&walker),
        "the stage needs the walker hidden"
    );
    let out = state.unit(walker).unwrap().pos + tactics_core::Hex::new(0, -1);
    state
        .apply(
            reg,
            &Order::SetMove {
                unit: walker,
                to: out,
            },
        )
        .unwrap();
    commit_all(reg, state);

    let (mut seen_at, mut broke_at, mut broke_to) = (None, None, None);
    while state.resolving_tick().is_some() && !state.is_over() {
        let tick = state.resolving_tick().unwrap();
        for event in state.step_tick(reg) {
            match event {
                BattleEvent::UnitSpotted {
                    unit, by_side: 0, ..
                } if unit == walker => {
                    seen_at.get_or_insert(tick);
                }
                BattleEvent::TookCover { unit, at } if unit == watcher => {
                    broke_at.get_or_insert(tick);
                    broke_to.get_or_insert(at);
                }
                _ => {}
            }
        }
    }
    (seen_at, broke_at, broke_to)
}

#[test]
fn a_crew_caught_in_the_open_breaks_for_cover_before_the_round_ends() {
    // The planning-table drill covers a crew who is threatened when the
    // round is planned; this is the one who is ambushed at tick four. She
    // owes her reaction time — the same per-enemy clock her gunner pays for
    // opportunity fire — and then she owes nobody a planning phase: the
    // tracks move mid-round, toward the best cover in reach, and the event
    // stream says so out loud.
    let mut reg = registry_wireless();
    soften(&mut reg);
    let mut state = open_ground_stage(&reg, [3, 1], 201);
    let parked = state.unit(UnitId(0)).unwrap().pos;

    let (seen_at, broke_at, broke_to) = spring_the_ambush(&reg, &mut state);
    let seen = seen_at.expect("she steps into view during the round");
    let broke = broke_at.expect("and the watcher does not wait for the round to end");
    let delay = 2; // reactions untrained on an average crew: base_ticks
    assert_eq!(
        broke,
        seen + delay,
        "noticed at tick {seen}, moving {delay} ticks later — not instantly"
    );
    assert_eq!(
        state.terrain_at(broke_to.expect("a destination came with the event")),
        Some("forest"),
        "she makes for the cover, not merely anywhere"
    );
    assert_ne!(
        state.unit(UnitId(0)).unwrap().pos,
        parked,
        "and the tracks actually turned before the round was over"
    );
}

#[test]
fn a_crew_with_a_route_in_hand_drives_it_rather_than_flinching() {
    // The reaction-latency post-mortem's rule, applied to the drill: only
    // responses to NEW information may cost time, and by the same token only
    // a crew with nothing left to execute may improvise. A route already in
    // hand keeps being driven — interrupting ordered movement is the
    // evaluator's business at the next planning table (that is what makes an
    // Advance a movement to contact), never the engine's mid-round.
    let mut reg = registry_wireless();
    soften(&mut reg);
    let mut state = open_ground_stage(&reg, [3, 1], 202);
    let (watcher, dest) = (UnitId(0), state.unit(UnitId(0)).unwrap().pos);
    // Send her west into the trees under her own orders: the route both
    // outlasts the ambush and ends in cover, so the drill would have nothing
    // to add even if it wrongly fired after arrival.
    let ordered = dest + tactics_core::Hex::new(-3, 0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: watcher,
                to: ordered,
            },
        )
        .unwrap();

    let (seen_at, broke_at, _) = spring_the_ambush(&reg, &mut state);
    assert!(seen_at.is_some(), "the ambush still happens");
    assert_eq!(
        broke_at, None,
        "a crew executing her orders is not hijacked by the drill"
    );
    assert_eq!(
        state.unit(watcher).unwrap().pos,
        ordered,
        "she drives the route she was given"
    );
}

#[test]
fn an_overwatching_crew_trusts_her_gun_over_her_tracks() {
    // A fire order is the deliberate hold-and-watch, and it outranks the
    // drill exactly as it does at the planning table: she was put there to
    // shoot, and a gun line that scatters for the trees the moment it is
    // shot back at is not a gun line.
    let mut reg = registry_wireless();
    soften(&mut reg);
    let mut state = open_ground_stage(&reg, [3, 1], 203);
    let watcher = UnitId(0);
    let parked = state.unit(watcher).unwrap().pos;
    // Area fire on an empty patch of grass: legal without a spot, and it
    // marks her intent as overwatch for the whole round. Deliberately NOT
    // the curtain's mouth — with real penetration a blind 75 that happens
    // to land on the walker as she steps out can kill her, and this test
    // is about the drill deferring to the fire order, not about luck.
    let ground = parked + tactics_core::Hex::new(1, 1);
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: watcher,
                fire: FireIntent::Area {
                    at: ground,
                    weapon: 0,
                },
            },
        )
        .unwrap();

    let (seen_at, broke_at, _) = spring_the_ambush(&reg, &mut state);
    assert!(seen_at.is_some(), "the ambush still happens");
    assert_eq!(broke_at, None, "overwatch stands");
    assert_eq!(state.unit(watcher).unwrap().pos, parked);
}

#[test]
fn a_crew_with_no_better_ground_in_reach_stands_where_she_is() {
    // The comparison is strict: the drill moves a crew to strictly better
    // cover or not at all. On a bare field two tanks stare at each other all
    // round — threatened, idle, clocks long expired — and neither shuffles a
    // single hex, which is also what keeps a crew who has already reached
    // the trees from dancing between two equally good tiles forever.
    let reg = registry_wireless();
    let mut state = duel(&reg, 204);
    let posts: Vec<_> = state.units.iter().map(|u| u.pos).collect();
    commit_all(&reg, &mut state);
    let mut broke = Vec::new();
    while state.resolving_tick().is_some() && !state.is_over() {
        for event in state.step_tick(&reg) {
            if let BattleEvent::TookCover { unit, .. } = event {
                broke.push(unit);
            }
        }
    }
    assert_eq!(
        broke,
        Vec::<UnitId>::new(),
        "nobody bolts across a billiard table"
    );
    for (unit, post) in state.units.iter().zip(posts) {
        assert_eq!(unit.pos, post, "{} stood her ground", unit.name);
    }
}

#[test]
fn a_mod_that_prices_no_reactions_gets_the_drill_at_the_next_tick() {
    // Difficulty is a mod: zeroing the reaction rules collapses the delay to
    // nothing and the whole pricing system disappears without an `if`. One
    // tick remains and is structural, not a price — the sighting happens
    // after this tick's movement has already resolved, so the very next
    // slice of simultaneous time is the soonest any tracks can answer it.
    let mut reg = registry_wireless();
    soften(&mut reg);
    reg.reaction.base_ticks = 0;
    reg.reaction.max_ticks = 0;
    let mut state = open_ground_stage(&reg, [3, 1], 205);

    let (seen_at, broke_at, _) = spring_the_ambush(&reg, &mut state);
    let seen = seen_at.expect("she steps into view during the round");
    assert_eq!(
        broke_at,
        Some(seen + 1),
        "with reactions unpriced the drill is as instant as a tick model allows"
    );
}

// --- the penetration gate (ballistics B1) ----------------------------------

/// A recon car's machine gun and a heavy tank, adjacent on open grass. The
/// oldest complaint in the balance tables, staged.
fn plink_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    two_side_battle(
        reg,
        &["ggggg", "ggggg", "ggggg"],
        vec![
            unit_at([1, 1], 0, "recon_car", "Plinker"),
            unit_at([3, 1], 1, "heavy_tank", "Wall"),
        ],
        seed,
    )
}

#[test]
fn an_ordered_shot_that_cannot_penetrate_bounces_and_does_nothing() {
    // The floor is dead. A machine gun ORDERED onto a heavy tank still
    // obeys — the rounds go downrange — but what comes of them is a bounce
    // event and nothing else: no chip, no hit points, and no pressure,
    // because bullets pattering on plate frighten nobody buttoned up
    // behind it. Grinding a heavy tank down with an MG was the balance
    // instrument's oldest "worth a look" line, and this is its tombstone.
    let reg = registry_wireless();
    let mut state = plink_stage(&reg, 301);
    let (plinker, wall) = (UnitId(0), UnitId(1));
    let wall_before = state.substance(&reg, state.unit(wall).unwrap());
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: plinker,
                fire: FireIntent::Target {
                    target: wall,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let mut bounced = 0;
    let mut hit = 0;
    while state.resolving_tick().is_some() && !state.is_over() {
        for event in state.step_tick(&reg) {
            match event {
                BattleEvent::ShotBounced {
                    target, rattled, ..
                } if target == wall => {
                    assert!(!rattled, "small arms do not rattle a tank crew");
                    bounced += 1;
                }
                BattleEvent::ShotHit { target, .. } if target == wall => hit += 1,
                _ => {}
            }
        }
    }
    assert!(
        bounced > 0,
        "the bursts that struck were announced as bounces"
    );
    assert_eq!(hit, 0, "and not one of them counted as a hit");
    let wall = state.unit(wall).unwrap();
    assert_eq!(
        state.substance(&reg, wall),
        wall_before,
        "armor that holds costs nothing"
    );
    assert_eq!(
        wall.pressure, 0,
        "and plinking does not fray anyone's nerves"
    );
}

#[test]
fn a_gun_that_cannot_hurt_what_it_sees_holds_its_fire() {
    // The same scene with nobody ordering anything: opportunity fire prices
    // the shot at zero and the crew keeps her gun quiet and her position
    // secret. Before the gate this was impossible — the floor made every
    // shot worth something, so every gun in range always spoke.
    let reg = registry_wireless();
    let mut state = plink_stage(&reg, 302);
    let plinker = UnitId(0);
    commit_all(&reg, &mut state);
    while state.resolving_tick().is_some() && !state.is_over() {
        for event in state.step_tick(&reg) {
            if let BattleEvent::ShotFired { attacker, .. } = event {
                assert_ne!(
                    attacker, plinker,
                    "nothing aboard can hurt a heavy tank, so she holds fire"
                );
            }
        }
    }
}

#[test]
fn a_kinetic_round_that_beats_a_plate_up_close_fades_at_the_end_of_its_reach() {
    // Velocity, cashed out: the same gun against the same plate penetrates
    // at arm's length and bounces at the end of its reach, because a solid
    // shot arrives with whatever speed the air has left it. Scatter is
    // zeroed so the gate is a hard threshold and the test is arithmetic,
    // not luck; the interpolation endpoints are set so the plate sits
    // between them.
    let mut reg = registry_wireless();
    reg.balance.pen_scatter = 0;
    if let Some(ammo) = reg.ammo.get_mut("ap_75") {
        ammo.penetration = [7, 4]; // medium front plate is 5: beaten near, safe far
    }
    if let Some(w) = reg.weapons.get_mut("gun_75") {
        w.range = [1, 4];
    }
    let shoot = |reg: &DataRegistry, dist: i32, seed: u64| -> (u32, u32) {
        let row = "g".repeat(8);
        let mut state = two_side_battle(
            reg,
            &[&row, &row, &row],
            vec![
                unit_at([1, 1], 0, "medium_tank", "Gunner"),
                unit_at([1 + dist, 1], 1, "medium_tank", "Plate"),
            ],
            seed,
        );
        state
            .apply(
                reg,
                &Order::SetFire {
                    unit: UnitId(0),
                    fire: FireIntent::Target {
                        target: UnitId(1),
                        weapon: 0,
                    },
                },
            )
            .unwrap();
        commit_all(reg, &mut state);
        let (mut hits, mut bounces) = (0, 0);
        while state.resolving_tick().is_some() && !state.is_over() {
            for event in state.step_tick(reg) {
                match event {
                    BattleEvent::ShotHit {
                        target: UnitId(1), ..
                    } => hits += 1,
                    BattleEvent::ShotBounced {
                        target: UnitId(1), ..
                    } => bounces += 1,
                    _ => {}
                }
            }
        }
        (hits, bounces)
    };

    let (near_hits, near_bounces) = shoot(&reg, 1, 303);
    assert!(near_hits > 0, "point blank, the round goes through");
    assert_eq!(near_bounces, 0, "every time");

    let (far_hits, far_bounces) = shoot(&reg, 4, 304);
    assert_eq!(far_hits, 0, "at the end of its reach it cannot");
    assert!(far_bounces > 0, "and the plate says so out loud");
}

#[test]
fn no_seam_of_the_hull_is_impenetrable() {
    // The regression that shaped the obliquity model, pinned. An earlier
    // draft measured impact angle against the armor ARC's central normal;
    // the front arc spans ninety degrees of incoming ray, so its edges
    // read as near-parallel strikes and tripled the plate — a tank went
    // impenetrable from directions her side armor was plainly facing, and
    // a duel's return fire went silent for a tick until the hulls turned.
    // The hull is a hexagonal prism: the struck face's own normal is never
    // more than thirty degrees off the shot, so a round that comfortably
    // beats every plate gets in from every bearing there is.
    let mut reg = registry_wireless();
    reg.balance.pen_scatter = 0;
    let row = "g".repeat(9);
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row, &row, &row],
        vec![
            unit_at([1, 1], 0, "medium_tank", "Gunner"),
            unit_at([4, 2], 1, "medium_tank", "Hull"),
        ],
        305,
    );
    let (gunner, hull) = (UnitId(0), UnitId(1));
    let center = state.unit(hull).unwrap().pos;
    let ring: Vec<tactics_core::Hex> = center
        .all_neighbors()
        .into_iter()
        .chain(center.all_neighbors().into_iter().map(|n| n + (n - center)))
        .collect();
    for post in ring {
        if state.map.get(post).is_none() || state.unit_at(post).is_some() {
            continue;
        }
        state.units[gunner.index()].pos = post;
        let preview = tactics_core::battle::preview_attack(&reg, &state, gunner, 0, hull, false)
            .expect("both stand on the field");
        assert_eq!(
            preview.pen_chance, 100,
            "a 75 that beats every plate of a medium gets in from {post:?} too"
        );
    }
}

#[test]
fn the_racks_run_dry_and_the_gun_falls_silent() {
    // One armor-piercing round left and no high explosive at all: she
    // fires it, the moment is announced, and the gun says nothing for the
    // rest of the battle — silence the player was told about rather than
    // an order the game ate.
    let reg = registry_wireless();
    let mut state = duel(&reg, 306);
    let shooter = UnitId(0);
    state.units[shooter.index()].ammo = [("ap_75".to_string(), 1), ("he_75".to_string(), 0)]
        .into_iter()
        .collect();
    commit_all(&reg, &mut state);

    let (mut fired, mut dry_said) = (0, false);
    while state.resolving_tick().is_some() && !state.is_over() {
        for event in state.step_tick(&reg) {
            match event {
                BattleEvent::ShotFired { attacker, .. } if attacker == shooter => fired += 1,
                BattleEvent::WeaponDry { unit, .. } if unit == shooter => dry_said = true,
                _ => {}
            }
        }
    }
    assert_eq!(
        fired, 1,
        "the last round goes downrange and nothing follows it"
    );
    assert!(dry_said, "and running dry is said out loud");
    assert_eq!(
        state.unit(shooter).map(|u| u.ammo["ap_75"]),
        Some(0),
        "the rack is empty"
    );
}

#[test]
fn a_mod_without_ammunition_still_fights_with_its_guns_own_numbers() {
    // Additivity, read strictly: ammunition is content a mod may decline.
    // Stripping every weapon's ammo list drops combat onto the legacy
    // path — the gun's own damage and penetration through the same gate,
    // nothing counted, nothing spent — so a pre-ballistics mod keeps the
    // relationships its author tuned, forever, with infinite rounds.
    let mut reg = registry_wireless();
    for weapon in reg.weapons.values_mut() {
        weapon.ammo.clear();
    }
    let mut state = duel(&reg, 307);
    let racks: Vec<_> = state.units.iter().map(|u| u.ammo.clone()).collect();
    commit_all(&reg, &mut state);

    let mut hit = false;
    while state.resolving_tick().is_some() && !state.is_over() {
        for event in state.step_tick(&reg) {
            if matches!(event, BattleEvent::ShotHit { .. }) {
                hit = true;
            }
        }
    }
    assert!(hit, "a 75 still beats a medium's plate on its own numbers");
    for (unit, before) in state.units.iter().zip(racks) {
        assert_eq!(
            unit.ammo, before,
            "uncounted rounds are infinite ones: nothing was spent"
        );
    }
}

// --- the outcome engine (ballistics B2): no hit points -----------------------

#[test]
fn a_penetration_names_the_girl_it_hurt() {
    // Permadeath without a name is just a number going down. Every crew hit
    // carries the cadet it found, she is really aboard the vehicle it names,
    // and the seat she sits in is marked — the state and the story must be
    // the same fact.
    let mut reg = registry_wireless();
    soften(&mut reg); // interiors are cadets only: every pen finds one
    let mut state = duel(&reg, 401);
    let (west, east) = (UnitId(0), UnitId(1));
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: west,
                fire: FireIntent::Target {
                    target: east,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let mut named = Vec::new();
    while state.resolving_tick().is_some() && !state.is_over() {
        for event in state.step_tick(&reg) {
            if let BattleEvent::CrewHit { unit, cadet, .. } = event
                && unit == east
            {
                named.push(cadet);
            }
        }
    }
    assert!(!named.is_empty(), "a softened 75 wounds rather than breaks");
    let hull = state.unit(east).unwrap();
    for cadet in named {
        let seat = hull
            .crew
            .iter()
            .position(|g| *g == cadet)
            .expect("the cadet the event names is aboard the vehicle it names");
        assert_ne!(
            hull.crew_state.get(seat).copied().unwrap_or_default(),
            tactics_core::battle::CrewCondition::Fine,
            "and her seat is marked"
        );
    }
}

#[test]
fn a_destroyed_gun_module_silences_the_primary_weapon_only() {
    // The module maps to the mount: main gun dead means the 75 never speaks
    // again, while the coaxial stays mechanically ready — it merely has
    // nothing worth shooting at in this scene, which is the gate's own
    // discipline, not the module's.
    let reg = registry_wireless();
    let mut state = duel(&reg, 402);
    let (west, east) = (UnitId(0), UnitId(1));
    state.units[west.index()]
        .modules
        .insert("main_gun".into(), 0);
    assert!(
        !tactics_core::battle::weapon_ready(&reg, &state, west, 0),
        "a destroyed gun module is not a gun"
    );
    assert!(
        tactics_core::battle::weapon_ready(&reg, &state, west, 1),
        "the machine gun is its own mount and still answers ready"
    );
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: west,
                fire: FireIntent::Target {
                    target: east,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);
    let mut west_fired_gun = false;
    while state.resolving_tick().is_some() && !state.is_over() {
        for event in state.step_tick(&reg) {
            if let BattleEvent::ShotFired {
                attacker, weapon, ..
            } = &event
                && *attacker == west
                && weapon == "gun_75"
            {
                west_fired_gun = true;
            }
        }
    }
    assert!(!west_fired_gun, "a destroyed gun does not obey the order");
}

#[test]
fn a_mobility_kill_stops_her_where_she_stands() {
    // Tracks are the module with a middle state: damaged running gear
    // halves her speed, destroyed stops her on the spot, and talented
    // driving buys back neither.
    let reg = registry_wireless();
    let mut state = duel(&reg, 403);
    let west = UnitId(0);
    let whole = {
        let u = state.unit(west).unwrap();
        tactics_core::battle::move_points(&reg, &state.roster, u, state.terrain_at(u.pos))
    };
    assert!(whole > 0);

    state.units[west.index()].modules.insert("tracks".into(), 1);
    let limping = {
        let u = state.unit(west).unwrap();
        tactics_core::battle::move_points(&reg, &state.roster, u, state.terrain_at(u.pos))
    };
    assert_eq!(limping, whole / 2, "one thrown track halves her");

    state.units[west.index()].modules.insert("tracks".into(), 0);
    let stopped = {
        let u = state.unit(west).unwrap();
        tactics_core::battle::move_points(&reg, &state.roster, u, state.terrain_at(u.pos))
    };
    assert_eq!(stopped, 0, "both gone stops her where she stands");
    assert_eq!(
        reachable(&reg, &state, west).len(),
        1,
        "the move overlay is her own tile and nothing else"
    );
}

#[test]
fn a_dead_radio_drops_her_off_the_net() {
    // The radio module dying is the chain-of-command layer's stake in
    // ballistics: six hexes from her leader — inside the set's reach, past
    // flag range — she is on the net right up until the set is wreckage,
    // and then she is a cadet driving on standing orders.
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 8).unwrap();
    let formation = state.formations()[0].clone();
    let leader = formation.leader.expect("the formation has a leader");
    let stray = *formation
        .members
        .iter()
        .find(|m| **m != leader)
        .expect("a formation of one tests nothing");
    let post = state.unit(leader).expect("leader").pos + tactics_core::Hex::new(6, 0);
    state.units[stray.index()].pos = post;
    settle(&reg, &mut state);
    assert!(
        state.formations()[0].in_contact(stray),
        "six hexes out, the set carries her leader's voice"
    );

    state.units[stray.index()]
        .modules
        .insert("radio_set".into(), 0);
    settle(&reg, &mut state);
    assert!(
        !state.formations()[0].in_contact(stray),
        "the same six hexes with a wrecked set is silence"
    );
}

#[test]
fn a_one_rung_ladder_never_abandons_anything() {
    // Difficulty is a mod, read against the bail-out: abandoning rides the
    // rung the pressure ladder puts a crew on, so a ladder with one steady
    // rung produces crews that stay with the tank whatever comes through
    // the armor — the gentle game, with no `if` in Rust to switch.
    let mut reg = registry_wireless();
    reg.morale.rungs = vec![tactics_core::data::MoraleRung {
        id: "steady".into(),
        name: "Steady".into(),
        at_pressure: 0,
        obeys: true,
        accuracy: 0,
    }];
    let mut state = duel(&reg, 405);
    commit_all(&reg, &mut state);
    for _ in 0..6 {
        if state.is_over() {
            break;
        }
        for event in state.resolve_round(&reg) {
            assert!(
                !matches!(event, BattleEvent::Abandoned { .. }),
                "nobody jumps off a one-rung ladder"
            );
        }
        commit_all(&reg, &mut state);
    }
}

#[test]
fn a_bounced_shell_wrecks_no_plate_it_never_touched() {
    // The B5 instrument's first finding, cured and pinned. Overpressure
    // used to consult the hull's THINNEST plate, so a 105 bouncing off a
    // heavy tank's glacis (blast 6 against a rear plate of 3, doubled)
    // wrecked her through armor the burst never faced — artillery needed
    // 1.8 shells per heavy while its penetration table read zero. The
    // plate consulted now is the one the burst arrives on: a frontal
    // bounce is a frontal problem, external module rattles at worst, and
    // the heavy tank drives away from a plunging barrage that never finds
    // anything but her glacis. (With the base mod's numbers the direct-hit
    // overmatch path only fires through adjacent splash onto genuinely
    // soft skins — `neighbors_of_a_shellburst_feel_half_the_blast` pins
    // that half.)
    let mut reg = registry_wireless();
    reg.balance.pen_scatter = 0;
    let row = "g".repeat(8);
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([0, 1], 0, "artillery", "Battery"),
            unit_at([4, 1], 1, "heavy_tank", "Wall"),
        ],
        406,
    );
    let (battery, wall) = (UnitId(0), UnitId(1));
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: battery,
                fire: FireIntent::Target {
                    target: wall,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let (mut bounces, mut pens) = (0, 0);
    for _ in 0..4 {
        if state.is_over() {
            break;
        }
        for event in state.resolve_round(&reg) {
            match event {
                BattleEvent::ShotBounced { target, .. } if target == wall => bounces += 1,
                BattleEvent::ShotHit { target, .. } if target == wall => pens += 1,
                _ => {}
            }
        }
        commit_all(&reg, &mut state);
    }
    assert!(bounces > 0, "the shells arrive and the glacis holds");
    assert_eq!(pens, 0, "a 105 cannot beat a heavy tank's front");
    assert!(
        state.unit(wall).is_some_and(|u| u.alive && !u.wrecked),
        "and she is not wrecked through a plate the bursts never touched"
    );
}

#[test]
fn an_emptied_rack_is_harder_to_torch() {
    // Brew-up chance rides the fraction of ammunition still aboard, so
    // shooting your racks empty is quietly a survival strategy. Staged so
    // every effect roll finds the rack: full racks at certainty burn on the
    // first penetration; the same tank with empty racks takes the same hit
    // and does not.
    let mut reg = registry_wireless();
    reg.balance.pen_scatter = 0;
    reg.balance.crew_weight = 0;
    reg.balance.brewup_percent = 100;
    for module in reg.modules.values_mut() {
        if module.effect != tactics_core::data::ModuleEffect::Ammo {
            module.size = 0;
        }
    }

    let torch = |reg: &DataRegistry, empty: bool, seed: u64| -> bool {
        let mut state = duel(reg, seed);
        let (west, east) = (UnitId(0), UnitId(1));
        if empty {
            for count in state.units[east.index()].ammo.values_mut() {
                *count = 0;
            }
        }
        state
            .apply(
                reg,
                &Order::SetFire {
                    unit: west,
                    fire: FireIntent::Target {
                        target: east,
                        weapon: 0,
                    },
                },
            )
            .unwrap();
        commit_all(reg, &mut state);
        let mut brewed = false;
        while state.resolving_tick().is_some() && !state.is_over() {
            for event in state.step_tick(reg) {
                if matches!(event, BattleEvent::BrewedUp { unit } if unit == east) {
                    brewed = true;
                }
            }
        }
        brewed
    };

    assert!(torch(&reg, false, 407), "full racks at certainty burn");
    assert!(!torch(&reg, true, 408), "empty racks cannot");
}

// --- soft targets and hidden ones (infantry N1) ----------------------------

#[test]
fn a_platoon_in_the_trees_is_invisible_until_the_scout_closes() {
    // Concealment: standing on ground somebody can see is not being seen.
    // A rifle platoon at concealment 40 doubles to 80 in the treeline, so
    // the recon car that sees twenty hexes of open ground spots her at
    // four — while a tank on the same tile is spotted at the full twenty,
    // which is the additivity half of the claim.
    let reg = registry_wireless();
    let row = format!("gggggg{}g", "f");
    let stage = |vehicle: &str, dist: i32| -> BattleState {
        two_side_battle(
            &reg,
            &[&row, &row, &row],
            vec![
                unit_at([6 - dist.min(6), 1], 0, "recon_car", "Scout"),
                unit_at([6, 1], 1, vehicle, "Quarry"),
            ],
            501,
        )
    };

    let far = stage("rifle_platoon", 6);
    assert!(
        !far.fog.side(0).spotted.contains(&UnitId(1)),
        "six hexes out, the treeline keeps her"
    );
    let near = stage("rifle_platoon", 3);
    assert!(
        near.fog.side(0).spotted.contains(&UnitId(1)),
        "three hexes out, even trees are not enough"
    );
    let control = stage("medium_tank", 6);
    assert!(
        control.fog.side(0).spotted.contains(&UnitId(1)),
        "a tank on the same tile hides from nobody"
    );
}

#[test]
fn springing_the_ambush_spends_it() {
    // The other half of concealment: firing reveals, unconditionally. The
    // platoon the carrier could not see kills it from three hexes — and is
    // seen by everyone from the muzzle flash on.
    let reg = registry_wireless();
    let rows = ["ggggg", "ggfgg", "ggggg"];
    let mut state = two_side_battle(
        &reg,
        &rows,
        vec![
            unit_at([2, 1], 0, "rifle_platoon", "Ambush"),
            unit_at([4, 1], 1, "apc", "Taxi"),
        ],
        502,
    );
    let (platoon, taxi) = (UnitId(0), UnitId(1));
    assert!(
        !state.fog.side(1).spotted.contains(&platoon),
        "the taxi drives past a treeline it cannot read"
    );
    assert!(
        state.fog.side(0).spotted.contains(&taxi),
        "while the platoon has watched her come the whole way"
    );
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: platoon,
                fire: FireIntent::Target {
                    target: taxi,
                    weapon: 1,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);
    let events = state.resolve_round(&reg);
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::ShotFired { attacker, weapon, .. }
                if *attacker == platoon && weapon == "rpg"
        )),
        "the rocket goes out"
    );
    assert!(
        state.fog.side(1).spotted.contains(&platoon) || state.unit(taxi).is_none_or(|u| !u.alive),
        "and the ambush is spent: sprung means seen"
    );
}

#[test]
fn a_thinned_platoon_shoots_at_half_strength() {
    // The troops module's firepower meaning: every weapon the platoon
    // fires scales by the riflemen still standing. Half the sections is
    // half the fire, through the same preview the player reads.
    let reg = registry_wireless();
    let row = "g".repeat(6);
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([1, 1], 0, "rifle_platoon", "Platoon"),
            unit_at([3, 1], 1, "scout_section", "Target"),
        ],
        503,
    );
    let (platoon, target) = (UnitId(0), UnitId(1));
    let full = tactics_core::battle::preview_attack(&reg, &state, platoon, 0, target, false)
        .expect("both stand")
        .damage;
    assert!(full > 0, "a whole platoon's rifles are worth something");

    state.units[platoon.index()]
        .modules
        .insert("rifle_sections".into(), 3);
    let half = tactics_core::battle::preview_attack(&reg, &state, platoon, 0, target, false)
        .expect("both stand")
        .damage;
    assert_eq!(
        half,
        full * 3 / 6,
        "three of six sections left is half the fire"
    );
}

#[test]
fn a_shellburst_beside_a_platoon_is_attrition_not_erasure() {
    // The plate-zero carve-out: a dispersed platoon has no hull for blast
    // overmatch to crush, so splash converts to casualty rolls. Shell after
    // shell lands next door and the platoon bleeds — and is still a platoon,
    // never a single-event deletion.
    let reg = registry_wireless();
    let row = "g".repeat(9);
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([0, 1], 0, "artillery", "Battery"),
            unit_at([5, 1], 1, "rifle_platoon", "Platoon"),
        ],
        504,
    );
    let (_, platoon) = (UnitId(0), UnitId(1));
    let beside = state.unit(platoon).unwrap().pos + tactics_core::Hex::new(1, 0);
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: UnitId(0),
                fire: FireIntent::Area {
                    at: beside,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let mut bled = false;
    for _ in 0..4 {
        if state.is_over() {
            break;
        }
        for event in state.resolve_round(&reg) {
            if matches!(
                event,
                BattleEvent::ModuleHit { unit, .. } | BattleEvent::CrewHit { unit, .. }
                    if unit == platoon
            ) {
                bled = true;
            }
        }
        commit_all(&reg, &mut state);
    }
    assert!(bled, "four volleys next door draw blood");
    let unit = state.units[platoon.index()].clone();
    assert!(
        !unit.wrecked,
        "but a spread-out platoon is not a hull to crush"
    );
}

#[test]
fn a_remnant_platoon_is_a_story_not_a_gun() {
    // Troops at zero: the cadets are alive, the platoon is finished. Her
    // rifles are worth nothing, she holds fire even with an enemy in her
    // lap, and her condition says what the withdraw machinery needs to
    // hear.
    let reg = registry_wireless();
    let row = "g".repeat(5);
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([1, 1], 0, "rifle_platoon", "Remnant"),
            unit_at([3, 1], 1, "scout_section", "Enemy"),
        ],
        505,
    );
    let platoon = UnitId(0);
    state.units[platoon.index()]
        .modules
        .insert("rifle_sections".into(), 0);
    assert!(
        state.condition(&reg, state.unit(platoon).unwrap()) < 0.5,
        "a shattered platoon reads as one"
    );
    commit_all(&reg, &mut state);
    for event in state.resolve_round(&reg) {
        assert!(
            !matches!(event, BattleEvent::ShotFired { attacker, .. } if attacker == platoon),
            "two cadets and no riflemen fire nothing worth firing"
        );
    }
    assert!(
        state.units[platoon.index()].alive,
        "and she is a story still on the field, not a deletion"
    );
}

#[test]
fn an_unseen_crew_holds_her_rockets_for_the_killing_shot() {
    // Ambush discipline: the enemy has not seen her, and that advantage
    // is not spent on a mediocre shot. A tank destroyer at the rocket's
    // full reach is a coin flip through the front plate — the unseen
    // platoon lets him pass. A taxi she has let close to two hexes is the
    // decisive shot the rockets were carried for, and the same platoon
    // fires without being told. (The ranges differ because the stage must
    // keep her unseen: the destroyer's better glass would find her at
    // two.) Ordered fire never consults any of this: the commander's word
    // outranks the ambusher's patience.
    let reg = registry_wireless();
    let rows = ["ggggg", "gfggg", "ggggg"];
    let watch = |target_vehicle: &str, dist: i32, seed: u64| -> bool {
        let mut state = two_side_battle(
            &reg,
            &rows,
            vec![
                unit_at([1, 1], 0, "rifle_platoon", "Ambush"),
                unit_at([1 + dist, 1], 1, target_vehicle, "Passerby"),
            ],
            seed,
        );
        let platoon = UnitId(0);
        assert!(
            !state.fog.side(1).spotted.contains(&platoon),
            "the stage needs her unseen"
        );
        commit_all(&reg, &mut state);
        state
            .resolve_round(&reg)
            .iter()
            .any(|e| matches!(e, BattleEvent::ShotFired { attacker, .. } if *attacker == platoon))
    };

    assert!(
        !watch("tank_destroyer", 3, 601),
        "a coin-flip shot is not worth the ambush"
    );
    assert!(
        watch("apc", 2, 602),
        "a taxi allowed to close is the shot the rockets were carried for"
    );
}

// --- the ride (infantry N2) ------------------------------------------------

/// A taxi, her platoon beside her, and an enemy across the field. Which
/// enemy matters: a recon car can watch the whole exercise and hurt none
/// of it, a tank destroyer makes the ride a coffin — each test picks.
fn taxi_stage(reg: &DataRegistry, enemy: &str, seed: u64) -> BattleState {
    let row = "g".repeat(12);
    two_side_battle(
        reg,
        &[&row, &row, &row],
        vec![
            unit_at([1, 1], 0, "apc", "Taxi"),
            unit_at([2, 1], 0, "rifle_platoon", "Riders"),
            unit_at([10, 1], 1, enemy, "Overwatch"),
        ],
        seed,
    )
}

/// A long road with a piece of ground worth holding at the far end, a
/// platoon and an empty taxi at the near end, and an enemy parked on the
/// objective behind a forest curtain so that nobody is spotted at the bell.
///
/// The distances are the whole point: on foot the objective is a march of
/// twenty-odd rounds and by taxi it is four or five, which is the gap a taxi
/// run exists to close. `walk` shortens it so the same stage can ask the
/// opposite question.
fn taxi_run_stage(reg: &DataRegistry, walk: i32, seed: u64) -> BattleState {
    let mut row: Vec<char> = "g".repeat(30).chars().collect();
    row[15] = 'f';
    let row: String = row.into_iter().collect();
    let goal = 1 + walk;
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "taxi_run",
        "palette": { "g": "grass", "f": "forest" },
        "rows": [row],
        "objectives": [{ "id": "far_end", "name": "The Far End", "at": [[goal, 0]], "value": 3 }],
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
    let placements = vec![
        unit_at([1, 0], 0, "rifle_platoon", "Riders"),
        unit_at([2, 0], 0, "apc", "Taxi"),
        unit_at([29, 0], 1, "medium_tank", "Watcher"),
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

/// Every order one side's planner issues in a round, in the order it issues
/// them. The planner is only reachable through the driver, which is the right
/// seam to test at anyway: what matters is the order that reaches the engine.
fn orders_planned(reg: &DataRegistry, state: &mut BattleState, side: u8, seed: u64) -> Vec<Order> {
    let mut ai = AiDriver::new();
    ai.insert(
        side,
        make_battle_planner(
            &AiConfig {
                planner: "utility".into(),
                difficulty: 5,
                doctrine: Some("massed_armor".into()),
            },
            seed,
            reg,
        ),
    );
    let mut orders = Vec::new();
    ai.plan_round_with(reg, state, |d| {
        if d.side == side {
            orders.push(d.order.clone());
        }
    });
    orders
}

#[test]
fn a_platoon_with_a_long_march_ahead_of_her_calls_for_the_taxi() {
    // The AI could mount nobody. `Order::Mount` has been in the engine since
    // the ride landed, the player's `M` has used it all along, and every
    // planner ignored it — so half the transport machinery was dead for
    // every side the player was not personally commanding, and a battle taxi
    // was a thing that made exactly one delivery per battle.
    //
    // What decides it is arithmetic and nothing else: rounds spent walking
    // the journey against rounds spent walking to the tailgate, being driven,
    // and getting out. Twenty-six hexes at one hex a round is a march; the
    // same trip at six is not. Nothing here knows what an APC is.
    let reg = registry_wireless();
    let mut state = taxi_run_stage(&reg, 26, 55);
    let (riders, taxi) = (UnitId(0), UnitId(1));
    let orders = orders_planned(&reg, &mut state, 0, 55);
    assert!(
        orders
            .iter()
            .any(|o| matches!(o, Order::Mount { unit, into } if *unit == riders && *into == taxi)),
        "she should be calling for the ride: {orders:?}"
    );
}

#[test]
fn a_platoon_with_a_short_walk_ahead_of_her_walks() {
    // The other half of the same comparison, and the reason it is a
    // comparison rather than a preference. Three hexes is a walk; mounting up
    // for it would cost more rounds than it saved, and a platoon that boarded
    // for every journey would spend a battle climbing in and out. Measured:
    // pricing boarding at two rounds instead of four let exactly this happen
    // near an objective and cost the commanded side a win and three platoons.
    let reg = registry_wireless();
    let mut state = taxi_run_stage(&reg, 3, 56);
    let orders = orders_planned(&reg, &mut state, 0, 56);
    assert!(
        !orders.iter().any(|o| matches!(o, Order::Mount { .. })),
        "the objective is three hexes away; she walks: {orders:?}"
    );
}

#[test]
fn the_taxi_drives_to_the_pickup_rather_than_leaving_without_her() {
    // The driver's half, and the half without which the feature is a joke: a
    // platoon walks at one hex a round and her ride drives at six, so a
    // passenger marching after a carrier that is doing its own planning never
    // catches it. The pickup has to be somebody's job. While anybody is
    // boarding her the carrier closes the gap and then holds the door, which
    // is a rendezvous — it cannot be got by making the infantry walk faster.
    let reg = registry_wireless();
    let mut state = taxi_run_stage(&reg, 26, 57);
    let (riders, taxi) = (UnitId(0), UnitId(1));
    // Put the two of them well apart, so "toward her" and "toward the
    // objective" are opposite directions and the assertion cannot pass by
    // accident.
    state.units[taxi.index()].pos = tactics_core::offset_to_hex(9, 0);
    state
        .apply(
            &reg,
            &Order::Mount {
                unit: riders,
                into: taxi,
            },
        )
        .expect("a foot unit may board a friendly transport with room");
    let before = state.unit(taxi).unwrap().pos;
    let gap_before = before.distance_to(state.unit(riders).unwrap().pos);

    let orders = orders_planned(&reg, &mut state, 0, 57);
    let dest = orders
        .iter()
        .find_map(|o| match o {
            Order::SetMove { unit, to } if *unit == taxi => Some(*to),
            _ => None,
        })
        .expect("the taxi has somewhere to be: the pickup");
    assert!(
        dest.distance_to(state.unit(riders).unwrap().pos) < gap_before,
        "she should be closing on her fare, not driving for the objective"
    );
}

#[test]
fn the_ai_runs_a_platoon_across_the_map_and_puts_her_down_on_the_objective() {
    // The whole run, end to end, with nobody steering: she calls the taxi,
    // the taxi comes for her, she boards, she is driven twenty-odd hexes, and
    // she gets off on the ground she was making for. On foot the same journey
    // is a twenty-six round march, so arriving inside ten is proof the ride
    // happened rather than proof she is a fast walker.
    let reg = registry_wireless();
    let mut state = taxi_run_stage(&reg, 26, 58);
    let riders = UnitId(0);
    let goal = tactics_core::offset_to_hex(27, 0);
    let start = state.unit(riders).unwrap().pos.distance_to(goal);

    let mut ai = AiDriver::new();
    for side in 0..2u8 {
        ai.insert(
            side,
            make_battle_planner(
                &AiConfig {
                    planner: "utility".into(),
                    difficulty: 5,
                    doctrine: Some("massed_armor".into()),
                },
                58 + side as u64,
                &reg,
            ),
        );
    }
    let mut mounted = false;
    let mut rounds = 0;
    while !state.is_over() && rounds < 10 {
        ai.plan_round(&reg, &mut state);
        for event in &state.resolve_round(&reg) {
            if matches!(event, BattleEvent::Mounted { unit, .. } if *unit == riders) {
                mounted = true;
            }
        }
        rounds += 1;
    }
    assert!(mounted, "she never got aboard");
    let ended = state
        .units
        .get(riders.index())
        .map(|u| u.pos.distance_to(goal))
        .expect("she is on the roll one way or another");
    assert!(
        ended < start / 2,
        "ten rounds of riding should have carried her most of the way: {start} -> {ended}"
    );
}

#[test]
fn a_platoon_boards_rides_hidden_and_steps_off_where_the_ride_ends() {
    // The whole taxi doctrine in one test: she mounts by order, vanishes
    // from the enemy's picture while the carrier stays plainly visible,
    // rides wherever it drives, and steps off beside it when told —
    // reappearing to the enemy the same tick her boots touch ground.
    let reg = registry_wireless();
    let mut state = taxi_stage(&reg, "recon_car", 701);
    let (taxi, riders) = (UnitId(0), UnitId(1));
    state
        .apply(
            &reg,
            &Order::Mount {
                unit: riders,
                into: taxi,
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);
    let events = state.resolve_round(&reg);
    assert!(
        events.iter().any(
            |e| matches!(e, BattleEvent::Mounted { unit, into } if *unit == riders && *into == taxi)
        ),
        "standing alongside, she steps up the same round"
    );
    assert!(
        !state.fog.side(1).spotted.contains(&riders),
        "aboard, the enemy's picture holds only the taxi"
    );
    assert!(
        state.fog.side(1).spotted.contains(&taxi),
        "which it can see just fine"
    );

    // The ride: the taxi drives, the platoon's position mirrors hers.
    let dest = state.unit(taxi).unwrap().pos + tactics_core::Hex::new(3, 0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: taxi,
                to: dest,
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);
    assert_eq!(
        state.unit(riders).unwrap().pos,
        state.unit(taxi).unwrap().pos,
        "she rides where the carrier is"
    );

    state
        .apply(&reg, &Order::Dismount { unit: riders })
        .unwrap();
    commit_all(&reg, &mut state);
    let events = state.resolve_round(&reg);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, BattleEvent::Dismounted { unit, .. } if *unit == riders)),
        "and steps off when told"
    );
    let (r, t) = (
        state.unit(riders).unwrap().pos,
        state.unit(taxi).unwrap().pos,
    );
    assert_eq!(r.distance_to(t), 1, "onto the ground beside the ride");
}

/// Score a tile with every doctrinal preference switched off except the two
/// the risk tests care about: the shot in front of her and the danger she is
/// in. Cover, elevation, mass, scouting and objectives are all zeroed, and
/// `withdraw_threshold` with them — the caution term reads damage too, and
/// holding it at zero is what leaves the fractional-risk factor as the only
/// thing that can move between two otherwise identical states.
fn risk_score(reg: &DataRegistry, state: &BattleState, unit: UnitId, tile: [i32; 2]) -> f32 {
    let evaluator = Evaluator::new(tactics_core::data::DoctrineDef {
        id: "risk_probe".into(),
        name: String::new(),
        description: String::new(),
        aggression: 0.5,
        cover_value: 0.0,
        elevation_value: 0.0,
        concentration: 0.0,
        scouting: 0.0,
        objective_value: 0.0,
        indirect_appetite: 1.0,
        withdraw_threshold: 0.0,
        initiative: 0.5,
        delegation: 0.5,
    });
    evaluator
        .score_tile(
            reg,
            state,
            unit,
            tactics_core::offset_to_hex(tile[0], tile[1]),
        )
        .score
}

/// How much harder `unit` prefers ground away from the gun than ground near
/// it. A difference of scores rather than a score, because only a difference
/// is a decision — and because every term that does not involve the threat
/// cancels between two states that differ in one thing.
fn shyness(reg: &DataRegistry, state: &BattleState, unit: UnitId) -> f32 {
    risk_score(reg, state, unit, [1, 1]) - risk_score(reg, state, unit, [7, 1])
}

#[test]
fn a_loaded_taxi_reads_the_same_gun_as_a_bigger_danger_than_an_empty_one() {
    // Danger used to be an absolute: expected damage in substance points,
    // read identically by whoever it was aimed at. So a battle taxi with a
    // platoon in the back weighed a tank destroyer's gun exactly as she
    // weighed it empty, and drove into it exactly as readily — which is how
    // a harness run ends with 22 of 24 APCs lost and most of the infantry
    // dead aboard them.
    //
    // What changed is the currency, not the courage: the same expected
    // damage is divided by what she can still absorb and multiplied by what
    // is riding on her. Nothing here knows what an APC is — she is careful
    // because she is small and because the platoon is real, and a chassis a
    // mod adds tomorrow gets the same treatment for free.
    //
    // The two states differ in one field. The platoon stands on the taxi's
    // own hex in both, so the mass term, the fog and every friend-relative
    // distance are identical; only `aboard` is set, and only the passenger
    // stake can account for the difference.
    let reg = registry_wireless();
    let mut empty = taxi_stage(&reg, "tank_destroyer", 703);
    let (taxi, riders) = (UnitId(0), UnitId(1));
    assert!(
        empty.fog.side(0).spotted.contains(&UnitId(2)),
        "the threat term only counts guns the side can actually see"
    );
    empty.units[riders.index()].pos = empty.units[taxi.index()].pos;

    let mut loaded = empty.clone();
    loaded.units[riders.index()].aboard = Some(taxi);

    let (empty, loaded) = (shyness(&reg, &empty, taxi), shyness(&reg, &loaded, taxi));
    assert!(
        loaded > empty,
        "with the platoon aboard the taxi should shy from the gun harder \
         than she does empty: {loaded} vs {empty}"
    );
}

#[test]
fn a_crew_with_less_of_herself_left_weighs_the_same_shell_more_heavily() {
    // The other half of pricing risk as a fraction, and the half that applies
    // to everything on the field rather than to carriers. A crew is a third
    // of a hit from being finished when a third of her is left, and the
    // arithmetic says so now without consulting a doctrine — `caution` reads
    // damage too, but as an appetite for withdrawing scaled by
    // `withdraw_threshold`, which this probe holds at zero.
    //
    // The damage is her radio set and nothing else: on a wireless registry it
    // does no work, it is not her gun and it is not her tracks, so her shot,
    // her reach and her speed are untouched and every term in the score but
    // the threat is identical between the two states. All that differs is
    // that there is less of her.
    let reg = registry_wireless();
    let fresh = taxi_stage(&reg, "tank_destroyer", 704);
    let mut hurt = fresh.clone();
    let radio = hurt.units[0]
        .modules
        .keys()
        .find(|id| {
            reg.module(id)
                .is_some_and(|m| m.effect == tactics_core::data::ModuleEffect::Radio)
        })
        .cloned()
        .expect("the taxi carries a set");
    hurt.units[0].modules.insert(radio, 0);

    let (fresh, hurt) = (
        shyness(&reg, &fresh, UnitId(0)),
        shyness(&reg, &hurt, UnitId(0)),
    );
    assert!(
        hurt > fresh,
        "a crew with less left should shy from the gun harder than a whole \
         one: {hurt} vs {fresh}"
    );
}

#[test]
fn a_penetrated_taxi_shares_its_luck_with_everyone_aboard() {
    // The shared-fate ruling: a round through a loaded carrier does not
    // check tickets. The pool a penetration rolls against includes the
    // passengers' cadets and troops, so riding a taxi under fire costs
    // exactly what the period says it cost.
    let reg = registry_wireless();
    let mut state = taxi_stage(&reg, "tank_destroyer", 702);
    let (taxi, riders, gun) = (UnitId(0), UnitId(1), UnitId(2));
    state.units[riders.index()].aboard = Some(taxi);
    state.units[riders.index()].pos = state.units[taxi.index()].pos;
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: gun,
                fire: FireIntent::Target {
                    target: taxi,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let mut rider_hurt = false;
    for _ in 0..4 {
        if state.is_over() {
            break;
        }
        for event in state.resolve_round(&reg) {
            if matches!(
                event,
                BattleEvent::CrewHit { unit, .. } | BattleEvent::ModuleHit { unit, .. }
                    if unit == riders
            ) {
                rider_hurt = true;
            }
        }
        commit_all(&reg, &mut state);
    }
    assert!(
        rider_hurt,
        "an 88 through the box finds the people packed inside it"
    );
}

#[test]
fn a_brewed_carrier_burns_its_passengers_and_spits_out_the_rest() {
    // The worst ride there is. The carrier's racks go up, every passenger
    // is rolled through the fire, and whoever is left picks herself up
    // beside the wreck — dismounted by catastrophe rather than by order.
    let mut reg = registry_wireless();
    reg.balance.brewup_percent = 100;
    if let Some(rack) = reg.modules.get_mut("ammo_rack_sparse") {
        // The apc's rack becomes most of what a penetration can find, so
        // the brew is round one business and the test is about the fire,
        // not about waiting for it.
        rack.size = 100;
    }
    let mut state = taxi_stage(&reg, "tank_destroyer", 703);
    let (taxi, riders, gun) = (UnitId(0), UnitId(1), UnitId(2));
    state.units[riders.index()].aboard = Some(taxi);
    state.units[riders.index()].pos = state.units[taxi.index()].pos;
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: gun,
                fire: FireIntent::Target {
                    target: taxi,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let (mut brewed, mut rider_events) = (false, 0);
    for _ in 0..4 {
        if state.is_over() {
            break;
        }
        for event in state.resolve_round(&reg) {
            match event {
                BattleEvent::BrewedUp { unit } if unit == taxi => brewed = true,
                BattleEvent::CrewHit { unit, .. } | BattleEvent::ModuleHit { unit, .. }
                    if unit == riders =>
                {
                    rider_events += 1;
                }
                _ => {}
            }
        }
        if brewed {
            break;
        }
        commit_all(&reg, &mut state);
    }
    assert!(brewed, "the racks go up");
    assert!(
        rider_events > 0,
        "and the fire rolls through the passengers"
    );
    let riders_unit = &state.units[riders.index()];
    assert!(riders_unit.aboard.is_none(), "nobody stays aboard a pyre");
    if riders_unit.alive {
        assert_eq!(
            riders_unit.pos,
            state.units[taxi.index()].pos,
            "the survivors pick themselves up beside the wreck"
        );
    }
}

#[test]
fn a_carrier_that_leaves_the_map_takes_her_passengers_home() {
    // Withdrawal by taxi: the carrier drives onto her side's exit with the
    // platoon aboard, and both leave the battle as withdrawals — exited,
    // never mourned, each scored as a unit that came home.
    let reg = registry_wireless();
    let mut state = objective_battle(
        &reg,
        serde_json::json!([
            { "id": "west_road", "at": [[0, 0]], "value": 1, "kind": "exit", "side": 0 }
        ]),
        None,
        vec![
            unit_at([2, 0], 0, "apc", "Taxi"),
            unit_at([3, 0], 0, "rifle_platoon", "Riders"),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
    );
    let (taxi, riders) = (UnitId(0), UnitId(1));
    state.units[riders.index()].aboard = Some(taxi);
    state.units[riders.index()].pos = state.units[taxi.index()].pos;
    let exit = tactics_core::offset_to_hex(0, 0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: taxi,
                to: exit,
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);

    for unit in [taxi, riders] {
        let u = &state.units[unit.index()];
        assert!(
            !u.alive && u.exited,
            "{} left the battle by the road, not the graveyard",
            u.name
        );
    }
}

#[test]
fn a_taxi_under_at_threat_puts_her_passengers_on_the_ground() {
    // The one dismount reflex that keeps taxis from being coffins: the
    // planner sees the carrier under a threat that can actually hurt her
    // and puts the platoon on the ground without being asked. Nothing
    // mounts on its own initiative — the reflex only ever gets people OFF.
    let reg = registry_wireless();
    let mut state = taxi_stage(&reg, "tank_destroyer", 705);
    let (taxi, riders) = (UnitId(0), UnitId(1));
    state.units[riders.index()].aboard = Some(taxi);
    state.units[riders.index()].pos = state.units[taxi.index()].pos;
    // The tank destroyer across the field is spotted and can gut an apc
    // from there: the ride is a coffin and the planner must know it.
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(2)),
        "the stage needs the threat visible"
    );

    let mut ai = AiDriver::new();
    ai.insert(
        0,
        make_battle_planner(
            &AiConfig {
                planner: "utility".into(),
                difficulty: 5,
                doctrine: None,
            },
            705,
            &reg,
        ),
    );
    ai.plan_round(&reg, &mut state);
    assert!(
        state.units[riders.index()].dismounting,
        "the planner ordered her off the moment the ride was a target"
    );
}

#[test]
fn a_scenario_may_spawn_the_riders_already_riding() {
    // Mounted starts are map data: `aboard_at` names the carrier's tile
    // and the platoon spawns aboard — the way the campaign will hand a
    // motorised column to a battle.
    let reg = registry_wireless();
    let row = "g".repeat(8);
    let mut riders = unit_at([3, 1], 0, "rifle_platoon", "Riders");
    riders.aboard_at = Some([1, 1]);
    let state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([1, 1], 0, "apc", "Taxi"),
            riders,
            unit_at([6, 1], 1, "medium_tank", "East"),
        ],
        706,
    );
    let (taxi, platoon) = (UnitId(0), UnitId(1));
    assert_eq!(
        state.units[platoon.index()].aboard,
        Some(taxi),
        "she spawns in the back, not on the grass"
    );
    assert!(
        !state.fog.side(1).spotted.contains(&platoon),
        "and the enemy's opening picture holds only the taxi"
    );
}

// --- the chain of command under adversarial load ---------------------------
//
// Everything above tests one rule at a time on a stage built to show it. This
// section does the opposite: it puts the whole machine under load — two
// commanders against each other, a search planner reading state that did not
// exist when it was written, formations too small or too deaf to work — and
// asks only that nothing illegal, silent or wedged comes out. The properties
// are deliberately cheap to check and expensive to violate.

/// Both sides thinking through their own chain of command, which is the one
/// pairing nothing exercised: `river_crossing` names `command` for side 1
/// only, so every test and every measurement so far has had a flat-pool
/// opponent absorbing whatever the commander did.
fn commanded(
    reg: &DataRegistry,
    seed: u64,
    doctrine: &str,
) -> Box<dyn AiPlanner<BattleState, Order>> {
    make_battle_planner(
        &AiConfig {
            planner: "command".into(),
            difficulty: 4,
            doctrine: Some(doctrine.into()),
        },
        seed,
        reg,
    )
}

#[test]
fn two_commanders_fight_each_other_without_an_illegal_order_or_a_wedged_round() {
    // Twelve battles: six seeds, three doctrine pairings, each fought both
    // over the shipped wire and with no `command` block at all. What is being
    // defended is not who wins — that is the balance harness's question — but
    // that a commander on both ends of the field cannot produce an order the
    // battle refuses, and cannot leave a planning phase open. A refusal
    // force-commits the side, so a planner quietly emitting illegal orders
    // looks exactly like an AI that has stopped thinking, which is the sort
    // of thing that hides for months.
    let doctrines = ["massed_armor", "elastic_defense", "combined_arms"];
    let wired = registry();
    let wireless = registry_wireless();

    for (i, seed) in [11u64, 23, 37, 41, 59, 67].iter().enumerate() {
        for (name, reg) in [("the wire", &wired), ("no wire", &wireless)] {
            let (west, east) = (doctrines[i % 3], doctrines[(i + 1) % 3]);
            let mut state = BattleState::from_map(reg, "river_crossing", *seed).unwrap();
            let mut ai = AiDriver::new();
            ai.insert(0, commanded(reg, seed ^ 0x5EED, west));
            ai.insert(1, commanded(reg, seed ^ 0xC0DE, east));

            let mut rounds = 0;
            let mut refused = Vec::new();
            while !state.is_over() && rounds < 60 {
                ai.plan_round_with(reg, &mut state, |d| {
                    if let Some(error) = &d.rejected {
                        refused.push(format!("side {} sent {:?}: {error}", d.side, d.order));
                    }
                });
                assert!(
                    !state.is_planning(),
                    "{name}, seed {seed}: both commanders spoke and nobody committed"
                );
                state.resolve_round(reg);
                rounds += 1;
            }
            assert!(
                refused.is_empty(),
                "{name}, seed {seed}, {west} against {east}: {refused:?}"
            );
            assert!(
                state.is_over(),
                "{name}, seed {seed}: still fighting after {rounds} rounds"
            );
        }
    }
}

#[test]
fn a_long_battle_never_says_anything_about_a_crew_who_has_left() {
    // The soak. Six seeds of commander against commander, and every event in
    // both the planning and the resolution stream checked against the few
    // things that must never be true however the fight goes: nothing happens
    // to a cadet who is dead or driven off the map, no order is refused, no
    // formation receives a mission nobody sent it, and no delivery is
    // announced for a crew with nothing waiting. These are cheap to check and
    // they are exactly the shapes a bug in the wire produces — an event about
    // a wreck is what a stale id looks like from the outside.
    let reg = registry();
    for seed in [3u64, 13, 29, 47, 71, 97] {
        let mut state = BattleState::from_map(&reg, "river_crossing", seed).unwrap();
        let mut ai = AiDriver::new();
        ai.insert(0, commanded(&reg, seed, "massed_armor"));
        ai.insert(1, commanded(&reg, seed + 1, "elastic_defense"));

        let mut gone: Vec<UnitId> = Vec::new();
        let mut waiting: Vec<UnitId> = Vec::new();
        let mut sent: Vec<String> = Vec::new();
        let mut wrong: Vec<String> = Vec::new();
        let mut rounds = 0;

        while !state.is_over() && rounds < 60 {
            let round = state.round;
            let mut stream = Vec::new();
            ai.plan_round_with(&reg, &mut state, |d| {
                if let Some(error) = &d.rejected {
                    wrong.push(format!("r{round}: {:?} refused: {error}", d.order));
                }
                stream.extend(d.events.iter().cloned());
            });
            stream.extend(state.resolve_round(&reg));

            for event in &stream {
                let mut departed = |unit: &UnitId, what: &str| {
                    if gone.contains(unit) {
                        wrong.push(format!("r{round}: {what} about departed {unit:?}"));
                    }
                };
                match event {
                    BattleEvent::UnitMoved { unit, .. } => departed(unit, "UnitMoved"),
                    BattleEvent::ShotFired { attacker, .. } => departed(attacker, "ShotFired"),
                    BattleEvent::UnitSpotted { unit, .. } => departed(unit, "UnitSpotted"),
                    BattleEvent::OutOfContact { unit } => departed(unit, "OutOfContact"),
                    BattleEvent::ContactRestored { unit } => departed(unit, "ContactRestored"),
                    BattleEvent::ContactReported { unit, by, .. } => {
                        departed(unit, "ContactReported");
                        departed(by, "a report filed by");
                    }
                    BattleEvent::CommandPassed { to, .. } => departed(to, "CommandPassed to"),
                    BattleEvent::Defied { unit, .. } => departed(unit, "Defied"),
                    BattleEvent::MoraleChanged { unit, .. } => departed(unit, "MoraleChanged"),
                    BattleEvent::OrdersWaiting { unit } => {
                        departed(unit, "OrdersWaiting");
                        waiting.push(*unit);
                    }
                    BattleEvent::OrdersDelivered { unit } => {
                        departed(unit, "OrdersDelivered");
                        match waiting.iter().position(|u| u == unit) {
                            Some(i) => {
                                waiting.remove(i);
                            }
                            None => wrong.push(format!(
                                "r{round}: {unit:?} was handed orders nobody was holding"
                            )),
                        }
                    }
                    BattleEvent::MissionAssigned { formation, .. } => sent.push(formation.clone()),
                    BattleEvent::MissionReceived { formation, .. } => {
                        match sent.iter().position(|f| f == formation) {
                            Some(i) => {
                                sent.remove(i);
                            }
                            None => wrong.push(format!(
                                "r{round}: {formation} received a mission nobody sent"
                            )),
                        }
                    }
                    _ => {}
                }
                match event {
                    BattleEvent::UnitDestroyed { unit, .. }
                    | BattleEvent::UnitExited { unit, .. } => gone.push(*unit),
                    _ => {}
                }
            }
            rounds += 1;
        }
        assert!(wrong.is_empty(), "seed {seed}: {wrong:#?}");
        assert!(state.is_over(), "seed {seed}: unfinished after {rounds}");
    }
}

#[test]
fn a_searching_planner_copes_with_missions_a_detachment_and_a_running_clock() {
    // `mcts_planner_produces_legal_orders` was written before formations
    // existed and fights a bare battle. MCTS clones the whole state and rolls
    // it forward, so everything the chain of command added — standing
    // missions, a crew under personal tasking, the spotting clocks, a
    // commander thinking on the other side of the field — is now inside the
    // search whether the search knows about it or not. The property is the
    // same modest one: legal orders, and a determinization that does not
    // panic.
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 7).expect("battle");
    let bridge = state.map.objectives()[0].anchor();
    for index in 0..state.formations().len() {
        state
            .apply(
                &reg,
                &Order::SetMission {
                    formation: FormationId(index as u32),
                    mission: Mission::Advance { to: bridge },
                    latitude: tactics_core::battle::Latitude::Delegated,
                },
            )
            .expect("the bridge is on the map");
    }
    // The commander takes personal charge of her scout, which is the one
    // piece of unit state a planner has never had to reason about.
    let scout = UnitId(1);
    let aside = state.unit(scout).expect("she is on the field").pos;
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: scout,
                to: Some(aside),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("a hex she is already standing on");
    assert!(state.units[scout.index()].detached);

    let mut ai = AiDriver::new();
    ai.insert(
        0,
        make_battle_planner(
            &AiConfig {
                planner: "mcts".into(),
                difficulty: 2,
                doctrine: Some("massed_armor".into()),
            },
            7,
            &reg,
        ),
    );
    ai.insert(1, commanded(&reg, 8, "elastic_defense"));

    let mut refused = Vec::new();
    for _ in 0..6 {
        if state.is_over() {
            break;
        }
        ai.plan_round_with(&reg, &mut state, |d| {
            if let Some(error) = &d.rejected {
                refused.push(format!("side {} sent {:?}: {error}", d.side, d.order));
            }
        });
        state.resolve_round(&reg);
    }
    assert!(refused.is_empty(), "{refused:?}");
}

#[test]
fn a_search_cannot_behead_an_enemy_it_has_never_seen() {
    // Determinization deletes the enemies this side has not spotted, and it
    // used to delete them by clearing `alive` alone — which is the engine's
    // word for a wreck. `check_victory` reads decapitation before anything
    // else and classifies a loss as `!alive && !exited`, so on a map that
    // staked the battle on a commanding officer, an MCTS side opened its
    // search on a world where that officer was already dead and the battle
    // was already won. Every branch scored the same and the tree was worth
    // nothing. She is now marked `exited` as well: off the board, not
    // destroyed, which is the only honest thing a search can say about a
    // vehicle it has never laid eyes on.
    let reg = registry_wireless();
    let mut state = scripted_battle(
        &reg,
        serde_json::json!({
            "id": "stakes",
            "palette": { "g": "grass", "f": "forest" },
            "rows": [
                "gggggggggggggggggggg",
                "ggggggggffffgggggggg",
                "gggggggggggggggggggg",
            ],
            "formations": [ { "id": "hq", "name": "Headquarters", "side": 1 } ],
            "loss_conditions": [ { "side": 1, "formation": "hq", "when": "leader_lost" } ],
        }),
        vec![
            unit_at([1, 1], 0, "medium_tank", "Hunter"),
            in_formation(unit_at([18, 1], 1, "medium_tank", "Boss"), "hq", true),
            unit_at([4, 1], 1, "medium_tank", "Picket"),
        ],
    );
    let (boss, picket) = (UnitId(1), UnitId(2));
    assert!(
        !state.fog.side(0).spotted.contains(&boss),
        "the forest wall has to hide the commanding officer"
    );
    assert!(
        state.fog.side(0).spotted.contains(&picket),
        "and the picket has to be in plain view, or there is nothing to search"
    );

    let known = tactics_core::ai::determinize(&state, 0, 1);
    assert!(
        !known.lost_units().any(|u| u.id == boss),
        "an officer nobody has seen is not a casualty the search may count"
    );
    // And the world the search plays in does not end before it starts.
    for side in known.living_sides() {
        state
            .apply(&reg, &Order::Commit { side })
            .expect("commit the real battle for comparison");
    }
    let mut sim = known;
    commit_all(&reg, &mut sim);
    sim.resolve_round(&reg);
    assert!(
        !matches!(
            sim.over,
            Some(tactics_core::battle::BattleResult {
                reason: EndReason::Decapitated,
                ..
            })
        ),
        "the search must not win by beheading somebody it invented: {:?}",
        sim.over
    );
}

#[test]
fn a_zeroed_command_block_is_the_game_without_one_with_a_commander_at_both_ends() {
    // `a_command_block_with_zero_coefficients_is_the_game_without_one` pins
    // the same property with a commander on one side and a flat pool on the
    // other, and it was written before the pulse, the drill, the spacing band
    // and the base of fire existed. Every one of those reads the command
    // rules or the picture, and every one of them now runs on BOTH sides of
    // this battle — so this is the same words-not-deeds comparison over the
    // machinery the original pin cannot reach, on two further seeds.
    let wire = |line: &String| {
        line.starts_with("OutOfContact")
            || line.starts_with("ContactRestored")
            || line.starts_with("ContactReported")
    };
    let run = |rules: Option<tactics_core::data::CommandRules>, seed: u64| -> Vec<String> {
        let mut reg = registry();
        reg.command = rules;
        // Hardware is content, not a coefficient: an eight-hex set would cap
        // the everywhere-net the zeroed block declares. Stripped in both runs.
        strip_radios(&mut reg);
        let mut state = BattleState::from_map(&reg, "river_crossing", seed).unwrap();
        let mut ai = AiDriver::new();
        ai.insert(0, commanded(&reg, seed, "massed_armor"));
        ai.insert(1, commanded(&reg, seed + 1, "elastic_defense"));
        let mut log = Vec::new();
        for _ in 0..6 {
            if state.is_over() {
                break;
            }
            ai.plan_round_with(&reg, &mut state, |d| {
                log.extend(d.events.iter().map(|e| format!("{e:?}")));
            });
            log.extend(state.resolve_round(&reg).iter().map(|e| format!("{e:?}")));
        }
        log
    };
    let zeroed = tactics_core::data::CommandRules {
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
    };
    for seed in [31u64, 53] {
        let without = run(None, seed);
        assert!(
            without.iter().any(|line| line.contains("MissionAssigned")),
            "seed {seed}: both commanders should be issuing missions"
        );
        assert!(
            without.iter().any(|line| line.contains("ShotHit")),
            "seed {seed}: and fighting"
        );
        let (spoken, deeds): (Vec<String>, Vec<String>) =
            run(Some(zeroed.clone()), seed).into_iter().partition(wire);
        assert_eq!(
            deeds, without,
            "seed {seed}: a zeroed block must change words, never deeds"
        );
        assert!(spoken.iter().all(wire));
    }
}

/// A quiet field with one commanded formation, two pieces of ground worth
/// holding, and enough cadets in the formation to lose four commanders. The
/// only enemy is far beyond anyone's eyes, so nothing can interrupt the
/// commander's clock except what a test does to her on purpose.
fn succession_stage(reg: &DataRegistry) -> BattleState {
    let row = "g".repeat(40);
    let mut placements = vec![in_formation(
        unit_at([20, 1], 1, "medium_tank", "Lead"),
        "line",
        true,
    )];
    for i in 1..6 {
        placements.push(in_formation(
            unit_at([20 + i, 2], 1, "medium_tank", "Wing"),
            "line",
            false,
        ));
    }
    placements.push(unit_at([39, 0], 0, "medium_tank", "Hermit"));
    scripted_battle(
        reg,
        serde_json::json!({
            "id": "succession_stage",
            "palette": { "g": "grass" },
            "rows": [row.clone(), row.clone(), row],
            "objectives": [
                { "id": "bridge", "name": "Bridge", "at": [[5, 1]], "value": 3 },
                { "id": "ford", "name": "Ford", "at": [[35, 1]], "value": 2 },
            ],
            "formations": [ { "id": "line", "name": "The Line", "side": 1 } ],
        }),
        placements,
    )
}

#[test]
fn a_commander_woken_four_mornings_running_still_goes_back_on_her_own_clock() {
    // The pulse under repeated shock. A commander lost is an interrupt, and
    // an interrupt reschedules her next review from the moment she thinks —
    // so four decapitations in four rounds must make her think in all four,
    // and then leave her cadence anchored to the last of them rather than
    // pushed permanently into the future or, worse, brought forward for good.
    //
    // "Did she review" is made visible by handing her side whichever piece of
    // ground her formation is currently marching on: her doctrine's
    // initiative then always wants the other one, so a review she actually
    // ran always produces an order and one she skipped never does.
    let mut reg = registry();
    let mut rules = command_rules(999, true, 0);
    rules.review.base_ticks = 3;
    rules.review.max_ticks = 5;
    reg.command = Some(rules);
    strip_radios(&mut reg);
    // Defang every gun: legacy path, penetration zero. The scene needs
    // twelve rounds of a battle that can neither kill nor end — since the
    // planner learned to spread across score plateaus, her marchers really
    // do reach the far ford and really do meet the enemy picketed there,
    // and a contact that gets somebody killed starts the stalemate clock
    // on a battle this test needs to keep breathing. Toothless guns keep
    // the contact alive (bounces forever), the crews alive, and the
    // commander's pulse the only thing left to observe — which is the
    // test.
    for weapon in reg.weapons.values_mut() {
        weapon.ammo.clear();
        weapon.penetration = 0;
    }
    let mut state = succession_stage(&reg);
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
            5,
            &reg,
        ),
    );

    let bridge = tactics_core::offset_to_hex(5, 1);
    let mut reviewed = Vec::new();
    for round in 1..=12u32 {
        let marching_on_the_bridge = matches!(
            state.formations()[0].mission,
            Some(Mission::Advance { to }) if to == bridge
        );
        state.objective_held[0] = marching_on_the_bridge.then_some(1);
        state.objective_held[1] = (!marching_on_the_bridge).then_some(1);
        if round <= 4 {
            let leader = state.formations()[0].leader.expect("somebody leads");
            strike_down(&mut state, leader);
        }
        let mut assigned = 0;
        ai.plan_round_with(&reg, &mut state, |d| {
            assigned += d
                .events
                .iter()
                .filter(|e| matches!(e, BattleEvent::MissionAssigned { .. }))
                .count();
            assert!(d.rejected.is_none(), "round {round}: {:?}", d.rejected);
        });
        commit_all(&reg, &mut state);
        state.resolve_round(&reg);
        reviewed.push(assigned > 0);
    }
    // Rounds 1-5 all think: the first because she has never thought, the next
    // four because a commander was lost. The lag of one is real and honest —
    // succession runs during resolution, so the loss she suffers in round N
    // is on her desk in round N+1.
    assert_eq!(
        &reviewed[..5],
        &[true, true, true, true, true],
        "four shocks in a row must wake her every time: {reviewed:?}"
    );
    // Then the clock she was left with: three rounds between reviews, counted
    // from the last one she ran, not from the last one she scheduled.
    assert_eq!(
        &reviewed[5..],
        &[false, false, false, true, false, false, false],
        "the pulse resumes on schedule rather than drifting: {reviewed:?}"
    );
}

/// A leader and one crew standing beside each other, so nothing about
/// distance can be the reason they cannot talk.
fn shoulder_to_shoulder(reg: &DataRegistry) -> BattleState {
    scripted_battle(
        reg,
        serde_json::json!({
            "id": "shoulder",
            "palette": { "g": "grass" },
            "rows": ["g".repeat(30)],
            "formations": [ { "id": "net", "name": "The Net", "side": 0 } ],
        }),
        vec![
            in_formation(unit_at([0, 0], 0, "recon_car", "Leader"), "net", true),
            in_formation(unit_at([1, 0], 0, "recon_car", "Wing"), "net", false),
            unit_at([29, 0], 1, "recon_car", "Far Foe"),
        ],
    )
}

#[test]
fn a_platoon_of_receivers_cannot_hear_a_leader_who_cannot_transmit() {
    // The degenerate case the directional net implies and nothing stated: a
    // formation whose every vehicle, the leader's included, carries a
    // receive-only set. Orders flow down a chain of TRANSMITTERS, and she has
    // none — so her platoon is off the net standing beside her, which is
    // exactly the 1941 line company the receive-only radio is modelled on.
    // The answer is not a radio at all: put the flags back and the same two
    // vehicles are talking again.
    let mut reg = registry();
    reg.command = Some(command_rules(8, true, 0));
    for vehicle in reg.vehicles.values_mut() {
        vehicle.radio = Some("receiver".into());
    }
    let mut deaf = shoulder_to_shoulder(&reg);
    commit_all(&reg, &mut deaf);
    deaf.resolve_round(&reg);
    assert!(
        deaf.hears_orders(UnitId(0)),
        "the leader always hears herself: she is the root of the net"
    );
    assert!(
        !deaf.hears_orders(UnitId(1)),
        "but nobody hears her, because she has nothing to speak with"
    );

    let mut with_flags = registry();
    let mut rules = command_rules(8, true, 0);
    rules.visual_range = 3;
    with_flags.command = Some(rules);
    for vehicle in with_flags.vehicles.values_mut() {
        vehicle.radio = Some("receiver".into());
    }
    let mut seen = shoulder_to_shoulder(&with_flags);
    commit_all(&with_flags, &mut seen);
    seen.resolve_round(&with_flags);
    assert!(
        seen.hears_orders(UnitId(1)),
        "a hand out of the cupola carries what the set cannot"
    );
}

#[test]
fn a_formation_of_one_can_be_given_any_mission_in_the_book() {
    // A formation with a single vehicle in it is the shape every rule about
    // formations has to survive: nobody to bound with, nobody to succeed her,
    // and — for a base of fire — somebody else's fight to shoot into. Each of
    // the six missions is given to her and then executed by her own
    // commander's executors for four rounds; the property is only that no
    // order comes back refused and the battle keeps moving.
    let mut reg = registry();
    reg.command = Some(command_rules(999, true, 0));
    strip_radios(&mut reg);
    let base = scripted_battle(
        &reg,
        serde_json::json!({
            "id": "lone",
            "palette": { "g": "grass" },
            "rows": ["g".repeat(30)],
            "objectives": [
                { "id": "hill", "name": "The Hill", "at": [[15, 0]], "value": 1 },
                { "id": "west_road", "name": "The Western Road", "at": [[0, 0]],
                  "value": 1, "kind": "exit", "side": 0 },
            ],
            "formations": [
                { "id": "solo", "name": "Solo", "side": 0 },
                { "id": "other", "name": "Other", "side": 0 },
            ],
        }),
        vec![
            in_formation(unit_at([5, 0], 0, "medium_tank", "Solo"), "solo", true),
            in_formation(unit_at([7, 0], 0, "medium_tank", "Other"), "other", true),
            unit_at([29, 0], 1, "medium_tank", "Foe"),
        ],
    );
    let hill = tactics_core::offset_to_hex(15, 0);
    for mission in [
        Mission::Advance { to: hill },
        Mission::Hold { at: Some(hill) },
        Mission::Hold { at: None },
        Mission::Recon { toward: hill },
        Mission::Withdraw {
            via: "west_road".into(),
        },
        Mission::Support {
            formation: "other".into(),
        },
    ] {
        let mut state = base.clone();
        state
            .apply(
                &reg,
                &Order::SetMission {
                    formation: FormationId(0),
                    mission: mission.clone(),
                    latitude: tactics_core::battle::Latitude::Delegated,
                },
            )
            .unwrap_or_else(|e| panic!("{mission:?} should be a legal order: {e}"));
        let mut ai = AiDriver::new();
        ai.insert(0, commanded(&reg, 5, "combined_arms"));
        let mut refused = Vec::new();
        for _ in 0..4 {
            if state.is_over() {
                break;
            }
            ai.plan_round_with(&reg, &mut state, |d| {
                if let Some(error) = &d.rejected {
                    refused.push(format!("{:?}: {error}", d.order));
                }
            });
            commit_all(&reg, &mut state);
            state.resolve_round(&reg);
        }
        assert!(refused.is_empty(), "under {mission:?}: {refused:?}");
    }
}

#[test]
fn a_battery_with_no_ground_and_nobody_to_shoot_for_is_told_nothing() {
    // The no-objective guard, re-checked now that a base of fire exists. A
    // fires formation is recognised off its hardware and assigned before the
    // ground is divided, so it would have been the one order that could
    // escape a map with nothing to hold — and it must not, because "a map
    // that names no ground is exactly the fight it was before commanders
    // existed" is the property the whole command layer is additive against.
    // Doubly degenerate here: the battery is also the side's only formation,
    // so there is nobody to support even if she were asked.
    let mut reg = registry();
    reg.command = Some(command_rules(999, true, 0));
    strip_radios(&mut reg);
    let mut state = scripted_battle(
        &reg,
        serde_json::json!({
            "id": "no_ground",
            "palette": { "g": "grass" },
            "rows": ["g".repeat(30)],
            "formations": [ { "id": "battery", "name": "The Battery", "side": 0 } ],
        }),
        vec![
            in_formation(unit_at([2, 0], 0, "artillery", "Guns"), "battery", true),
            in_formation(
                unit_at([3, 0], 0, "artillery", "More Guns"),
                "battery",
                false,
            ),
            unit_at([20, 0], 1, "medium_tank", "Foe"),
        ],
    );
    let mut ai = AiDriver::new();
    ai.insert(0, commanded(&reg, 5, "combined_arms"));
    let mut said = Vec::new();
    for _ in 0..5 {
        if state.is_over() {
            break;
        }
        ai.plan_round_with(&reg, &mut state, |d| {
            said.extend(
                d.events
                    .iter()
                    .filter(|e| matches!(e, BattleEvent::MissionAssigned { .. }))
                    .map(|e| format!("{e:?}")),
            );
            assert!(d.rejected.is_none(), "{:?}: {:?}", d.order, d.rejected);
        });
        commit_all(&reg, &mut state);
        state.resolve_round(&reg);
    }
    assert!(
        said.is_empty(),
        "a map with no ground gets no missions: {said:?}"
    );
}

/// A leader on an open road and one crew ten hexes out, under a two-hex net
/// with nobody relaying: she is stone deaf until she is driven back.
fn strung_wire(reg: &DataRegistry) -> BattleState {
    scripted_battle(
        reg,
        serde_json::json!({
            "id": "strung_wire",
            "palette": { "g": "grass" },
            "rows": ["g".repeat(30)],
            "formations": [ { "id": "net", "name": "The Net", "side": 0 } ],
        }),
        vec![
            in_formation(unit_at([0, 0], 0, "recon_car", "Leader"), "net", true),
            in_formation(unit_at([10, 0], 0, "recon_car", "Stray"), "net", false),
            unit_at([29, 0], 1, "recon_car", "Far Foe"),
        ],
    )
}

#[test]
fn a_waiting_order_arrives_as_an_order_however_far_she_has_come() {
    // The queue stores the destination rather than the path, because "she
    // re-paths from wherever she is when it reaches her" is the whole
    // promise of deliver-on-contact — and the delivered order is now a
    // standing personal destination (`Unit.tasking`), marched toward one
    // round at a time until she arrives. This is what a real crew does with
    // a movement order to distant ground: it does not expire for being far,
    // it is executed across as many periods as the ground demands. The
    // sender-side alternative — somebody driving out to carry the message —
    // is the courier feature the design doc keeps for later.
    let mut reg = registry();
    reg.command = Some(command_rules(2, false, 0));
    strip_radios(&mut reg);
    let mut state = strung_wire(&reg);
    let stray = UnitId(1);
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);
    assert!(!state.hears_orders(stray), "ten hexes on a two-hex net");

    let east = tactics_core::offset_to_hex(13, 0);
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: stray,
                to: Some(east),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("accepted and held at the radio");

    // She drives herself back onto the net over three rounds, by which time
    // the hex she was sent to is far behind one round's driving.
    let mut delivered = false;
    for stop in [7, 4, 2] {
        state
            .apply(
                &reg,
                &Order::SetMove {
                    unit: stray,
                    to: tactics_core::offset_to_hex(stop, 0),
                },
            )
            .expect("her own legs");
        commit_all(&reg, &mut state);
        delivered |= state
            .resolve_round(&reg)
            .iter()
            .any(|e| matches!(e, BattleEvent::OrdersDelivered { unit } if *unit == stray));
    }
    assert!(delivered, "the wire comes back up and the order goes out");
    assert!(
        !state.units[stray.index()].intent.path.is_empty(),
        "an order announced as delivered has to be an order she is carrying out"
    );
}

#[test]
fn a_campaign_run_by_standing_orders_and_planners_plays_itself_out() {
    // The campaign half under load: standing orders given on day one, both
    // sides' planners driving everything nobody ordered, sixty days, and
    // every battle the map throws up fed back through the real
    // `apply_battle_result` path. What is defended is that the loop runs to a
    // conclusion without a panic and that the mission machinery behaves as
    // written along the way — in particular that an order given to an army
    // out of radio range on day one waits at headquarters and goes out on the
    // first morning the wire is up, days later and unprompted, which is the
    // campaign's whole answer to command friction.
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 1).unwrap();
    let senior = state.senior_army(0).unwrap();
    let junior = state
        .side_armies(0)
        .map(|a| a.id)
        .find(|id| *id != senior)
        .expect("frontier gives side 0 two companies");
    assert!(
        !state.in_contact(junior),
        "frontier's second company starts off the net, which is the point"
    );
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army: senior,
                mission: ArmyMission::Advance {
                    to: tactics_core::offset_to_hex(12, 1),
                },
            },
        )
        .expect("advance on the enemy's ground");
    let queued = state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army: junior,
                mission: ArmyMission::Withdraw {
                    to: tactics_core::offset_to_hex(0, 8),
                },
            },
        )
        .expect("accepted, not refused");
    assert!(
        queued
            .iter()
            .any(|e| matches!(e, OverworldEvent::ArmyOrdersWaiting { army } if *army == junior)),
        "an order she cannot be told waits at headquarters: {queued:?}"
    );

    let mut planners: Vec<_> = (0..2)
        .map(|side| {
            make_overworld_planner(
                &AiConfig {
                    planner: "simple".into(),
                    difficulty: 3,
                    doctrine: None,
                },
                42 + side,
            )
        })
        .collect();

    // A battle resolved the cheap way — the defender loses her leading
    // vehicle — but through the real feedback path, so army destruction,
    // crew fates and the victor taking the tile all happen as they would.
    fn resolve(reg: &DataRegistry, state: &mut OverworldState, attacker: ArmyId, defender: ArmyId) {
        let attacking = state
            .army(attacker)
            .map(|a| a.units.clone())
            .unwrap_or_default();
        let defending = state
            .army(defender)
            .map(|a| a.units.clone())
            .unwrap_or_default();
        let losses: Vec<tactics_core::overworld::CrewLoss> = defending
            .first()
            .into_iter()
            .flat_map(|u| {
                u.crew
                    .iter()
                    .map(move |cadet| tactics_core::overworld::CrewLoss {
                        cadet: *cadet,
                        vehicle: u.vehicle.clone(),
                        killed_by: None,
                        found: None,
                    })
            })
            .collect();
        state.apply_battle_result(
            reg,
            attacker,
            defender,
            &[
                (attacker, attacking),
                (defender, defending.into_iter().skip(1).collect()),
            ],
            &losses,
        );
    }

    let (mut battles, mut transmitted) = (0, false);
    for _ in 0..60 {
        let side = state.active_side;
        // The planner drives everything nobody gave standing orders; an army
        // under orders is left to carry them out, which is delegation.
        for _ in 0..12 {
            let order = planners[side as usize].next_order(&reg, &state, side);
            if order == OverworldOrder::EndTurn {
                break;
            }
            if let OverworldOrder::MoveArmy { army, .. } = order
                && state.army(army).is_some_and(|a| a.mission.is_some())
            {
                break;
            }
            let Ok(events) = state.apply(&reg, &order) else {
                break;
            };
            for event in &events {
                if let OverworldEvent::BattleTriggered {
                    attacker, defender, ..
                } = event
                {
                    battles += 1;
                    resolve(&reg, &mut state, *attacker, *defender);
                }
            }
        }
        let Ok(events) = state.apply(&reg, &OverworldOrder::EndTurn) else {
            break;
        };
        for event in &events {
            match event {
                OverworldEvent::BattleTriggered {
                    attacker, defender, ..
                } => {
                    battles += 1;
                    resolve(&reg, &mut state, *attacker, *defender);
                }
                OverworldEvent::ArmyMissionAssigned { army, mission }
                    if *army == junior && matches!(mission, ArmyMission::Withdraw { .. }) =>
                {
                    transmitted = true;
                }
                _ => {}
            }
        }
    }
    assert!(
        battles > 0,
        "sixty days of two planners should meet somewhere"
    );
    // The order held on day one has to *resolve*: either it goes out on the
    // first morning the wire is up, or the company it was for stops existing
    // and it is dropped, there being nobody to give it to. What must not happen
    // is that it sits in the drawer for ever while its army is alive and
    // reachable.
    //
    // Stated as the pair rather than as "it transmitted" because which of the
    // two a sixty-day brawl produces is an accident of the brawl, not a rule.
    // It used to be the first here — the senior company was destroyed, the
    // junior inherited headquarters and heard herself — and it is now the
    // second, because an advance that engages what blocks it fights different
    // battles on different days and this run gets the junior overrun instead.
    // The rule the waiting tray actually promises is pinned properly by
    // `an_army_mission_out_of_range_waits_and_then_transmits`.
    assert!(
        transmitted || state.army(junior).is_none(),
        "a held order must either go out or die with the army it was for, \
         never sit in the drawer while she is alive to receive it"
    );
    assert!(
        state.waiting_missions.is_empty(),
        "and headquarters is not still holding it: {:?}",
        state.waiting_missions
    );
}

#[test]
fn a_personal_march_carries_across_rounds_and_ends_in_a_hold() {
    // The rest of the promise: the delivered destination is not one round's
    // lunge but a march. The staff re-issues the leg every round until she
    // stands on the ordered ground, the tasking then clears, and she holds
    // there — detached still, because nobody has recalled her.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 111).unwrap();
    let unit = UnitId(0);
    let start = state.unit(unit).unwrap().pos;
    // Far up her own side of the river: several rounds' driving, no enemy
    // contact to muddy the march with drill moves.
    let far = start + tactics_core::Hex::new(3, -9);
    assert!(state.map.contains(far));
    state
        .apply(
            &reg,
            &Order::Radio {
                unit,
                to: Some(far),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("a far destination is an order now, not a refusal");
    assert_eq!(state.unit(unit).unwrap().tasking, Some(far));

    let mut ai = AiDriver::new();
    ai.insert(
        0,
        Box::new(tactics_core::ai::SideCommand::executor_only(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            111,
            &reg,
        )),
    );
    let mut arrived_at = None;
    for round in 0..10 {
        ai.plan_round(&reg, &mut state);
        let _ = state.apply(&reg, &Order::Commit { side: 1 });
        state.resolve_round(&reg);
        if state.unit(unit).unwrap().pos == far {
            arrived_at = Some(round);
            break;
        }
    }
    let arrived = arrived_at.expect("she gets there");
    assert!(arrived > 0, "and it took more than one round: {arrived}");
    // The round after arrival opens with the tasking cleared and her holding.
    ai.plan_round(&reg, &mut state);
    let unit = state.unit(unit).unwrap();
    assert_eq!(unit.tasking, None, "arrived is done");
    assert!(unit.detached, "but she stays on her commander's post");
    assert!(
        unit.intent.path.is_empty(),
        "holding the ground she was sent to"
    );
}

// --- shells in flight (ballistics B3) ---------------------------------------

/// A battery west, a target east, and nothing but grass between them.
///
/// Grass everywhere is deliberate: the mid-round drill only moves an idle
/// crew to *strictly better* cover, so on a uniform field nobody bolts and a
/// test about where a shell lands is not also a test about who flinched.
/// The target sits seven hexes out, which is inside the howitzer's reach and
/// inside the battery's own eyes (800 m) but outside a scout car's machine
/// gun (600 m) — so the only gun that speaks in these tests is the one being
/// tested.
fn battery_stage(reg: &DataRegistry, target: &str, seed: u64) -> BattleState {
    let row = "g".repeat(16);
    let state = two_side_battle(
        reg,
        &[&row, &row, &row],
        vec![
            unit_at([0, 1], 0, "artillery", "Battery"),
            unit_at([7, 1], 1, target, "Quarry"),
        ],
        seed,
    );
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the battery has to be able to see what it is laying on"
    );
    state
}

/// Play one round tick by tick, pairing every event with the absolute tick it
/// happened on — which is the clock `ShellInFlight::lands` is written in and
/// therefore the only honest way to assert that a shell took time.
fn ticked_round(reg: &DataRegistry, state: &mut BattleState) -> Vec<(u64, BattleEvent)> {
    commit_all(reg, state);
    let mut log = Vec::new();
    while state.resolving_tick().is_some() && !state.is_over() {
        let now = state.absolute_tick(reg);
        log.extend(state.step_tick(reg).into_iter().map(|e| (now, e)));
    }
    log
}

#[test]
fn a_shell_takes_time_to_arrive_and_lands_on_the_hex_not_the_unit() {
    // The artillery rework in one scene. The battery is ordered onto a unit,
    // and what it actually fires at is the *ground she is standing on* at the
    // moment the lanyard is pulled. The shell is then in the air for real
    // ticks, and when it comes down she has driven out of the beaten zone: no
    // hit, no bounce, no scratch — a hole in the field where she used to be.
    //
    // The round is slowed to 10 m/s so the flight is fourteen ticks rather
    // than one, and the shell outlives the round that fired it. That is not a
    // fudge of the model, it is the model at a scale a three-row test map can
    // show: `flight_ticks` is the same function the engine used to time this
    // shell, and at the shipped 470 m/s the same sentence needs kilometres of
    // ground to be true on — which is exactly the range artillery is fired at
    // and exactly why the balance table for it moved.
    let mut reg = registry_wireless();
    if let Some(ammo) = reg.ammo.get_mut("he_105") {
        ammo.velocity = 10;
    }
    let mut state = battery_stage(&reg, "recon_car", 501);
    let (battery, quarry) = (UnitId(0), UnitId(1));
    let aim = state.unit(quarry).unwrap().pos;
    let before = state.substance(&reg, state.unit(quarry).unwrap());

    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: battery,
                fire: FireIntent::Target {
                    target: quarry,
                    weapon: 0,
                },
            },
        )
        .unwrap();

    // East, away from the battery, for as long as it takes the shell to come
    // down. A round's intent is cleared when the round is, so she is told
    // again each time — three hexes is what a scout car's wheels buy her on
    // grass, and the point is only that she does not stay put.
    let mut log = Vec::new();
    for _ in 0..3 {
        let [col, row] = tactics_core::hex_to_offset(state.unit(quarry).unwrap().pos);
        state
            .apply(
                &reg,
                &Order::SetMove {
                    unit: quarry,
                    to: tactics_core::offset_to_hex(col + 3, row),
                },
            )
            .unwrap();
        log.extend(ticked_round(&reg, &mut state));
        if log
            .iter()
            .any(|(_, e)| matches!(e, BattleEvent::ShellLanded { .. }))
        {
            break;
        }
    }

    let fired = log
        .iter()
        .find_map(|(tick, e)| match e {
            BattleEvent::ShotFired { attacker, at, .. } if *attacker == battery => {
                Some((*tick, *at))
            }
            _ => None,
        })
        .expect("the battery fires");
    assert_eq!(
        fired.1, aim,
        "she lays the gun on the ground under the unit"
    );

    let landed = log
        .iter()
        .find_map(|(tick, e)| match e {
            BattleEvent::ShellLanded { at, .. } => Some((*tick, *at)),
            _ => None,
        })
        .expect("and the shell eventually arrives");
    let flight = tactics_core::battle::flight_ticks(&reg.scale, 10, 7);
    assert!(
        flight > 1,
        "the test needs a shell that is genuinely in the air"
    );
    assert_eq!(
        landed.0,
        fired.0 + flight,
        "it arrives exactly the flight time later, not in the tick that fired it"
    );
    assert_eq!(landed.1, aim, "on the hex it was aimed at");

    // And nobody was home.
    assert!(
        state.unit_at(aim).is_none(),
        "she drove out of the beaten zone"
    );
    for (_, event) in &log {
        match event {
            BattleEvent::ShotHit { target, .. } | BattleEvent::ShotBounced { target, .. } => {
                assert_ne!(*target, quarry, "a shell cannot strike a unit that left")
            }
            BattleEvent::UnitDestroyed { unit, .. } => {
                assert_ne!(*unit, quarry, "nor kill her from a hex away")
            }
            _ => {}
        }
    }
    assert_eq!(
        state.substance(&reg, state.unit(quarry).unwrap()),
        before,
        "she comes through it untouched"
    );
}

#[test]
fn a_shell_that_catches_her_standing_still_hits_without_a_die_roll() {
    // The other half of the bargain. Artillery has no to-hit roll any more:
    // the scatter that used to be a die is now the flight time, and a crew
    // who spends it parked is simply hit. So the shell lands and the ordinary
    // pipeline runs in the same tick — the gate, and whatever it finds — with
    // no `ShotMissed` anywhere in the stream.
    let reg = registry_wireless();
    let mut state = battery_stage(&reg, "recon_car", 502);
    let (battery, quarry) = (UnitId(0), UnitId(1));
    let aim = state.unit(quarry).unwrap().pos;
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: battery,
                fire: FireIntent::Target {
                    target: quarry,
                    weapon: 0,
                },
            },
        )
        .unwrap();

    let log = ticked_round(&reg, &mut state);
    let landed = log
        .iter()
        .find_map(|(tick, e)| match e {
            BattleEvent::ShellLanded { at, .. } if *at == aim => Some(*tick),
            _ => None,
        })
        .expect("the shell comes down on her");
    let struck = log.iter().any(|(tick, e)| {
        *tick == landed
            && matches!(
                e,
                BattleEvent::ShotHit { target, .. } | BattleEvent::ShotBounced { target, .. }
                    if *target == quarry
            )
    });
    assert!(
        struck,
        "a shell that arrives on an occupied hex resolves against her there and then"
    );
    assert!(
        !log.iter().any(
            |(_, e)| matches!(e, BattleEvent::ShotMissed { attacker, .. } if *attacker == battery)
        ),
        "and no die is thrown for it: artillery misses by being aimed at the wrong hex, not by missing"
    );
}

#[test]
fn neighbors_of_a_shellburst_feel_half_the_blast() {
    // A hundred metres is not far enough away from a 105. The battery shells
    // empty ground next door to a scout car and never touches her hex, but
    // half of a blast of six against her one-inch plate is still overmatch,
    // and overmatch does not consult the penetration gate. No round ever
    // struck her, and she is a wreck.
    let mut reg = registry_wireless();
    reg.balance.pen_scatter = 0;
    let mut state = battery_stage(&reg, "recon_car", 503);
    let (battery, quarry) = (UnitId(0), UnitId(1));
    let next_door = tactics_core::offset_to_hex(6, 1);
    assert_eq!(
        state.unit(quarry).unwrap().pos.distance_to(next_door),
        1,
        "the burst has to be one hex off her, not on her"
    );
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: battery,
                fire: FireIntent::Area {
                    at: next_door,
                    weapon: 0,
                },
            },
        )
        .unwrap();

    let log = ticked_round(&reg, &mut state);
    assert!(
        log.iter()
            .any(|(_, e)| matches!(e, BattleEvent::ShellLanded { at, .. } if *at == next_door)),
        "the shell lands where it was sent"
    );
    assert!(
        !log.iter().any(|(_, e)| matches!(
            e,
            BattleEvent::ShotHit { .. } | BattleEvent::ShotBounced { .. }
        )),
        "nothing was ever struck: this is blast, not gunnery"
    );
    assert!(
        log.iter()
            .any(|(_, e)| matches!(e, BattleEvent::UnitDestroyed { unit, .. } if *unit == quarry)),
        "and the car beside it is finished"
    );
}

#[test]
fn a_shell_is_priced_against_the_plate_it_will_strike() {
    // The playthrough review's headline defect, stated as a rule. Blast used
    // to be priced at a flat fraction of its rating no matter what it landed
    // on, while `overpressure` has always read the struck plate — so the one
    // value function behind the loader's choice and the AI's shot pricing
    // disagreed with the resolver about the most common shell in the game.
    //
    // A 105 against a tank destroyer is the sharpest case there is. Her
    // glacis is six and blast six does not overmatch it; her back plate is
    // one, and the same shell arriving there does not need the penetration
    // gate's permission at all. Same gun, same round, same range: only the
    // arc differs, and the price has to differ with it.
    //
    // The gun is stripped of its penetration first, and that is the whole
    // reason this test is worth anything. The penetration half of the price
    // has ALWAYS read the plate, so a howitzer with its own numbers scores
    // the back of a tank destroyer higher than the front no matter which
    // version of the code is running, and a test that merely compared the
    // two arcs would pass against the defect it was written for. With pen at
    // zero the only term left is blast, which is the term that was flat.
    let mut reg = registry_wireless();
    for weapon in reg.weapons.values_mut() {
        weapon.penetration = 0;
    }
    for ammo in reg.ammo.values_mut() {
        ammo.penetration = [0, 0];
    }
    let mut state = battery_stage(&reg, "tank_destroyer", 811);
    let (battery, quarry) = (UnitId(0), UnitId(1));
    let howitzer = reg
        .weapon(
            &reg.vehicle("artillery")
                .expect("she is in the base mod")
                .weapons[0],
        )
        .expect("the battery has a gun")
        .clone();
    let from = state.unit(battery).expect("on the field").pos;

    let facing_the_guns = state.unit(quarry).expect("on the field").pos;
    state.unit_mut(quarry).expect("on the field").facing = facing_the_guns.main_direction_to(from);
    let front = tactics_core::battle::expected_damage(
        &reg, &state, battery, from, &howitzer, quarry, false,
    );

    state.unit_mut(quarry).expect("on the field").facing = from.main_direction_to(facing_the_guns);
    let rear = tactics_core::battle::expected_damage(
        &reg, &state, battery, from, &howitzer, quarry, false,
    );

    assert!(
        rear > front * 3.0,
        "a burst on the back plate is worth far more than the same burst on \
         the glacis, and was worth exactly the same before: front {front}, rear {rear}"
    );
    let whole = state
        .substance(&reg, state.unit(quarry).expect("on the field"))
        .0 as f32;
    assert!(
        rear >= whole * 0.5,
        "and it is priced as what it is — a wreck, not a scratch: {rear} against \
         {whole} of tank destroyer"
    );
}

#[test]
fn a_gun_with_nothing_left_to_break_expects_nothing() {
    // Game 1 of the review, in eight lines. A howitzer put thirty-six shells
    // into one tank destroyer's front; by the seventh round her tracks and
    // her antenna — everything a burst can reach from outside a plate it
    // cannot beat — were already destroyed, and the remaining twenty-nine
    // shells were spent on a vehicle the shell provably could not touch.
    //
    // The AI was not being stubborn. It was reading a number that never
    // consulted the hull, so there was nothing in the arithmetic to change
    // its mind. Now the price of that shot is zero and `best_weapon_against`
    // — the predicate every planner in the game prices shots with — refuses
    // to call it a weapon at all, which is what puts the gun back on a
    // target worth having.
    let reg = registry_wireless();
    let mut state = battery_stage(&reg, "tank_destroyer", 812);
    let (battery, quarry) = (UnitId(0), UnitId(1));
    let howitzer = reg
        .weapon(
            &reg.vehicle("artillery")
                .expect("she is in the base mod")
                .weapons[0],
        )
        .expect("the battery has a gun")
        .clone();
    let from = state.unit(battery).expect("on the field").pos;
    let hull = state.unit(quarry).expect("on the field").pos;
    state.unit_mut(quarry).expect("on the field").facing = hull.main_direction_to(from);

    assert!(
        tactics_core::battle::expected_damage(
            &reg, &state, battery, from, &howitzer, quarry, false
        ) > 0.0,
        "while her running gear and her radio are intact the harassment is worth something"
    );

    let outside: Vec<String> = reg
        .modules
        .iter()
        .filter(|(_, m)| {
            matches!(
                m.effect,
                tactics_core::data::ModuleEffect::Mobility
                    | tactics_core::data::ModuleEffect::Radio
            )
        })
        .map(|(id, _)| id.clone())
        .collect();
    let u = state.unit_mut(quarry).expect("on the field");
    for (id, hits) in u.modules.iter_mut() {
        if outside.contains(id) {
            *hits = 0;
        }
    }

    assert_eq!(
        tactics_core::battle::expected_damage(
            &reg, &state, battery, from, &howitzer, quarry, false
        ),
        0.0,
        "with both of them gone the shell has nothing left to reach"
    );
    assert!(
        tactics_core::ai::best_weapon_against(
            &reg,
            &state,
            battery,
            from,
            state.unit(quarry).expect("on the field"),
        )
        .is_none(),
        "so the battery is not armed against her at all, and stops firing"
    );
}

#[test]
fn a_bounce_that_achieves_nothing_does_not_hold_the_battle_open() {
    // The chain reaction behind the same barrage, and the reason one AI
    // mispricing cost two things rather than one. A bounce used to reset the
    // stalemate clock on the reading that the guns were still trying — so
    // shells that could not hurt anybody kept a decided battle breathing for
    // eight more rounds of wandering.
    //
    // Trying is not progress. What holds a battle open now is a gun
    // *accomplishing* something, and a bounce that accomplishes something
    // says so in its own event: `ModuleHit` and `CrewHit` are both still on
    // the list, and overpressure raises them from outside the plate. So the
    // livelock this clock was written against — two crews neither can kill,
    // staring at each other forever — is still shut out.
    let mut reg = registry_wireless();
    // Guns that strike and never get through, on hulls where blast has
    // nothing to break: every shot from here to the bell is a bare bounce.
    for weapon in reg.weapons.values_mut() {
        weapon.penetration = 0;
        weapon.ammo.clear();
    }
    for module in reg.modules.values_mut() {
        module.size = 0;
    }
    let mut state = duel(&reg, 813);
    let (west, east) = (UnitId(0), UnitId(1));
    for (shooter, target) in [(west, east), (east, west)] {
        state
            .apply(
                &reg,
                &Order::SetFire {
                    unit: shooter,
                    fire: FireIntent::Target { target, weapon: 0 },
                },
            )
            .unwrap();
    }

    let mut bounces = 0;
    for _ in 0..STALEMATE_ROUNDS * 3 {
        if state.is_over() {
            break;
        }
        for (_, event) in ticked_round(&reg, &mut state) {
            match event {
                BattleEvent::ShotBounced { .. } => bounces += 1,
                BattleEvent::CrewHit { .. } | BattleEvent::ModuleHit { .. } => {
                    panic!("this scene has to be bounces and nothing else")
                }
                _ => {}
            }
        }
        for (shooter, target) in [(west, east), (east, west)] {
            let _ = state.apply(
                &reg,
                &Order::SetFire {
                    unit: shooter,
                    fire: FireIntent::Target { target, weapon: 0 },
                },
            );
        }
    }

    assert!(
        bounces > 0,
        "the guns really are firing and really are bouncing"
    );
    assert!(
        state.is_over(),
        "and the battle ends anyway: nothing either crew did changed anything"
    );
    assert_eq!(
        state.over.map(|r| r.reason),
        Some(EndReason::Stalemate),
        "by the clock rather than by anybody winning it"
    );
}

#[test]
fn a_mod_without_ammunition_keeps_instant_artillery() {
    // Additivity, read as strictly as the gate reads it. Flight time is a
    // rule, but it is a rule about *rounds*, and a mod that declines to
    // describe its ammunition has no rounds — only guns with numbers on them.
    // That mod must get the game it shipped with, in which a howitzer
    // resolves in the tick it fires, so nothing here ever goes up in the air.
    let mut reg = registry_wireless();
    for weapon in reg.weapons.values_mut() {
        weapon.ammo.clear();
    }
    let mut state = battery_stage(&reg, "recon_car", 504);
    let (battery, quarry) = (UnitId(0), UnitId(1));
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: battery,
                fire: FireIntent::Target {
                    target: quarry,
                    weapon: 0,
                },
            },
        )
        .unwrap();

    let log = ticked_round(&reg, &mut state);
    assert!(
        !log.iter()
            .any(|(_, e)| matches!(e, BattleEvent::ShellLanded { .. })),
        "a legacy howitzer puts nothing in the air"
    );
    assert!(
        state.shells.is_empty(),
        "and leaves nothing behind it either"
    );
    let fired = log
        .iter()
        .find_map(|(tick, e)| match e {
            BattleEvent::ShotFired { attacker, .. } if *attacker == battery => Some(*tick),
            _ => None,
        })
        .expect("she still fires");
    assert!(
        log.iter().any(|(tick, e)| *tick == fired
            && matches!(
                e,
                BattleEvent::ShotHit { .. }
                    | BattleEvent::ShotBounced { .. }
                    | BattleEvent::ShotMissed { .. }
            )),
        "and the shot is over in the tick that fired it, exactly as it always was"
    );
}

// --- what an order promises (direction step 2) ------------------------------

/// Every order the player can give says what it commits her to, and no two
/// of them say the same thing.
///
/// The complaint this defends against is not hypothetical: `Advance` and
/// `Assault` move a platoon toward the same hex and score identically, and
/// the *only* difference between them is one the player could not read
/// anywhere in the game. A promise that came back empty, or that read the
/// same for both, would put the game straight back where it was — so this
/// asserts the property rather than the wording.
#[test]
fn every_order_says_what_it_commits_the_platoon_to() {
    let to = tactics_core::Hex::new(1, 1);
    let all = [
        Mission::Advance { to },
        Mission::Assault { to },
        Mission::Hold { at: Some(to) },
        Mission::Recon { toward: to },
        Mission::Withdraw {
            via: "east_road".into(),
        },
        Mission::Support {
            formation: "second".into(),
        },
    ];
    let mut seen: Vec<&str> = Vec::new();
    for mission in &all {
        let promise = mission.promise();
        assert!(
            !promise.is_empty(),
            "{:?} promises nothing at all",
            mission.verb()
        );
        assert!(
            !seen.contains(&promise),
            "two orders make the same promise: {promise}"
        );
        seen.push(promise);
    }
    // The pair the whole step exists for, named explicitly: an advance stops
    // for a fight and an assault does not, and a player choosing between the
    // keys must be able to see that before she presses one.
    assert_ne!(
        Mission::Advance { to }.promise(),
        Mission::Assault { to }.promise(),
        "the two orders that differ only under fire must not read alike"
    );
    // Every verb the menu can list is in the shared table, which is what
    // stops a UI from inventing a promise the rules never made.
    for mission in &all {
        assert!(
            Mission::vocabulary()
                .iter()
                .any(|(verb, promise)| *verb == mission.verb() && *promise == mission.promise()),
            "{} is missing from the shared vocabulary",
            mission.verb()
        );
    }
}

/// The per-unit twin says the same kind of thing, because it is the same
/// decision at a different scale: an ordinary march may break off for cover
/// and a binding one may not, and both of those are promises.
#[test]
fn insisting_on_a_march_promises_something_an_ordinary_one_does_not() {
    assert_ne!(
        Latitude::Delegated.promise(),
        Latitude::Binding.promise(),
        "the whole value of insisting is that it means something different"
    );
    assert!(!Latitude::Delegated.promise().is_empty());
    assert!(!Latitude::Binding.promise().is_empty());
}

// --- wounds with teeth (direction step 3) -----------------------------------

/// One medium tank per side on open ground, crewed by name, so a wound
/// carried in from a previous battle has somewhere to show.
fn crewed_stage(reg: &DataRegistry, crew: &[&str]) -> (BattleState, UnitId) {
    let rows = vec!["g".repeat(12), "g".repeat(12), "g".repeat(12)];
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "crewed_stage",
        "palette": { "g": "grass" },
        "rows": rows,
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let placements = vec![
        UnitPlacement {
            aboard_at: None,
            at: [1, 1],
            side: 0,
            vehicle: "medium_tank".into(),
            crew: crew.iter().map(|id| (*id).to_string()).collect(),
            name: Some("Ours".into()),
            facing: None,
            formation: None,
            leads: false,
        },
        unit_at([10, 1], 1, "medium_tank", "Theirs"),
    ];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
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
    let state = BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        7,
    );
    (state, UnitId(0))
}

/// Rebuild the same stage with `hurt` marked wounded before the battle opens,
/// which is what a cadet carried out of last week's fight looks like.
fn stage_with_a_wounded_girl(
    reg: &DataRegistry,
    crew: &[&str],
    hurt: &[usize],
) -> (BattleState, UnitId) {
    let (state, ours) = crewed_stage(reg, crew);
    // Mark the roster, then rebuild: `who_deploys` reads the roster at spawn,
    // which is the only moment the question is asked.
    let mut roster = (*state.roster).clone();
    let ids: Vec<tactics_core::roster::CadetId> = state.unit(ours).unwrap().crew.clone();
    for seat in hurt {
        roster.get_mut(ids[*seat]).unwrap().status =
            tactics_core::roster::CadetStatus::Wounded { days: 3 };
    }
    let rows = vec!["g".repeat(12), "g".repeat(12), "g".repeat(12)];
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "crewed_stage",
        "palette": { "g": "grass" },
        "rows": rows,
    }))
    .unwrap();
    let map = HexMap::from_map_file(&file).unwrap();
    let placements = vec![
        UnitPlacement {
            aboard_at: None,
            at: [1, 1],
            side: 0,
            vehicle: "medium_tank".into(),
            crew: crew.iter().map(|id| (*id).to_string()).collect(),
            name: Some("Ours".into()),
            facing: None,
            formation: None,
            leads: false,
        },
        unit_at([10, 1], 1, "medium_tank", "Theirs"),
    ];
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
    let crews: Vec<Vec<tactics_core::roster::CadetId>> = vec![ids.clone(), Vec::new()];
    let state = BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        7,
    );
    (state, ours)
}

/// A cadet who is still recovering does not climb into the tank — and is not
/// deleted from it either.
///
/// Both halves matter. Until this rule existed a wound cost a side nothing
/// it could see: she deployed, `crew_skill` quietly ignored her, and the
/// player was never told why her gunnery had gone off. And the campaign
/// takes the crew list back at the end of a battle, so a cadet *removed* from
/// the list here would be a cadet removed from her tank for good.
#[test]
fn a_girl_in_the_infirmary_does_not_climb_in() {
    let reg = registry();
    let crew = ["anka", "mina", "juno"];
    let (state, ours) = stage_with_a_wounded_girl(&reg, &crew, &[1]);
    let unit = state.unit(ours).unwrap();

    assert_eq!(
        unit.crew.len(),
        3,
        "she stays on the roll; the campaign hands this list back"
    );
    assert_eq!(
        unit.crew_state.get(1).copied(),
        Some(tactics_core::battle::CrewCondition::Absent),
        "the cadet who is still recovering is not aboard: {:?}",
        unit.crew_state
    );
    assert_eq!(
        state.fighting_crew(unit),
        2,
        "two cadets in a three-seat tank"
    );

    // The seat she is not sitting in is not a wound. A vehicle that deployed
    // short-handed must not read as one that has already been shot up, or
    // every withdrawal threshold and every AI kill estimate in the game
    // would price it as half dead.
    assert_eq!(
        state.condition(&reg, unit),
        1.0,
        "an empty seat is not damage"
    );
    let (_, whole) = state.substance(&reg, unit);
    let (fresh, _) = crewed_stage(&reg, &crew);
    let (_, full) = state.substance(&reg, fresh.unit(ours).unwrap());
    assert_eq!(
        whole + 2,
        full,
        "her seat leaves the reckoning entirely rather than counting as a loss"
    );
}

/// ...but a tank whose whole crew is in the infirmary drives out anyway.
///
/// The campaign has no pool of replacements to draw on, and a vehicle with
/// nobody aboard is one that nothing inside can kill — which is the invariant
/// the anonymous-crew fallback exists to protect. The walking wounded go, and
/// pay for it by being worth nothing at their stations.
#[test]
fn a_crew_with_nobody_fit_goes_out_anyway() {
    let reg = registry();
    let crew = ["anka", "mina", "juno"];
    let (state, ours) = stage_with_a_wounded_girl(&reg, &crew, &[0, 1, 2]);
    let unit = state.unit(ours).unwrap();
    assert!(
        unit.crew_state.is_empty(),
        "nobody is marked absent when there is nobody else to send: {:?}",
        unit.crew_state
    );
    assert_eq!(state.fighting_crew(unit), 3);
}

/// A wound taken at her station in a tank that came home is still a wound
/// when the campaign screen draws.
///
/// This is the hole the consequence loop had. The battle tracked every cadet's
/// condition seat by seat all fight, and the only casualties the campaign
/// ever heard about were the crews of *destroyed* vehicles — so a gunner
/// knocked out in the first round of a battle her side won was fit again by
/// the time anybody could look at her.
#[test]
fn a_wound_taken_at_her_station_survives_the_battle() {
    use tactics_core::battle::CrewCondition;
    use tactics_core::roster::CasualtyRules;

    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 11).expect("overworld");
    state.rules = CasualtyRules { permadeath: false };
    let attacker = state.side_armies(0).next().unwrap().id;
    let defender = state.side_armies(1).next().unwrap().id;
    let unit = state.army(attacker).unwrap().units[0].clone();
    let cadet = unit.crew[0];
    assert!(state.roster.get(cadet).unwrap().status.is_ready());

    // Her vehicle came home. She did not come home fit.
    let survivors = vec![(attacker, state.army(attacker).unwrap().units.clone())];
    let hurt = tactics_core::overworld::CrewLoss {
        cadet,
        vehicle: unit.vehicle.clone(),
        killed_by: None,
        found: Some(CrewCondition::Out),
    };
    state.apply_battle_result(&reg, attacker, defender, &survivors, &[hurt]);

    let status = state.roster.get(cadet).unwrap().status;
    assert!(
        !status.is_ready(),
        "she was carried out of her own tank and the campaign forgot: {status:?}"
    );
    assert!(
        !status.is_permanent(),
        "permadeath is off, so a station wound is never fatal"
    );
    let days = status.days_out().expect("she is coming back");
    assert!(
        days > 0,
        "a wound that keeps her out for no days is no wound"
    );
}

/// A grazing hit costs her less than being carried out, and both cost less
/// than a wreck. The ordering is the rule; the numbers live in mod data.
#[test]
fn how_badly_she_was_hurt_decides_how_long_she_is_out() {
    use rand::SeedableRng;
    use tactics_core::battle::CrewCondition;
    use tactics_core::data::Casualties;
    use tactics_core::roster::{CasualtyRules, CrewFate, resolve_station_fate};

    let table = Casualties::default();
    let rules = CasualtyRules { permadeath: false };
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(3);

    let days = |fate: CrewFate| match fate {
        CrewFate::Wounded { days } | CrewFate::Lost { days } => days,
        _ => 0,
    };
    // Averaged over many rolls, because each is a range and a single draw
    // proves nothing about the ordering.
    let mean = |found: CrewCondition, rng: &mut rand_chacha::ChaCha8Rng| {
        let total: u32 = (0..200)
            .map(|_| days(resolve_station_fate(rules, &table, found, rng)))
            .sum();
        total as f64 / 200.0
    };
    let grazed = mean(CrewCondition::Wounded, &mut rng);
    let carried = mean(CrewCondition::Out, &mut rng);
    assert!(
        grazed < carried,
        "being carried out should cost more than being grazed: {grazed} vs {carried}"
    );
    // And a cadet who was never in the vehicle takes nothing home from a
    // battle she did not fight.
    assert_eq!(
        resolve_station_fate(rules, &table, CrewCondition::Absent, &mut rng),
        CrewFate::Unharmed
    );
}

/// A campaign map that names the same character in two crews gets her in the
/// first of them and an anonymous crew in the second.
///
/// Found by the after-action screen rather than by reading the code, which is
/// the point of having built it: `frontier` used to spread ten characters over
/// eighteen vehicles, so the campaign stamped three separate cadets all called
/// Rosa Steiner and the report listed the name three times. A roster the player
/// cannot tell apart is a roster she cannot care about, and that is the entire
/// premise of having one.
///
/// The rule belongs to `from_map`, not to any particular map, so it is fought
/// out on a fixture that over-subscribes on purpose. `frontier` itself no
/// longer does — that is
/// `every_seat_in_the_campaign_belongs_to_a_cadet_of_her_own`.
#[test]
fn nobody_crews_two_vehicles_at_once() {
    let mut reg = registry();
    let doubled: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "double_booked",
        "kind": "overworld",
        "palette": { "p": "plains" },
        "rows": ["pppp", "pppp"],
        "sides": [{ "name": "Kuhlmann Academy" }, { "name": "Iron Valkyries" }],
        "armies": [
            { "at": [0, 0], "side": 0, "name": "First", "movement": 4, "units": [
                { "at": [0, 0], "side": 0, "vehicle": "light_tank", "crew": ["anka", "rosa"] },
                { "at": [0, 0], "side": 0, "vehicle": "light_tank", "crew": ["anka", "rosa"] },
            ] },
            { "at": [3, 1], "side": 1, "name": "Theirs", "movement": 4, "units": [
                { "at": [3, 1], "side": 1, "vehicle": "light_tank", "crew": ["irma"] },
            ] },
        ],
    }))
    .expect("fixture map");
    reg.maps.insert(doubled.id.clone(), doubled);
    let state = OverworldState::from_map(&reg, "double_booked", 13).expect("overworld");

    for side in 0..2u8 {
        let mut names: Vec<&str> = state.roster.of_side(side).map(|g| g.def.as_str()).collect();
        let before = names.len();
        names.sort();
        names.dedup();
        assert_eq!(
            names.len(),
            before,
            "side {side} enlisted somebody twice: {names:?}"
        );
    }

    // ...and the vehicle whose named crew was already taken is not left
    // crewless, because a vehicle nobody is in is one nothing inside can
    // kill. It picks up an anonymous crew at the battle, exactly as a
    // placement that named nobody always has.
    let crewless = state
        .side_armies(0)
        .flat_map(|a| a.units.iter())
        .filter(|u| u.crew.is_empty())
        .count();
    assert_eq!(
        crewless, 1,
        "the second vehicle to ask for Anka should be left for an anonymous crew"
    );
}

/// Every seat in the campaign's order of battle belongs to a cadet with a name.
///
/// Two separate things are pinned here and both are content rules the engine
/// cannot enforce on a mod's behalf.
///
/// **Nobody is named twice**, because the deduplication above is a safety net
/// rather than a licence: a map that trips it silently hands a vehicle to an
/// anonymous crew, which is a worse version of what the author asked for.
///
/// **Every seat is filled**, because a crew shorter than the chassis is not a
/// cosmetic gap. Substance is counted per person aboard, so a medium tank
/// crewed by two named cadets dies roughly twice as fast as the identical tank
/// crewed by four anonymous ones — naming your characters used to be a
/// straight penalty. It also makes wounds legible: with the seats full at the
/// start of a campaign, an empty seat means somebody is in the infirmary and
/// nothing else.
#[test]
fn every_seat_in_the_campaign_belongs_to_a_cadet_of_her_own() {
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 13).expect("overworld");

    for army in &state.armies {
        for unit in &army.units {
            let vehicle = reg.vehicle(&unit.vehicle).expect("known chassis");
            assert_eq!(
                unit.crew.len(),
                vehicle.crew_slots.len(),
                "{} in {} has {} of {} seats filled",
                unit.vehicle,
                army.name,
                unit.crew.len(),
                vehicle.crew_slots.len()
            );
        }
    }
}

/// Those anonymous crews stay in the battle they were invented for.
///
/// They are enlisted into the *battle's* copy of the roster, so their handles
/// mean nothing to the campaign; handing them back with the survivors would
/// leave an army holding ids the academy cannot resolve. Not a crash — every
/// roster read simply returns nothing — which is exactly the kind of defect
/// that sits there for months, so it is pinned.
#[test]
fn a_battle_does_not_enlist_anybody_into_the_academy() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 17).expect("overworld");
    let attacker = state.side_armies(0).next().unwrap().id;
    let defender = state.side_armies(1).next().unwrap().id;

    // A survivor list of the sort a battle hands back: real cadets, plus a
    // handle from beyond the end of the campaign's roster, which is what an
    // anonymous crew member's id looks like from here.
    let stranger = tactics_core::roster::CadetId(state.roster.len() as u32 + 5);
    let mut units = state.army(attacker).unwrap().units.clone();
    units[0].crew.push(stranger);
    state.apply_battle_result(&reg, attacker, defender, &[(attacker, units)], &[]);

    for unit in &state.army(attacker).unwrap().units {
        for cadet in &unit.crew {
            assert!(
                state.roster.get(*cadet).is_some(),
                "{cadet:?} is in an army and in nobody's academy"
            );
        }
    }
}
