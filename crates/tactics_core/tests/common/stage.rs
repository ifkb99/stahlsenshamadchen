//! The stage builders several binaries share.
//!
//! `engine.rs` split into nine test binaries by subject (see each binary's
//! own module doc); a stage builder used by only one of them stayed there,
//! private, unchanged. These are the ones more than one binary calls — a
//! two-side battle on a bare map, a formation under a doctrine, a duel
//! staged to a particular round — promoted here so nine copies did not have
//! to agree by hand. `pub` only because Cargo compiles this module into
//! every binary that names it; nothing here is part of the crate's public
//! surface.

#![allow(dead_code)]

use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, make_battle_planner};
use tactics_core::battle::{
    BattleState, Event as BattleEvent, FireIntent, FormationId, Latitude, Order, SideState, UnitId,
};
use tactics_core::data::{DataRegistry, RoundPressure};
use tactics_core::map::{HexMap, UnitPlacement};

/// Close every side's planning and play the round out.
pub fn play_round(reg: &DataRegistry, state: &mut BattleState) -> Vec<BattleEvent> {
    let mut events = Vec::new();
    for side in state.living_sides() {
        if !state.has_committed(side) {
            events.extend(state.apply(reg, &Order::Commit { side }).expect("commit"));
        }
    }
    events.extend(state.resolve_round(reg));
    events
}

pub fn two_side_battle(
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
    .expect("the staged placements are content the base mod ships")
}

