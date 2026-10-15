//! Why a flank pays, as rules (REIMAGINED.md, *Flanks*; the designer's
//! rulings of 2026-10-15): a flank defeats terrain, it divides a defender's
//! attention, and closing a crew up blinds her to it. Each rule is data whose
//! zero is the game before, and each test here stages the one fact it is
//! about.
//!
//! - cover from the ground crossed
//! - turret traverse
//! - awareness has a direction
//! - the shock of the flank
//! - prepared positions
//! - smoke

use tactics_core::battle::{BattleState, Event, FireIntent, Order, SideState, UnitId, hit_chance};
use tactics_core::data::DataRegistry;
use tactics_core::map::Battlefield;

mod common;
use common::{commit_all, laid, registry_wireless, seen, unit_at};

/// A battle on `rows` (`g` grass, `h` hedgerow, `f` forest) with optional
/// elevation rows, the placements given.
fn stage(
    reg: &DataRegistry,
    rows: &[&str],
    elevation: Option<&[&str]>,
    placements: Vec<tactics_core::map::UnitPlacement>,
    seed: u64,
) -> BattleState {
    let mut json = serde_json::json!({
        "id": "flank_test",
        "palette": { "g": "grass", "h": "hedgerow", "f": "forest" },
        "rows": rows,
    });
    if let Some(levels) = elevation {
        json["elevation"] = serde_json::json!(levels);
    }
    let file: tactics_core::map::MapFile = serde_json::from_value(json).unwrap();
    let map = Battlefield::from_map_file(&file).unwrap();
    let sides = ["West", "East"]
        .map(|name| SideState {
            name: name.into(),
            ai: None,
        })
        .to_vec();
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
    .expect("staged content the base mod ships")
}

/// The chance a medium's 75 has against unit `target` from unit `shooter`.
fn chance(reg: &DataRegistry, state: &BattleState, shooter: u32, target: u32) -> i32 {
    let gun = reg.weapon("gun_75").expect("the 75");
    let from = state.unit(UnitId(shooter)).unwrap().pos;
    let at = state.unit(UnitId(target)).unwrap().pos;
    hit_chance(
        reg,
        state,
        UnitId(shooter),
        from,
        gun,
        UnitId(target),
        at,
        false,
    )
}

// --- cover from the ground crossed ------------------------------------------

/// A tank on grass with a hedge on her west side; one gun three hexes west,
/// firing across the hedge, and one three hexes north, whose line does not
/// touch it.
fn hedge_stage(reg: &DataRegistry) -> BattleState {
    stage(
        reg,
        &[
            "ggggggg", "ggggggg", "ggggggg", "ggghggg", "ggggggg", "ggggggg", "ggggggg",
        ],
        None,
        vec![
            unit_at([1, 3], 0, "medium_tank", "Front"),
            unit_at([4, 0], 0, "medium_tank", "Flank"),
            unit_at([4, 3], 1, "medium_tank", "Behind the hedge"),
        ],
        11,
    )
}

#[test]
fn a_hedge_covers_her_against_the_gun_in_front_and_not_against_the_flank() {
    // The rule's absence first: no lip, cover is her own hex's, grass from
    // both sides. (The base mod ships the lip at 100.)
    let mut reg = registry_wireless();
    reg.balance.cover_lip_percent = 0;
    reg.balance.hull_down_cover = 0;
    let state = hedge_stage(&reg);
    let (front, flank, her) = (0, 1, 2);
    let d = |a: u32| {
        state
            .unit(UnitId(a))
            .unwrap()
            .pos
            .distance_to(state.unit(UnitId(her)).unwrap().pos)
    };
    assert_eq!(
        d(front),
        d(flank),
        "the stage needs the two guns equally far"
    );
    assert_eq!(
        chance(&reg, &state, front, her),
        chance(&reg, &state, flank, her),
        "with no lip, cover is her own hex's, grass from both sides"
    );

    reg.balance.cover_lip_percent = 100;
    let state = hedge_stage(&reg);
    let through = chance(&reg, &state, front, her);
    let round = chance(&reg, &state, flank, her);
    let hedge = reg.terrain("hedgerow").unwrap().cover;
    assert_eq!(
        round - through,
        reg.balance.cover_against_accuracy(hedge),
        "the gun firing across the hedge meets its cover ({through}%), the flanker does \
         not ({round}%)"
    );
}

