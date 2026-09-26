//! The world: ground with one owner, held whole or a chunk at a time.
//!
//! WORLD.md W0.3–W0.4 and W1.1. What is pinned here is the geometry a streamed
//! world rests on — which campaign hex a tile lies under, and that the answer
//! is exact — and the three things a world can say about a hex, because the
//! difference between *outside* and *unloaded* is the difference between a
//! sight line over the edge of the map and a sight line through a ridge
//! nobody paged in.

use hexx::Hex;
use tactics_core::battle::BattleState;
use tactics_core::world::{Presence, World, chunk_centre, chunk_hexes, chunk_of};

mod common;
use common::registry;

/// The radius the base mod's scale derives: one campaign hex, 4 km of 100 m
/// tiles.
const R: u32 = 20;

#[test]
fn a_chunk_is_the_chunk_hexx_would_name() {
    // `chunk_of` is integer arithmetic standing in for `to_lower_res`, which
    // divides in f32. They must agree everywhere a world will ever reach, at
    // every radius the game uses: the terrain reader's regions, a campaign
    // hex, and the degenerate one.
    for radius in [0, 1, 4, R] {
        for hex in Hex::ZERO.range(300) {
            assert_eq!(
                chunk_of(hex, radius),
                hex.to_lower_res(radius),
                "{hex:?} at radius {radius}"
            );
        }
    }
}

#[test]
fn chunks_tile_the_plane_with_no_gap_and_no_overlap() {
    // Every tile of a chunk says it belongs to that chunk, a chunk has the
    // 1261 tiles a battle map has, and every tile near the origin is in the
    // chunk it names — so loading chunks can never leave a hole or load a
    // tile twice.
    let chunks: Vec<Hex> = Hex::ZERO.range(3).collect();
    for &chunk in &chunks {
        let tiles: Vec<Hex> = chunk_hexes(chunk, R).collect();
        assert_eq!(tiles.len(), 1261);
        assert_eq!(tiles[0], chunk_centre(chunk, R), "the centre comes first");
        for hex in tiles {
            assert_eq!(chunk_of(hex, R), chunk, "{hex:?} strays out of {chunk:?}");
        }
    }
    for hex in Hex::ZERO.range(60) {
        let chunk = chunk_of(hex, R);
        assert!(
            chunk_hexes(chunk, R).any(|h| h == hex),
            "{hex:?} is in no chunk"
        );
    }
}

#[test]
fn a_world_held_whole_says_outside_past_its_edge_and_never_unloaded() {
    // Every battle today stands on a whole world. A hex off the map is
    // outside — a sight line passing over it is passing over the edge of the
    // known world, and open sky is the right answer there.
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 1).expect("battle");
    let world = &state.world;
    let (inside, _) = world.iter().next().expect("the map has tiles");
    assert!(matches!(world.presence(inside), Presence::Known(_)));
    let far = Hex::new(10_000, -10_000);
    assert_eq!(world.presence(far), Presence::Outside);
    assert!(world.resident_chunks().is_none());
}

fn grass(chunk: Hex) -> Vec<(Hex, &'static str, i32)> {
    chunk_hexes(chunk, R).map(|h| (h, "grass", 0)).collect()
}

#[test]
fn a_chunk_not_loaded_is_unloaded_and_a_chunk_loaded_empty_is_outside() {
    let reg = registry();
    let mut world = World::chunked(R);
    let home = Hex::ZERO;
    let next = Hex::new(1, 0);
    world.load_chunk(&reg, home, grass(home));

    let here = chunk_centre(home, R);
    let there = chunk_centre(next, R);
    assert!(matches!(world.presence(here), Presence::Known(_)));
    assert_eq!(
        world.presence(there),
        Presence::Unloaded,
        "ground nobody has paged in is not the edge of the world"
    );

    // A chunk past the edge of the world is loaded with nothing in it, and
    // that is how the world learns the difference.
    world.load_chunk(&reg, next, Vec::new());
    assert_eq!(world.presence(there), Presence::Outside);
    assert_eq!(
        world.resident_chunks().unwrap().collect::<Vec<_>>(),
        [home, next]
    );
}

