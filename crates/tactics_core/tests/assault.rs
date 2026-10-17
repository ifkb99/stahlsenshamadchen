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
use common::{play_round, registry, seen, strike_down, unit_at};

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

/// A rifle squad of East's between two West scout cars, a medium tank of
/// hers out of the pincer, and East's break point as `breaking` says. No
/// gun is ready (the threat walk reads what a gun could do) and everybody
/// holds where she is told, so the drill does not walk anybody out of the
/// pincer: nothing is lost but what her side cannot count on.
fn pincer(reg: &DataRegistry, breaking: serde_json::Value) -> BattleState {
    let row = "g".repeat(13);
    let rows: Vec<String> = (0..13).map(|_| row.clone()).collect();
    let file: MapFile = serde_json::from_value(serde_json::json!({
        "id": "pincer",
        "palette": { "g": "grass" },
        "rows": rows,
        "sides": [{ "name": "West" }, { "name": "East" }],
        "breaking": [breaking],
    }))
    .expect("a valid stage");
    let mut state = stage(
        reg,
        &file,
        vec![
            unit_at([1, 6], 0, "recon_car", "West gun"),
            unit_at([11, 6], 0, "recon_car", "East gun"),
            unit_at([6, 6], 1, "rifle_squad", "Caught"),
            unit_at([6, 12], 1, "medium_tank", "Clear"),
        ],
    );
    for unit in state.units.iter_mut() {
        for cd in &mut unit.cooldowns {
            *cd = 1000;
        }
        unit.orders = Some(tactics_core::battle::PersonalOrder::Holding {
            latitude: tactics_core::battle::Latitude::Binding,
        });
    }
    state
}

#[test]
fn a_crew_her_side_cannot_count_on_counts_against_its_will_by_the_larger_share_never_the_sum() {
    // The record's largest single reason a force broke off was being
    // enveloped, encircled or penetrated; losses were the reason about
    // one time in ten (ASSAULT.md, *Breaking by manoeuvre*). So a crew
    // cut off, or whose nerve has gone, is that share of her lost to her
    // side before she is lost to the enemy — and she is one crew, so two
    // reasons are the larger of them, not more of her than there is.
    let reg = registry();
    let mut state = pincer(
        &reg,
        serde_json::json!({ "side": 1, "lost_percent": 90,
                            "cut_off_percent": 50, "unnerved_percent": 30 }),
    );
    let squad = UnitId(2);
    let (have, _) = state.substance(&reg, state.unit(squad).unwrap());
    let full: u32 = state
        .units
        .iter()
        .filter(|u| u.side == 1)
        .map(|u| state.substance(&reg, u).1)
        .sum();
    let share = |percent: u32| {
        let left = full - have + have * (100 - percent) / 100;
        1000 - (u64::from(left) * 1000 / u64::from(full)) as u32
    };
    assert_eq!(
        state.will_lost(&reg, 1),
        0,
        "nobody is cut off or unnerved yet"
    );

    state.unit_mut(squad).unwrap().cut_off = true;
    assert_eq!(state.will_lost(&reg, 1), share(50), "cut off: half of her");
    assert_eq!(
        state.strength_lost(&reg, 1),
        0,
        "and none of her strength is gone"
    );

    state.unit_mut(squad).unwrap().pressure = 1000;
    assert_eq!(
        state.will_lost(&reg, 1),
        share(50),
        "unnerved too: the larger share, not 80"
    );
    state.unit_mut(squad).unwrap().cut_off = false;
    assert_eq!(state.will_lost(&reg, 1), share(30), "unnerved alone");
}

#[test]
fn a_side_whose_crews_are_cut_off_breaks_before_it_has_lost_them() {
    // The pincer is asked at the top of the round — the battle opens before
    // anybody has looked, so the first round it is asked is the second —
    // and with the break
    // point counting a cut-off crew as wholly lost, East has lost the
    // squad's share of her will with no shot fired — past a point set
    // just under it. Counting nothing, the same pincer breaks nobody.
    // Everybody in plain sight, and found whichever way she faces: a test
    // of what her side counts, not of whether she saw them coming.
    let mut reg = seen(registry());
    // And the ladder charges nothing for it: the break point asks for
    // itself.
    reg.morale.cut_off = 0;
    reg.balance.flank_search_percent = 100;
    reg.balance.buttoned_flank_search_percent = 100;
    for vehicle in reg.vehicles.values_mut() {
        vehicle.flank_search_percent = None;
        vehicle.buttoned_flank_search_percent = None;
    }
    let point =
        |cut: u32| serde_json::json!({ "side": 1, "lost_percent": 1, "cut_off_percent": cut });
    let mut state = pincer(&reg, point(100));
    let ended = play_out(&reg, &mut state, 2, |_| {});
    assert!(
        state.unit(UnitId(2)).unwrap().cut_off,
        "the squad is cut off"
    );
    assert!(!state.unit(UnitId(3)).unwrap().cut_off, "the tank is not");
    assert_eq!(state.strength_lost(&reg, 1), 0, "nobody fired");
    assert_eq!(
        ended.map(|(w, r, _)| (w, r)),
        Some((Some(0), EndReason::Broken)),
        "East broke on a crew she could not count on"
    );

    let mut state = pincer(&reg, point(0));
    assert_eq!(
        play_out(&reg, &mut state, 2, |_| {}),
        None,
        "counting nothing"
    );
}

