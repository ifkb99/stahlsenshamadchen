//! An attack on defended ground, and how a fight like that ends (ASSAULT.md).
//!
//! A defender starts on the ground she holds. The battle is judged when the
//! fighting stops — not by a clock, but because a side breaks, a side is
//! gone, or the attack has stalled — and then the ground decides: a broken
//! defender has given hers up, and otherwise the attacker wins if she holds
//! what she needs. The points still accrue and decide nothing.
//!
//! - ground held at the start
//! - the ground decides an assault
//! - an attack still closing in has not stalled
//! - breaking

use tactics_core::battle::{
    BattleState, EndReason, Event as BattleEvent, Order, SideState, UnitId,
};
use tactics_core::data::{DataRegistry, ValidationReport};
use tactics_core::map::{Battlefield, MapFile, UnitPlacement};

mod common;
use common::{play_round, registry, strike_down, unit_at};

/// A map file for these stages: open grass with a forest curtain at
/// column `curtain`, so the two sides are out of each other's sight, and
/// whatever scenario `extra` declares.
fn assault_map(width: usize, curtain: usize, extra: serde_json::Value) -> MapFile {
    let row: String = (0..width)
        .map(|c| {
            if c == curtain || c == curtain + 1 {
                'f'
            } else {
                'g'
            }
        })
        .collect();
    let mut file = serde_json::json!({
        "id": "assault_map",
        "palette": { "g": "grass", "f": "forest" },
        "rows": [row.clone(), row.clone(), row],
        "sides": [{ "name": "West" }, { "name": "East" }],
    });
    for (key, value) in extra.as_object().expect("an object of fields") {
        file[key] = value.clone();
    }
    serde_json::from_value(file).expect("the stage is a valid map file")
}

fn stage(reg: &DataRegistry, file: &MapFile, placements: Vec<UnitPlacement>) -> BattleState {
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
        Battlefield::from_map_file(file).unwrap(),
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        1,
    )
    .expect("the staged placements are content the base mod ships")
}

/// Play rounds until the battle ends or `rounds` have gone, returning how
/// it ended and in which round.
fn play_out(
    reg: &DataRegistry,
    state: &mut BattleState,
    rounds: u32,
    mut each: impl FnMut(&mut BattleState),
) -> Option<(Option<u8>, EndReason, u32)> {
    for round in 1..=rounds {
        each(state);
        let events = play_round(reg, state);
        if let Some(BattleEvent::BattleEnded { winner, reason }) = events
            .iter()
            .find(|e| matches!(e, BattleEvent::BattleEnded { .. }))
        {
            return Some((*winner, *reason, round));
        }
    }
    None
}

// --- ground held at the start ------------------------------------------------

#[test]
fn ground_a_defender_holds_when_the_battle_opens_is_hers_and_pays_her_from_the_first_round() {
    // A defended position is ground its defender already holds: before
    // this, every objective opened as nobody's and a defender sitting on
    // hers had to "take" it on the first tick like anybody passing by. The
    // proof that it is hers rather than merely occupied is that she is paid
    // for ground she is not standing on.
    let reg = registry();
    let file = assault_map(
        14,
        6,
        serde_json::json!({
            "objectives": [
                { "id": "village", "at": [[11, 1]], "value": 2, "held": 1 },
                { "id": "open", "at": [[3, 2]], "value": 1 },
            ],
        }),
    );
    let mut state = stage(
        &reg,
        &file,
        vec![
            unit_at([0, 0], 0, "medium_tank", "West"),
            unit_at([13, 0], 1, "medium_tank", "East"),
        ],
    );
    let held: Vec<Option<u8>> = state.objectives().map(|(_, h)| h).collect();
    assert_eq!(
        held,
        vec![Some(1), None],
        "hers from the start, and nobody's"
    );
    play_round(&reg, &mut state);
    assert_eq!(state.score(1), 2, "paid for the village in the first round");
    assert_eq!(state.score(0), 0);
}