pub fn unit_at(at: [i32; 2], side: u8, vehicle: &str, name: &str) -> UnitPlacement {
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
pub fn duel(reg: &DataRegistry, seed: u64) -> BattleState {
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
pub fn objective_battle(
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
    .expect("the staged placements are content the base mod ships")
}

/// The two crews of an objective test, out of contact behind the curtain.
pub fn curtained_pair() -> Vec<UnitPlacement> {
    vec![
        unit_at([0, 0], 0, "medium_tank", "West"),
        unit_at([10, 0], 1, "medium_tank", "East"),
    ]
}

/// The formation a test orders about: `river_crossing`'s first, which is
/// Kuhlmann's armored platoon on side 0. Named through the map rather than by
/// a bare index so that a map edit that reorders the declarations fails here
/// loudly instead of quietly testing a different platoon.
pub fn formation_named(state: &BattleState, id: &str) -> FormationId {
    let index = state
        .formations()
        .iter()
        .position(|f| f.id == id)
        .unwrap_or_else(|| panic!("river_crossing declares no formation `{id}`"));
    FormationId(index as u32)
}

pub fn standoff(reg: &DataRegistry, seed: u64) -> BattleState {
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

/// Force one response for every crew, so a test about what flight *does* is
/// not also a test about who reaches for it. `base` outranks the `core` term
/// by more than any core can differ, which is the point: temperament has its
/// own tests.
pub fn always(reg: &mut DataRegistry, response: &str) {
    reg.morale.defiance = vec![tactics_core::data::DefianceDef {
        id: response.into(),
        name: "does it".into(),
        response: serde_json::from_value(serde_json::json!(response)).expect("a real response"),
        core: None,
        base: 100,
    }];
}

/// The pressure that puts a crew off the end of the shipped ladder.
pub fn breaking(reg: &DataRegistry) -> u32 {
    reg.morale
        .rungs
        .last()
        .expect("the shipped ladder has rungs")
        .at_pressure
}

/// A sharp-eyed utility planner for one side, for tests that assert where
/// units choose to go: difficulty 5 is zero scoring noise, so the assertion
/// is about the evaluator rather than the dice.
pub fn sharp_planner(
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

/// Take the radio sets out of every vehicle, so a test that engineers a net
/// with `command_rules(radius, ..)` is testing the radius it wrote rather
/// than the 8-hex hardware the base vehicles carry.
pub fn strip_radios(reg: &mut DataRegistry) {
    for vehicle in reg.vehicles.values_mut() {
        vehicle.radio = None;
    }
}

/// Command rules built white-box, so these tests can state the rules at any
/// coefficient — including the zero coefficients that must give back today's
/// game exactly — without moving the determinism baseline an inch.
pub fn command_rules(
    radius: u32,
    relay: bool,
    base_ticks: u32,
) -> tactics_core::data::CommandRules {
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
pub fn commit_all(reg: &DataRegistry, state: &mut BattleState) {
    for side in state.living_sides() {
        if !state.has_committed(side) {
            state.apply(reg, &Order::Commit { side }).expect("commit");
        }
    }
}

/// A battle on a map written out in the test, so a chain of command and the
/// stakes a scenario places on it can be declared in one place and read in
/// one place. `file` is the map file minus its units, which come in as
/// placements the way every other battle helper here takes them.
pub fn scripted_battle(
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
    .expect("the staged placements are content the base mod ships")
}

/// A placement that answers to a formation, and optionally commands it.
pub fn in_formation(mut placement: UnitPlacement, formation: &str, leads: bool) -> UnitPlacement {
    placement.formation = Some(formation.into());
    placement.leads = leads;
    placement
}

/// Beat a vehicle down to the state the old tests wrote as `hp = 1`:
/// every cadet wounded, everything but the running gear destroyed. Her
/// condition falls below any doctrine's breaking point while she stays
/// alive, mobile, and reapable by nothing — exactly what a withdraw test
/// needs its casualties to be.
pub fn maul(reg: &DataRegistry, state: &mut BattleState, unit: UnitId) {
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
pub fn strike_down(state: &mut BattleState, unit: UnitId) {
    let victim = state.unit_mut(unit).expect("she was alive");
    victim.destroy();
}

/// Play a round out so contact is computed and the next planning phase opens.
pub fn settle(reg: &DataRegistry, state: &mut BattleState) -> Vec<BattleEvent> {
    commit_all(reg, state);
    state.resolve_round(reg)
}

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
pub fn marching_under_fire(
    reg: &DataRegistry,
    latitude: Latitude,
    seed: u64,
) -> (BattleState, UnitId) {
    // Woods flank the road as far as x = 12; the gun sits at 13 on bare
    // ground, with nothing better within a bound of it.
    //
    // She starts at 4 rather than 5, and the one hex is the whole margin this
    // stage has. It has to bruise and not kill, and once suppression joined
    // the currency it killed at 5 — three 88 rounds a round at a crew whose
    // nerve is now in the ledger beside her plate. Standing further back is
    // the only lever and it runs out at once: at 2 the tank destroyer cannot
    // see her at all and the fire order this stage rests on is refused with
    // `TargetNotSpotted`. The band is {3, 4}, bounded by lethality above and
    // by sight below.
    //
    // 4 rather than 3 because two of the five callers were not going through
    // `seen(..)`, so the gun's sight of her was a die roll that nearly always
    // came in at eight hexes and stopped coming in at ten. They do now — the
    // rule CLAUDE.md already states for any stage that needs two crews in
    // plain sight — which is what buys the hex. Anybody who needs more should
    // move the gun back rather than the crew.
    let flank = format!("{}{}", "f".repeat(12), "g".repeat(18));
    let road = "g".repeat(30);
    let mut state = two_side_battle(
        reg,
        &[&flank, &road, &flank],
        vec![
            unit_at([4, 1], 0, "medium_tank", "Ordered"),
            unit_at([13, 1], 1, "tank_destroyer", "Gun Tank"),
        ],
        seed,
    );
    let crew = UnitId(0);
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the stage needs her to see the danger she is being asked to drive past"
    );
    // The gun is *ordered* to watch the road, and that is staging rather than
    // decoration. A crew with a fire order is on deliberate overwatch and the
    // battle drill will not touch her (see CLAUDE.md under Defiance); without
    // the order the gun is an idle crew who gets threatened as the tank closes
    // and takes cover in the woods on her own initiative, which leaves
    // `second_round_plan` with no threat left to stage. That cost this test two
    // reseeds across the resolver-depth arc before the cause was clear: the
    // fragility was never the seed, it was a stage whose furniture could walk
    // away.
    state
        .apply(
            reg,
            &Order::SetFire {
                unit: UnitId(1),
                fire: FireIntent::Target {
                    target: crew,
                    weapon: 0,
                },
            },
        )
        .expect("a gun may always be told what to watch");
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
    assert_eq!(
        state.unit(crew).unwrap().march().map(|m| m.latitude),
        Some(latitude)
    );
    (state, crew)
}

/// The ground she is sent to, well east of the gun watching the road.
pub const MARCH_TO: tactics_core::Hex = tactics_core::Hex::new(28, 1);

pub fn executor_only_side(reg: &DataRegistry, seed: u64) -> AiDriver {
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

/// Pull the 75's teeth without pulling its threat: one point of effect
/// budget still prices the shot above zero — she is being shot at by
/// something that CAN hurt her, which is what `threatened` and the drill
/// read — but a penetration wounds one cadet or dings one module instead of
/// savaging the vehicle. The clock and drill tests need their subjects
/// alive, mobile and unbroken long enough to watch them decide.
pub fn soften(reg: &mut DataRegistry) {
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

/// A long road with a piece of ground worth holding at the far end, a
/// platoon and an empty taxi at the near end, and an enemy parked on the
/// objective behind a forest curtain so that nobody is spotted at the bell.
///
/// The distances are the whole point: on foot the objective is a march of
/// twenty-odd rounds and by taxi it is four or five, which is the gap a taxi
/// run exists to close. `walk` shortens it so the same stage can ask the
/// opposite question.
pub fn taxi_run_stage(reg: &DataRegistry, walk: i32, seed: u64) -> BattleState {
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
    .expect("the staged placements are content the base mod ships")
}

/// Every order one side's planner issues in a round, in the order it issues
/// them. The planner is only reachable through the driver, which is the right
/// seam to test at anyway: what matters is the order that reaches the engine.
pub fn orders_planned(
    reg: &DataRegistry,
    state: &mut BattleState,
    side: u8,
    seed: u64,
) -> Vec<Order> {
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

/// One medium tank per side on open ground, crewed by name, so a wound
/// carried in from a previous battle has somewhere to show.
pub fn crewed_stage(reg: &DataRegistry, crew: &[&str]) -> (BattleState, UnitId) {
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
    )
    .expect("the staged placements are content the base mod ships");
    (state, UnitId(0))
}

/// The pressure facts of a round, read off an event the way `apply_pressure`
/// reads them. Shared by the tests below so that a test cannot accidentally
/// check the ladder against its own restatement of the lookup.
pub fn felt(reg: &DataRegistry, ammo: &Option<String>, small_arms: bool) -> RoundPressure {
    RoundPressure {
        small_arms,
        suppression: ammo
            .as_ref()
            .and_then(|id| reg.ammo(id))
            .map(|a| a.suppression)
            .unwrap_or(0),
    }
}
