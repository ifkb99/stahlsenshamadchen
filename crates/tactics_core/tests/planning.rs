//! The utility chooser: what ground is worth, and to whom.
//!
//! The goal chooser reads the road rather than the crow-flight distance to
//! it, subordinate initiative widens her candidate list by exactly one
//! entry, and the planner numbers are mod data precisely so a sweep can ask
//! what each one does. What an order is worth against the terrain closes
//! the loop: an order competing with her own objectives is a suggestion,
//! priced in the same currency as the ground itself. All four sections are
//! the AI's *judgement* about where to stand, as against `drill.rs`'s
//! *reflexes* about what to do this instant.
//!
//! - the goal chooser reads the road
//! - subordinate initiative
//! - the planner numbers are data
//! - what an order is worth against the terrain

use tactics_core::ai::{
    AiConfig, AiDriver, AiPlanner, Evaluator, UtilityPlanner, make_battle_planner,
};
use tactics_core::battle::{BattleState, Latitude, Mission, Order, SideState, UnitId};
use tactics_core::data::DataRegistry;
use tactics_core::map::{HexMap, UnitPlacement};

mod common;
use common::{
    curtained_pair, formation_named, maul, objective_battle, orders_planned, registry,
    registry_wireless, seen, taxi_run_stage, two_side_battle, unit_at,
};

// --- the goal chooser reads the road ---------------------------------------

/// A doctrine with one field changed, so a test about the goal chooser's
/// terms can hold every other appetite still.
fn doctrine_with(
    reg: &DataRegistry,
    id: &str,
    edit: impl FnOnce(&mut tactics_core::data::DoctrineDef),
) -> tactics_core::data::DoctrineDef {
    let mut doctrine = reg.doctrine(id).expect("base doctrine").clone();
    edit(&mut doctrine);
    doctrine
}

#[test]
fn a_commander_who_reads_the_ground_goes_where_the_road_goes() {
    // The chooser priced a march as the crow flies, which on a map with a
    // river in it is not a small error: the far bank is five hexes away and
    // twenty-odd hexes of driving, and a crew who cannot tell the difference
    // marches at the water.
    //
    // Two objectives worth the same. One is across a river reach with the
    // only crossing at the bottom of the map; the other is straight down her
    // own bank. As the crow flies the far one is nearer. By road it is not.
    let reg = seen(registry_wireless());
    let mut rows: Vec<&str> = vec!["gggwgggg"; 15];
    // The one crossing, at the bottom of the map.
    rows.push("gggggggg");
    let state = goal_battle(
        &reg,
        &rows,
        serde_json::json!([
            { "id": "far_bank", "name": "The Far Bank", "at": [[5, 0]], "value": 2 },
            { "id": "down_river", "name": "Down River", "at": [[0, 8]], "value": 2 },
        ]),
        vec![
            unit_at([0, 0], 0, "medium_tank", "Chooser"),
            // Off at the other end of the map and out of sight: a visible
            // enemy is worth several points of shot and several more of
            // advance, which would swamp the thing being measured.
            unit_at([7, 15], 1, "medium_tank", "Somebody"),
        ],
        11,
    );
    assert!(
        state.fog.side(0).spotted.is_empty(),
        "the stage needs no enemy in sight — this is a test about ground"
    );
    let far = tactics_core::offset_to_hex(5, 0);
    let down = tactics_core::offset_to_hex(0, 8);
    let me = state.units[0].pos;
    // The stage, stated rather than assumed. Without both halves the test
    // proves nothing about which of the two the chooser read.
    assert!(
        me.distance_to(far) < me.distance_to(down),
        "the far bank has to be nearer as the crow flies: {} against {}",
        me.distance_to(far),
        me.distance_to(down)
    );
    let roads = tactics_core::battle::roads(&reg, &state, UnitId(0), 12);
    let (far_road, down_road) = (
        roads.cost(far).expect("reachable the long way round"),
        roads.cost(down).expect("reachable straight down the bank"),
    );
    assert!(
        far_road > down_road,
        "and much further by road: {far_road} against {down_road}"
    );

    // Route caution and contest aversion are switched off, so the only thing
    // foresight can be reading here is how long the drive is.
    let doctrine = doctrine_with(&reg, "massed_armor", |d| {
        d.route_caution = 0.0;
        d.contest_aversion = 0.0;
    });
    assert_eq!(
        goal_with_foresight(&reg, &state, UnitId(0), doctrine.clone(), 0.0),
        Some(tactics_core::battle::Goal::Take(far)),
        "a commander who reads a straight line off the map marches at the river"
    );
    assert_eq!(
        goal_with_foresight(&reg, &state, UnitId(0), doctrine, 1.0),
        Some(tactics_core::battle::Goal::Take(down)),
        "one who reads the ground takes the objective she can drive to"
    );
}

#[test]
fn a_road_under_a_gun_is_worth_going_round() {
    // The second thing the chooser could not see: what happens on the way.
    // Two objectives the same distance off by roads that cost the same, and
    // one of those roads is walked in front of a tank. A doctrine that minds,
    // in the hands of a commander with the foresight to notice, takes the
    // other one.
    //
    // The timber is what makes the difference a fact about the *route*
    // rather than about the destination: it breaks the sight line along the
    // northern road without changing what either objective is worth.
    let reg = seen(registry_wireless());
    // The wood is crossed cheaply at its western end and expensively
    // everywhere else, so the road behind it is a *single* cheapest road
    // rather than one of several ties — the chooser prices the road the
    // pathfinder actually found, and a tie would leave which one to chance.
    let rows = [
        "ggggggggg",
        "rffffffff",
        "ggggggggg",
        "gggggffff",
        "ggggggggg",
    ];
    let state = goal_battle(
        &reg,
        &rows,
        serde_json::json!([
            // Twenty rather than eight, which is Wave 1's tax on this stage
            // and not a claim about what ground is worth. Both objectives are
            // worth the same, so their value cancels between them — what it
            // has to beat is the *other* candidate on the list, the tile her
            // own sweep picked, and that tile is priced in a currency that
            // grew by a round of fire. At eight she took her own firing
            // position and never chose between the two roads at all, which
            // would have made this a test about nothing. Measured: eighteen
            // upwards restores the choice at the shipped `route_caution` of
            // 1.2, and twenty holds it at every caution from 1.2 to 16.
            { "id": "open_road", "name": "Down the Open Road", "at": [[8, 2]], "value": 20 },
            { "id": "behind_the_wood", "name": "Behind the Wood", "at": [[8, 0]], "value": 20 },
        ]),
        vec![
            unit_at([0, 2], 0, "medium_tank", "Chooser"),
            // Parked south-west behind her, watching the open ground and
            // blind to everything north of the treeline. Behind rather than
            // beside the southern objective, so that the two objectives are
            // worth the same to stand on — one of them offering a shot at
            // her would be worth several times what the road is.
            unit_at([2, 4], 1, "medium_tank", "The Gun"),
        ],
        11,
    );
    let open = tactics_core::offset_to_hex(8, 2);
    let covered = tactics_core::offset_to_hex(8, 0);
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "she has to be able to see the gun to route around it"
    );
    let roads = tactics_core::battle::roads(&reg, &state, UnitId(0), 12);
    assert!(
        roads.cost(covered) >= roads.cost(open),
        "the covered road must not also be the shorter one, or the test would \
         pass for the wrong reason: {:?} against {:?}",
        roads.cost(covered),
        roads.cost(open)
    );

    // Massed armour, whose low `cover_value` keeps the treeline itself from
    // outbidding both objectives, carrying elastic defence's appetite for
    // the covered road. Contest aversion off: the two objectives are about
    // equally far from the one enemy on the field, but leaving it in would
    // mean the test could pass for a second reason.
    let doctrine = doctrine_with(&reg, "massed_armor", |d| {
        d.route_caution = 1.2;
        d.contest_aversion = 0.0;
    });
    assert_eq!(
        goal_with_foresight(&reg, &state, UnitId(0), doctrine.clone(), 0.0),
        Some(tactics_core::battle::Goal::Take(open)),
        "a commander who does not look at the road takes the first one offered"
    );
    assert_eq!(
        goal_with_foresight(&reg, &state, UnitId(0), doctrine, 1.0),
        Some(tactics_core::battle::Goal::Take(covered)),
        "one who does takes the road the gun cannot see"
    );
}

#[test]
fn ground_the_enemy_reaches_first_is_worth_less_marching_for() {
    // The third thing: what the enemy will do about it. Two objectives, and
    // a tank already sitting a hex from one of them — she will not have that
    // one to herself when she arrives, and a commander who has noticed goes
    // for the other.
    //
    // Isolated by comparing the same battle under two values of the one
    // term, so `score_tile` is identical in both runs and the only thing that
    // can have moved the answer is the arithmetic under test. That matters
    // here more than in the tests above: an enemy beside an objective makes
    // that objective worth *more* to an aggressive doctrine, because there is
    // something to shoot from it, so the contested ground starts ahead.
    //
    // How far ahead is a number this test has had to restate twice, and the
    // claim it makes has not changed either time: what moved is the size of
    // the head start the aversion has to overcome.
    //
    // Phase 2: the threat term used to fall off as `1/distance` from the
    // enemy, so the hill with a tank a hex away looked five times as
    // dangerous as ground five hexes off and the head start was mostly
    // cancelled before `contest_aversion` was consulted at all. The resolver
    // disagrees — a 75mm loses two points of accuracy a hex, so on open grass
    // the far objective is barely safer than the near one — and the term
    // began flipping the choice from 4 upwards instead of 3.
    //
    // Wave 1: the same head start is a *round* of the medium's fire rather
    // than one shot, four times the size, so the aversion has four times as
    // much to overcome. It flips from 12 upwards, and 10 is not enough.
    // Asserted at 15 rather than 12 so the stage is not sitting on its own
    // threshold — the same margin Phase 2 left when it asserted 5 against a
    // threshold of 4.
    let reg = seen(registry_wireless());
    let rows = ["ggggggggg"; 9];
    let state = goal_battle(
        &reg,
        &rows,
        serde_json::json!([
            { "id": "contested", "name": "The Contested Hill", "at": [[8, 4]], "value": 8 },
            { "id": "open_ground", "name": "The Open Ground", "at": [[4, 8]], "value": 8 },
        ]),
        vec![
            unit_at([0, 4], 0, "medium_tank", "Chooser"),
            unit_at([7, 4], 1, "medium_tank", "Already There"),
        ],
        11,
    );
    let contested = tactics_core::offset_to_hex(8, 4);
    let open = tactics_core::offset_to_hex(4, 8);
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "she has to be able to see who she is racing"
    );
    let me = state.units[0].pos;
    let them = state.units[1].pos;
    assert!(
        them.distance_to(contested) < me.distance_to(contested),
        "the stage needs the enemy nearer to the contested ground"
    );

    let racing = |aversion: f32| {
        goal_with_foresight(
            &reg,
            &state,
            UnitId(0),
            doctrine_with(&reg, "massed_armor", |d| {
                d.route_caution = 0.0;
                d.contest_aversion = aversion;
            }),
            1.0,
        )
    };
    assert_eq!(
        racing(0.0),
        Some(tactics_core::battle::Goal::Take(contested)),
        "a commander who does not ask marches at the ground with a tank on it"
    );
    assert_eq!(
        racing(15.0),
        Some(tactics_core::battle::Goal::Take(open)),
        "one who does takes the ground that will still be empty when she gets there"
    );
}