// --- the ground decides an assault ------------------------------------------

/// West attacks East's two villages and needs one of them. The near one
/// is on her side of the curtain, so she can take it without a shot.
fn two_villages(reg: &DataRegistry) -> BattleState {
    let file = assault_map(
        14,
        6,
        serde_json::json!({
            "objectives": [
                { "id": "near", "at": [[3, 1]], "value": 1, "held": 1 },
                { "id": "far", "at": [[11, 1]], "value": 3, "held": 1 },
            ],
            "assault": { "attacker": 0, "needs": 1 },
        }),
    );
    stage(
        reg,
        &file,
        vec![
            unit_at([0, 1], 0, "medium_tank", "West"),
            unit_at([13, 1], 1, "medium_tank", "East"),
        ],
    )
}

#[test]
fn an_assault_that_stalls_goes_to_whoever_the_ground_says_whatever_the_points_say() {
    // The attacker takes the near village in the first round and sits on
    // it; the far one is worth three times as much and the defender is
    // paid for it every round, so the points are hers. The attack has stalled — nothing
    // is being achieved — and the ground says the attacker holds what she
    // needed. An attack called off keeps what it took.
    let reg = registry();
    let mut state = two_villages(&reg);
    let near = tactics_core::offset_to_hex(3, 1);
    let ended = play_out(&reg, &mut state, 30, |state| {
        if state.unit(UnitId(0)).is_some_and(|u| u.pos != near) {
            let _ = state.apply(
                &reg,
                &Order::SetMove {
                    unit: UnitId(0),
                    to: near,
                },
            );
        }
    });
    assert!(
        state.score(1) > state.score(0),
        "the defender leads on points ({} to {})",
        state.score(1),
        state.score(0)
    );
    assert_eq!(
        ended.map(|(w, r, _)| (w, r)),
        Some((Some(0), EndReason::Stalemate)),
        "and the attacker wins on the ground"
    );

    // The same stall with the attacker never moving: she holds nothing,
    // and the defender wins.
    let mut state = two_villages(&reg);
    let ended = play_out(&reg, &mut state, 30, |_| {});
    assert_eq!(
        ended.map(|(w, r, _)| (w, r)),
        Some((Some(1), EndReason::Stalemate))
    );
}

#[test]
fn an_assault_needing_more_ground_than_the_map_has_is_refused() {
    // The verdict is read off the ground, so an attacker who needs more
    // objectives than there are has lost before she starts, and one who
    // needs none has won: both are typos, and both fail validation.
    let reg = registry();
    for needs in [0, 3] {
        let file = assault_map(
            14,
            6,
            serde_json::json!({
                "objectives": [
                    { "id": "near", "at": [[3, 1]], "held": 1 },
                    { "id": "far", "at": [[11, 1]], "held": 1 },
                ],
                "assault": { "attacker": 0, "needs": needs },
            }),
        );
        let mut report = ValidationReport::default();
        file.validate_into(&reg, &mut report);
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.contains("the attacker needs")),
            "needs {needs} of two should be refused: {:?}",
            report.errors
        );
    }
}

// --- an attack still closing in has not stalled -----------------------------

/// A rifle platoon walking a hex a round at a village twelve hexes off,
/// the defender out of sight beyond the curtain: no shot is fired for the
/// whole approach. `assault` says whether the battle is one.
fn long_walk(reg: &DataRegistry, assault: bool) -> Option<(Option<u8>, EndReason, u32)> {
    let mut extra = serde_json::json!({
        "objectives": [{ "id": "village", "at": [[13, 1]], "value": 1, "held": 1 }],
    });
    if assault {
        extra["assault"] = serde_json::json!({ "attacker": 0, "needs": 1 });
    }
    let file = assault_map(24, 16, extra);
    let mut state = stage(
        reg,
        &file,
        vec![
            unit_at([0, 1], 0, "rifle_platoon", "West"),
            unit_at([23, 1], 1, "medium_tank", "East"),
        ],
    );
    let target = tactics_core::offset_to_hex(13, 1);
    play_out(reg, &mut state, 14, |state| {
        let Some(pos) = state.unit(UnitId(0)).map(|u| u.pos) else {
            return;
        };
        if pos != target {
            let step = tactics_core::offset_to_hex(tactics_core::hex_to_offset(pos)[0] + 1, 1);
            let _ = state.apply(
                reg,
                &Order::SetMove {
                    unit: UnitId(0),
                    to: step,
                },
            );
        }
    })
}