#[test]
fn unloading_a_chunk_forgets_its_ground_and_both_grids_forget_it_too() {
    let reg = registry();
    let mut world = World::chunked(R);
    world.load_chunk(&reg, Hex::ZERO, grass(Hex::ZERO));
    world.load_chunk(&reg, Hex::new(1, 0), grass(Hex::new(1, 0)));
    assert_eq!(world.len(), 2 * 1261);
    assert_eq!(world.sight().len(), 2 * 1261);

    world.unload_chunk(Hex::new(1, 0));
    assert_eq!(world.len(), 1261);
    assert_eq!(world.sight().len(), 1261, "the grids go with the tiles");
    assert!(world.moves().len() == 1261);
    assert_eq!(
        world.presence(chunk_centre(Hex::new(1, 0), R)),
        Presence::Unloaded
    );
}

#[test]
#[should_panic(expected = "tile")]
fn ground_arriving_under_the_wrong_campaign_hex_is_refused() {
    let reg = registry();
    let mut world = World::chunked(R);
    world.load_chunk(&reg, Hex::ZERO, grass(Hex::new(1, 0)));
}

#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "not loaded")]
fn a_sight_line_through_ground_nobody_loaded_is_a_loud_failure() {
    // The grid would read the unloaded hexes as open sky. The residency rule
    // keeps the question from being asked; this is what happens if it ever
    // is, in every debug build and every test.
    let reg = registry();
    let mut world = World::chunked(R);
    world.load_chunk(&reg, Hex::ZERO, grass(Hex::ZERO));
    let here = chunk_centre(Hex::ZERO, R);
    let far = chunk_centre(Hex::new(2, 0), R);
    let _ = world.sight_clear(here, far);
}

#[test]
fn a_world_held_a_chunk_at_a_time_is_not_saved_as_if_it_were_whole() {
    // Saved as tiles, it would come back whole with every chunk it had not
    // loaded suddenly outside the world.
    let reg = registry();
    let mut world = World::chunked(R);
    world.load_chunk(&reg, Hex::ZERO, grass(Hex::ZERO));
    assert!(serde_json::to_string(&world).is_err());
}

#[test]
fn a_force_whose_names_do_not_rise_is_refused() {
    // A battle keeps its units in id order and walks them in that order
    // wherever determinism is at stake. Names that do not rise with the
    // placements would make id order a different order from placement
    // order, so the setup refuses them rather than quietly re-sorting.
    use tactics_core::battle::UnitId;
    let reg = registry();
    let count = reg.map("river_crossing").unwrap().units.len();
    let mut names: Vec<UnitId> = (0..count as u32).map(|i| UnitId(10 * i)).collect();
    names.swap(0, 1);
    let err = BattleState::from_map_numbered(&reg, "river_crossing", 1, Some(&names))
        .expect_err("names out of order");
    assert!(err.to_string().contains("rise"), "{err}");
    let short = &names[..count - 1];
    assert!(BattleState::from_map_numbered(&reg, "river_crossing", 1, Some(short)).is_err());
}

// --- dice per engagement (W0.6) ---

use tactics_core::field::Clash;
use tactics_core::overworld::OverworldState;
use tactics_core::save::SaveGame;
use tactics_core::world::{EngagementKey, engagement_seed};

fn key(when: u64) -> EngagementKey {
    EngagementKey {
        when,
        at: Hex::new(3, -2),
        attacker: 0,
        defender: 1,
    }
}

#[test]
fn an_engagement_draws_the_same_dice_on_every_machine_and_every_compiler() {
    // Pinned to the bit. `engagement_seed` is written out rather than going
    // through `std::hash` so that it cannot move under a toolchain upgrade;
    // this is the check that it did not move under an edit either. A change
    // here changes every campaign battle ever fought from a seed.
    assert_eq!(engagement_seed(7, key(2)), engagement_seed(7, key(2)));
    assert_eq!(engagement_seed(7, key(2)), 0x0e78_61b4_7a4c_71c9);
}

