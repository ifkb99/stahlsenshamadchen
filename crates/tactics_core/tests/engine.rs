//! End-to-end tests against the real `assets/mods` content.

use std::path::PathBuf;
use tactics_core::ai::{make_battle_planner, AiConfig};
use tactics_core::battle::{
    los_clear, reachable, BattleState, EndReason, Event as BattleEvent, Order, SideState,
    STALEMATE_TURNS,
};
use tactics_core::data::DataRegistry;
use tactics_core::map::{HexMap, UnitPlacement};
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
fn a_battle_with_no_shots_fired_is_called_off() {
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 3).unwrap();
    let alive_before = state.alive_units().count();

    let mut ended = None;
    for _ in 0..(STALEMATE_TURNS as usize + 2) * 2 {
        let Ok(events) = state.apply(&reg, &Order::EndTurn) else {
            break;
        };
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
        state.turn <= STALEMATE_TURNS + 1,
        "the call should come promptly, not after {} rounds",
        state.turn
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
        },
        UnitPlacement {
            at: [2, 1],
            side: 1,
            vehicle: "medium_tank".into(),
            crew: Vec::new(),
            name: Some("Watched".into()),
        },
    ];
    let mut state = BattleState::from_placements(&reg, map, sides, &placements, 1);
    assert!(
        !state.fog.side(0).spotted.is_empty(),
        "test needs the two units to start in sight of one another"
    );

    for _ in 0..(STALEMATE_TURNS as usize + 4) * 2 {
        state.apply(&reg, &Order::EndTurn).unwrap();
    }
    assert!(
        state.over.is_none(),
        "a battle under observation is not a stalemate, but ended as {:?} on turn {}",
        state.over,
        state.turn
    );
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
fn overworld_reachability_respects_budget_and_blockers() {
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier").unwrap();
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
        assert!(state.map.get(*hex).is_some(), "reachable tile is on the map");
    }
}

#[test]
fn reinforcements_are_adjacent_and_attackers_must_be_fresh() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier").unwrap();
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
    assert!(state
        .reinforcement_candidates(at, 0, principal, true)
        .is_empty());
}

#[test]
fn battle_results_are_returned_to_each_army() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier").unwrap();
    let attacker = state.side_armies(0).next().unwrap().id;
    let helper = state.side_armies(0).nth(1).unwrap().id;
    let defender = state.side_armies(1).next().unwrap().id;
    let defender_pos = state.army(defender).unwrap().pos;

    // The attacker loses everything but one tank, the helper is untouched,
    // and the defender is wiped out.
    let survivor = state.army(attacker).unwrap().units[..1].to_vec();
    let helper_units = state.army(helper).unwrap().units.clone();
    let events = state.apply_battle_result(
        attacker,
        defender,
        &[
            (attacker, survivor.clone()),
            (helper, helper_units.clone()),
            (defender, Vec::new()),
        ],
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
            &reg, &state, attacker.id, attacker.pos, weapon, target.pos, blind,
        );
        let breakdown = tactics_core::battle::hit_breakdown(
            &reg, &state, attacker.id, attacker.pos, weapon, target.pos, blind,
        );
        assert_eq!(breakdown.total, chance, "breakdown must agree with the roll");
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

fn two_side_battle(
    reg: &DataRegistry,
    rows: &[&str],
    placements: Vec<UnitPlacement>,
    seed: u64,
) -> BattleState {
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "test_map",
        "palette": { "g": "grass" },
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
    BattleState::from_placements(reg, map, sides, &placements, seed)
}