#[test]
fn an_attack_still_closing_on_its_objective_has_not_stalled() {
    // The stalemate clock counts accomplishment, and before an assault
    // there was none on an approach march: start lines kilometres apart
    // ended the battle before the first shot. An attacker who is still
    // getting closer to ground she does not hold is making progress.
    let reg = registry();
    let walked = long_walk(&reg, true);
    assert!(
        walked.is_none_or(|(_, reason, round)| reason != EndReason::Stalemate || round > 13),
        "the walk is thirteen rounds and the attack had not stalled on it: {walked:?}"
    );
    // The same walk in a battle that is not an assault is the rule as it
    // always was: nobody is shooting, so the clock runs out.
    let control = long_walk(&reg, false);
    assert!(
        control.is_some_and(|(_, reason, round)| reason == EndReason::Stalemate
            && round <= reg.balance.stalemate_rounds + 1),
        "{control:?}"
    );
}

// --- breaking ----------------------------------------------------------------

/// Two tanks a side, curtained apart, and whatever scenario `extra` says.
fn pairs(reg: &DataRegistry, extra: serde_json::Value) -> BattleState {
    let file = assault_map(14, 6, extra);
    stage(
        reg,
        &file,
        vec![
            unit_at([0, 0], 0, "medium_tank", "West 1"),
            unit_at([0, 2], 0, "medium_tank", "West 2"),
            unit_at([13, 0], 1, "medium_tank", "East 1"),
            unit_at([13, 2], 1, "medium_tank", "East 2"),
        ],
    )
}

#[test]
fn a_side_that_has_lost_what_it_can_afford_breaks_and_the_battle_ends_with_it() {
    // Units stop long before they are gone. West can lose two fifths of
    // her strength; losing one of two tanks is half, and she is done.
    let reg = registry();
    let mut state = pairs(
        &reg,
        serde_json::json!({ "breaking": [{ "side": 0, "lost_percent": 40 }] }),
    );
    strike_down(&mut state, UnitId(0));
    let ended = play_out(&reg, &mut state, 1, |_| {});
    assert_eq!(
        ended.map(|(w, r, _)| (w, r)),
        Some((Some(1), EndReason::Broken))
    );

    // East declared no break point, and loses the same tank without
    // breaking: a side that says nothing fights until it is gone.
    let mut state = pairs(
        &reg,
        serde_json::json!({ "breaking": [{ "side": 0, "lost_percent": 40 }] }),
    );
    strike_down(&mut state, UnitId(2));
    assert_eq!(play_out(&reg, &mut state, 1, |_| {}), None);
}