#[test]
fn every_part_of_an_engagements_name_changes_its_dice() {
    let base = engagement_seed(7, key(2));
    let mut moved = key(2);
    moved.at = Hex::new(3, -1);
    let mut swapped = key(2);
    (swapped.attacker, swapped.defender) = (1, 0);
    for (what, other) in [
        ("the world", engagement_seed(8, key(2))),
        ("the day", engagement_seed(7, key(3))),
        ("the place", engagement_seed(7, moved)),
        ("who attacked", engagement_seed(7, swapped)),
    ] {
        assert_ne!(base, other, "{what} made no difference");
    }
}

#[test]
fn a_campaign_loaded_from_a_save_fights_the_battle_it_would_have_fought() {
    // Before W0.6 the game seeded a campaign's battles from the wall clock
    // and the harness from how many battles had come before, so neither a
    // save nor a second campaign in the same process met the same fight.
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 11).expect("campaign");
    let (a, b) = (
        tactics_core::overworld::ElementId(0),
        tactics_core::overworld::ElementId(1),
    );
    let first = Clash::muster(&state, a, b, &[], &[], "river_crossing".into());

    let text = SaveGame::<BattleState>::new(&reg, Some(state.clone()), None)
        .to_json()
        .expect("saves");
    let loaded = SaveGame::from_json(&reg, &text)
        .expect("loads")
        .0
        .overworld
        .expect("the campaign came back");
    let again = Clash::muster(&loaded, a, b, &[], &[], "river_crossing".into());
    assert_eq!(first.seed, again.seed);

    let at = state.army(b).unwrap().pos;
    assert_eq!(
        first.seed,
        engagement_seed(
            11,
            EngagementKey {
                when: state.turn as u64,
                at,
                attacker: a.0,
                defender: b.0,
            }
        ),
        "the fight's dice are its world's seed and its own name, nothing else"
    );
}

// --- residency (W1.5) ---

use std::sync::Arc;
use tactics_core::ai::{AiConfig, AiDriver, make_battle_planner};
use tactics_core::battle::{Muster, SideState};
use tactics_core::data::WorldGen;
use tactics_core::map::{HexMap, Scenario};
use tactics_core::world::needed_chunks;
use tactics_core::worldgen::GeneratedWorld;

#[test]
fn the_chunks_a_battle_needs_cover_every_tile_its_crews_can_reach() {
    // Pure in its argument and conservative: every hex within `reach` of a
    // position lies in a chunk the answer names, whatever order the
    // positions come in.
    let positions = [
        (Hex::new(3, -40), 12),
        (Hex::new(-55, 17), 30),
        (Hex::new(0, 0), 0),
        (Hex::new(41, -20), 25),
    ];
    let needed = needed_chunks(positions, R);
    let mut reversed = positions;
    reversed.reverse();
    assert_eq!(needed, needed_chunks(reversed, R));
    for (hex, reach) in positions {
        for tile in hex.range(reach) {
            let c = chunk_of(tile, R);
            assert!(
                needed.contains(&(c.x, c.y)),
                "{tile:?} near {hex:?} is in no needed chunk"
            );
        }
    }
}

