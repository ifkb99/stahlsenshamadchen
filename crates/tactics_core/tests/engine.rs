//! Content validation, determinism, the overworld and gunnery previews.
//!
//! What is left in the file the split started from: the sections that do
//! not belong to any one subject binary because they *are* the substrate
//! every other binary stands on — the mod loads and validates, a battle
//! plays out deterministically from a seed, the overworld drives a
//! campaign, and a player is shown a shot's arithmetic before firing it.
//! The eight binaries it split off are `sight.rs` (fog, detection,
//! line of sight), `planning.rs` (the goal chooser and the planner
//! numbers), `missions.rs` (campaign missions, defiance, goals),
//! `net.rs` (contact, command and the radio net), `drill.rs` (the battle
//! drill, latitude, formations), `shots.rs` (penetration, the outcome
//! engine, shells in flight), `crews.rs` (boarding, the chain of command,
//! wounds) and `currency.rs` (the resolver's one currency).
//!
//! - content, the scale contract and validation
//! - determinism, and a battle fought to the end
//! - the overworld
//! - gunnery previews: the arithmetic a player is shown
//!
//! Shared setup lives in `tests/common/mod.rs`; stage builders more than one
//! binary needs live in `tests/common/stage.rs`.

use tactics_core::ai::{AiConfig, AiDriver, make_battle_planner};
use tactics_core::battle::{
    BattleState, EndReason, Event as BattleEvent, FireIntent, Order, SideState, UnitId,
};
use tactics_core::map::{Battlefield, UnitPlacement};
use tactics_core::overworld::{
    BattleReport, OverworldEvent, OverworldOrder, OverworldState, make_overworld_planner,
};

mod common;
use common::{duel, play_round, registry, standoff};

// --- content, the scale contract and validation ----------------------------

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

    let map = Battlefield::from_map_file(file).unwrap();
    assert_eq!(map.terrain.len() as u32, reg.scale.battle_map_tiles());
    let centre = map.terrain.center();
    assert!(
        map.terrain.contains(centre),
        "a centroid that lands off-map would be a broken rotation pivot"
    );
    for hex in centre.range(reg.scale.battle_map_radius()) {
        assert!(map.terrain.contains(hex), "hexagon has a hole at {hex:?}");
    }
}

/// A map that declares elevation must give every tile a level; one that is
/// flat declares none.
///
/// The failure this closes is invisible by construction. `from_map_file`
/// reads one character per glyph and defaults anything past the end of a row
/// — or past the end of the grid — to zero, so an author who adds a row of
/// terrain and forgets the matching row of digits gets a map that *works*,
/// with a strip of it silently flattened: no error, no crash, just ground
/// that is not the ground they drew. Validation used to warn when an
/// elevation row's length did not match its terrain row, which caught the
/// short row and never the missing one.
///
/// It is checked per tile rather than per string, and that is not
/// fussiness. A row of `rows` may be padded with spaces where there is no
/// tile, and an elevation row that stops before them has promised nothing it
/// failed to keep — a length check would reject a file with nothing wrong
/// with it, and a rule that cries wolf is a rule authors turn off.
#[test]
fn an_elevation_grid_that_stops_short_of_the_map_is_rejected() {
    let reg = registry();
    let map = |elevation: serde_json::Value| -> tactics_core::data::ValidationReport {
        let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
            "id": "slope",
            "shape": "free",
            "palette": { "g": "grass" },
            "rows": ["ggg", "gg ", "ggg"],
            "elevation": elevation,
        }))
        .unwrap();
        let mut report = tactics_core::data::ValidationReport::default();
        file.validate_into(&reg, &mut report);
        report
    };

    assert!(
        map(serde_json::json!(["012", "34", "567"])).is_ok(),
        "a row may stop where the tiles do: the third column of row 1 is not a tile"
    );
    assert!(
        map(serde_json::json!([])).is_ok(),
        "a map that declares no elevation is flat, and that is how most of them say it"
    );

    let missing_row = map(serde_json::json!(["012", "34"]));
    assert!(
        missing_row
            .errors
            .iter()
            .any(|e| e.contains("stops short") && e.contains("[0, 2]")),
        "a grid a row short flattened the bottom of the map in silence: {missing_row:?}"
    );
    let short_row = map(serde_json::json!(["01", "34", "567"]));
    assert!(
        short_row
            .errors
            .iter()
            .any(|e| e.contains("stops short") && e.contains("[2, 0]")),
        "a row that stops over a tile flattened it in silence: {short_row:?}"
    );
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

// --- determinism, and a battle fought to the end ---------------------------

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
    for _ in 0..(reg.balance.stalemate_rounds as usize + 2) {
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
        state.round <= reg.balance.stalemate_rounds + 1,
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
    let map = Battlefield::from_map_file(&file).unwrap();
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
    )
    .expect("the staged placements are content the base mod ships");
    assert!(
        !state.fog.side(0).spotted.is_empty(),
        "test needs the two units to start in sight of one another"
    );

    for _ in 0..(reg.balance.stalemate_rounds as usize + 4) {
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

// --- the overworld ---------------------------------------------------------

/// An army takes the ground it stands on, and the side keeps it through the
/// night. Was `overworld_income_capture_and_battle_trigger`, which never
/// triggered a battle and whose second half checked that the city paid
/// funds into a treasury nothing spent; the treasury went on 2026-09-23 and
/// this half asks what ownership still means — that it persists.
#[test]
fn an_army_takes_the_ground_it_stands_on_and_keeps_it_overnight() {
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

    // End both turns: nobody contests it, so it is still hers at dawn.
    let _ = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    let _ = state.apply(&reg, &OverworldOrder::EndTurn).unwrap();
    assert_eq!(
        state.owners.get(&city),
        Some(&0),
        "a capture is kept until somebody takes it back"
    );
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
        &BattleReport::of(
            attacker,
            defender,
            vec![
                (attacker, survivor.clone()),
                (helper, helper_units.clone()),
                (defender, Vec::new()),
            ],
            Vec::new(),
        ),
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

// --- gunnery previews: the arithmetic a player is shown --------------------

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
            target.pos,
            blind,
        );
        let breakdown = tactics_core::battle::hit_breakdown(
            &reg,
            &state,
            attacker.id,
            attacker.pos,
            weapon,
            target.id,
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