#[test]
fn a_defender_who_breaks_gives_up_her_ground_and_an_attacker_who_breaks_keeps_what_she_holds() {
    let reg = registry();
    let scenario = |attacker_holds: bool| {
        serde_json::json!({
            "objectives": [
                { "id": "near", "at": [[3, 1]], "held": if attacker_holds { 0 } else { 1 } },
                { "id": "far", "at": [[11, 1]], "held": 1 },
            ],
            "assault": { "attacker": 0, "needs": 1 },
            "breaking": [
                { "side": 0, "lost_percent": 40 },
                { "side": 1, "lost_percent": 40 },
            ],
        })
    };
    // The defender breaks holding everything: she has given it up.
    let mut state = pairs(&reg, scenario(false));
    strike_down(&mut state, UnitId(2));
    assert_eq!(
        play_out(&reg, &mut state, 1, |_| {}).map(|(w, r, _)| (w, r)),
        Some((Some(0), EndReason::Broken)),
        "a broken defender has lost her ground, whatever she stood on"
    );
    // The attacker breaks holding nothing: the attack failed.
    let mut state = pairs(&reg, scenario(false));
    strike_down(&mut state, UnitId(0));
    assert_eq!(
        play_out(&reg, &mut state, 1, |_| {}).map(|(w, r, _)| (w, r)),
        Some((Some(1), EndReason::Broken))
    );
    // The attacker breaks holding what she needed: she keeps it.
    let mut state = pairs(&reg, scenario(true));
    strike_down(&mut state, UnitId(0));
    assert_eq!(
        play_out(&reg, &mut state, 1, |_| {}).map(|(w, r, _)| (w, r)),
        Some((Some(0), EndReason::Broken)),
        "an attack that has spent itself still holds what it took"
    );
}

// --- prepared positions ------------------------------------------------------

#[test]
fn a_prepared_defender_starts_dug_in_keeps_her_hole_and_leaves_it_when_she_moves() {
    // Hours with a shovel and a dozer before the attack: deeper than
    // anything a crew scrapes under fire, so the round's digging, capped
    // at what can be dug in a fight, must not fill it in; and like any
    // hole it stays behind when she drives out of it, which is what a
    // counterattack costs her.
    let reg = registry();
    let file = assault_map(14, 6, serde_json::json!({ "prepared": [1] }));
    let mut state = stage(
        &reg,
        &file,
        vec![
            unit_at([0, 1], 0, "medium_tank", "West"),
            unit_at([13, 1], 1, "medium_tank", "East"),
        ],
    );
    let deep = reg.vehicle("medium_tank").unwrap().prepared_cover;
    assert!(
        deep > reg.balance.dig_in_max,
        "the base mod's prepared position ({deep}) is deeper than a scrape ({})",
        reg.balance.dig_in_max
    );
    assert_eq!(state.unit(UnitId(1)).unwrap().dug_in, deep);
    assert_eq!(
        state.unit(UnitId(0)).unwrap().dug_in,
        0,
        "the attacker dug nothing"
    );
    play_round(&reg, &mut state);
    assert_eq!(
        state.unit(UnitId(1)).unwrap().dug_in,
        deep,
        "a round sat in it"
    );
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(1),
                to: tactics_core::offset_to_hex(12, 1),
            },
        )
        .expect("she can drive a hex");
    play_round(&reg, &mut state);
    assert_eq!(state.unit(UnitId(1)).unwrap().dug_in, 0, "out of her hole");
}

// --- mines -------------------------------------------------------------------

/// West's tank at the west end of a three-row strip, East's out of sight,
/// and `fields` sown.
fn sown(reg: &DataRegistry, fields: serde_json::Value, west: &str) -> BattleState {
    let file = assault_map(14, 8, serde_json::json!({ "minefields": fields }));
    stage(
        reg,
        &file,
        vec![
            unit_at([0, 1], 0, west, "West"),
            unit_at([13, 1], 1, "medium_tank", "East"),
        ],
    )
}

fn drive(
    reg: &DataRegistry,
    state: &mut BattleState,
    unit: UnitId,
    to: [i32; 2],
) -> Vec<BattleEvent> {
    state
        .apply(
            reg,
            &Order::SetMove {
                unit,
                to: tactics_core::offset_to_hex(to[0], to[1]),
            },
        )
        .expect("the road is in reach");
    play_round(reg, state)
}

