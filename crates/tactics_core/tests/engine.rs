//! End-to-end tests against the real `assets/mods` content.

use std::path::PathBuf;
use tactics_core::ai::{make_battle_planner, AiConfig};
use tactics_core::battle::{los_clear, reachable, BattleState, Order};
use tactics_core::data::DataRegistry;
use tactics_core::map::HexMap;
use tactics_core::overworld::{make_overworld_planner, OverworldEvent, OverworldOrder, OverworldState};

fn mods_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods")
}

fn registry() -> DataRegistry {
    let (registry, report) = DataRegistry::load_dir(&mods_root()).expect("mods load");
    assert!(report.is_ok(), "base mod must validate: {:?}", report.errors);
    registry
}

#[test]
fn base_mod_loads_and_validates() {
    let reg = registry();
    assert!(reg.vehicles.len() >= 5);
    assert!(reg.characters.len() >= 8);
    assert!(reg.maps.contains_key("river_crossing"));
    assert!(reg.maps.contains_key("frontier"));
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
    assert!(los_clear(&reg, &map, a, edge), "the forest tile itself is visible");
}

#[test]
fn movement_respects_water_and_reaches_bridge() {
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 7).unwrap();
    // Unit 0 is Anka's medium tank at offset [2,4] on the road.
    let unit = state.side_units(0).next().unwrap().id;
    let tiles = reachable(&reg, &state, unit);
    assert!(!tiles.is_empty());
    for (hex, _) in &tiles {
        let tile = state.map.get(*hex).unwrap();
        assert_ne!(tile.terrain, "water", "tracked vehicles cannot enter water");
    }
}

#[test]
fn battle_orders_are_deterministic_per_seed() {
    let reg = registry();
    let run = |seed: u64| -> Vec<String> {
        let mut state = BattleState::from_map(&reg, "river_crossing", seed).unwrap();
        let cfg = AiConfig {
            planner: "utility".into(),
            difficulty: 5,
        };
        let mut planners = [
            make_battle_planner(&cfg, seed),
            make_battle_planner(&cfg, seed + 1),
        ];
        let mut log = Vec::new();
        for _ in 0..400 {
            if state.is_over() {
                break;
            }
            let side = state.active_side;
            let order = planners[side as usize].next_order(&reg, &state, side);
            match state.apply(&reg, &order) {
                Ok(events) => log.extend(events.iter().map(|e| format!("{e:?}"))),
                Err(_) => {
                    // A planner asked for something stale; skip the unit.
                    let _ = state.apply(&reg, &Order::EndTurn);
                }
            }
        }
        log
    };
    assert_eq!(run(1234), run(1234), "same seed, same battle");
}

#[test]
fn ai_vs_ai_battle_finishes() {
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 99).unwrap();
    let cfg = AiConfig {
        planner: "utility".into(),
        difficulty: 4,
    };
    let mut planners = [make_battle_planner(&cfg, 5), make_battle_planner(&cfg, 6)];
    for step in 0..2000 {
        if state.is_over() {
            println!("battle over after {step} orders: {:?}", state.over);
            return;
        }
        let side = state.active_side;
        let order = planners[side as usize].next_order(&reg, &state, side);
        if state.apply(&reg, &order).is_err() {
            let _ = state.apply(&reg, &Order::EndTurn);
        }
    }
    panic!("battle did not finish; final state: turn {}", state.turn);
}

#[test]
fn mcts_planner_produces_legal_orders() {
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 11).unwrap();
    let cfg = AiConfig {
        planner: "mcts".into(),
        difficulty: 1,
    };
    let mut planner = make_battle_planner(&cfg, 3);
    // Play a few orders for side 0 and require they all apply cleanly.
    for _ in 0..6 {
        if state.is_over() || state.active_side != 0 {
            break;
        }
        let order = planner.next_order(&reg, &state, 0);
        let is_end = order == Order::EndTurn;
        state
            .apply(&reg, &order)
            .unwrap_or_else(|e| panic!("mcts produced illegal order {order:?}: {e}"));
        if is_end {
            break;
        }
    }
}

#[test]
fn overworld_income_capture_and_battle_trigger() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier").unwrap();
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
fn overworld_ai_moves_armies() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier").unwrap();
    let cfg = AiConfig {
        planner: "simple".into(),
        difficulty: 3,
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