#[test]
fn a_crest_between_them_puts_her_hull_down_and_higher_ground_sees_over_it() {
    let mut reg = registry_wireless();
    reg.balance.hull_down_cover = 20;
    let rows = ["ggggggg", "ggggggg", "ggggggg"];
    // A rise one hex in front of her; the low gun is level with her, the high
    // one stands above the rise.
    let low = ["0000000", "0001000", "0000000"];
    let high = ["0000000", "2001000", "0000000"];
    let placements = || {
        vec![
            unit_at([0, 1], 0, "medium_tank", "Gun"),
            unit_at([4, 1], 1, "medium_tank", "Behind the rise"),
        ]
    };
    let flat = stage(&reg, &rows, None, placements(), 12);
    let behind = stage(&reg, &rows, Some(&low), placements(), 12);
    let above = stage(&reg, &rows, Some(&high), placements(), 12);
    let crest = reg.balance.cover_against_accuracy(20);
    assert_eq!(
        chance(&reg, &flat, 0, 1) - chance(&reg, &behind, 0, 1),
        crest,
        "a crest between a level gun and her is the hull-down cover"
    );
    assert!(
        chance(&reg, &above, 0, 1) >= chance(&reg, &flat, 0, 1),
        "and a gun on higher ground than the crest sees the hull over it"
    );
}

#[test]
fn a_hedge_hides_a_squad_from_the_side_it_faces_and_not_from_the_flank() {
    // The same rule for the looker: a squad behind a hedge, a tank six
    // hexes off across the hedge and another six hexes off her open side —
    // past the near band, where finding her is certain from anywhere. Over
    // many first looks, the one across the hedge finds her less often.
    //
    // The hedge is staged as thick as a thicket (concealment 100): past the
    // near band a terrain's concealment is faded in with distance, and the
    // shipped hedge's 15 is a point or two at this range — true, and too
    // small for a test whose question is only which way the hedge faces.
    let mut reg = registry_wireless();
    reg.balance.cover_lip_percent = 100;
    reg.terrain.get_mut("hedgerow").unwrap().concealment = 100;
    let rows = [
        "gggggggggg",
        "gggggggggg",
        "gggggggggg",
        "gggggggggg",
        "gggggggggg",
        "gggggggggg",
        "gggggggggg",
        "gggggghggg",
        "gggggggggg",
    ];
    let looks = |looker: [i32; 2]| -> u32 {
        (0..1000)
            .map(|seed| {
                let state = stage(
                    &reg,
                    &rows,
                    None,
                    vec![
                        unit_at(looker, 0, "medium_tank", "Looker"),
                        unit_at([7, 7], 1, "rifle_squad", "Behind the hedge"),
                    ],
                    seed,
                );
                state.fog.side(0).spotted.contains(&UnitId(1)) as u32
            })
            .sum()
    };
    let through = looks([1, 7]);
    let round = looks([7, 1]);
    assert!(
        through + 50 < round,
        "across the hedge she is found {through} times in 1000, from her open side {round}"
    );
}

// --- turret traverse ---------------------------------------------------------