#[test]
fn a_crew_who_drives_into_an_enemy_minefield_is_stopped_crippled_and_her_side_learns_where_it_lies()
{
    let reg = registry();
    let field = serde_json::json!([{
        "side": 1, "mine": "anti_tank_mine", "density": 100, "at": [[2, 1], [2, 0], [2, 2]],
    }]);
    let mut state = sown(&reg, field, "medium_tank");
    let mined = tactics_core::offset_to_hex(2, 1);
    assert!(
        !state.known_mines(&reg, 0, mined),
        "nobody has found the field yet"
    );
    let events = drive(&reg, &mut state, UnitId(0), [4, 1]);
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::MineStruck {
                unit: UnitId(0),
                ..
            }
        )),
        "a field sown this thickly is certain"
    );
    let tank = state.unit(UnitId(0)).unwrap();
    assert_eq!(tank.pos, mined, "she stopped on the mine");
    let mobility_left: u32 = tank
        .modules
        .iter()
        .filter(|(id, _)| {
            reg.module(id)
                .is_some_and(|m| m.effect == tactics_core::data::ModuleEffect::Mobility)
        })
        .map(|(_, hits)| *hits)
        .sum();
    assert_eq!(mobility_left, 0, "and her running gear went with it");
    assert!(
        state.known_mines(&reg, 0, tactics_core::offset_to_hex(2, 2)),
        "her side now knows where the whole field lies"
    );
    assert!(
        !state.known_mines(&reg, 1, mined),
        "and a side never reads its own mines as a hazard"
    );
}

#[test]
fn nobody_sets_off_her_own_side_s_mines_and_a_man_walks_over_an_anti_tank_mine() {
    let reg = registry();
    let at = |side: u8| {
        serde_json::json!([{
            "side": side, "mine": "anti_tank_mine", "density": 100, "at": [[2, 1]],
        }])
    };
    // Her own side laid them: she knows the lanes.
    let mut state = sown(&reg, at(0), "medium_tank");
    let events = drive(&reg, &mut state, UnitId(0), [4, 1]);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::MineStruck { .. }))
    );
    assert_eq!(
        state.unit(UnitId(0)).unwrap().pos,
        tactics_core::offset_to_hex(4, 1)
    );
    // The enemy's, but an anti-tank mine needs a hull's weight.
    let mut state = sown(&reg, at(1), "rifle_platoon");
    let mut struck = false;
    for _ in 0..3 {
        let pos = tactics_core::hex_to_offset(state.unit(UnitId(0)).unwrap().pos);
        let events = drive(&reg, &mut state, UnitId(0), [pos[0] + 1, 1]);
        struck |= events
            .iter()
            .any(|e| matches!(e, BattleEvent::MineStruck { .. }));
    }
    assert!(!struck, "a man walks over it");
    assert_eq!(
        state.unit(UnitId(0)).unwrap().pos,
        tactics_core::offset_to_hex(3, 1),
        "and on through the field"
    );
}

#[test]
fn a_road_bends_round_a_field_her_side_has_found_and_not_round_one_it_has_not() {
    // Knowledge is the side's, never the field's: a crew whose side has
    // not found the mines drives the cheap road straight into them, so no
    // road gives away what nobody has seen, and one whose side has goes
    // round.
    let reg = registry();
    let file = assault_map(
        14,
        8,
        serde_json::json!({
            "minefields": [{
                "side": 1, "mine": "anti_tank_mine", "density": 100,
                "at": [[3, 0], [3, 1]],
            }],
        }),
    );
    // Row 2's crossing is clear: the way round is a hex longer, which a
    // light tank's round still reaches.
    let mut state = stage(
        &reg,
        &file,
        vec![
            unit_at([0, 0], 0, "medium_tank", "Finder"),
            unit_at([0, 1], 0, "light_tank", "Follower"),
            unit_at([13, 1], 1, "medium_tank", "East"),
        ],
    );
    let field: Vec<_> = [[3, 0], [3, 1]]
        .iter()
        .map(|at| tactics_core::offset_to_hex(at[0], at[1]))
        .collect();
    let target = tactics_core::offset_to_hex(5, 1);
    let through = |state: &BattleState| {
        tactics_core::battle::path_to(&reg, state, UnitId(1), target)
            .expect("in reach")
            .0
            .iter()
            .any(|h| field.contains(h))
    };
    assert!(
        through(&state),
        "nobody has found it: the cheap road is through it"
    );
    drive(&reg, &mut state, UnitId(0), [4, 0]);
    assert!(state.known_mines(&reg, 0, field[1]), "the finder found it");
    assert!(!through(&state), "and now the road goes round");
}