/// Fight `river_crossing`'s armies, formations and objectives for `rounds`
/// on generated ground, and write down everything that happened.
/// Returns the transcript, the most chunks held at once, and how many rounds
/// ended with a different set of chunks from the one they began with.
fn fight_on(
    reg: &tactics_core::data::DataRegistry,
    world: World,
    rounds: usize,
) -> (String, usize, usize) {
    let file = reg.map("river_crossing").expect("the baseline map");
    let scenario = Scenario::from_map_file(file);
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &file.units);
    let ids: Vec<tactics_core::battle::UnitId> = (0..file.units.len() as u32)
        .map(tactics_core::battle::UnitId)
        .collect();
    let sides: Vec<SideState> = file
        .sides
        .iter()
        .map(|s| SideState {
            name: s.name.clone(),
            ai: None,
        })
        .collect();
    let mut state = BattleState::from_muster_on(
        reg,
        world,
        scenario,
        sides,
        Muster {
            placements: &file.units,
            crews: &crews,
            ids: &ids,
        },
        Arc::new(roster),
        17,
    )
    .expect("the baseline's armies stand on generated ground");
    let mut ai = AiDriver::new();
    for (side, doctrine) in [(0u8, "massed_armor"), (1, "elastic_defense")] {
        ai.insert(
            side,
            make_battle_planner(
                &AiConfig {
                    planner: "utility".into(),
                    difficulty: 3,
                    doctrine: Some(doctrine.into()),
                },
                17 + side as u64,
                reg,
            ),
        );
    }
    let mut out = String::new();
    let mut most = 0;
    let mut paged = 0;
    let held = |s: &BattleState| -> Vec<Hex> {
        s.world
            .resident_chunks()
            .map_or(Vec::new(), |c| c.collect())
    };
    for _ in 0..rounds {
        if state.is_over() {
            break;
        }
        let before = held(&state);
        ai.plan_round_with(reg, &mut state, |d| {
            out.push_str(&format!("order {:?}\n", d.order))
        });
        for event in state.resolve_round(reg) {
            out.push_str(&format!("{event:?}\n"));
        }
        let after = held(&state);
        most = most.max(after.len());
        paged += (before != after) as usize;
    }
    for unit in &state.units {
        out.push_str(&format!(
            "final {} {:?} {:?}\n",
            unit.name, unit.pos, unit.fate
        ));
    }
    (out, most, paged)
}

#[test]
fn a_battle_on_a_window_of_the_world_is_the_battle_on_the_whole_of_it() {
    // The residency rule, tested the only way that means anything: the same
    // battle fought on the whole of a generated world and on a window onto
    // it that loads and forgets chunks as the crews move. If any question a
    // battle asks reached ground the window had not loaded, the two would
    // come apart (and in a debug build a sight line would panic first).
    let reg = registry();
    let rules = WorldGen {
        radius: 4,
        ..reg.worldgen.clone().unwrap()
    };
    let made = Arc::new(GeneratedWorld::with_rules(rules, R, 9).unwrap());
    let mut whole = HexMap::default();
    for chunk in made.chunks() {
        for (hex, terrain, level) in made.chunk_tiles(chunk) {
            whole.insert(hex, terrain, level);
        }
    }
    let total = made.chunks().count();
    let (on_whole, _, _) = fight_on(&reg, World::build(&reg, whole), 12);
    let (on_window, most, paged) = fight_on(&reg, World::window_onto(made), 12);
    assert!(
        most < total,
        "the window held all {total} chunks at once, so it never paged anything"
    );
    assert!(
        paged > 0,
        "the crews never moved the window, so the rule was never exercised"
    );
    assert!(
        on_window.contains("ShotHit"),
        "a battle nobody hit anybody in proves little"
    );
    if on_whole != on_window {
        let line = on_whole
            .lines()
            .zip(on_window.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(0);
        panic!(
            "the battles part at line {line}:\n  whole:  {}\n  window: {}",
            on_whole.lines().nth(line).unwrap_or(""),
            on_window.lines().nth(line).unwrap_or("")
        );
    }
}

// --- saves (W1.6) ---

use tactics_core::save::SaveGame as Save;

/// `river_crossing`'s armies on a window onto a radius-4 generated world.
fn staged_on_window(reg: &tactics_core::data::DataRegistry, seed: u64) -> BattleState {
    let rules = WorldGen {
        radius: 4,
        ..reg.worldgen.clone().unwrap()
    };
    let made = Arc::new(GeneratedWorld::with_rules(rules, R, seed).unwrap());
    let file = reg.map("river_crossing").unwrap();
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &file.units);
    let ids: Vec<tactics_core::battle::UnitId> = (0..file.units.len() as u32)
        .map(tactics_core::battle::UnitId)
        .collect();
    let sides = file
        .sides
        .iter()
        .map(|s| SideState {
            name: s.name.clone(),
            ai: None,
        })
        .collect();
    BattleState::from_muster_on(
        reg,
        World::window_onto(made),
        Scenario::from_map_file(file),
        sides,
        Muster {
            placements: &file.units,
            crews: &crews,
            ids: &ids,
        },
        Arc::new(roster),
        seed,
    )
    .unwrap()
}