// --- subordinate initiative ------------------------------------------------

/// A crew under orders to distant ground, with a reason to fight where she
/// stands: an enemy in view, and the whole march still ahead of her.
///
/// The comparison the chunk turns on. What she may deviate to is the tile her
/// own sweep picked — a firing position — and not some other objective, so
/// this map declares no objectives at all: with orders in hand they would not
/// be candidates anyway, and leaving them out says so.
fn pressed_stage(reg: &DataRegistry, seed: u64) -> (BattleState, tactics_core::Hex) {
    let rows: Vec<&str> = vec!["gggggggggggggggggggg"; 3];
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "pressed",
        "palette": { "g": "grass" },
        "rows": rows,
        "formations": [{ "id": "platoon", "name": "Platoon", "side": 0 }],
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
    let mut hers = unit_at([1, 0], 0, "medium_tank", "Subordinate");
    hers.formation = Some("platoon".into());
    hers.leads = true;
    let placements = vec![hers, unit_at([8, 0], 1, "medium_tank", "Somebody")];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    let mut state = BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
    .expect("the staged placements are content the base mod ships");
    let told = tactics_core::offset_to_hex(18, 2);
    state
        .apply(
            reg,
            &Order::SetMission {
                formation: formation_named(&state, "platoon"),
                mission: Mission::Advance { to: told },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("a legal mission");
    (state, told)
}

/// The goal a doctrine of this initiative chooses on that stage, with route
/// caution and contest aversion held at zero so that initiative is the only
/// thing moving.
fn goal_at_initiative(
    reg: &DataRegistry,
    state: &BattleState,
    initiative: f32,
) -> Option<tactics_core::battle::Goal> {
    let doctrine = doctrine_with(reg, "massed_armor", |d| {
        d.initiative = initiative;
        d.route_caution = 0.0;
        d.contest_aversion = 0.0;
    });
    goal_with_foresight(reg, state, UnitId(0), doctrine, 1.0)
}

#[test]
fn a_crew_with_no_initiative_marches_at_the_map_reference() {
    // The additivity half, as behaviour rather than as a list length: a
    // doctrine that devolves nothing drives at the hex she was given, past
    // every reason of her own to stop, exactly as every ordered crew did
    // before initiative existed.
    let reg = seen(registry_wireless());
    let (state, told) = pressed_stage(&reg, 41);
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the stage needs the enemy in sight, or she has nothing to stop for"
    );
    assert_eq!(
        goal_at_initiative(&reg, &state, 0.0),
        Some(tactics_core::battle::Goal::Take(told)),
        "no initiative is no deviation"
    );
}

#[test]
fn a_subordinate_who_is_trusted_fights_from_the_ground_she_chose() {
    // The other half, and the same stage, same seed, same doctrine: the only
    // difference between these two tests is `initiative`. That is what makes
    // it a comparison rather than a preference — a test that only showed the
    // trusted crew stopping would pass against an engine in which nobody ever
    // obeys anything.
    //
    // And it is a *deviation*, not a refusal: she is still fighting the fight
    // her orders sent her to, from a position her own sweep picked, which is
    // the whole of what "how, not whether" means.
    let reg = seen(registry_wireless());
    let (state, told) = pressed_stage(&reg, 41);
    let chose = goal_at_initiative(&reg, &state, 0.9);
    assert_ne!(
        chose,
        Some(tactics_core::battle::Goal::Take(told)),
        "a subordinate trusted with her own judgment is still driving at the map reference"
    );
    assert!(
        matches!(chose, Some(tactics_core::battle::Goal::Take(_))),
        "and she chose ground rather than giving up on the order: {chose:?}"
    );
}

#[test]
fn the_shipped_doctrines_straddle_the_price_of_deviating() {
    // The calibration, and the reason to believe the number means anything.
    // At the shipped `deviation_cost` massed armour (initiative 0.3) presses
    // on to the hex she was given and elastic defence (0.7) and recon pull
    // (0.9) stop to fight from ground of their own — so the field separates
    // the doctrines that ship rather than sitting outside all of them, which
    // is what `balance.blind_penalty` turned out to be doing.
    //
    // This test is why `deviation_cost` has moved twice — 2.0 to 3.0 in
    // Phase 2, 3.0 to 9.0 in Wave 1 — and it is the whole argument for having
    // written it: the field is quoted in the evaluator's currency, so it has
    // to move whenever the currency does, and nothing else in the tree would
    // have said so out loud.
    //
    // Phase 2: the threat term stopped being a six-hex gate and became the
    // resolver's own arithmetic, several points larger everywhere a found gun
    // can reach. At 2.0 all three doctrines deviated. Measured then: 2.0 none
    // obey, 3.0 to 6.0 massed armour alone obeys, 8.0 elastic defence too.
    //
    // Wave 1: both the attack and the threat term became a *round* of fire
    // rather than a single shot, and the guns in the base mod fire two to
    // twelve times a round, so the fighting half of the score grew by roughly
    // its cadence and 3.0 stopped separating anything again. Measured on this
    // stage twice, because the mechanism and the content it enables move the
    // currency by different amounts: with suppression declared nowhere the
    // straddle holds from **8.5 to 19**, and with the base mod's shipped
    // suppression and `point_worth` from **12 to 29**. Shipped at 12.0, the
    // bottom of the intersection — one value has to serve both, or the field
    // would mean something different in a mod that declines the new rule.
    //
    // Wave 2 moved the band without moving the field, which is the first time
    // that has happened and is worth recording. An order is a share of the
    // crew now (`planner.order_worth` times what she has left) rather than a
    // flat 2.0, so on this stage — a medium tank with thirteen substance
    // points — the ordered ground is worth 3.25 where it was 3.0 and its
    // slope is half again as steep. Re-measured: massed armour obeys from
    // **10** and elastic defence starts obeying at **24**, so the straddle is
    // 10 to 23 where it was 12 to 29. 12.0 still sits inside it with room at
    // both ends and did not have to move; if a later wave moves the currency
    // again, this is the paragraph to re-measure before touching the number.
    let reg = seen(registry_wireless());
    let (state, told) = pressed_stage(&reg, 41);
    let ordered = Some(tactics_core::battle::Goal::Take(told));
    assert_eq!(
        reg.doctrine("massed_armor").unwrap().initiative,
        0.3,
        "the shipped value moved; re-read this test before changing it"
    );
    assert_eq!(
        goal_at_initiative(&reg, &state, 0.3),
        ordered,
        "massed armour devolves little and should drive at the hex she was given"
    );
    for (doctrine, initiative) in [("elastic_defense", 0.7), ("recon_pull", 0.9)] {
        assert_eq!(
            reg.doctrine(doctrine).unwrap().initiative,
            initiative,
            "the shipped value moved; re-read this test before changing it"
        );
        assert_ne!(
            goal_at_initiative(&reg, &state, initiative),
            ordered,
            "{doctrine} trusts its subordinates and should let her fight where she is"
        );
    }
}

#[test]
fn what_it_costs_a_subordinate_to_have_her_own_idea_is_a_mod_decision() {
    // `planner.deviation_cost` is the price and `initiative` is how much of
    // it she pays, so the same crew with the same idea obeys or does not
    // depending on one number in `mod.json`. This is the check that the field
    // is read at all, and it is the knob a mod reaches for when its
    // subordinates are too literal or not literal enough.
    let mut reg = seen(registry_wireless());
    let (state, told) = pressed_stage(&reg, 41);
    let ordered = Some(tactics_core::battle::Goal::Take(told));
    assert_eq!(reg.planner.deviation_cost, 12.0);

    reg.planner.deviation_cost = 0.0;
    assert_ne!(
        goal_at_initiative(&reg, &state, 0.3),
        ordered,
        "costing nothing, even a literal-minded doctrine backs its own judgment"
    );

    reg.planner.deviation_cost = 100.0;
    assert_eq!(
        goal_at_initiative(&reg, &state, 0.9),
        ordered,
        "and priced out of reach, the most trusting doctrine in the mod does as she is told"
    );
}

// --- the planner numbers are data ------------------------------------------

//
// Five constants lived in `ai/` until 2026-08-27 against this project's own
// rule that a number a modder would want to change does not belong in Rust.
// They are the `planner` block now, and each of these tests is the check that
// its field is *read* — the failure this guards against is not a wrong number
// but a declared one that nothing consults, which is exactly what
// `Scale::elevation_meters` was for months.
//
// The block is deliberately separate from `balance`: nothing in it reaches a
// rule, so a mod that rewrote all five would leave a human-versus-human
// battle bit-for-bit identical. What it changes is only what a commander
// decides.

/// How much a round of driving costs is the mod's to choose.
///
/// Without `impatience` nothing prices the walk at all and every crew on the
/// field marches at whichever single hex scores highest, which is the queue
/// the plateau rule was invented to break up rebuilt one layer higher.
#[test]
fn what_a_round_of_driving_costs_a_commander_is_a_mod_decision() {
    // Two pieces of ground worth the same, one under her tracks and one at
    // the far end of a long field. Standing on either is worth the same to
    // the evaluator — an objective pays for being stood on — so after the
    // terms are equalised the *only* thing separating them is the drive.
    let mut reg = seen(registry_wireless());
    let rows: Vec<&str> = vec!["gggggggggggggggggggg"; 3];
    let state = goal_battle(
        &reg,
        &rows,
        serde_json::json!([
            { "id": "far", "name": "The Far End", "at": [[18, 1]], "value": 2 },
            { "id": "near", "name": "Underfoot", "at": [[2, 1]], "value": 2 },
        ]),
        vec![
            unit_at([1, 1], 0, "medium_tank", "Chooser"),
            unit_at([19, 0], 1, "medium_tank", "Somebody"),
        ],
        23,
    );
    assert!(
        state.fog.side(0).spotted.is_empty(),
        "the stage needs no enemy in sight — this is a test about ground"
    );
    let far = tactics_core::offset_to_hex(18, 1);
    let near = tactics_core::offset_to_hex(2, 1);
    // Route caution and contest aversion off, so the drive is the only term
    // foresight can be reading.
    let doctrine = doctrine_with(&reg, "massed_armor", |d| {
        d.route_caution = 0.0;
        d.contest_aversion = 0.0;
    });

    // Free driving. The two goals tie on worth and the tie goes to the
    // earlier candidate, which is the far one: map order, and `candidates`
    // is ordered precisely so that this is decidable.
    reg.planner.impatience = 0.0;
    assert_eq!(
        goal_with_foresight(&reg, &state, UnitId(0), doctrine.clone(), 1.0),
        Some(tactics_core::battle::Goal::Take(far)),
        "with the march free, ground at the far end is as good as ground underfoot"
    );

    // A crew who begrudges every round takes what she can reach.
    reg.planner.impatience = 2.0;
    assert_eq!(
        goal_with_foresight(&reg, &state, UnitId(0), doctrine, 1.0),
        Some(tactics_core::battle::Goal::Take(near)),
        "and a crew charged for the drive takes the ground she is standing next to"
    );
}

/// How far ahead a commander looks decides what she believes about a road she
/// has not walked.
///
/// The horizon is not a tidy pruning parameter: ground with no road inside it
/// is priced at the horizon rather than at its real cost, so a *short* one is
/// a commander who cannot tell an unfordable river from a five-hex stroll.
/// This is the same stage as
/// `a_commander_who_reads_the_ground_goes_where_the_road_goes`, with
/// foresight held at one, so the only thing varying is how far she looked.
#[test]
fn a_commander_who_looks_no_further_than_her_nose_marches_at_the_river() {
    let mut reg = seen(registry_wireless());
    let mut rows: Vec<&str> = vec!["gggwgggg"; 15];
    rows.push("gggggggg");
    let state = goal_battle(
        &reg,
        &rows,
        serde_json::json!([
            { "id": "far_bank", "name": "The Far Bank", "at": [[5, 0]], "value": 2 },
            { "id": "down_river", "name": "Down River", "at": [[0, 8]], "value": 2 },
        ]),
        vec![
            unit_at([0, 0], 0, "medium_tank", "Chooser"),
            unit_at([7, 15], 1, "medium_tank", "Somebody"),
        ],
        11,
    );
    let far = tactics_core::offset_to_hex(5, 0);
    let down = tactics_core::offset_to_hex(0, 8);
    let doctrine = doctrine_with(&reg, "massed_armor", |d| {
        d.route_caution = 0.0;
        d.contest_aversion = 0.0;
    });

    // A horizon of one round reaches neither objective, so both are priced at
    // "further than I have looked" — which is the crow flight for anything
    // beyond it, and by the crow the far bank is nearer.
    reg.planner.horizon_rounds = 1;
    assert_eq!(
        goal_with_foresight(&reg, &state, UnitId(0), doctrine.clone(), 1.0),
        Some(tactics_core::battle::Goal::Take(far)),
        "full foresight over a road she never walked is still a straight line on a map"
    );

    // Far enough to find the one crossing, and she stops liking the far bank.
    reg.planner.horizon_rounds = 12;
    assert_eq!(
        goal_with_foresight(&reg, &state, UnitId(0), doctrine, 1.0),
        Some(tactics_core::battle::Goal::Take(down)),
        "a horizon that reaches the crossing is what makes reading the ground possible"
    );
}

/// What a ride costs over and above the driving is the mod's to choose.
///
/// The twin of `a_platoon_with_a_short_walk_ahead_of_her_walks`: three hexes
/// is a walk *because boarding costs four rounds*, and at nothing a round it
/// is a ride. Priced at two on the mechanics alone this was the live bug —
/// a delivered platoon re-boarded for a three-hex hop and cost the commanded
/// side a win and three platoons over 36 battles.
#[test]
fn what_climbing_in_and_out_of_a_taxi_costs_is_a_mod_decision() {
    let mut reg = registry_wireless();
    assert_eq!(reg.planner.boarding_rounds, 4.0);
    let mut state = taxi_run_stage(&reg, 3, 56);
    assert!(
        !orders_planned(&reg, &mut state, 0, 56)
            .iter()
            .any(|o| matches!(o, Order::Mount { .. })),
        "at four rounds a ride, three hexes is a walk"
    );

    reg.planner.boarding_rounds = 0.0;
    let mut state = taxi_run_stage(&reg, 3, 56);
    assert!(
        orders_planned(&reg, &mut state, 0, 56)
            .iter()
            .any(|o| matches!(o, Order::Mount { .. })),
        "and with mounting up free she takes the ride for the same three hexes"
    );
}

/// Where a commander stops assigning ground is the mod's to choose.
///
/// The twin of `a_devolved_commander_issues_no_ground_missions`: elastic
/// defence keeps its own judgment because its `delegation` of 0.7 is at or
/// beyond `devolved`, and moving the threshold past it makes the same
/// doctrine take orders. Which of a mod's *shipped* doctrines devolve is
/// therefore this one number, which is exactly why it should not be in Rust.
#[test]
fn where_a_commander_stops_assigning_ground_is_a_mod_decision() {
    let mut reg = registry_wireless();
    assert_eq!(reg.doctrine("elastic_defense").unwrap().delegation, 0.7);
    reg.planner.devolved = 0.8;

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
            .any(|f| f.mission.is_some()),
        "past the threshold the same doctrine is told where to stand"
    );
}