/// The tick of the round on which a medium facing east, ordered to fire on a
/// medium three hexes behind her, first fires — and whether she said she was
/// traversing.
fn first_shot_behind(reg: &DataRegistry) -> (Option<u32>, bool) {
    let mut gunner = unit_at([4, 1], 0, "medium_tank", "Gunner");
    gunner.facing = Some(tactics_core::map::Facing::East);
    let mut state = stage(
        reg,
        &["ggggggggg", "ggggggggg", "ggggggggg"],
        None,
        vec![gunner, unit_at([1, 1], 1, "medium_tank", "Behind her")],
        21,
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
    let mut first = None;
    let mut traversed = false;
    while let Some(tick) = state.resolving_tick() {
        if state.is_over() {
            break;
        }
        for e in state.step_tick(reg) {
            match e {
                Event::ShotFired {
                    attacker: UnitId(0),
                    ..
                } => {
                    first.get_or_insert(tick);
                }
                Event::Traversed {
                    unit: UnitId(0), ..
                } => traversed = true,
                _ => {}
            }
        }
    }
    (first, traversed)
}

#[test]
fn a_turret_swinging_onto_a_target_behind_her_fires_late() {
    // Half a circle at a minute a full turn is thirty seconds: six ticks.
    // Awareness taken out: the subject is the turret, and she has to have
    // found the tank behind her to be ordered onto it.
    let mut reg = all_round(seen(registry_wireless()));
    reg.vehicles
        .get_mut("medium_tank")
        .unwrap()
        .traverse_seconds = 0;
    let (instant, said) = first_shot_behind(&reg);
    assert!(
        !said,
        "a turret with no traverse figure lays at once, as before"
    );
    reg.vehicles
        .get_mut("medium_tank")
        .unwrap()
        .traverse_seconds = 60;
    let (slow, said) = first_shot_behind(&reg);
    assert!(said, "a turret that has to swing says so");
    let (instant, slow) = (instant.expect("she fires at once"), slow.expect("and late"));
    assert!(
        slow >= instant + 6,
        "half a turn at a minute a circle costs six ticks: tick {instant} against {slow}"
    );
}

// --- awareness has a direction ------------------------------------------------

/// The registry with awareness all round, the rule's absence: the base mod
/// ships 70 off the front and 25 closed up, with its own figures for
/// infantry and recon.
fn all_round(mut reg: DataRegistry) -> DataRegistry {
    reg.balance.flank_search_percent = 100;
    reg.balance.buttoned_flank_search_percent = 100;
    for vehicle in reg.vehicles.values_mut() {
        vehicle.flank_search_percent = None;
        vehicle.buttoned_flank_search_percent = None;
    }
    reg
}

/// A medium facing east with a medium three hexes in front of her and
/// another three hexes behind: which of them her side has found.
fn looks_both_ways(reg: &DataRegistry, buttoned: bool) -> (bool, bool) {
    let mut looker = unit_at([4, 1], 0, "medium_tank", "Looker");
    looker.facing = Some(tactics_core::map::Facing::East);
    let mut state = stage(
        reg,
        &["ggggggggggg", "ggggggggggg", "ggggggggggg"],
        None,
        vec![
            looker,
            unit_at([7, 1], 1, "medium_tank", "In front"),
            unit_at([1, 1], 1, "medium_tank", "Behind"),
        ],
        31,
    );
    if buttoned {
        // Closed up, and looking again: nothing held, one tick for the look.
        state.unit_mut(UnitId(0)).unwrap().buttoned = 12;
        state.fog.side_mut(0).spotted.clear();
        commit_all(reg, &mut state);
        state.step_tick(reg);
    }
    let spotted = &state.fog.side(0).spotted;
    (spotted.contains(&UnitId(1)), spotted.contains(&UnitId(2)))
}

#[test]
fn a_crew_who_does_not_look_off_her_front_does_not_find_what_is_behind_her() {
    let mut reg = all_round(registry_wireless());
    assert_eq!(
        looks_both_ways(&reg, false),
        (true, true),
        "at three hexes on open grass a crew finds both, as before"
    );
    reg.balance.flank_search_percent = 0;
    assert_eq!(
        looks_both_ways(&reg, false),
        (true, false),
        "keeping none of her chance off her front, she finds the one in front alone"
    );
}

#[test]
fn closed_up_she_is_blind_to_her_flanks_and_not_to_her_front() {
    let mut reg = all_round(registry_wireless());
    reg.balance.buttoned_flank_search_percent = 0;
    assert_eq!(
        looks_both_ways(&reg, false),
        (true, true),
        "head out, the closed-up rule does not touch her"
    );
    assert_eq!(
        looks_both_ways(&reg, true),
        (true, false),
        "through periscopes she finds the tank in front and not the one behind"
    );
}

#[test]
fn a_chassis_trained_to_watch_all_round_says_so_over_the_mod() {
    // The mod keeps nothing off the front; the medium's own figure says her
    // crew looks every way, and hers is the one read.
    let mut reg = all_round(registry_wireless());
    reg.balance.flank_search_percent = 0;
    assert_eq!(looks_both_ways(&reg, false), (true, false));
    reg.vehicles
        .get_mut("medium_tank")
        .unwrap()
        .flank_search_percent = Some(100);
    assert_eq!(
        looks_both_ways(&reg, false),
        (true, true),
        "her chassis's figure overrides the mod's"
    );
}

// --- the shock of the flank -----------------------------------------------------

/// The pressure a squad facing west has taken after a round of a scout car's
/// belt from `shooter`, three hexes off, summed over `seeds` battles.
fn frightened(reg: &DataRegistry, shooter: [i32; 2], seeds: u64) -> u32 {
    (0..seeds)
        .map(|seed| {
            let mut squad = unit_at([4, 4], 1, "rifle_squad", "Squad");
            squad.facing = Some(tactics_core::map::Facing::West);
            let mut state = stage(
                reg,
                &["ggggggggg"; 9],
                None,
                vec![unit_at(shooter, 0, "recon_car", "Gun"), squad],
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
            // The squad holds still and holds her fire, so the only thing
            // that moves her nerve is the belt.
            state
                .apply(
                    reg,
                    &Order::SetFire {
                        unit: UnitId(1),
                        fire: FireIntent::Hold,
                    },
                )
                .unwrap();
            commit_all(reg, &mut state);
            state.resolve_round(reg);
            state.unit(UnitId(1)).map_or(0, |u| u.pressure)
        })
        .sum()
}

#[test]
fn fire_from_her_flank_frightens_her_more_than_the_same_fire_from_in_front() {
    let mut reg = all_round(seen(laid(registry_wireless())));
    reg.morale.recovery = 0;
    reg.morale.recovery_near_leader = 0;
    let (front, flank) = ([1, 4], [4, 1]);
    let before_front = frightened(&reg, front, 30);
    let before_flank = frightened(&reg, flank, 30);
    reg.morale.flank_percent = 300;
    let after_front = frightened(&reg, front, 30);
    let after_flank = frightened(&reg, flank, 30);
    assert_eq!(
        before_front, after_front,
        "the price of fire from in front does not move"
    );
    assert!(before_flank > 0, "the stage needs the belt to frighten her");
    assert!(
        after_flank >= 2 * before_flank,
        "from her flank at three times the price: {before_flank} -> {after_flank}"
    );
}

// --- prepared positions -------------------------------------------------------

/// A squad facing west on open grass, alone: nobody to shoot or be shot by.
fn digging_stage(reg: &DataRegistry) -> BattleState {
    let mut squad = unit_at([4, 4], 0, "rifle_squad", "Squad");
    squad.facing = Some(tactics_core::map::Facing::West);
    stage(
        reg,
        &["ggggggggg"; 9],
        None,
        vec![squad, unit_at([8, 8], 1, "rifle_squad", "Far off")],
        41,
    )
}

#[test]
fn a_crew_who_stays_put_digs_in_and_leaves_it_behind_when_she_moves() {
    let mut reg = registry_wireless();
    reg.balance.dig_in_max = 10;
    reg.vehicles
        .get_mut("rifle_squad")
        .unwrap()
        .dig_in_per_round = 4;
    let mut state = digging_stage(&reg);
    let dug = |state: &BattleState| state.unit(UnitId(0)).unwrap().dug_in;
    assert_eq!(dug(&state), 0, "she arrives with nothing dug");
    for expected in [4, 8, 10, 10] {
        commit_all(&reg, &mut state);
        state.resolve_round(&reg);
        assert_eq!(dug(&state), expected, "four a round to the cap of ten");
    }
    let next = tactics_core::offset_to_hex(5, 4);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: next,
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);
    assert_eq!(dug(&state), 0, "and the hole stays where she dug it");
}

#[test]
fn a_scrape_faces_the_enemy_it_was_dug_against() {
    let mut reg = registry_wireless();
    reg.balance.dig_in_max = 20;
    reg.balance.dig_in_flank_percent = 50;
    let state = digging_stage(&reg);
    // A medium three hexes in front of her and another three off her side.
    let gun = reg.weapon("gun_75").unwrap();
    let mut placed = state.clone();
    placed.unit_mut(UnitId(0)).unwrap().dug_in = 20;
    let her = placed.unit(UnitId(0)).unwrap().pos;
    let front = tactics_core::offset_to_hex(1, 4);
    let side = tactics_core::offset_to_hex(4, 1);
    assert_eq!(front.distance_to(her), side.distance_to(her));
    let shot = |state: &BattleState, from| {
        hit_chance(&reg, state, UnitId(1), from, gun, UnitId(0), her, false)
    };
    let full = reg.balance.cover_against_accuracy(20);
    let half = reg.balance.cover_against_accuracy(10);
    assert_eq!(
        shot(&state, front) - shot(&placed, front),
        full,
        "whole from in front"
    );
    assert_eq!(
        shot(&state, side) - shot(&placed, side),
        half,
        "half from her side"
    );
}

// --- smoke --------------------------------------------------------------------

#[test]
fn a_smoke_round_blinds_the_line_it_lands_on_and_clears_in_its_time() {
    let reg = laid(seen(registry_wireless()));
    let mut state = stage(
        &reg,
        &["ggggggggg"; 3],
        None,
        vec![
            unit_at([1, 1], 0, "medium_tank", "Smoker"),
            unit_at([7, 1], 1, "medium_tank", "Watched"),
        ],
        51,
    );
    let (a, b) = (
        state.unit(UnitId(0)).unwrap().pos,
        state.unit(UnitId(1)).unwrap().pos,
    );
    assert!(state.sees_through(a, b), "open grass: they see each other");
    let between = tactics_core::offset_to_hex(4, 1);
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: UnitId(0),
                fire: FireIntent::Smoke {
                    at: between,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    // The other side holds its fire, so the stage is about the cloud.
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: UnitId(1),
                fire: FireIntent::Hold,
            },
        )
        .unwrap();
    let events = commit_and_resolve(&reg, &mut state);
    assert!(
        events.iter().any(|e| matches!(e, Event::SmokeLaid { .. })),
        "the round goes down as smoke"
    );
    assert!(state.smoked(between));
    assert!(!state.sees_through(a, b), "and the line across it is blind");
    let spent = state.unit(UnitId(0)).unwrap().ammo.get("smoke_75").copied();
    assert_eq!(spent, Some(3), "one of her four smoke rounds");
    // Ninety seconds is eighteen ticks; two more rounds see it gone.
    for _ in 0..2 {
        commit_and_resolve(&reg, &mut state);
    }
    assert!(!state.smoked(between), "the cloud has cleared");
    assert!(
        state.sees_through(a, b),
        "and they can see each other again"
    );
}

#[test]
fn a_gun_with_no_smoke_round_cannot_be_told_to_fire_smoke() {
    let reg = registry_wireless();
    let mut state = stage(
        &reg,
        &["ggggggggg"; 3],
        None,
        vec![
            unit_at([1, 1], 0, "light_tank", "No smoke"),
            unit_at([7, 1], 1, "medium_tank", "Far"),
        ],
        52,
    );
    let refused = state.apply(
        &reg,
        &Order::SetFire {
            unit: UnitId(0),
            fire: FireIntent::Smoke {
                at: tactics_core::offset_to_hex(4, 1),
                weapon: 0,
            },
        },
    );
    assert!(refused.is_err(), "the 37 carries no smoke: {refused:?}");
}

/// Commit both sides and play the round out.
fn commit_and_resolve(reg: &DataRegistry, state: &mut BattleState) -> Vec<Event> {
    commit_all(reg, state);
    state.resolve_round(reg)
}