/// Play `rounds` with fresh planners seeded `ai_seed`; the transcript.
fn play_rounds(
    reg: &tactics_core::data::DataRegistry,
    state: &mut BattleState,
    rounds: usize,
    ai_seed: u64,
) -> String {
    let mut ai = AiDriver::new();
    for (side, doctrine) in [(0u8, "massed_armor"), (1, "elastic_defense")] {
        ai.insert(
            side,
            make_battle_planner(
                &AiConfig {
                    planner: "utility".into(),
                    difficulty: 3,
                    doctrine: Some(doctrine.into()),
                },
                ai_seed + side as u64,
                reg,
            ),
        );
    }
    let mut out = String::new();
    for _ in 0..rounds {
        if state.is_over() {
            break;
        }
        ai.plan_round_with(reg, state, |d| {
            out.push_str(&format!("order {:?}\n", d.order))
        });
        for event in state.resolve_round(reg) {
            out.push_str(&format!("{event:?}\n"));
        }
    }
    out
}

#[test]
fn a_battle_on_generated_ground_goes_through_a_save_and_fights_on_the_same() {
    // The future round-trips, which is the property `tests/save.rs` pins for
    // a battle on a map, pinned here for one on a window: the ground comes
    // back from the generator's inputs rather than from the file, the same
    // chunks are resident, and what happens next is the same.
    let reg = registry();
    let mut original = staged_on_window(&reg, 21);
    play_rounds(&reg, &mut original, 3, 5);

    let text = Save::new(&reg, None, Some(original.clone()))
        .to_json()
        .expect("a window saves");
    assert!(
        !text.contains("\"palette\""),
        "a window is saved as how to make it, not as its tiles"
    );
    let mut restored = Save::from_json(&reg, &text)
        .expect("and loads")
        .0
        .battle
        .expect("the battle came back");
    assert_eq!(
        restored
            .world
            .resident_chunks()
            .unwrap()
            .collect::<Vec<_>>(),
        original
            .world
            .resident_chunks()
            .unwrap()
            .collect::<Vec<_>>()
    );
    assert_eq!(restored.world.len(), original.world.len());

    let expected = play_rounds(&reg, &mut original, 5, 5);
    let actual = play_rounds(&reg, &mut restored, 5, 5);
    assert!(!expected.is_empty(), "the battle is still live");
    assert_eq!(
        actual, expected,
        "a reloaded window must fight on exactly as before"
    );
}

#[test]
fn ground_that_was_changed_stays_changed_when_forgotten_and_when_saved() {
    let reg = registry();
    let mut state = staged_on_window(&reg, 21);
    let world = Arc::make_mut(&mut state.world);
    let chunk = world.resident_chunks().unwrap().next().unwrap();
    let hex = chunk_centre(chunk, R);
    world.edit(&reg, hex, "water", 0);
    assert_eq!(world.get(hex).unwrap().terrain, "water");

    world.unload_chunk(chunk);
    assert_eq!(world.presence(hex), Presence::Unloaded);
    let needed = std::iter::once((chunk.x, chunk.y))
        .chain(world.resident_chunks().unwrap().map(|c| (c.x, c.y)))
        .collect();
    world.settle(&reg, &needed);
    assert_eq!(
        world.get(hex).unwrap().terrain,
        "water",
        "the edit is laid over the chunk when it comes back"
    );

    let text = Save::new(&reg, None, Some(state)).to_json().unwrap();
    let back = Save::from_json(&reg, &text).unwrap().0.battle.unwrap();
    assert_eq!(
        back.world.get(hex).unwrap().terrain,
        "water",
        "and it is in the save"
    );
}