/// How badly a finished crew wants the lane is the mod's to choose.
///
/// The twin of `an_intact_crew_will_not_run_for_the_exit_but_a_broken_one
/// _will`: the same mauled crew, and at zero urgency the road home is worth
/// nothing to her. It is deliberately not the exit's own `value` — what a
/// lane *pays* and how badly somebody wants it are different quantities, and
/// a retreat lane has to be worth almost no points while still pulling hard
/// enough to cross a map.
#[test]
fn how_badly_a_finished_crew_wants_the_lane_is_a_mod_decision() {
    let mut reg = registry();
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
    let mut hurt = state;
    maul(&reg, &mut hurt, UnitId(0));

    assert!(
        eval.score_tile(&reg, &hurt, UnitId(0), exit).score
            > eval.score_tile(&reg, &hurt, UnitId(0), away).score,
        "at the shipped urgency a crew that is nearly finished runs for the road"
    );

    reg.planner.exit_urgency = 0.0;
    assert!(
        eval.score_tile(&reg, &hurt, UnitId(0), exit).score
            <= eval.score_tile(&reg, &hurt, UnitId(0), away).score,
        "and a mod that prices the lane at nothing has a crew who fights where she stands"
    );
}

/// A shot that would finish her is worth more than the substance it happens
/// to take with it.
///
/// Without `kill_bonus` the attack term is pure arithmetic: a killing shot is
/// priced exactly like any other round of the same damage and pressure, so a
/// crew who could reach a decisive shot by driving somewhere is given no
/// reason to bother — the ordinary shot she already has is just as good by
/// the numbers. The bonus is the one judgment layered on the resolver's own
/// arithmetic, and this is the check that it is actually read rather than
/// computed and discarded.
#[test]
fn a_shot_that_finishes_her_is_worth_more_than_the_substance_it_takes() {
    let mut reg = seen(registry_wireless());
    let row: String = "g".repeat(20);
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([0, 1], 0, "medium_tank", "Gunner"),
            unit_at([6, 1], 1, "medium_tank", "Wreck"),
        ],
        29,
    );
    // Wounded at every station and stripped of everything but her tracks:
    // as little substance as a hull that is still fighting can carry.
    {
        let her = state.unit_mut(UnitId(1)).unwrap();
        her.crew_state = vec![tactics_core::battle::CrewCondition::Wounded; her.crew.len()];
        for hits in her.modules.values_mut() {
            *hits = 0;
        }
    }
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the stage needs the wreck in plain sight"
    );
    let tile = state.unit(UnitId(0)).unwrap().pos;
    let target_pos = state.unit(UnitId(1)).unwrap().pos;
    let kill = tactics_core::ai::best_weapon_against(
        &reg,
        &state,
        UnitId(0),
        tile,
        state.unit(UnitId(1)).unwrap(),
        target_pos,
    )
    .expect("she still has a shot on a wreck this close")
    .2;
    assert!(
        kill,
        "the stage needs a shot that would actually finish her, or the bonus has nothing \
         to price"
    );

    let eval = Evaluator::new(reg.doctrine("massed_armor").cloned().unwrap());
    reg.planner.kill_bonus = 0.0;
    let bare = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    reg.planner.kill_bonus = 4.0;
    let shipped = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    assert!(
        shipped > bare,
        "a shot that ends the argument is worth more than the damage alone: \
         {shipped} against {bare}"
    );

    reg.planner.kill_bonus = 12.0;
    assert!(
        eval.score_tile(&reg, &state, UnitId(0), tile).score > shipped,
        "and a commander who prizes a decisive shot more still pays more for the ground \
         it is taken from"
    );
}