/// An open field, everybody found whichever way she faces, nobody's nerve
/// mending and no other price of position charged: the stage for one
/// pressure term at a time.
fn plain(
    reg: &DataRegistry,
    objectives: serde_json::Value,
    formations: serde_json::Value,
    placements: Vec<UnitPlacement>,
) -> (DataRegistry, BattleState) {
    let mut reg = seen(reg.clone());
    reg.balance.flank_search_percent = 100;
    reg.balance.buttoned_flank_search_percent = 100;
    for vehicle in reg.vehicles.values_mut() {
        vehicle.flank_search_percent = None;
        vehicle.buttoned_flank_search_percent = None;
    }
    reg.morale.cut_off = 0;
    reg.morale.recovery = 0;
    reg.morale.recovery_per_skill = 0;
    reg.morale.recovery_near_leader = 0;
    let row = "g".repeat(13);
    let rows: Vec<String> = (0..13).map(|_| row.clone()).collect();
    let file: MapFile = serde_json::from_value(serde_json::json!({
        "id": "plain",
        "palette": { "g": "grass" },
        "rows": rows,
        "sides": [{ "name": "West" }, { "name": "East" }],
        "objectives": objectives,
        "formations": formations,
    }))
    .expect("a valid stage");
    let state = stage(&reg, &file, placements);
    (reg, state)
}

/// Nobody can fire and everybody holds where she is told.
fn still(state: &mut BattleState) {
    for unit in state.units.iter_mut() {
        for cd in &mut unit.cooldowns {
            *cd = 1000;
        }
        unit.orders = Some(tactics_core::battle::PersonalOrder::Holding {
            latitude: tactics_core::battle::Latitude::Binding,
        });
    }
}

/// East's squad in the middle of the field with her way home at the east
/// edge, a West scout car at `gun`, and `price` a round for being
/// bypassed: whether she is, and her pressure after two rounds.
fn passed(gun: [i32; 2], price: u32) -> (bool, u32) {
    let mut reg = registry();
    reg.morale.bypassed = price;
    let (reg, mut state) = plain(
        &reg,
        serde_json::json!([{ "id": "home", "at": [[12, 6]], "kind": "exit", "side": 1 }]),
        serde_json::json!([]),
        vec![
            unit_at(gun, 0, "recon_car", "Gun"),
            unit_at([6, 6], 1, "rifle_squad", "Squad"),
        ],
    );
    still(&mut state);
    let behind = tactics_core::battle::bypassed(&reg, &state, UnitId(1));
    play_round(&reg, &mut state);
    play_round(&reg, &mut state);
    (behind, state.unit(UnitId(1)).map_or(0, |u| u.pressure))
}

#[test]
fn a_crew_with_a_gun_between_her_and_her_way_home_has_been_bypassed_and_feels_it() {
    // "They were moving in behind us": the veterans' words for what ended
    // most of HERO Report 117's defences. A gun nearer her side's rear than
    // she is — here, her way off the field — is a fact about where the
    // enemy stands against her line home, and it is charged at the top of
    // the round like being cut off. The same gun on the far side of her is
    // just the enemy in front.
    let (behind, free) = passed([9, 6], 0);
    assert!(behind, "a gun between her and home");
    let (_, charged) = passed([9, 6], 5);
    assert!(
        charged > free,
        "costs her something: {charged} against {free}"
    );
    let (ahead, free) = passed([3, 6], 0);
    assert!(!ahead, "a gun in front of her has not got past her");
    let (_, charged) = passed([3, 6], 5);
    assert_eq!(charged, free, "and costs nothing");
}

#[test]
fn with_no_way_home_her_rear_is_the_middle_of_the_ground_her_side_stood_on() {
    // A defence with no road out is most of the sectors: her rear is where
    // her side stood when the battle opened, read as a centre. A gun
    // inside her side's ground is behind her front crew; one outside it,
    // in front of her, is not.
    let reg = registry();
    let crews = |gun: [i32; 2]| {
        vec![
            unit_at(gun, 0, "recon_car", "Gun"),
            unit_at([6, 6], 1, "rifle_squad", "Front"),
            unit_at([12, 6], 1, "rifle_squad", "Back"),
        ]
    };
    for (gun, behind) in [([9, 6], true), ([3, 6], false)] {
        let (reg, mut state) = plain(
            &reg,
            serde_json::json!([]),
            serde_json::json!([]),
            crews(gun),
        );
        still(&mut state);
        assert_eq!(
            tactics_core::battle::bypassed(&reg, &state, UnitId(1)),
            behind,
            "a gun at {gun:?}"
        );
    }
}