#[test]
fn a_generated_world_is_saved_as_how_to_make_it_again() {
    let reg = registry();
    let made = GeneratedWorld::new(&reg, 3).unwrap();
    let text = serde_json::to_string(&made).unwrap();
    assert!(text.len() < 4_000, "{} bytes for a world", text.len());
    let back: GeneratedWorld = serde_json::from_str(&text).unwrap();
    assert_eq!(back.skeleton, made.skeleton);
    let chunk = Hex::new(1, -1);
    assert_eq!(back.chunk_tiles(chunk), made.chunk_tiles(chunk));
}

// --- a person in command (WORLD.md W4.2–W4.4) ---

use tactics_core::battle::{Event, Latitude, Order, UnitId};

/// river_crossing, and two crews of side 0 in different companies: one in
/// the company its first formation's leader leads, one in another.
fn two_companies(reg: &tactics_core::data::DataRegistry) -> (BattleState, UnitId, UnitId, UnitId) {
    let state = BattleState::from_map(reg, "river_crossing", 3).unwrap();
    let side0: Vec<_> = state.formations().iter().filter(|f| f.side == 0).collect();
    assert!(side0.len() >= 2, "the baseline has two companies a side");
    let her = side0[0].leader.unwrap();
    let own = *side0[0].members.iter().find(|m| **m != her).unwrap_or(&her);
    let other = side0[1].members[0];
    (state, her, own, other)
}

fn radio(
    state: &mut BattleState,
    reg: &tactics_core::data::DataRegistry,
    unit: UnitId,
) -> Vec<Event> {
    let to = state.lookup(unit).unwrap().pos;
    let to = to
        .all_neighbors()
        .into_iter()
        .find(|h| state.world.contains(*h) && state.occupants(*h).next().is_none())
        .unwrap();
    state
        .apply(
            reg,
            &Order::Radio {
                unit,
                to: Some(to),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("the order is legal")
}

#[test]
fn an_order_past_her_companies_lands_a_round_late() {
    // The designer's ruling: she orders her companies; an order to a crew in
    // a company she does not ride with reaches down past its commander, and
    // travels the net to land a round late — and on landing takes the crew
    // out of her company's plan, as any direct order does.
    let reg = registry();
    let (mut state, her, own, other) = two_companies(&reg);
    state.commanders = vec![(0, Some(her))];
    assert!(
        !state.reaches_down(own),
        "her own company is one level down"
    );
    assert!(state.reaches_down(other));

    let held = radio(&mut state, &reg, other);
    assert!(
        held.iter()
            .any(|e| matches!(e, Event::OrdersWaiting { unit } if *unit == other)),
        "reaching down is held at the radio: {held:?}"
    );
    assert!(state.lookup(other).unwrap().orders.is_none(), "not yet");

    let direct = radio(&mut state, &reg, own);
    assert!(direct.is_empty(), "her own company's crew is told at once");
    assert!(state.lookup(own).unwrap().orders.is_some());

    for side in 0..state.sides.len() as u8 {
        let _ = state.apply(&reg, &Order::Commit { side });
    }
    let round = state.resolve_round(&reg);
    let _ = round;
    assert!(
        state.lookup(other).unwrap().orders.is_some() || !state.lookup(other).unwrap().alive(),
        "a round later it has landed, and she is out of her company's plan"
    );
}

#[test]
fn with_nobody_commanding_in_person_no_order_reaches_down() {
    // Every battle before this, and every side the AI commands: the rule's
    // absence is the game as it was.
    let reg = registry();
    let (mut state, _, _, other) = two_companies(&reg);
    assert!(state.commanders.is_empty());
    assert!(!state.reaches_down(other));
    assert!(radio(&mut state, &reg, other).is_empty());
}