/// How much a round of fire from a tile is worth, independent of any
/// doctrine's taste for a fight, is the mod's to choose.
///
/// `attack_worth` is the multiplier the whole attack half of the sum rides
/// on. At zero it is not that fire becomes unattractive — it is that no
/// doctrine, however aggressive, can price a shot from anywhere at all,
/// which is a different and much worse failure than a doctrine merely
/// declining to fire.
#[test]
fn what_a_round_of_fire_from_a_tile_is_worth_is_a_mod_decision() {
    let mut reg = seen(registry_wireless());
    let row: String = "g".repeat(20);
    let state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([0, 1], 0, "medium_tank", "Gunner"),
            unit_at([6, 1], 1, "medium_tank", "Target"),
        ],
        31,
    );
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the stage needs the target in plain sight"
    );
    let tile = state.unit(UnitId(0)).unwrap().pos;
    let eval = Evaluator::new(reg.doctrine("massed_armor").cloned().unwrap());
    assert!(
        eval.score_tile(&reg, &state, UnitId(0), tile)
            .attack
            .is_some(),
        "the stage needs a shot on offer, or there is nothing for the field to price"
    );

    reg.planner.attack_worth = 0.0;
    let priceless = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    reg.planner.attack_worth = 2.0;
    let shipped = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    assert!(
        shipped > priceless,
        "a round of fire priced at nothing does not raise the value of the ground it is \
         taken from: {shipped} against {priceless}"
    );

    reg.planner.attack_worth = 6.0;
    assert!(
        eval.score_tile(&reg, &state, UnitId(0), tile).score > shipped,
        "and a commander who prizes fire more pays more for the ground that offers it"
    );
}

/// A doctrine with no taste for a fight at all still has to value a free
/// shot, or "cautious" and "blind" become the same word.
///
/// `attack_floor` is the appetite `attack_worth`'s partner leaves behind at
/// `aggression: 0`: without it a commander who feels no aggression at all
/// would decline a shot sitting right in front of her to gain a scrap of
/// cover, which is not a cautious commander, it is one who has not noticed
/// the gun in her hand.
#[test]
fn a_doctrine_with_no_aggression_still_values_a_free_shot_because_of_the_floor() {
    let mut reg = seen(registry_wireless());
    let row: String = "g".repeat(20);
    let state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([0, 1], 0, "medium_tank", "Gunner"),
            unit_at([6, 1], 1, "medium_tank", "Target"),
        ],
        33,
    );
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the stage needs the target in plain sight"
    );
    let tile = state.unit(UnitId(0)).unwrap().pos;
    let doctrine = doctrine_with(&reg, "massed_armor", |d| d.aggression = 0.0);
    let eval = Evaluator::new(doctrine);
    assert!(
        eval.score_tile(&reg, &state, UnitId(0), tile)
            .attack
            .is_some(),
        "the stage needs a shot on offer, or there is nothing for the floor to price"
    );

    reg.planner.attack_floor = 0.0;
    let blind = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    reg.planner.attack_floor = 0.5;
    let shipped = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    assert!(
        shipped > blind,
        "a commander with no aggression at all still values the shot she has, because of \
         the floor: {shipped} against {blind}"
    );
}

/// A crew who has been ordered out still answers what is in front of her,
/// but does not go looking for a fight — and how much of the ordinary
/// appetite for fire survives the order is the mod's to choose.
///
/// `withdrawn_attack` is a share of what a round of fire is worth to
/// everybody else. At the shipped value it is worth a little: enough that a
/// retreating crew still shoots what is in her way, not enough that she
/// drives toward a better shot instead of her lane home. At 1.0 the rule is
/// simply absent — a withdrawal weighs fire exactly as any other march does,
/// which is the failure `an_ordered_withdrawal_needs_no_wounds` was written
/// against.
#[test]
fn a_withdrawing_crew_still_answers_a_shot_but_does_not_seek_one() {
    let mut reg = seen(registry_wireless());
    let rows: Vec<&str> = vec!["gggggggggggggggggggg"; 3];
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "withdrawing",
        "palette": { "g": "grass" },
        "rows": rows,
        "objectives": [
            { "id": "east_exit", "name": "East Exit", "at": [[19, 1]], "value": 5,
              "kind": "exit", "side": 0 },
        ],
        "formations": [{ "id": "platoon", "name": "Platoon", "side": 0 }],
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
    let mut hers = unit_at([1, 1], 0, "medium_tank", "Retreater");
    hers.formation = Some("platoon".into());
    hers.leads = true;
    let placements = vec![hers, unit_at([7, 1], 1, "medium_tank", "Target")];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(&reg, &placements);
    let mut state = BattleState::from_placements(
        &reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        37,
    )
    .expect("the staged placements are content the base mod ships");
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the stage needs a target in range, or withdrawn_attack has nothing to scale"
    );
    let tile = state.unit(UnitId(0)).unwrap().pos;
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: formation_named(&state, "platoon"),
                mission: Mission::Withdraw {
                    via: "east_exit".into(),
                },
                latitude: Latitude::Delegated,
            },
        )
        .expect("a legal mission");
    let eval = Evaluator::new(reg.doctrine("massed_armor").cloned().unwrap());
    assert!(
        eval.score_tile(&reg, &state, UnitId(0), tile)
            .attack
            .is_some(),
        "the stage needs a shot on offer to a crew who has been ordered out"
    );

    reg.planner.withdrawn_attack = 0.0;
    let silent = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    reg.planner.withdrawn_attack = 0.0625;
    let shipped = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    assert!(
        shipped > silent,
        "an ordered retreat still answers a shot in front of her, worth a little: \
         {shipped} against {silent}"
    );

    reg.planner.withdrawn_attack = 1.0;
    assert!(
        eval.score_tile(&reg, &state, UnitId(0), tile).score > shipped,
        "and at 1.0 the rule is simply absent: fire is worth exactly what it is to any \
         other crew"
    );
}

/// Most a crew may multiply the danger she reads by, for being small or worn
/// down, has a ceiling — past it the term stops discriminating and only
/// makes the arithmetic loud.
///
/// `exposure_cap` is a genuine ceiling rather than a scale: raising it lets a
/// fragile crew read more danger into the same gun right up to what she
/// actually is, and no further, so a mod that raises it far past a crew's
/// real fragility gets exactly the same answer as one that raised it a
/// little past it.
#[test]
fn how_careful_a_crew_may_become_for_being_small_or_worn_has_a_ceiling() {
    let mut reg = seen(registry_wireless());
    let row: String = "g".repeat(20);
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([1, 1], 0, "recon_car", "Remnant"),
            unit_at([10, 1], 1, "tank_destroyer", "Gun"),
        ],
        41,
    );
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the stage needs the gun in plain sight, or there is no threat to weigh"
    );
    // Both seats wounded and nothing left but her running gear: what she has
    // left is small next to what a battlefield's hulls typically carry.
    {
        let her = state.unit_mut(UnitId(0)).unwrap();
        her.crew_state = vec![tactics_core::battle::CrewCondition::Wounded; her.crew.len()];
        for hits in her.modules.values_mut() {
            *hits = 0;
        }
    }
    let tile = state.unit(UnitId(0)).unwrap().pos;
    assert!(
        tactics_core::battle::incoming(&reg, &state, UnitId(0), tile).worth > 0.0,
        "the stage needs a real threat at her own tile"
    );
    let eval = Evaluator::new(reg.doctrine("massed_armor").cloned().unwrap());

    reg.planner.exposure_cap = 1.0;
    let capped_low = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    reg.planner.exposure_cap = 4.0;
    let shipped = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    assert!(
        shipped < capped_low,
        "raising the ceiling lets a worn crew read more danger into the same gun, which \
         costs the tile more: {shipped} against {capped_low}"
    );

    reg.planner.exposure_cap = 400.0;
    let uncapped = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    assert!(
        uncapped < shipped,
        "and past the shipped ceiling she still reads more danger into the same gun: \
         {uncapped} against {shipped}"
    );

    reg.planner.exposure_cap = 4000.0;
    assert!(
        (eval.score_tile(&reg, &state, UnitId(0), tile).score - uncapped).abs() < 1e-3,
        "but only up to what she actually is: raising the ceiling past her real fragility \
         changes nothing, which is what makes this a ceiling and not merely another scale"
    );
}

/// A crew to be scored, a friend already posted at a fixed hex, and a
/// masking wood between two of the candidate columns — the spacing band's
/// stage, shared by every field in the mass term because moving each of
/// them wants the same friend and the same columns re-scored.
fn spacing_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    let row = {
        let mut r: Vec<char> = std::iter::repeat_n('g', 46).collect();
        r[4] = 'f';
        r.into_iter().collect::<String>()
    };
    let mut state = two_side_battle(
        reg,
        &[&row, &row, &row],
        vec![
            unit_at([15, 1], 0, "medium_tank", "Scored"),
            unit_at([6, 1], 0, "medium_tank", "Friend"),
            unit_at([45, 1], 1, "medium_tank", "Far Foe"),
        ],
        seed,
    );
    assert!(
        state.fog.side(0).spotted.is_empty(),
        "the spacing stage needs no enemy in sight — this is a test about a friend, not \
         about fire"
    );
    // The friend is already under orders to stand where she stands, so the
    // band reads her planned destination.
    state.unit_mut(UnitId(1)).unwrap().intent.path = vec![tactics_core::offset_to_hex(6, 1)];
    state
}

/// A doctrine with no opinions of its own, so a spacing test reads the mass
/// term alone.
fn spacing_doctrine() -> tactics_core::data::DoctrineDef {
    tactics_core::data::DoctrineDef {
        id: "spacing_probe".into(),
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
        route_caution: 0.0,
        contest_aversion: 0.0,
        screening: 0.0,
    }
}