/// Two formations of East's side by side and one West gun out of reach,
/// with `price` for a neighbour ordered out: the second formation's
/// pressure after the first is ordered to withdraw.
fn left_alone(price: u32, apart: i32) -> u32 {
    let mut reg = registry();
    reg.morale.neighbour_withdrew = price;
    reg.morale.neighbour_within = 3;
    let mut first = unit_at([6, 4], 1, "rifle_squad", "First");
    first.formation = Some("first".into());
    first.leads = true;
    let mut second = unit_at([6, 4 + apart], 1, "rifle_squad", "Second");
    second.formation = Some("second".into());
    second.leads = true;
    let (reg, mut state) = plain(
        &reg,
        serde_json::json!([{ "id": "home", "at": [[12, 6]], "kind": "exit", "side": 1 }]),
        serde_json::json!([
            { "id": "first", "side": 1 },
            { "id": "second", "side": 1 },
        ]),
        vec![unit_at([0, 0], 0, "recon_car", "Far"), first, second],
    );
    still(&mut state);
    let formation = common::formation_named(&state, "first");
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation,
                mission: tactics_core::battle::Mission::Withdraw { via: "home".into() },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("a way off the field her side may take");
    play_round(&reg, &mut state);
    state.unit(UnitId(2)).map_or(0, |u| u.pressure)
}

#[test]
fn a_formation_whose_neighbour_is_ordered_out_feels_its_flank_go() {
    // McQuie's second reason a force broke off (13%): the unit beside it
    // withdrew. News that travels the line, so it reaches the neighbour
    // whole, once — and only a neighbour.
    let quiet = left_alone(0, 2);
    assert_eq!(left_alone(4, 2), quiet + 4, "beside her: charged once");
    assert_eq!(left_alone(4, 6), left_alone(0, 6), "out of reach: nothing");
}

/// A tank destroyer ordered onto a medium tank four hexes off, the medium
/// unable to answer, for one tick (a second 88 would leave nobody to be
/// frightened); whether East had found the gun before it, and `surprise`
/// percent for a gun she had not: the medium's pressure after it.
fn ambushed(found: bool, surprise: u32) -> u32 {
    let mut reg = registry();
    reg.morale.surprise_percent = surprise;
    let (reg, mut state) = plain(
        &reg,
        serde_json::json!([]),
        serde_json::json!([]),
        vec![
            unit_at([2, 6], 0, "tank_destroyer", "Gun"),
            unit_at([6, 6], 1, "medium_tank", "Target"),
        ],
    );
    for cd in &mut state.units[1].cooldowns {
        *cd = 1000;
    }
    if !found {
        state.fog.side_mut(1).spotted.remove(&UnitId(0));
    }
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: UnitId(0),
                fire: tactics_core::battle::FireIntent::Target {
                    target: UnitId(1),
                    weapon: 0,
                },
            },
        )
        .expect("the gun can see her");
    common::commit_all(&reg, &mut state);
    let shots = state
        .step_tick(&reg)
        .iter()
        .filter(|e| {
            matches!(
                e,
                BattleEvent::ShotHit { .. }
                    | BattleEvent::ShotMissed { .. }
                    | BattleEvent::ShotBounced { .. }
            )
        })
        .count();
    assert!(shots > 0, "the stage is a gun firing on the first tick");
    state.unit(UnitId(1)).map_or(0, |u| u.pressure)
}