#[test]
fn units_can_move_through_friendlies() {
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["gggg"],
        vec![
            UnitPlacement {
                at: [0, 0],
                side: 0,
                vehicle: "medium_tank".into(),
                crew: Vec::new(),
                name: Some("Mover".into()),
            },
            UnitPlacement {
                at: [1, 0],
                side: 0,
                vehicle: "medium_tank".into(),
                crew: Vec::new(),
                name: Some("Blocker".into()),
            },
            // An enemy far away so the battle has two sides.
            UnitPlacement {
                at: [3, 0],
                side: 1,
                vehicle: "medium_tank".into(),
                crew: Vec::new(),
                name: Some("Bystander".into()),
            },
        ],
        1,
    );
    let mover = state
        .side_units(0)
        .find(|u| u.name == "Mover")
        .unwrap()
        .id;
    let dest = tactics_core::offset_to_hex(2, 0);
    let events = state
        .apply(&reg, &Order::Move { unit: mover, to: dest })
        .expect("moving through a friend should be legal");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, BattleEvent::UnitMoved { .. })),
        "expected a move, got {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::UnitTrapped { .. })),
        "a friend is not an ambush: {events:?}"
    );
    assert_eq!(state.unit(mover).unwrap().pos, dest);
    assert!(
        !state.unit(mover).unwrap().acted,
        "passing through a friend must not burn the action"
    );
}

#[test]
fn unspotted_enemies_still_ambush() {
    let reg = registry();
    // Medium tank vision is 3; park the ambusher at distance 4 so the
    // mover plans a path through a tile it cannot see.
    let mut state = two_side_battle(
        &reg,
        &["ggggggg"],
        vec![
            UnitPlacement {
                at: [0, 0],
                side: 0,
                vehicle: "medium_tank".into(),
                crew: Vec::new(),
                name: Some("Mover".into()),
            },
            UnitPlacement {
                at: [4, 0],
                side: 1,
                vehicle: "medium_tank".into(),
                crew: Vec::new(),
                name: Some("Ambusher".into()),
            },
        ],
        2,
    );
    let mover = state.side_units(0).next().unwrap().id;
    let ambusher = state.side_units(1).next().unwrap().id;
    assert!(
        !state.fog.side(0).spotted.contains(&ambusher),
        "ambusher must start unseen for this test"
    );
    let dest = tactics_core::offset_to_hex(5, 0);
    let events = state
        .apply(&reg, &Order::Move { unit: mover, to: dest })
        .expect("pathing through fog should be attempted");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, BattleEvent::UnitTrapped { .. })),
        "bumping an unspotted enemy must trap: {events:?}"
    );
    assert_eq!(
        state.unit(mover).unwrap().pos,
        tactics_core::offset_to_hex(3, 0),
        "should stop on the tile before the ambusher, short of {dest:?}"
    );
    assert!(state.unit(mover).unwrap().acted, "an ambush spends the action");
}

#[test]
fn hidden_enemies_do_not_show_up_as_holes_in_the_move_range() {
    // Refusing a move because an unseen enemy stands there would announce
    // its position, so the tile stays offered and the order becomes an
    // ambush instead.
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["ggggggg"],
        vec![
            UnitPlacement {
                at: [0, 0],
                side: 0,
                vehicle: "medium_tank".into(),
                crew: Vec::new(),
                name: Some("Mover".into()),
            },
            UnitPlacement {
                at: [4, 0],
                side: 1,
                vehicle: "medium_tank".into(),
                crew: Vec::new(),
                name: Some("Hidden".into()),
            },
        ],
        4,
    );
    let mover = state.side_units(0).next().unwrap().id;
    let hidden = state.side_units(1).next().unwrap().id;
    let hidden_pos = state.unit(hidden).unwrap().pos;
    assert!(
        !state.fog.side(0).spotted.contains(&hidden),
        "precondition: the enemy is unseen"
    );
    assert!(
        reachable(&reg, &state, mover).contains_key(&hidden_pos),
        "an unseen enemy must not punch a hole in the move overlay"
    );

    // Ordering the move onto that tile is legal and resolves as an ambush.
    let events = state
        .apply(&reg, &Order::Move { unit: mover, to: hidden_pos })
        .expect("the order must be accepted, not refused with NoPath");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, BattleEvent::UnitTrapped { .. })),
        "expected an ambush, got {events:?}"
    );
    assert_ne!(state.unit(mover).unwrap().pos, hidden_pos);
}