/// What standing one hex from a friend costs is the mod's to choose, and it
/// is the half of the spacing band no doctrine may buy off: one shell taking
/// two vehicles is survival rather than taste.
///
/// Zero is the game before the band existed, when mass was a monotonic pull
/// toward the nearest friend all the way to adjacency and massed armour
/// clumped into artillery bait.
#[test]
fn standing_one_hex_from_a_friend_costs_what_the_mod_says() {
    let mut reg = seen(registry_wireless());
    let state = spacing_stage(&reg, 51);
    let friend = tactics_core::offset_to_hex(6, 1);
    let hugging = tactics_core::offset_to_hex(7, 1);
    assert_eq!(
        hugging.distance_to(friend),
        1,
        "the stage needs a tile one hex from the friend"
    );
    let eval = Evaluator::new(spacing_doctrine());

    reg.planner.crowding_adjacent = 0.0;
    let free = eval.score_tile(&reg, &state, UnitId(0), hugging).score;
    reg.planner.crowding_adjacent = 0.45;
    let shipped = eval.score_tile(&reg, &state, UnitId(0), hugging).score;
    assert!(
        shipped < free,
        "hugging a friend costs something at the shipped price and nothing at zero: \
         {shipped} against {free}"
    );

    reg.planner.crowding_adjacent = 1.5;
    assert!(
        eval.score_tile(&reg, &state, UnitId(0), hugging).score < shipped,
        "and a steeper price costs her more for standing in the same place"
    );
}

/// The same, two hexes out — the far edge of the crowding penalty, taken up
/// separately because it is [`crowding_adjacent`]'s own taper rather than a
/// second reading of the same number.
///
/// The band is two hexes wide because a shell's splash is: this is the field
/// that decides how far the close-in penalty reaches before the supported
/// interval takes over.
#[test]
fn standing_two_hexes_from_a_friend_costs_what_the_mod_says() {
    let mut reg = seen(registry_wireless());
    let state = spacing_stage(&reg, 52);
    let friend = tactics_core::offset_to_hex(6, 1);
    let near = tactics_core::offset_to_hex(8, 1);
    assert_eq!(
        near.distance_to(friend),
        2,
        "the stage needs a tile two hexes from the friend"
    );
    let eval = Evaluator::new(spacing_doctrine());

    reg.planner.crowding_near = 0.0;
    let free = eval.score_tile(&reg, &state, UnitId(0), near).score;
    reg.planner.crowding_near = 0.15;
    let shipped = eval.score_tile(&reg, &state, UnitId(0), near).score;
    assert!(
        shipped < free,
        "standing two hexes off costs something at the shipped price and nothing at zero: \
         {shipped} against {free}"
    );

    reg.planner.crowding_near = 0.6;
    assert!(
        eval.score_tile(&reg, &state, UnitId(0), near).score < shipped,
        "and a steeper price costs her more for the same two hexes"
    );
}

/// How far a friend can stand and still count as support is the mod's to
/// choose, and it decides *whether* the penalty applies at all — widening it
/// moves where straggling starts, which is a different thing from
/// [`out_of_support`] deciding how hard it bites once it has.
#[test]
fn how_far_a_friend_still_counts_as_support_is_a_mod_decision() {
    let mut reg = seen(registry_wireless());
    let state = spacing_stage(&reg, 53);
    let friend = tactics_core::offset_to_hex(6, 1);
    let tile = tactics_core::offset_to_hex(11, 1);
    assert_eq!(
        tile.distance_to(friend),
        5,
        "the stage needs a tile just past the shipped support range"
    );
    let eval = Evaluator::new(spacing_doctrine());

    reg.planner.support_range = 2.0;
    let tight = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    reg.planner.support_range = 4.0;
    let shipped = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    assert!(
        shipped > tight,
        "widening the range shrinks how far past it this tile has strayed: {shipped} \
         against {tight}"
    );

    reg.planner.support_range = 8.0;
    let wide = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    assert!(
        wide > shipped,
        "and past five hexes the friend is supporting again, which costs nothing at all: \
         {wide} against {shipped}"
    );
}

/// What each hex beyond the supported interval costs is the mod's to
/// choose — the old monotonic pull toward the nearest friend, kept as the
/// outer half of the band. At zero, straggling from a friend costs nothing
/// at all, which is the game before the interval existed.
#[test]
fn straggling_beyond_support_costs_what_the_mod_says() {
    let mut reg = seen(registry_wireless());
    let state = spacing_stage(&reg, 55);
    let friend = tactics_core::offset_to_hex(6, 1);
    let tile = tactics_core::offset_to_hex(14, 1);
    assert_eq!(
        tile.distance_to(friend),
        8,
        "the stage needs a tile well outside the supported interval"
    );
    let eval = Evaluator::new(spacing_doctrine());

    reg.planner.out_of_support = 0.0;
    let free = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    reg.planner.out_of_support = 0.12;
    let shipped = eval.score_tile(&reg, &state, UnitId(0), tile).score;
    assert!(
        shipped < free,
        "straggling costs nothing at zero and something at the shipped price: {shipped} \
         against {free}"
    );

    reg.planner.out_of_support = 0.5;
    assert!(
        eval.score_tile(&reg, &state, UnitId(0), tile).score < shipped,
        "and it costs more the steeper the field is set"
    );
}

/// With something to shoot at but nothing yet in range, closing on it is the
/// mod's to choose how hard to pull.
///
/// Zero is a defensible doctrine on its own terms — elastic defence never
/// closes for its own sake — but that has to be something the mod says
/// about a doctrine, not a slope wired into `score_tile` that every doctrine
/// pays whether it wants to or not.
#[test]
fn with_something_to_shoot_she_closes_on_it_by_a_slope_the_mod_sets() {
    let mut reg = seen(registry_wireless());
    let row: String = "g".repeat(20);
    let state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([1, 1], 0, "recon_car", "Scout"),
            unit_at([16, 1], 1, "tank_destroyer", "Gun"),
        ],
        57,
    );
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the stage needs the gun in plain sight"
    );
    let near = tactics_core::offset_to_hex(8, 1);
    let far = tactics_core::offset_to_hex(2, 1);
    let eval = Evaluator::new(reg.doctrine("massed_armor").cloned().unwrap());
    assert!(
        eval.score_tile(&reg, &state, UnitId(0), near)
            .attack
            .is_none()
            && eval
                .score_tile(&reg, &state, UnitId(0), far)
                .attack
                .is_none(),
        "the stage needs no shot of her own at either tile, or the attack term would \
         swamp the slope being measured"
    );
    let gap = |reg: &DataRegistry| {
        eval.score_tile(reg, &state, UnitId(0), near).score
            - eval.score_tile(reg, &state, UnitId(0), far).score
    };

    reg.planner.advance_slope = 0.0;
    let flat = gap(&reg);
    reg.planner.advance_slope = 0.3;
    let shipped = gap(&reg);
    assert!(
        shipped > flat,
        "with the slope read, standing nearer the gun she has found is relatively more \
         attractive than standing well back of it: {shipped} against {flat}"
    );

    reg.planner.advance_slope = 1.0;
    assert!(
        gap(&reg) > shipped,
        "and a commander with more appetite for contact closes harder still"
    );
}

/// With nobody found, no ground named and no orders given, a crew searches
/// toward the middle of the map — and how hard the middle pulls her is the
/// mod's to choose.
///
/// Deliberately not `distance_decay`'s slope despite shipping at the same
/// magnitude: that one is how far a *named* piece of ground still reaches a
/// crew who can see it, and this is what a lost crew does with an empty map
/// that has named her nothing. At zero she simply stands still, which is a
/// fair answer to a map that gave her nothing to do, if a dull one.
#[test]
fn with_nothing_found_she_searches_the_middle_by_a_slope_the_mod_sets() {
    let mut reg = seen(registry_wireless());
    let row: String = "g".repeat(21);
    let state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([1, 1], 0, "medium_tank", "Lost"),
            unit_at([19, 1], 1, "medium_tank", "Far Side"),
        ],
        59,
    );
    assert!(
        state.fog.side(0).spotted.is_empty(),
        "the stage needs no contact, or advance would be reading the enemy rather than \
         the empty map"
    );
    assert!(
        state.map.objectives().is_empty(),
        "the stage needs no objectives, or the objective term would be reading the map \
         instead of this one"
    );
    let center = state.map.center();
    let edge = state.unit(UnitId(0)).unwrap().pos;
    assert_ne!(
        center, edge,
        "the stage needs the centre to be somewhere she is not already standing"
    );
    let eval = Evaluator::new(reg.doctrine("massed_armor").cloned().unwrap());
    let gap = |reg: &DataRegistry| {
        eval.score_tile(reg, &state, UnitId(0), center).score
            - eval.score_tile(reg, &state, UnitId(0), edge).score
    };

    reg.planner.search_slope = 0.0;
    let flat = gap(&reg);
    assert!(
        flat.abs() < 1e-6,
        "with no slope the middle of an empty map is worth exactly what the edge is: {flat}"
    );

    reg.planner.search_slope = 0.15;
    let shipped = gap(&reg);
    assert!(
        shipped > 0.0,
        "and at the shipped slope a lost crew is pulled toward the middle"
    );

    reg.planner.search_slope = 0.5;
    assert!(
        gap(&reg) > shipped,
        "a steeper slope pulls her there harder"
    );
}

// --- what an order is worth against the terrain ----------------------------

//
// Four more numbers that were bare Rust until 2026-08-28, and unlike the five
// above they are not five separate knobs: they are four terms in one sum, and
// the sum decides the thing DIRECTION.md's original complaint was about —
// "the order competes with the terrain, so `take that hill` can lose to `this
// hedge scores better`, and the unit ends up *near* where it was pointed."
//
// Each test below is the check that its field is *read*, and each was
// mutation-checked by pinning the field back to the constant it replaced and
// requiring the test to fail. But they are written as behaviour rather than
// as arithmetic, because a designer reaching for one of these wants to know
// what it does at the keyboard, not that a multiplication happened.