#[test]
fn a_gun_nobody_had_found_frightens_more_than_one_she_was_watching() {
    // Surprise: the stated reason a force broke off one time in twelve, and
    // ten of ten regimental attacks that achieved it forced the defender
    // off. Only the gun her side had not found when the tick opened is
    // priced up; the same gun fired from where she was watching it is the
    // price it always was, which is why no planner's expectation moves.
    assert!(
        ambushed(false, 300) > ambushed(false, 100),
        "the first shot from a gun nobody knew of is the dearer"
    );
    assert_eq!(
        ambushed(true, 300),
        ambushed(true, 100),
        "a gun she had found surprises nobody"
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

// --- the weather and the ground ---------------------------------------------

#[test]
fn nobody_sees_through_fog_until_it_lifts() {
    // Noville and Arracourt were fought in fog that cancelled the long
    // guns until it lifted: two tanks five hexes apart on open grass do not
    // see each other in a two-hex murk, and do the round it clears.
    let reg = common::seen(registry());
    let file: MapFile = serde_json::from_value(serde_json::json!({
        "id": "fog",
        "palette": { "g": "grass" },
        "rows": ["gggggggg"],
        "visibility": [
            { "from_round": 0, "hexes": 2 },
            { "from_round": 3, "hexes": null },
        ],
    }))
    .unwrap();
    let mut state = stage(
        &reg,
        &file,
        vec![
            unit_at([0, 0], 0, "medium_tank", "West"),
            unit_at([5, 0], 1, "medium_tank", "East"),
        ],
    );
    let east_seen = |state: &BattleState| state.fog.side(0).spotted.contains(&UnitId(1));
    assert!(!east_seen(&state), "round 1, in the fog");
    play_round(&reg, &mut state);
    assert!(!east_seen(&state), "round 2, still");
    play_round(&reg, &mut state);
    play_round(&reg, &mut state);
    assert!(east_seen(&state), "the fog has lifted");
}

#[test]
fn a_hand_drawn_map_may_have_more_relief_than_ten_levels() {
    // Real ground is not ten levels of ten metres deep: letters carry the
    // levels on past nine.
    let file: MapFile = serde_json::from_value(serde_json::json!({
        "id": "relief",
        "palette": { "g": "grass" },
        "rows": ["ggg"],
        "elevation": ["09z"],
    }))
    .unwrap();
    let ground = Battlefield::from_map_file(&file).unwrap().terrain;
    let level = |col| {
        ground
            .get(tactics_core::offset_to_hex(col, 0))
            .unwrap()
            .elevation
    };
    assert_eq!((level(0), level(1), level(2)), (0, 9, 35));
}

// --- standing orders ---------------------------------------------------------

/// A defender dug in at the east end of a strip, with ground her side holds
/// at the west end worth driving for, and the attacker out of sight
/// between. `standing` gives her formation the order to hold before the
/// battle.
fn holding(reg: &DataRegistry, standing: bool) -> BattleState {
    let mut extra = serde_json::json!({
        "objectives": [{ "id": "far", "at": [[2, 1]], "value": 3, "held": 1 }],
        "prepared": [1],
        "formations": [
            { "id": "west", "side": 0 },
            { "id": "east", "side": 1 },
        ],
    });
    if standing {
        extra["standing"] =
            serde_json::json!([{ "side": 1, "mission": { "hold": { "at": null } } }]);
    }
    let file = assault_map(16, 6, extra);
    let mut west = unit_at([0, 0], 0, "medium_tank", "West");
    west.formation = Some("west".into());
    west.leads = true;
    let mut lead = unit_at([12, 1], 1, "medium_tank", "East 1");
    lead.formation = Some("east".into());
    lead.leads = true;
    let mut second = unit_at([14, 2], 1, "medium_tank", "East 2");
    second.formation = Some("east".into());
    stage(reg, &file, vec![west, lead, second])
}

/// Round one, planned by the utility AI on both sides: how many of the
/// defender's crews are still in their holes at the end of it.
fn still_dug_in_after_a_round(reg: &DataRegistry, state: &mut BattleState) -> usize {
    use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, UtilityPlanner};
    let mut ai = AiDriver::new();
    for side in 0..2u8 {
        let config = AiConfig {
            planner: "utility".into(),
            difficulty: 5,
            doctrine: Some("elastic_defense".into()),
        };
        let planner: Box<dyn AiPlanner<BattleState, Order>> =
            Box::new(UtilityPlanner::from_config(&config, u64::from(side), reg));
        ai.insert(side, planner);
    }
    ai.plan_round(reg, state);
    state.resolve_round(reg);
    state
        .units
        .iter()
        .filter(|u| u.side == 1 && u.dug_in > 0)
        .count()
}

#[test]
fn a_defence_opens_under_the_orders_it_was_given_before_the_battle() {
    // An order given in round one lands a round late, and in that round a
    // crew choosing for herself drove out of her prepared position for the
    // next ground worth having — at Noville, a zone two kilometres behind
    // her, the defence scattered over every zone it held. A standing order
    // is on the formation from the start.
    let reg = registry();
    let mut free = holding(&reg, false);
    assert_eq!(free.formations()[1].latest_mission(), None);
    assert!(
        still_dug_in_after_a_round(&reg, &mut free) < 2,
        "with nothing said, the far ground draws somebody out of her hole"
    );
    let mut held = holding(&reg, true);
    assert_eq!(
        held.formations()[1].latest_mission(),
        Some(&tactics_core::battle::Mission::Hold { at: None }),
        "the standing order is on the formation before round one"
    );
    assert_eq!(
        still_dug_in_after_a_round(&reg, &mut held),
        2,
        "and nobody leaves for the far ground"
    );
}
