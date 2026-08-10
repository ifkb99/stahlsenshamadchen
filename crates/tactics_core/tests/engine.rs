//! End-to-end tests against the real `assets/mods` content.

use std::path::PathBuf;
use tactics_core::ai::{AiConfig, AiPlanner, Evaluator, UtilityPlanner, make_battle_planner};
use tactics_core::battle::{
    BattleState, EndReason, Event as BattleEvent, FireIntent, Order, STALEMATE_ROUNDS, SideState,
    UnitId, los_clear, reachable,
};
use tactics_core::data::DataRegistry;
use tactics_core::map::{HexMap, UnitPlacement};
use tactics_core::overworld::{
    OverworldEvent, OverworldOrder, OverworldState, make_overworld_planner,
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
        let mut planners = [
            make_battle_planner(&cfg, seed, &reg),
            make_battle_planner(&cfg, seed + 1, &reg),
        ];
        let mut log = Vec::new();
        for _ in 0..40 {
            if state.is_over() {
                break;
            }
            // Both sides write orders, then the round plays out at once.
            for side in [0u8, 1u8] {
                for _ in 0..64 {
                    if state.has_committed(side) || !state.is_planning() {
                        break;
                    }
                    let order = planners[side as usize].next_order(&reg, &state, side);
                    if state.apply(&reg, &order).is_err() {
                        let _ = state.apply(&reg, &Order::Commit { side });
                    }
                }
            }
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
    let mut planners = [
        make_battle_planner(&cfg, 5, &reg),
        make_battle_planner(&cfg, 6, &reg),
    ];
    for round in 0..200 {
        if state.is_over() {
            println!("battle over after {round} rounds: {:?}", state.over);
            return;
        }
        for side in [0u8, 1u8] {
            for _ in 0..64 {
                if state.has_committed(side) || !state.is_planning() {
                    break;
                }
                let order = planners[side as usize].next_order(&reg, &state, side);
                if state.apply(&reg, &order).is_err() {
                    let _ = state.apply(&reg, &Order::Commit { side });
                }
            }
        }
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
        },
        UnitPlacement {
            at: [2, 1],
            side: 1,
            vehicle: "medium_tank".into(),
            crew: Vec::new(),
            name: Some("Watched".into()),
            facing: None,
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