/// A crew under orders, an enemy she cannot see, and a hedge worth stopping
/// in: the stage on which an order argues with the ground.
///
/// The forest sits right beside her and the ordered hex is at the far end of
/// a long field, so the two tiles disagree about everything at once — cover
/// against no cover, arrived against sixteen hexes to go — which is what
/// makes the crossover between them a statement about the *order's* weight
/// rather than about a rounding difference. No enemy in sight on purpose: an
/// advance under fire is a different question, and it is the next test.
fn ordered_against_the_ground(reg: &DataRegistry, seed: u64) -> BattleState {
    ordered_against_the_ground_in(reg, "medium_tank", seed)
}

/// The same stage, crewed by whichever chassis the caller wants.
///
/// Parameterised because an order is a share of the crew who was given it
/// now, so the interesting comparison is the *same* order given to two
/// different vehicles — and the honest way to make it is two runs of one
/// stage rather than two crews on one, which would put each of them in the
/// other's mass term and confound the thing being measured.
fn ordered_against_the_ground_in(reg: &DataRegistry, vehicle: &str, seed: u64) -> BattleState {
    let rows: Vec<&str> = vec![
        "gggggggggggggggggggg",
        "ggfggggggggggggggggg",
        "gggggggggggggggggggg",
    ];
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "ordered",
        "palette": { "g": "grass", "f": "forest" },
        "rows": rows,
        "formations": [{ "id": "platoon", "name": "Platoon", "side": 0 }],
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
    let mut hers = unit_at([1, 1], 0, vehicle, "Subordinate");
    hers.formation = Some("platoon".into());
    hers.leads = true;
    let placements = vec![hers, unit_at([19, 0], 1, "medium_tank", "Somebody")];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    let mut state = BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
    .expect("the staged placements are content the base mod ships");
    state
        .apply(
            reg,
            &Order::SetMission {
                formation: formation_named(&state, "platoon"),
                mission: Mission::Advance {
                    to: tactics_core::offset_to_hex(18, 1),
                },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("a legal mission");
    state
}

/// The same shape as [`objective_under_a_gun`], with an *order* on the
/// dangerous hex instead of an objective.
///
/// What it is for is the sentence `order_worth` is quoted in: an order is
/// worth a share of what she has left, per round, *against the danger at the
/// ordered ground*. So the stage has to put a real price on the ordered hex
/// and nothing else on it — no objectives, no cover, no shot of her own, one
/// crew a side — and then the only question left is how large the share is
/// and whether her commander said she meant it.
fn ordered_into_a_gun(reg: &DataRegistry, seed: u64) -> BattleState {
    let rows: Vec<&str> = vec!["gggggggggggggggggg"; 3];
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "ordered_under_fire",
        "palette": { "g": "grass" },
        "rows": rows,
        "formations": [{ "id": "platoon", "name": "Platoon", "side": 0 }],
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
    let mut hers = unit_at([1, 1], 0, "recon_car", "Subordinate");
    hers.formation = Some("platoon".into());
    hers.leads = true;
    let placements = vec![hers, unit_at([16, 1], 1, "tank_destroyer", "Gun")];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    let mut state = BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
    .expect("the staged placements are content the base mod ships");
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the order has to be into a gun she can see, or the danger is not in the sum"
    );
    state
        .apply(
            reg,
            &Order::SetMission {
                formation: formation_named(&state, "platoon"),
                mission: Mission::Advance {
                    to: tactics_core::offset_to_hex(9, 1),
                },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("a legal mission");
    state
}

/// A delegated crew takes the ordered ground until its danger outweighs her
/// share of herself; a binding one takes it from further down the scale.
///
/// The two halves of Wave 2's order term meeting: what an order is worth is a
/// share of the crew (`order_worth`), and what latitude buys is the floor
/// under the doctrine's discount and nothing else. So a loose doctrine
/// reading a delegated order needs the share to be larger before the ordered
/// hex beats the safe one, and the same doctrine told "I mean it" crosses
/// earlier — by exactly the ratio of the two strictnesses and no more, which
/// is the rule `a_binding_mission_is_not_discounted_by_a_loose_doctrine`
/// pins from the other side.
///
/// Written as two thresholds rather than as one comparison at the shipped
/// value on purpose: a single comparison would pass on a stage where the
/// order beats the gun at every price, and the thing worth defending is that
/// there *is* a price at which she declines.
///
/// Measured, so the margins are on the record: a recon car with nine
/// substance points left, seven hexes from a tank destroyer's 88, is looking
/// at 29.2 points of expected fire a round. She crosses at `order_worth` 1.20
/// delegated and 0.95 binding — four fifths of it, to the twentieth the scan
/// steps in. At the shipped quarter she declines this order, which is the
/// right answer: a quarter of nine points is a little over two, against
/// twenty-nine.
#[test]
fn a_delegated_crew_takes_ordered_ground_until_it_costs_more_than_her_share_of_herself() {
    let mut reg = seen(registry_wireless());
    let ordered = tactics_core::offset_to_hex(9, 1);
    let safe = tactics_core::offset_to_hex(1, 1);
    // Elastic defence devolves, so `1.5 - delegation` is 0.8 delegated and is
    // floored at 1.0 when the order is binding: this is the one doctrine on
    // which latitude has anything to say.
    let doctrine = reg
        .doctrine("elastic_defense")
        .cloned()
        .expect("base doctrine");
    let eval = Evaluator::new(doctrine);

    let takes_it = |reg: &DataRegistry, state: &BattleState| {
        eval.score_tile(reg, state, UnitId(0), ordered).score
            > eval.score_tile(reg, state, UnitId(0), safe).score
    };
    // The smallest share at which she goes, to a twentieth. `None` would mean
    // no price buys the march, which is a broken stage rather than a result.
    let threshold = |reg: &mut DataRegistry, latitude: Latitude| {
        let mut state = ordered_into_a_gun(reg, 31);
        state
            .apply(
                reg,
                &Order::SetMission {
                    formation: formation_named(&state, "platoon"),
                    mission: Mission::Advance { to: ordered },
                    latitude,
                },
            )
            .expect("a legal mission");
        (0..=40)
            .map(|n| n as f32 * 0.05)
            .find(|share| {
                reg.planner.order_worth = *share;
                takes_it(reg, &state)
            })
            .expect("some share of herself buys the march")
    };

    let delegated = threshold(&mut reg, Latitude::Delegated);
    let binding = threshold(&mut reg, Latitude::Binding);
    assert!(
        delegated > 0.0,
        "an order worth nothing does not send her into a gun: she crossed at {delegated}"
    );
    assert!(
        binding < delegated,
        "insisting buys the march at a smaller share of her: binding at {binding}, \
         delegated at {delegated}"
    );
    // And by the letter of the order and no further: the two thresholds are
    // in the ratio of the two strictnesses, 0.8 against the floored 1.0, so
    // the binding one crosses at four fifths of the delegated one. Checked to
    // within the twentieth the scan steps in.
    assert!(
        (binding - delegated * 0.8).abs() <= 0.05 + 1e-6,
        "the gap is the delegation floor and nothing else: {binding} against \
         {} expected",
        delegated * 0.8
    );

    // And the danger is really what she is weighing against, rather than the
    // distance: at the share she crossed at, the ordered ground is worth
    // about what the gun expects to take out of her there.
    let state = ordered_into_a_gun(&reg, 31);
    let left = state.substance(&reg, state.unit(UnitId(0)).unwrap()).0 as f32;
    let danger = tactics_core::battle::incoming(&reg, &state, UnitId(0), ordered).worth;
    assert!(
        danger > left,
        "the ordered hex has to cost her more than she is worth, or a quarter of \
         herself would buy the march and the thresholds above would both be zero: \
         {danger} against {left} left aboard"
    );
}

/// A piece of scoring ground inside a gun's envelope, and safe ground behind
/// her, with nothing else to argue about.
///
/// The choice the ridge arena found the evaluator getting wrong, staged small
/// enough to reason about: an objective worth three points seven hexes from
/// an 88, against the hex she is standing on fifteen hexes from it. Whether
/// she takes it is exactly the question "what is a point of score worth in
/// substance points a round", which is [`PlannerRules::score_worth`].
///
/// A recon car on purpose, and the reason is that the stage has to be about
/// the objective rather than about the shot. Her only weapon reaches six
/// hexes and the tank destroyer is seven away from the near tile and fifteen
/// from the far one, so the attack term is zero at both ends and cannot
/// stand in for the objective's pull; her twenty-hex vision means her own
/// side finds the gun from either tile, so the threat term is honest at both;
/// and she is the only crew on her side, so the mass term is zero rather than
/// a fourth thing to hold constant.
fn objective_under_a_gun(reg: &DataRegistry, seed: u64) -> BattleState {
    let rows: Vec<&str> = vec![
        "gggggggggggggggggg",
        "gggggggggggggggggg",
        "gggggggggggggggggg",
    ];
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "prize",
        "palette": { "g": "grass" },
        "rows": rows,
        "objectives": [
            { "id": "ford", "name": "The Ford", "at": [[9, 1]], "value": 3 },
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
    let placements = vec![
        unit_at([1, 1], 0, "recon_car", "Scout"),
        unit_at([16, 1], 1, "tank_destroyer", "Gun"),
    ];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
    let state = BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    )
    .expect("the staged placements are content the base mod ships");
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the stage rests on her side having found the gun: the choice is between known \
         danger and known ground, not between a guess and a guess"
    );
    state
}

/// What the ground a commander names is worth is the mod's to choose, and it
/// is worth a share of the crew she named it to.
///
/// This is the complaint the whole design memo opens with, made checkable:
/// an order is a term in a sum, so there is some weight at which the hedge
/// wins and some weight at which the hill does, and which of those the game
/// ships at is a design decision rather than a constant nobody chose.
#[test]
fn what_the_ground_a_commander_names_is_worth_is_a_mod_decision() {
    let mut reg = seen(registry_wireless());
    let state = ordered_against_the_ground(&reg, 17);
    assert!(
        state.fog.side(0).spotted.is_empty(),
        "the stage needs no enemy in sight — this is a test about ground, not about fire"
    );
    let hedge = tactics_core::offset_to_hex(2, 1);
    let hill = tactics_core::offset_to_hex(18, 1);
    let eval = Evaluator::new(reg.doctrine("massed_armor").cloned().unwrap());
    let gap = |reg: &DataRegistry| {
        eval.score_tile(reg, &state, UnitId(0), hill).score
            - eval.score_tile(reg, &state, UnitId(0), hedge).score
    };

    // Worth nothing, and the order may as well not have been given: she reads
    // the map exactly as an unordered crew does and the cover beside her
    // wins.
    reg.planner.order_worth = 0.0;
    assert!(
        gap(&reg) < 0.0,
        "an order worth nothing loses to a hedge, which is the complaint in one line"
    );

    // At what the base mod ships, the sixteen hexes she was told to cross are
    // worth crossing.
    reg.planner.order_worth = 0.25;
    let shipped = gap(&reg);
    assert!(
        shipped > 0.0,
        "at the shipped weight the ground she was given beats the ground she is standing next to"
    );

    // And it is a weight rather than a switch: heavier orders pull harder,
    // which is what makes sweeping it mean something.
    reg.planner.order_worth = 0.75;
    assert!(
        gap(&reg) > shipped,
        "and a heavier order pulls harder still"
    );
}

/// The same order is worth more to a heavy tank than to a scout car.
///
/// The whole content of quoting an order as a *share* of the crew rather than
/// as a number. Under the flat `mission_weight` these two read "go to that
/// hex" identically, while the danger of going there was priced in substance
/// points that meant three times as much to one of them as to the other — so
/// the small vehicle was the one who obeyed, which is backwards.
///
/// Two chassis on one stage under one mission, scored on the same two tiles,
/// so nothing but the size of the crew differs. Mutation-checked by pinning
/// the weight back to a constant: `order_worth * left` -> `order_worth * 11.0`
/// makes the two gaps equal and fails the strict comparison.
#[test]
fn an_order_is_worth_a_share_of_herself_so_it_asks_more_of_a_heavier_crew() {
    let reg = seen(registry_wireless());
    let hedge = tactics_core::offset_to_hex(2, 1);
    let hill = tactics_core::offset_to_hex(18, 1);
    let eval = Evaluator::new(reg.doctrine("massed_armor").cloned().unwrap());
    let pull = |vehicle| {
        let state = ordered_against_the_ground_in(&reg, vehicle, 19);
        let left = state.substance(&reg, state.unit(UnitId(0)).unwrap()).0;
        let gap = eval.score_tile(&reg, &state, UnitId(0), hill).score
            - eval.score_tile(&reg, &state, UnitId(0), hedge).score;
        (left, gap)
    };
    let (bigger, heavy) = pull("heavy_tank");
    let (smaller, scout) = pull("recon_car");
    assert!(
        bigger > smaller,
        "the stage needs two chassis of different size, got {bigger} and {smaller}"
    );
    assert!(
        heavy > scout,
        "the same order pulls the heavier crew harder: {heavy} against {scout}"
    );
    // And it pulls her harder *in proportion to what she is*, which is the
    // claim rather than merely "differently". Everything on the stage is
    // shared except the chassis, and the only term that does not scale with
    // her is the hedge's cover prior, so the two pulls should track the two
    // complements to within a few percent.
    let by_pull = heavy / scout;
    let by_size = bigger as f32 / smaller as f32;
    assert!(
        (by_pull - by_size).abs() < 0.1 * by_size,
        "the pull should scale with the crew: {by_pull} against a size ratio of {by_size}"
    );
}

/// A worn crew still holds her orders against what she was.
///
/// `order_worth` is a share of what she has left and the threat term is a
/// fraction of the same number, so as a crew wore down the order shrank and
/// the danger grew, and the ratio between them fell with the *square* of her
/// condition — a consequence of two rules nobody had chosen. The designer's
/// ruling is `planner.order_complement`: the order is quoted against her
/// and, scaled down, against her full complement. Two claims, on one stage
/// with the same order and the same gun:
///
/// - a fresh crew reads her orders identically at every value of the field,
///   because what she has left *is* what she was — the additivity half;
/// - a crew with half her seats empty holds the order harder the more of
///   it is quoted against what she was, monotonically.
///
/// Mutation-checked by pinning `full` to `left` in `mission_value`: the worn
/// crew's gap then stops moving and the strict inequalities fail.
#[test]
fn a_worn_crew_still_holds_her_orders_against_what_she_was() {
    let mut reg = seen(registry_wireless());
    let hedge = tactics_core::offset_to_hex(2, 1);
    let hill = tactics_core::offset_to_hex(18, 1);
    let eval = Evaluator::new(reg.doctrine("massed_armor").cloned().unwrap());
    let fresh = ordered_against_the_ground_in(&reg, "heavy_tank", 19);
    let mut worn = fresh.clone();
    {
        // `crew_state` may be shorter than the crew — a seat it does not
        // name is read as fine — so it is written out whole: every other
        // seat knocked out, which takes about half of her.
        let her = &mut worn.units[0];
        her.crew_state = (0..her.crew.len())
            .map(|seat| {
                if seat % 2 == 0 {
                    tactics_core::battle::CrewCondition::Out
                } else {
                    tactics_core::battle::CrewCondition::Fine
                }
            })
            .collect();
    }
    let (left, full) = worn.substance(&reg, worn.unit(UnitId(0)).unwrap());
    assert!(
        left < full && left > 0,
        "the stage needs a crew with something taken off her: {left} of {full}"
    );
    let gap = |reg: &DataRegistry, state: &BattleState| {
        eval.score_tile(reg, state, UnitId(0), hill).score
            - eval.score_tile(reg, state, UnitId(0), hedge).score
    };

    reg.planner.order_complement = 0.0;
    let (fresh_at_none, worn_at_none) = (gap(&reg, &fresh), gap(&reg, &worn));
    reg.planner.order_complement = 0.5;
    let (fresh_at_half, worn_at_half) = (gap(&reg, &fresh), gap(&reg, &worn));
    reg.planner.order_complement = 1.0;
    let (fresh_at_whole, worn_at_whole) = (gap(&reg, &fresh), gap(&reg, &worn));

    assert!(
        (fresh_at_none - fresh_at_half).abs() < 1e-4
            && (fresh_at_none - fresh_at_whole).abs() < 1e-4,
        "a fresh crew reads the same order at every value: {fresh_at_none} / {fresh_at_half} / {fresh_at_whole}"
    );
    assert!(
        worn_at_none < worn_at_half && worn_at_half < worn_at_whole,
        "and a worn one holds it harder the more of it is quoted against what she was: \
         {worn_at_none} < {worn_at_half} < {worn_at_whole}"
    );
    assert!(
        worn_at_whole <= fresh_at_whole + 1e-4,
        "quoted wholly against her complement, the order pulls a worn crew no harder than a \
         fresh one — it is the danger that differs between them, not the order: \
         {worn_at_whole} against {fresh_at_whole}"
    );
}

/// What a point on the scoreboard is worth is the mod's to choose.
///
/// The other half of Wave 2, and the thing `ridge_arena` measured the absence
/// of: an objective's `value` used to arrive at the evaluator on a scale of
/// its own, so a crest worth 3 argued with a threat term the resolver had
/// grown to eight or twelve substance points a round and lost. There is some
/// rate at which taking ground under a gun is worth it and some rate at which
/// it is not, and which the game ships at is a design decision.
///
/// Staged as exactly that choice: an objective inside a found gun's envelope,
/// against safe ground outside it.
#[test]
fn a_point_of_score_is_priced_in_the_currency() {
    let mut reg = seen(registry_wireless());
    let state = objective_under_a_gun(&reg, 23);
    let prize = tactics_core::offset_to_hex(6, 1);
    let safe = tactics_core::offset_to_hex(1, 1);
    assert!(
        tactics_core::battle::incoming(&reg, &state, UnitId(0), prize).worth
            > tactics_core::battle::incoming(&reg, &state, UnitId(0), safe).worth,
        "the stage needs the objective to be the dangerous half of the choice"
    );
    let eval = Evaluator::new(reg.doctrine("massed_armor").cloned().unwrap());
    let gap = |reg: &DataRegistry| {
        eval.score_tile(reg, &state, UnitId(0), prize).score
            - eval.score_tile(reg, &state, UnitId(0), safe).score
    };

    // A point of score worth nothing: she is a crew with no reason to be
    // here, and the gun decides.
    reg.planner.score_worth = 0.0;
    assert!(
        gap(&reg) < 0.0,
        "with the scoreboard worth nothing she declines the ground, which is exactly \
         what the ridge arena measured"
    );

    // And a rate at which the ground is worth what it costs. Found by
    // doubling from the shipped 1.0 rather than asserted: what matters is
    // that some rate flips the choice, because that is what makes the field a
    // design decision instead of a decoration.
    reg.planner.score_worth = 8.0;
    assert!(
        gap(&reg) > 0.0,
        "and at a rate that says the battle is about its ground, she takes it under fire"
    );

    // Monotone in between, so a sweep of it means something.
    reg.planner.score_worth = 1.0;
    let shipped = gap(&reg);
    reg.planner.score_worth = 4.0;
    assert!(
        gap(&reg) > shipped,
        "a dearer point of score pulls harder: {} against {shipped}",
        gap(&reg)
    );
}

/// Whether being shot at suspends a movement to contact is the mod's to
/// choose.
///
/// `Advance` halts and fights; `Assault` presses on. That distinction is the
/// whole difference between the two verbs, so this field is the one number in
/// the block whose neutral-looking value is a trap: at 1.0 nothing is turned
/// off, the two orders have simply become synonyms.
#[test]
fn whether_fire_suspends_an_advance_is_a_mod_decision() {
    let mut reg = seen(registry_wireless());
    // `pressed_stage` is the one with an enemy in view who can shoot her
    // where she stands, which is exactly the condition the damping asks
    // about.
    let (state, told) = pressed_stage(&reg, 41);
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the damping only applies under fire, so the stage needs somebody shooting"
    );
    let behind = tactics_core::offset_to_hex(1, 0);
    let eval = Evaluator::new(reg.doctrine("massed_armor").cloned().unwrap());
    let pull = |reg: &DataRegistry| {
        eval.score_tile(reg, &state, UnitId(0), told).score
            - eval.score_tile(reg, &state, UnitId(0), behind).score
    };

    // Contact damping is a scale on the mission term alone, so what it moves
    // is how much of the order survives the shooting.
    reg.planner.pull_under_fire = 0.25;
    let halted = pull(&reg);
    reg.planner.pull_under_fire = 1.0;
    let pressing = pull(&reg);
    assert!(
        pressing > halted,
        "an advance that does not stop for fire pulls harder than one that does: \
         {pressing} against {halted}"
    );

    // And the floor: an order that evaporates entirely on contact leaves her
    // choosing ground for her own reasons, which is what a doctrine that
    // never presses would be asking for.
    reg.planner.pull_under_fire = 0.0;
    assert!(
        pull(&reg) < halted,
        "and one suspended outright pulls less than the shipped quarter"
    );
}

/// How far a piece of ground reaches is the mod's to choose — and it is one
/// number for orders and objectives alike.
///
/// The slope exists because a greedy one-round planner can only see the tiles
/// it can reach this round: ground whose value is flat until you arrive is
/// ground it cannot navigate to. So at zero decay a crew twenty hexes off has
/// no idea which way to drive, which is the failure this term prevents rather
/// than the gentle setting of it.
#[test]
fn how_far_the_pull_of_ground_reaches_is_a_mod_decision() {
    let mut reg = seen(registry_wireless());
    let state = ordered_against_the_ground(&reg, 17);
    // Two tiles on the same terrain, differing only in how far they are from
    // the hex she was told to take.
    let near = tactics_core::offset_to_hex(14, 1);
    let far = tactics_core::offset_to_hex(4, 1);
    let eval = Evaluator::new(reg.doctrine("massed_armor").cloned().unwrap());
    let slope = |reg: &DataRegistry| {
        eval.score_tile(reg, &state, UnitId(0), near).score
            - eval.score_tile(reg, &state, UnitId(0), far).score
    };

    reg.planner.distance_decay = 0.0;
    assert!(
        slope(&reg).abs() < 1e-6,
        "with no slope, ten hexes of progress toward the ordered ground is worth nothing \
         and she cannot tell which way to drive"
    );

    reg.planner.distance_decay = 0.15;
    let shipped = slope(&reg);
    assert!(shipped > 0.0, "the shipped slope leads her there");

    reg.planner.distance_decay = 0.45;
    assert!(
        slope(&reg) > shipped,
        "and a steeper one leads her there harder"
    );
}

/// The same field is the slope on a map objective, which is the thing that
/// makes `mission_weight` quotable in objective-value units.
///
/// Two slopes would be two answers to one question — how far can ground tell
/// you which way to drive — and the sentence "an order pulls about as hard as
/// the ford" would quietly stop being true, because the two rewards would no
/// longer be measured against the same yardstick.
#[test]
fn an_order_and_an_objective_are_led_to_by_the_same_slope() {
    let mut reg = seen(registry_wireless());
    let rows: Vec<&str> = vec!["gggggggggggggggggggg"; 3];
    let state = goal_battle(
        &reg,
        &rows,
        serde_json::json!([
            { "id": "ford", "name": "The Ford", "at": [[18, 1]], "value": 2 },
        ]),
        vec![
            unit_at([1, 1], 0, "medium_tank", "Chooser"),
            unit_at([19, 0], 1, "medium_tank", "Somebody"),
        ],
        17,
    );
    let near = tactics_core::offset_to_hex(14, 1);
    let far = tactics_core::offset_to_hex(4, 1);
    let eval = Evaluator::new(reg.doctrine("massed_armor").cloned().unwrap());
    let slope = |reg: &DataRegistry| {
        eval.score_tile(reg, &state, UnitId(0), near).score
            - eval.score_tile(reg, &state, UnitId(0), far).score
    };

    reg.planner.distance_decay = 0.0;
    assert!(
        slope(&reg).abs() < 1e-6,
        "no mission anywhere on this stage, and the objective's slope answers to the \
         same field"
    );
    reg.planner.distance_decay = 0.15;
    assert!(
        slope(&reg) > 0.0,
        "and the shipped slope leads her to the ford"
    );
}

/// How much better a tile has to be before she will drive for it is the mod's
/// to choose.
///
/// The plateau band is a dispersion rule before it is a movement-economy one:
/// open ground scores in broad plateaus, and without it every identical crew
/// on a side makes the identical choice and arrives as a queue. That is what
/// once made a noiseless side play *worse* than a randomly scattered one and
/// inverted the entire difficulty ladder, so zero is emphatically not the
/// gentle setting of this field.
///
/// **The stage declares no objectives, and it has to.** The band decides the
/// tile sweep's answer, and since the goal layer landed that answer is one
/// candidate among the goals rather than her destination: give this map a
/// ford and she takes `Take(ford)` as a goal and walks a leg of it through
/// `step_toward`, which never consults the band at all. So what this field
/// still governs is the choice between pieces of ground that mean nothing in
/// particular — which is exactly the case it was introduced for, a side
/// spread over featureless grass, and is worth knowing before anyone sweeps
/// it expecting a large number.
#[test]
fn how_much_better_a_tile_must_be_before_she_drives_for_it_is_a_mod_decision() {
    let mut reg = seen(registry_wireless());
    // A wood four hexes off: somewhere meaningfully better than the grass she
    // is parked on, at a cost in driving. No objectives and nobody in sight,
    // so the wood is the only thing on the map with an opinion.
    let rows: Vec<&str> = vec![
        "gggggggggggggggggggg",
        "gggggffggggggggggggg",
        "gggggggggggggggggggg",
    ];
    let state = goal_battle(
        &reg,
        &rows,
        serde_json::json!([]),
        vec![
            unit_at([1, 1], 0, "medium_tank", "Chooser"),
            unit_at([19, 0], 1, "medium_tank", "Somebody"),
        ],
        17,
    );
    assert!(
        state.fog.side(0).spotted.is_empty(),
        "nobody in sight, so nothing but the ground is speaking"
    );
    let home = tactics_core::offset_to_hex(1, 1);
    let driven = |reg: &DataRegistry| {
        move_chosen(reg, &state, UnitId(0)).map_or(0, |to| home.distance_to(to))
    };

    reg.planner.plateau = 0.0;
    let bare = driven(&reg);
    assert!(
        bare > 0,
        "with no band at all she takes the best tile she can reach, whatever it costs her"
    );

    // A band wider than anything this map has to offer makes every tile she
    // can reach indistinguishable, and among ground she cannot tell apart she
    // stays where she is.
    reg.planner.plateau = 100.0;
    assert_eq!(
        driven(&reg),
        0,
        "and a crew who can tell no two hexes apart has no reason to burn a drop of fuel"
    );
}

/// The destination the planner picks for `unit`, or `None` if it leaves her
/// where she is. The twin of [`goal_with_foresight`], one layer down: that
/// one asks what she means to do, this one asks where she actually drives.
fn move_chosen(reg: &DataRegistry, state: &BattleState, unit: UnitId) -> Option<tactics_core::Hex> {
    let doctrine = doctrine_with(reg, "massed_armor", |d| {
        d.route_caution = 0.0;
        d.contest_aversion = 0.0;
    });
    // Difficulty 5 so the per-round lean is exactly zero and the only thing
    // deciding between two tiles is the band under test.
    let mut planner = UtilityPlanner::new(Evaluator::new(doctrine), 5, 99);
    planner.foresight = 1.0;
    let mut state = state.clone();
    loop {
        let order = planner.next_order(reg, &state, 0);
        if let Order::SetMove { unit: who, to } = order
            && who == unit
        {
            return Some(to);
        }
        if matches!(order, Order::Commit { .. }) {
            return None;
        }
        state.apply(reg, &order).ok()?;
    }
}

/// A battle on a hand-drawn map that declares ground worth holding.
///
/// The twin of [`two_side_battle`] for tests about the *goal* layer, which
/// needs somewhere to go: a crew with no objectives has two candidates — the
/// tile this round's sweep picked, and standing still — and neither of them
/// says anything about how she chose. Distinct from `objective_battle`
/// further down, which draws its own one-row map and is about scoring rather
/// than about choosing.
fn goal_battle(
    reg: &DataRegistry,
    rows: &[&str],
    objectives: serde_json::Value,
    placements: Vec<UnitPlacement>,
    seed: u64,
) -> BattleState {
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "goal_map",
        "palette": { "g": "grass", "f": "forest", "w": "water", "r": "road" },
        "rows": rows,
        "objectives": objectives,
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
    .expect("the staged placements are content the base mod ships")
}

/// What ground this planner sends `unit` to, with the blur switched off and
/// foresight set by hand.
///
/// Difficulty is two numbers now and a test about one of them must hold the
/// other still: at difficulty 1 a commander both misjudges the map and does
/// not read it, and a test that changed difficulty would not know which of
/// the two moved its answer.
fn goal_with_foresight(
    reg: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    doctrine: tactics_core::data::DoctrineDef,
    foresight: f32,
) -> Option<tactics_core::battle::Goal> {
    let mut planner = UtilityPlanner::new(Evaluator::new(doctrine), 5, 99);
    planner.foresight = foresight;
    let mut state = state.clone();
    loop {
        let order = planner.next_order(reg, &state, 0);
        if let Order::SetGoal { unit: who, goal } = order
            && who == unit
        {
            return Some(goal);
        }
        if matches!(order, Order::Commit { .. }) {
            return None;
        }
        state.apply(reg, &order).ok()?;
    }
}