// --- the defended sector -----------------------------------------------------

/// Field `index` of `size`, with both sides' forces bought as `rollout`
/// buys them.
fn sector_battle(
    reg: &DataRegistry,
    worlds: &tactics_core::harness::open::Worlds,
    index: u64,
    size: tactics_core::harness::sector::SectorSize,
) -> (tactics_core::harness::sector::Sector, BattleState) {
    use tactics_core::force::{Purchase, buy};
    use tactics_core::harness::sector;
    let purchase = Purchase {
        points: sector::budget(size, index),
        max_vehicles: size.shape().max_vehicles,
    };
    let a = buy(reg, purchase, index * 7 + 1).expect("something is affordable");
    let b = buy(reg, purchase, index * 7 + 2).expect("something is affordable");
    let made = sector::sector(
        reg,
        worlds,
        index,
        size,
        sector::SectorRules::default(),
        [&a, &b],
    )
    .unwrap_or_else(|e| panic!("field {index}: {e}"));
    let state = sector::stage(reg, &made, [&a, &b], index).expect("every vehicle finds ground");
    (made, state)
}

#[test]
fn every_size_of_sector_is_an_assault_on_ground_the_defender_already_holds() {
    use tactics_core::harness::sector::SectorSize;
    let reg = registry();
    let worlds = tactics_core::harness::open::Worlds::default();
    for size in SectorSize::ALL {
        let (lo, hi) = size.shape().zones;
        for index in 0..4 {
            let (made, state) = sector_battle(&reg, &worlds, index, size);
            let zones: Vec<Option<u8>> = state.objectives().map(|(_, h)| h).collect();
            assert!(
                (lo as usize..=hi as usize).contains(&zones.len()),
                "{} field {index}: {} zones",
                size.name(),
                zones.len()
            );
            assert!(
                zones.iter().all(|h| *h == Some(1)),
                "the defender holds them all"
            );
            let assault = state.scenario.assault().expect("an assault");
            assert_eq!(assault.attacker, 0);
            assert!(
                assault.needs > zones.len() as u32 / 2 && assault.needs <= zones.len() as u32,
                "the attacker needs most of them: {} of {}",
                assault.needs,
                zones.len()
            );
            assert!(state.scenario.break_point(0) < state.scenario.break_point(1));
            // A prepared defender's riflemen start on the ground, dug in,
            // not inside a carrier.
            if made.prepared {
                for unit in state.units.iter().filter(|u| u.side == 1) {
                    assert!(unit.aboard.is_none(), "{} is aboard", unit.name);
                    assert!(unit.dug_in > 0, "{} has not dug in", unit.name);
                }
            } else {
                assert!(state.scenario.minefields().is_empty() && made.obstacles.is_empty());
            }
        }
    }
}

#[test]
fn a_sector_is_the_same_sector_whoever_asks_for_it_first() {
    // A world is made once and shared between threads; a field drawn from
    // it must not depend on which other fields were drawn before it.
    use tactics_core::harness::sector::SectorSize;
    let reg = registry();
    let fresh = tactics_core::harness::open::Worlds::default();
    let used = tactics_core::harness::open::Worlds::default();
    for index in [5, 2, 7] {
        sector_battle(&reg, &used, index, SectorSize::Medium);
    }
    let (a, _) = sector_battle(&reg, &fresh, 3, SectorSize::Medium);
    let (b, _) = sector_battle(&reg, &used, 3, SectorSize::Medium);
    assert_eq!(a.zones, b.zones);
    assert_eq!(a.deployment, b.deployment);
    assert_eq!(a.obstacles, b.obstacles);
    assert_eq!(a.scenario, b.scenario);
}