#[test]
fn spotted_enemies_still_block_a_destination() {
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["ggg"],
        vec![
            UnitPlacement {
                at: [0, 0],
                side: 0,
                vehicle: "medium_tank".into(),
                crew: Vec::new(),
                name: Some("Mover".into()),
            },
            UnitPlacement {
                at: [1, 0],
                side: 1,
                vehicle: "medium_tank".into(),
                crew: Vec::new(),
                name: Some("Seen".into()),
            },
        ],
        5,
    );
    let mover = state.side_units(0).next().unwrap().id;
    let seen = state.side_units(1).next().unwrap().id;
    let seen_pos = state.unit(seen).unwrap().pos;
    assert!(
        state.fog.side(0).spotted.contains(&seen),
        "precondition: the enemy is in plain sight"
    );
    assert_eq!(
        state.apply(&reg, &Order::Move { unit: mover, to: seen_pos }),
        Err(tactics_core::battle::OrderError::NoPath),
        "you cannot drive onto an enemy you can see; that is an attack"
    );
}

#[test]
fn return_fire_comes_back_the_next_round() {
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["ggggg", "ggggg", "ggggg"],
        vec![
            UnitPlacement {
                at: [0, 1],
                side: 0,
                vehicle: "medium_tank".into(),
                crew: Vec::new(),
                name: Some("Defender".into()),
            },
            UnitPlacement {
                at: [2, 1],
                side: 1,
                vehicle: "medium_tank".into(),
                crew: Vec::new(),
                name: Some("Attacker".into()),
            },
        ],
        6,
    );
    let defender = state.side_units(0).next().unwrap().id;

    state.unit_mut(defender).unwrap().can_return_fire = false;
    let turn_before = state.turn;
    // Side 0 ends, side 1 ends: that wraps the round.
    state.apply(&reg, &Order::EndTurn).unwrap();
    assert!(
        !state.unit(defender).unwrap().can_return_fire,
        "mid-round hand-off must not refresh opportunity fire"
    );
    state.apply(&reg, &Order::EndTurn).unwrap();
    assert!(
        state.turn > turn_before,
        "two hand-offs with two sides should start a new round"
    );
    assert!(
        state.unit(defender).unwrap().can_return_fire,
        "a new round restores opportunity fire"
    );
}

#[test]
fn units_return_fire_after_acting() {
    let reg = registry();
    let mut state = two_side_battle(
        &reg,
        &["ggggg", "ggggg", "ggggg"],
        vec![
            UnitPlacement {
                at: [0, 1],
                side: 0,
                vehicle: "medium_tank".into(),
                crew: Vec::new(),
                name: Some("Defender".into()),
            },
            UnitPlacement {
                at: [2, 1],
                side: 1,
                vehicle: "medium_tank".into(),
                crew: Vec::new(),
                name: Some("Attacker".into()),
            },
        ],
        3,
    );
    let defender = state.side_units(0).next().unwrap().id;
    let attacker = state.side_units(1).next().unwrap().id;
    assert!(
        state.fog.side(0).spotted.contains(&attacker)
            && state.fog.side(1).spotted.contains(&defender),
        "both need LoS for return fire"
    );

    // Spend the defender's action (the old bug: acted stayed true all enemy turn).
    state
        .apply(&reg, &Order::Wait { unit: defender })
        .unwrap();
    assert!(state.unit(defender).unwrap().acted);
    assert!(state.unit(defender).unwrap().can_return_fire);

    state.apply(&reg, &Order::EndTurn).unwrap();
    assert_eq!(state.active_side, 1);
    assert!(
        state.unit(defender).unwrap().acted,
        "acted only clears on the unit's own turn"
    );

    let events = state
        .apply(
            &reg,
            &Order::Attack {
                unit: attacker,
                target: defender,
                weapon: 0,
            },
        )
        .expect("attack should resolve");
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::ShotFired {
                counter: true,
                ..
            }
        )),
        "a unit that already acted must still return fire: {events:?}"
    );
    if let Some(d) = state.unit(defender) {
        assert!(
            !d.can_return_fire,
            "return fire is spent for the rest of the round"
        );
    }
}
