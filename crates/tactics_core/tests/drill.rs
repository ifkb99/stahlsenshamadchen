//! What a crew does this instant: the mid-round drill, latitude over her
//! own judgement, and the hardware and habits that make a formation act
//! as one.
//!
//! The battle drill and the crew's loop are the same mid-round reflex from
//! two ends of the file; latitude is the one knob that tells that reflex
//! whether an order outranks it. Fighting as one, movement to contact
//! versus the deliberate attack, radios as hardware, the commander's pulse,
//! detachment and base of fire are the mechanics that reflex has to work
//! through — a formation's shared clock, a broken radio, an order that
//! outranks the standing mission. Ten sections, all about the second a
//! crew decides what to do next rather than the round she decided where to
//! be.
//!
//! - the battle drill
//! - latitude: an order the crew may not set aside
//! - fighting as one: the formation acts together
//! - movement to contact versus the deliberate attack
//! - radios as hardware
//! - the commander's pulse
//! - detachment: a personal order outranks the standing mission
//! - base of fire
//! - the crew's clock
//! - the crew's loop: the mid-round drill

use tactics_core::ai::{AiConfig, AiDriver, Evaluator, make_battle_planner};
use tactics_core::battle::{
    BattleState, Event as BattleEvent, FireIntent, FormationId, Latitude, Mission, Order,
    PersonalOrder, SideState, UnitId,
};
use tactics_core::data::DataRegistry;
use tactics_core::map::Battlefield;

mod common;
use common::{
    MARCH_TO, calm, command_rules, commit_all, duel, executor_only_side, formation_named,
    marching_under_fire, maul, play_round, registry, registry_wireless, seen, soften, strike_down,
    strip_radios, two_side_battle, unit_at,
};

// --- the battle drill ------------------------------------------------------

/// Open grass with a forest stand to the west: her, unordered and outside
/// any formation, and a gun tank well inside range to the east.
fn drill_stage(reg: &DataRegistry, enemy_at: i32, seed: u64) -> BattleState {
    let row = format!("gggf{}", "g".repeat(26));
    two_side_battle(
        reg,
        &[&row, &row, &row],
        vec![
            unit_at([5, 1], 0, "recon_car", "Unordered"),
            unit_at([enemy_at, 1], 1, "medium_tank", "Gun Tank"),
        ],
        seed,
    )
}

#[test]
fn a_crew_under_fire_takes_cover_instead_of_waiting_for_orders() {
    // The battle drill: nobody under fire waits for permission to survive.
    // An unordered unit the delegation layer used to park with a bare
    // hold-fire now returns fire and seeks cover when something that can
    // hit her is in sight — and only then; explicit orders still outrank
    // the drill because they mark her planned before it is consulted.
    let reg = registry_wireless();
    let mut state = drill_stage(&reg, 9, 51);
    let (crew, enemy) = (UnitId(0), UnitId(1));
    assert!(
        state.fog.side(0).spotted.contains(&enemy),
        "the stage needs her to see the danger"
    );

    let mut ai = AiDriver::new();
    ai.insert(
        0,
        Box::new(tactics_core::ai::SideCommand::executor_only(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            51,
            &reg,
        )),
    );
    ai.plan_round(&reg, &mut state);

    let unit = state.unit(crew).unwrap();
    assert!(unit.planned, "the drill plans her");
    let dest = unit.planned_destination();
    assert_ne!(dest, unit.pos, "she does not sit in the open");
    assert_eq!(
        state.terrain_at(dest),
        Some("forest"),
        "she makes for the cover, not merely anywhere"
    );
}

/// Taking cover is one answer, whoever asks it: the planning table's drill
/// for a crew nobody has ordered sends her where the engine's own reflex
/// would, [`drill_destination`](tactics_core::battle::drill_destination).
///
/// It used to run the whole evaluator under a synthetic `drill` doctrine —
/// a second model of the same reaction — and on open ground the two parted:
/// with the gun at column 9 of this stage the old drill sent her to (3,2)
/// and the reflex's rule to (2,1), so a crew the planning table had placed
/// could be moved again by the reflex a tick after she arrived. Found by
/// scanning stage layouts for where the two disagreed; the forest in
/// `drill_stage` is one where they happened to agree.
#[test]
fn the_planning_table_takes_cover_where_the_reflex_would() {
    let reg = registry_wireless();
    let row = format!("f{}", "g".repeat(29));
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([5, 1], 0, "recon_car", "Unordered"),
            unit_at([9, 1], 1, "medium_tank", "Gun Tank"),
        ],
        51,
    );
    let crew = UnitId(0);
    let threats = tactics_core::battle::threats(&reg, &state, crew);
    assert!(!threats.is_empty(), "the stage needs a gun bearing on her");
    let reflex = tactics_core::battle::drill_destination(&reg, &state, crew, &threats)
        .expect("there is quieter ground in reach");

    executor_only_side(&reg, 51).plan_round(&reg, &mut state);
    let unit = state.unit(crew).unwrap();
    assert!(unit.planned, "the drill plans her");
    assert_eq!(
        unit.planned_destination(),
        reflex,
        "the planning table sends her where the reflex would"
    );
}

#[test]
fn an_idle_crew_out_of_danger_stays_put() {
    // The other half of the bargain: the drill is survival, not initiative.
    // Nothing spotted that can reach her means the parking lot stays parked,
    // exactly as it did before the drill existed.
    let reg = registry_wireless();
    let mut state = drill_stage(&reg, 28, 52);
    let (crew, enemy) = (UnitId(0), UnitId(1));
    assert!(
        !state.fog.side(0).spotted.contains(&enemy),
        "the stage needs the danger out of sight"
    );
    let parked = state.unit(crew).unwrap().pos;

    let mut ai = AiDriver::new();
    ai.insert(
        0,
        Box::new(tactics_core::ai::SideCommand::executor_only(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            52,
            &reg,
        )),
    );
    ai.plan_round(&reg, &mut state);

    let unit = state.unit(crew).unwrap();
    assert!(unit.planned && unit.intent.path.is_empty());
    assert_eq!(unit.pos, parked);
}

// --- latitude: an order the crew may not set aside --------------------------

/// Fight the opening round, then plan the next one and report where she
/// stands, where she is being sent from there, and whether that is cover.
///
/// The second round is the one that matters and it took a failing test to
/// see why: `radio()` marches her itself the moment the order lands, so on
/// the round she is ordered she is already planned and no planner is
/// consulted at all. The drill can only ever preempt a march that is
/// *already under way* — which is exactly the case the player was
/// complaining about, the tank that sets off and then never arrives.
fn second_round_plan(
    reg: &DataRegistry,
    mut state: BattleState,
    crew: UnitId,
    seed: u64,
) -> (tactics_core::Hex, tactics_core::Hex, bool) {
    executor_only_side(reg, seed).plan_round(reg, &mut state);
    let _ = state.apply(reg, &Order::Commit { side: 1 });
    state.resolve_round(reg);
    let unit = state.unit(crew).expect("she survives being shot at");
    assert!(unit.alive(), "the stage is meant to bruise, not to kill");
    assert!(
        state.unit(UnitId(1)).is_some_and(|e| e.alive()),
        "and the gun watching the road has to still be watching it, or there \
         is no threat left for the drill to answer"
    );
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "and she has to still be able to see it"
    );
    let from = unit.pos;
    executor_only_side(reg, seed).plan_round(reg, &mut state);
    let to = state.unit(crew).unwrap().planned_destination();
    let cover = state.terrain_at(to) == Some("forest");
    (from, to, cover)
}

#[test]
fn a_binding_march_presses_on_where_an_ordinary_one_takes_cover() {
    // The rule this chunk exists for, and its additivity twin, in one test
    // because they are one comparison: same crew, same gun, same ground,
    // same seed, and the *only* difference is whether her commander said she
    // meant it.
    //
    // Stated as a comparison rather than against a named tile on purpose.
    // What is being defended is that latitude changes what she does, and in
    // which direction — not which particular hedge the drill happens to
    // like, which is a tuning detail that should be free to move without
    // failing this.
    // The seed is staging, not the rule. It is 4 rather than the original 62
    // because the resolver-depth arc changed what a shot is worth twice over;
    // `marching_under_fire` now also *orders* the gun to watch the road,
    // which is the fix that should stop this from recurring — the fragility
    // was never the seed, it was a stage whose furniture could walk away.
    // What the seed still decides is whether she lives through the opening
    // round, and if that has to move again, scan for another rather than
    // weakening what is asserted below.
    let reg = calm(seen(registry_wireless()));
    // Hunted rather than written down, for the reason the currency test's
    // twin is: a `const 4` chosen because it bruised without killing stopped
    // doing either the moment loaders started reaching their guns.
    let survives = |seed: u64| {
        let (mut state, crew) = marching_under_fire(&reg, Latitude::Delegated, seed);
        executor_only_side(&reg, seed).plan_round(&reg, &mut state);
        let _ = state.apply(&reg, &Order::Commit { side: 1 });
        state.resolve_round(&reg);
        state.unit(crew).is_some_and(|u| u.alive())
            && state.unit(UnitId(1)).is_some_and(|e| e.alive())
    };
    let seed = (0u64..40)
        .find(|seed| survives(*seed))
        .expect("some seed leaves both of them on the field");
    let (delegated_from, delegated_to, delegated_took_cover) = {
        let (state, crew) = marching_under_fire(&reg, Latitude::Delegated, seed);
        second_round_plan(&reg, state, crew, seed)
    };
    let (binding_from, binding_to, _) = {
        let (state, crew) = marching_under_fire(&reg, Latitude::Binding, seed);
        second_round_plan(&reg, state, crew, seed)
    };

    // Latitude is read when the *executor* plans, and on the opening round
    // the order plans her itself, so the two runs must be identical up to
    // that point. If this ever fails, latitude has leaked somewhere it does
    // not belong.
    assert_eq!(
        delegated_from, binding_from,
        "the two runs are the same battle until the second round's planning"
    );

    assert!(
        MARCH_TO.distance_to(binding_to) < MARCH_TO.distance_to(delegated_to),
        "told she is meant, she keeps driving at ground she has been shown is \
         dangerous; told to use her judgment, she does not: \
         from {binding_from:?}, binding -> {binding_to:?}, delegated -> {delegated_to:?}"
    );
    assert!(
        MARCH_TO.distance_to(binding_to) < MARCH_TO.distance_to(binding_from),
        "and pressing on is progress toward the ordered ground, not merely \
         a different tile: {binding_from:?} -> {binding_to:?}"
    );
    // Not merely "somewhere else": the crew who was given her judgment used
    // it for the thing the drill is for.
    assert!(
        delegated_took_cover,
        "the delegated crew breaks off into the woods: {delegated_from:?} -> {delegated_to:?}"
    );
}

#[test]
fn a_recall_forgets_that_she_was_pressed_on() {
    // Latitude belongs to an order, not to a crew: take the order back and
    // the insistence goes with it, or the next thing she is told inherits an
    // urgency nobody attached to it.
    // `seen`, because this stage rests on the gun having found her —
    // `marching_under_fire` gives it a fire order and a fire order at an
    // unspotted target is refused. It got away without it while the crew
    // stood eight hexes off and the roll nearly always came in; she stands
    // further back now, and a test about latitude must not also be a test
    // of whether anybody happened to find anybody.
    let reg = seen(registry_wireless());
    let (mut state, crew) = marching_under_fire(&reg, Latitude::Binding, 62);
    state
        .apply(&reg, &Order::ClearIntent { unit: crew })
        .expect("a recall is always sayable");
    let unit = state.unit(crew).unwrap();
    assert_eq!(unit.march(), None, "the march is off");
    assert!(
        !unit.detached(),
        "a recall takes back the whole order, and so the insistence behind it"
    );
}

#[test]
fn an_arrival_keeps_the_insistence_she_arrived_under() {
    // "Get there, I mean it" is not "get there and then use your judgment".
    // The first draft of `PersonalOrder` dropped the latitude at the last
    // hex — `Holding` carried none — so a binding crew who reached her
    // ground was, one tick later, an ordinary crew under fire whom the
    // reflex backed straight off it. The designer's ruling is that the same
    // gate that keeps her on the road keeps her on the ground, and the
    // shape that makes that true is the hold carrying the march's latitude.
    //
    // The delegated twin is the additivity half: a crew nobody insisted on
    // arrives yielding, as every crew in this engine always has.
    //
    // Nobody on the stage can threaten her — a scout section's rifles are
    // worth nothing against a medium tank in either currency — so neither
    // drill has any reason to divert either twin, and what is being read is
    // the arrival alone.
    //
    // And nobody near misses: the watcher has to live through the round
    // for there to be a next one, and the arrival is read when the round
    // turns. Once a belt going past frightened a section, the tank's own
    // opportunity fire broke her in round one and the battle ended with
    // the march still standing.
    // Nor a price on closing her up: the section's rifles cannot hurt her,
    // but since 2026-09-23 they can button her up (`balance.buttoning_worth`),
    // which would make them a threat and the drill the thing being tested.
    let mut reg = seen(registry_wireless());
    reg.balance.buttoning_worth = 0;
    reg.morale.near_miss_percent = 0;
    reg.morale.targeted = 0;
    let arrived = |latitude: Latitude| {
        let row = "g".repeat(10);
        let mut state = two_side_battle(
            &reg,
            &[&row, &row, &row],
            vec![
                unit_at([3, 1], 0, "medium_tank", "Ordered"),
                unit_at([0, 1], 1, "scout_section", "Watching"),
            ],
            303,
        );
        let watcher = UnitId(0);
        assert!(
            !tactics_core::battle::threatened(&reg, &state, watcher),
            "the stage must put nothing on her, or the drill is what is being tested"
        );
        let next_door = tactics_core::offset_to_hex(3, 0);
        state
            .apply(
                &reg,
                &Order::Radio {
                    unit: watcher,
                    to: Some(next_door),
                    fire: None,
                    latitude,
                },
            )
            .expect("one hex is a march");
        play_round(&reg, &mut state);
        let unit = state
            .unit(watcher)
            .expect("she is a hex further on, not gone");
        assert_eq!(unit.pos, next_door, "she got there");
        (unit.orders, unit.yields_to_drill())
    };
    assert_eq!(
        arrived(Latitude::Binding),
        (
            Some(PersonalOrder::Holding {
                latitude: Latitude::Binding
            }),
            false
        ),
        "she holds the ground she was sent to, and still means it"
    );
    assert_eq!(
        arrived(Latitude::Delegated),
        (
            Some(PersonalOrder::Holding {
                latitude: Latitude::Delegated
            }),
            true
        ),
        "and a crew nobody insisted on arrives as every crew always has"
    );
}

#[test]
fn an_order_about_her_gun_says_nothing_about_her_march() {
    // The two halves of a radioed order are independent, and latitude rides
    // with the *route*. Telling a crew who is pressing on what to shoot at
    // must not quietly relax the march she is already under.
    // `seen`, because this stage rests on the gun having found her —
    // `marching_under_fire` gives it a fire order and a fire order at an
    // unspotted target is refused. It got away without it while the crew
    // stood eight hexes off and the roll nearly always came in; she stands
    // further back now, and a test about latitude must not also be a test
    // of whether anybody happened to find anybody.
    let reg = seen(registry_wireless());
    let (mut state, crew) = marching_under_fire(&reg, Latitude::Binding, 63);
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: None,
                fire: Some(FireIntent::Hold),
                latitude: Latitude::Delegated,
            },
        )
        .expect("hold fire is always sayable");
    assert_eq!(
        state.unit(crew).unwrap().march().map(|m| m.latitude),
        Some(Latitude::Binding),
        "she is still pressing on"
    );
}

// --- fighting as one: the formation acts together --------------------------

#[test]
fn a_formation_keeps_its_interval_and_its_sight_lines() {
    // The spacing band, measured directly: with every other term zeroed,
    // the mass term should prefer the supported interval over hugging
    // (one shell, one vehicle), over straggling (out of support), and over
    // standing near a friend who cannot see you (near but masked is not
    // mutual support).
    let reg = registry_wireless();
    let row = {
        let mut r: Vec<char> = std::iter::repeat_n('g', 46).collect();
        r[4] = 'f';
        r.into_iter().collect::<String>()
    };
    let state = {
        let mut s = two_side_battle(
            &reg,
            &[&row, &row, &row],
            vec![
                unit_at([15, 1], 0, "medium_tank", "Scored"),
                unit_at([6, 1], 0, "medium_tank", "Friend"),
                unit_at([45, 1], 1, "medium_tank", "Far Foe"),
            ],
            9,
        );
        assert!(
            s.fog.side(0).spotted.is_empty(),
            "the stage needs no enemy in sight"
        );
        // The friend is already under orders to stand where she stands, so
        // the band reads her planned destination.
        s.unit_mut(UnitId(1)).unwrap().intent.path = vec![tactics_core::offset_to_hex(6, 1)];
        s
    };
    let evaluator = Evaluator::new(tactics_core::data::DoctrineDef {
        id: "band_probe".into(),
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
        teaches: Vec::new(),
    });
    let score = |col: i32| {
        evaluator
            .score_tile(&reg, &state, UnitId(0), tactics_core::offset_to_hex(col, 1))
            .score
    };

    let hugging = score(7);
    let interval = score(9);
    let straggling = score(14);
    let masked = score(3);
    assert!(
        interval > hugging,
        "the interval beats hugging: {interval} vs {hugging}"
    );
    assert!(
        interval > straggling,
        "the interval beats straggling: {interval} vs {straggling}"
    );
    assert!(
        interval > masked,
        "a friend who cannot see you is not support: {interval} vs {masked}"
    );
}

/// A two-car section under one leader on a long road, ordered east, with a
/// gun tank visible far beyond — inside their eyes, outside everyone's guns,
/// so contact exists and nobody dies while the section moves.
fn bounding_stage(
    reg: &DataRegistry,
    enemy_at: i32,
    doctrine: Option<&str>,
    seed: u64,
) -> BattleState {
    let row = "g".repeat(40);
    let mut formation = serde_json::json!({ "id": "section", "name": "The Section", "side": 0 });
    if let Some(doctrine) = doctrine {
        formation["doctrine"] = serde_json::json!(doctrine);
    }
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "bounding_stage",
        "palette": { "g": "grass" },
        "rows": [row.clone(), row.clone(), row],
        "formations": [ formation ],
    }))
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
    let mut lead = unit_at([2, 1], 0, "recon_car", "Lead");
    lead.formation = Some("section".into());
    lead.leads = true;
    let mut wing = unit_at([2, 2], 0, "recon_car", "Wing");
    wing.formation = Some("section".into());
    let enemy = unit_at([enemy_at, 1], 1, "medium_tank", "Gun Tank");
    let placements = vec![lead, wing, enemy];
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

/// Plan one executor-only round for side 0 and say who moved.
fn bound_once(reg: &DataRegistry, state: &mut BattleState, seed: u64) -> Vec<bool> {
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
    ai.plan_round(reg, state);
    [UnitId(0), UnitId(1)]
        .iter()
        .map(|id| !state.unit(*id).unwrap().intent.path.is_empty())
        .collect()
}

#[test]
fn a_section_in_contact_bounds_by_element() {
    // Fire and movement, with WEGO rounds as the bounds: in contact and
    // under a movement mission, one element advances while the other stands
    // with guns up, and the elements swap every round.
    let reg = registry_wireless();
    let mut state = bounding_stage(&reg, 16, None, 61);
    assert!(
        !state.fog.side(0).spotted.is_empty(),
        "the stage needs contact"
    );
    let section = formation_named(&state, "section");
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: section,
                mission: Mission::Advance {
                    to: tactics_core::offset_to_hex(30, 1),
                },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();

    let first = bound_once(&reg, &mut state, 61);
    assert_eq!(
        first.iter().filter(|moved| **moved).count(),
        1,
        "one element bounds while the other overwatches: {first:?}"
    );
    let _ = state.apply(&reg, &Order::Commit { side: 1 });
    state.resolve_round(&reg);

    let second = bound_once(&reg, &mut state, 61);
    assert_eq!(
        second.iter().filter(|moved| **moved).count(),
        1,
        "and again next round: {second:?}"
    );
    assert_ne!(first, second, "with the elements swapped");
}

#[test]
fn a_section_out_of_contact_travels() {
    // No contact, no ceremony: everyone moves, which is exactly the game
    // before bounding existed.
    let reg = registry_wireless();
    let mut state = bounding_stage(&reg, 39, None, 62);
    assert!(
        state.fog.side(0).spotted.is_empty(),
        "the stage needs the danger out of sight"
    );
    let section = formation_named(&state, "section");
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: section,
                mission: Mission::Advance {
                    to: tactics_core::offset_to_hex(30, 1),
                },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    let moved = bound_once(&reg, &mut state, 62);
    assert_eq!(moved, vec![true, true], "traveling, not bounding");
}

#[test]
fn an_aggressive_doctrine_travels_in_overwatch() {
    // Traveling versus bounding is doctrine's call: massed armour trades
    // security for tempo and keeps everyone moving even in contact — which
    // is both the textbook and what the harness demanded, since universal
    // bounding cost the aggressive doctrine half its wins.
    let reg = registry_wireless();
    let mut state = bounding_stage(&reg, 16, Some("massed_armor"), 63);
    assert!(!state.fog.side(0).spotted.is_empty());
    let section = formation_named(&state, "section");
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: section,
                mission: Mission::Advance {
                    to: tactics_core::offset_to_hex(30, 1),
                },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    let moved = bound_once(&reg, &mut state, 63);
    assert_eq!(moved, vec![true, true], "tempo over ceremony");
}

// --- movement to contact versus the deliberate attack ----------------------

/// A scout in the woods with a gun on her flank, and open ground between her
/// and the hex she has been ordered to take.
///
/// Everything about the stage exists to isolate the mission term. She is a
/// `recon_car` and the enemy a `tank_destroyer` seven hexes off her flank:
/// he outranges her badly, so she is genuinely under fire (his 88 reaches
/// sixteen hexes and she can see him for twenty) while her own machine gun
/// reaches nobody from any tile compared — the attack term is zero on both
/// and cannot decide anything. Seven hexes is also outside the six-hex band
/// the threat term prices, so that is zero on both too. What is left arguing
/// against walking is the wood she is sitting in and the crew's own appetite
/// to keep the contact close, and what argues for it is the mission — which
/// is exactly the argument the damping settles.
fn contact_stage(reg: &DataRegistry, seed: u64) -> (BattleState, FormationId) {
    let open = "g".repeat(40);
    let mut wood: Vec<char> = open.chars().collect();
    wood[COVER.0 as usize] = 'f';
    let wood: String = wood.into_iter().collect();
    let mut rows: Vec<String> = std::iter::repeat_n(open, 15).collect();
    rows[7] = wood;
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "contact_stage",
        "palette": { "g": "grass", "f": "forest" },
        "rows": rows,
        "formations": [ { "id": "section", "name": "The Section", "side": 0 } ],
    }))
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
    let mut scout = unit_at([COVER.0, COVER.1], 0, "recon_car", "Scout");
    scout.formation = Some("section".into());
    scout.leads = true;
    let placements = vec![scout, unit_at([GUN.0, GUN.1], 1, "tank_destroyer", "Gun")];
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
    let section = formation_named(&state, "section");
    (state, section)
}

/// Where she stands, where she was told to be, where the gun is, and the
/// ground between: the tiles every test below compares.
///
/// `FORWARD` is twenty-two hexes along the lane — far enough that the mission
/// has something to say about it, and still inside the gun's envelope, which
/// is what makes the comparison a comparison at all.
///
/// **This stage has been lengthened twice for the same reason, and the reason
/// is worth stating once.** The two orders differ in exactly one thing, the
/// contact damping, so the test asks whether the mission's slope from `COVER`
/// to `FORWARD` is worth more than the wood at `COVER` is. Every time the
/// threat term grows, the wood grows with it and the march has to get longer
/// to keep up.
///
/// It was seven hexes until Phase 2 taught the threat term to read the
/// candidate tile — seven was chosen when the old six-hex gate made danger
/// *zero* at both tiles, so the wood cost the advance only its cover bonus —
/// and ten afterwards, when the wood became worth about 1.5 substance points.
/// Cadence made a round of the tank destroyer's fire three shots instead of
/// one, so the wood is now worth about 3.4 points of avoided fire a round and
/// ten hexes of slope no longer pays for it: the assault fell 0.54 short.
///
/// Twenty-two hexes is the repair, and it needed the gun moved as well as the
/// tile. From the old firing position at (10, 0) the envelope along this lane
/// ran out at x = 22, so the only forward tiles long enough sat exactly at
/// the 88's maximum range — a stage balanced on a cliff, since one hex
/// further is *no fire at all* and both orders press on for a reason that has
/// nothing to do with either of them. Moved to (10, 6) the gun covers the
/// whole march with a hex to spare: `COVER` is 8 hexes from it and `FORWARD`
/// 15, against a reach of 16. Margins on this stage: the advance halts by
/// 2.97 and the assault presses on by 1.98, against 3.24 and −0.54 before.
/// `the_stage_keeps_the_forward_tile_under_the_gun` pins the envelope.
const COVER: (i32, i32) = (2, 7);

const FORWARD: (i32, i32) = (24, 7);

const GUN: (i32, i32) = (10, 6);

/// Score both tiles for a scout under `mission`, in the order (cover,
/// forward). The balanced doctrine on purpose: it is what the player's own
/// delegated formations fight under, so this is the case the playtest was
/// complaining about.
fn contact_scores(
    reg: &DataRegistry,
    state: &mut BattleState,
    section: FormationId,
    mission: Mission,
) -> (f32, f32) {
    state
        .apply(
            reg,
            &Order::SetMission {
                formation: section,
                mission,
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("the lane is on the map");
    let evaluator = Evaluator::new(tactics_core::data::DoctrineDef::default());
    let score = |at: (i32, i32)| {
        evaluator
            .score_tile(
                reg,
                state,
                UnitId(0),
                tactics_core::offset_to_hex(at.0, at.1),
            )
            .score
    };
    (score(COVER), score(FORWARD))
}

/// What the forward tile is worth to a scout ordered onto it, under one
/// doctrine at one latitude. Everything else about the calls a test makes is
/// identical, so the difference between two of them is the mission term and
/// nothing else.
fn ordered_pull(
    reg: &DataRegistry,
    doctrine: &tactics_core::data::DoctrineDef,
    latitude: Latitude,
) -> f32 {
    let (mut state, section) = contact_stage(reg, 4);
    let forward = tactics_core::offset_to_hex(FORWARD.0, FORWARD.1);
    state
        .apply(
            reg,
            &Order::SetMission {
                formation: section,
                mission: Mission::Advance { to: forward },
                latitude,
            },
        )
        .expect("the lane is on the map");
    Evaluator::new(doctrine.clone())
        .score_tile(reg, &state, UnitId(0), forward)
        .score
}

#[test]
fn a_binding_mission_is_not_discounted_by_a_loose_doctrine() {
    // Step 1 part 3 of DIRECTION.md, and the complaint it answers: a
    // player's order was quietly worth less because of who she gave it to.
    // Elastic defence devolves (delegation 0.7), so it read "take that
    // ground" at four fifths of face value; massed armour does not (0.3) and
    // read the same sentence at 1.2. Nobody issues an order meaning four
    // fifths of it.
    //
    // Fought out on one doctrine with only `delegation` moved, because the
    // score of a tile is a whole doctrine's opinion of it — cover, threat,
    // the shot available — and comparing two *different* doctrines' totals
    // would be comparing everything except the thing under test.
    //
    // Wireless on purpose: with a command block the order is still in the
    // air when the tile is scored, and a mission nobody has heard yet has no
    // latitude to read. What is under test is the executor, not the wire.
    let reg = registry_wireless();
    let base = reg
        .doctrine("elastic_defense")
        .cloned()
        .expect("base doctrine");
    let loose = tactics_core::data::DoctrineDef {
        delegation: 0.7,
        ..base.clone()
    };
    let neutral = tactics_core::data::DoctrineDef {
        delegation: 0.5,
        ..base.clone()
    };
    let tight = tactics_core::data::DoctrineDef {
        delegation: 0.3,
        ..base
    };

    let loose_delegated = ordered_pull(&reg, &loose, Latitude::Delegated);
    let loose_binding = ordered_pull(&reg, &loose, Latitude::Binding);
    assert!(
        loose_binding > loose_delegated,
        "insisting has to reach a formation that would otherwise have used its \
         own judgment: delegated {loose_delegated}, binding {loose_binding}"
    );

    // How far it reaches, exactly: to the letter of the order and no
    // further. A binding order read by a devolving doctrine is worth what an
    // ordinary order read by a neutral one is worth — it raises a floor, it
    // does not turn every commander into a martinet.
    assert_eq!(
        loose_binding,
        ordered_pull(&reg, &neutral, Latitude::Delegated),
        "insisting should buy the letter of the order, not more than it"
    );

    // And the other half of the rule, which is what keeps "I mean it"
    // honest: `delegation` may make a subordinate *more* literal than she
    // was asked to be, never less. A doctrine already holding to the letter
    // hears nothing new in being told it twice.
    assert_eq!(
        ordered_pull(&reg, &tight, Latitude::Binding),
        ordered_pull(&reg, &tight, Latitude::Delegated),
        "a formation that was already going to follow the letter of it must \
         not be pulled harder for being insisted on"
    );
}

#[test]
fn an_order_held_on_the_wire_arrives_as_hard_as_it_was_meant() {
    // Latitude travels with the mission rather than being applied when it is
    // sent, for the same reason `WaitingOrders` carries a unit's: an order
    // that waits two ticks for a signaller has to land meaning what the
    // commander meant, not what she happens to mean by the time it lands.
    let mut reg = registry();
    reg.command = Some(command_rules(999, true, 2));
    let mut state = BattleState::from_map(&reg, "river_crossing", 5).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let bridge = state.scenario.objectives()[0].anchor();

    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: bridge },
                latitude: Latitude::Binding,
            },
        )
        .expect("the bridge is on the map");
    assert_eq!(
        state.formations()[armor.index()]
            .incoming
            .as_ref()
            .map(|(c, _)| c.latitude()),
        Some(Latitude::Binding),
        "the insistence is in the envelope, not left behind at headquarters"
    );

    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    state.step_tick(&reg);
    assert_eq!(
        state.formations()[armor.index()].latitude,
        Latitude::Binding,
        "and it is still there when the order lands"
    );
}

#[test]
fn a_movement_to_contact_pauses_under_fire_and_resumes_after() {
    // What `Advance` means, and the playtest that forced it to mean this: a
    // delegated advance drove through effective fire to the hex it had been
    // given and was gone by round three. A movement to contact halts and
    // fights when it is fired on — the mission does not outrank the drill —
    // so while the gun is on her the tree line she is sitting in outscores
    // the ground she was told to take.
    //
    // Nothing about that is latched, which is the other half of the rule:
    // the damping is a scale on the mission term, so the moment the gun is
    // dead or lost the full pull is back with no state for anybody to clear.
    let reg = registry_wireless();
    let (mut state, section) = contact_stage(&reg, 71);
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the stage needs her to know she is being shot at"
    );

    let lane = Mission::Advance {
        to: tactics_core::offset_to_hex(38, 7),
    };
    let (cover, forward) = contact_scores(&reg, &mut state, section, lane.clone());
    assert!(
        cover > forward,
        "under fire she stops and fights: cover {cover} vs forward {forward}"
    );

    // The gun is gone. Nobody re-issues anything and nothing is reset.
    strike_down(&mut state, UnitId(1));
    let (cover, forward) = contact_scores(&reg, &mut state, section, lane);
    assert!(
        forward > cover,
        "and with nothing shooting at her the march resumes on its own: \
         cover {cover} vs forward {forward}"
    );
}

/// The forward tile is under the gun, and so is the wood she is sitting in.
///
/// The guard Phase 2's own comment promised and nobody wrote, which is why
/// this stage could drift onto the edge of the 88's envelope unnoticed. Every
/// test on this stage compares two tiles *under fire*; push `FORWARD` one hex
/// past the gun's reach and the comparison silently becomes "fire against no
/// fire", both orders drive forward, and the assault test passes for a reason
/// that has nothing to do with assaults.
///
/// Asserted through the danger arithmetic rather than through a distance, so
/// it stays true if sight, cover or range ever stop agreeing with the tape
/// measure.
#[test]
fn the_stage_keeps_the_forward_tile_under_the_gun() {
    let reg = registry_wireless();
    let (state, _) = contact_stage(&reg, 71);
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "she has to have found the gun, or nothing is priced at all"
    );
    for (name, at) in [("the wood", COVER), ("the forward tile", FORWARD)] {
        let here = tactics_core::offset_to_hex(at.0, at.1);
        let under = tactics_core::battle::incoming(&reg, &state, UnitId(0), here);
        assert!(
            under.worth > 0.0,
            "{name} at {at:?} has to be inside the gun's envelope, or the two \
             tiles are not comparable: {under:?}"
        );
    }
    // ...and the wood is the safer of the two, which is what gives the
    // advance something to halt for.
    let safer = tactics_core::battle::incoming(
        &reg,
        &state,
        UnitId(0),
        tactics_core::offset_to_hex(COVER.0, COVER.1),
    );
    let exposed = tactics_core::battle::incoming(
        &reg,
        &state,
        UnitId(0),
        tactics_core::offset_to_hex(FORWARD.0, FORWARD.1),
    );
    assert!(
        safer.worth < exposed.worth,
        "the wood must be worth sitting in: {} against {}",
        safer.worth,
        exposed.worth
    );
}

#[test]
fn an_assault_presses_through_what_an_advance_pauses_for() {
    // The same crew, the same gun on her flank, the same hex to take — and
    // the two orders a commander can give for it. An advance is a movement
    // to contact and stops; an assault is the deliberate attack and does
    // not, which is the entire difference between them and is why the two
    // score identically everywhere except here.
    let reg = registry_wireless();
    let to = tactics_core::offset_to_hex(38, 7);

    let (mut state, section) = contact_stage(&reg, 71);
    let (cover, forward) = contact_scores(&reg, &mut state, section, Mission::Advance { to });
    assert!(
        cover > forward,
        "the advance pauses: cover {cover} vs forward {forward}"
    );

    let (mut state, section) = contact_stage(&reg, 71);
    let (cover, forward) = contact_scores(&reg, &mut state, section, Mission::Assault { to });
    assert!(
        forward > cover,
        "the assault presses through the same fire: cover {cover} vs forward {forward}"
    );
}

// --- radios as hardware ----------------------------------------------------

/// A leader with a transceiver and a wing with a receive-only set, ten hexes
/// out — far beyond any flag, well inside the set — with an enemy scout
/// visible only to the wing. `ridge` raises a wall of ground between them;
/// `forest` plants trees there instead.
fn hardware_stage(reg: &DataRegistry, ridge: bool, forest: bool, seed: u64) -> BattleState {
    let mut row: Vec<char> = std::iter::repeat_n('g', 30).collect();
    if forest {
        row[5] = 'f';
    }
    let row: String = row.into_iter().collect();
    let elev = if ridge {
        format!("000003{}", "0".repeat(24))
    } else {
        "0".repeat(30)
    };
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "hardware_stage",
        "palette": { "g": "grass", "f": "forest" },
        "rows": [row.clone(), row.clone(), row],
        "elevation": [elev.clone(), elev.clone(), elev],
        "formations": [ { "id": "net", "name": "The Net", "side": 0 } ],
    }))
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
    let mut lead = unit_at([0, 1], 0, "medium_tank", "Lead");
    lead.formation = Some("net".into());
    lead.leads = true;
    let mut wing = unit_at([10, 1], 0, "light_tank", "Wing");
    wing.formation = Some("net".into());
    let enemy = unit_at([21, 1], 1, "recon_car", "Prowler");
    let placements = vec![lead, wing, enemy];
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

#[test]
fn a_receive_only_tank_hears_orders_and_files_no_reports() {
    // The early-war fit, working as it did in 1941: the line tank's
    // receiver keeps her on her leader's net — orders reach her — but her
    // sightings die in her silence until a flag can carry them to somebody
    // with a set.
    let reg = registry();
    let mut state = hardware_stage(&reg, false, false, 71);
    let (lead, wing, enemy) = (UnitId(0), UnitId(1), UnitId(2));

    commit_all(&reg, &mut state);
    state.step_tick(&reg);

    let net = formation_named(&state, "net");
    assert!(
        state.formations()[net.index()].in_contact(wing),
        "ten hexes is far beyond any flag, and her receiver hears the set"
    );
    assert!(
        state.fog.side(0).spotted.contains(&enemy),
        "she sees the prowler her leader cannot"
    );
    assert!(
        !state.picture(0).iter().any(|c| c.unit == enemy),
        "and the commander learns nothing: a receiver files no reports"
    );

    // Fall back beside the leader: the flag reaches a transmitter, and the
    // sighting she is still holding goes through.
    if let Some(unit) = state.unit_mut(wing) {
        unit.pos = tactics_core::offset_to_hex(2, 1);
    }
    let events = state.step_tick(&reg);
    let still_seen = state.fog.side(0).spotted.contains(&enemy);
    assert_eq!(
        state.picture(0).iter().any(|c| c.unit == enemy),
        still_seen,
        "beside the set, whatever she still sees is reported"
    );
    if still_seen {
        assert!(
            events
                .iter()
                .any(|e| matches!(e, BattleEvent::ContactReported { unit, .. } if *unit == enemy)),
            "and said out loud"
        );
    }
    let _ = lead;
}

#[test]
fn a_hill_masks_the_radio_and_a_forest_does_not() {
    // VHF is line-of-sight-ish: the ground stands in the way, the canopy
    // does not. The same two vehicles at the same ten hexes are on the net
    // through a forest and off it behind a ridge.
    let reg = registry();
    let wing = UnitId(1);

    let mut behind_trees = hardware_stage(&reg, false, true, 72);
    commit_all(&reg, &mut behind_trees);
    behind_trees.step_tick(&reg);
    let net = formation_named(&behind_trees, "net");
    assert!(
        behind_trees.formations()[net.index()].in_contact(wing),
        "a forest does not mask a radio"
    );

    let mut behind_ridge = hardware_stage(&reg, true, false, 73);
    commit_all(&reg, &mut behind_ridge);
    behind_ridge.step_tick(&reg);
    let net = formation_named(&behind_ridge, "net");
    assert!(
        !behind_ridge.formations()[net.index()].in_contact(wing),
        "a ridge does: she is in a radio shadow"
    );
}

// --- the commander's pulse -------------------------------------------------

/// Drive one planning round for a command side and count what it assigned.
fn pulse_round(reg: &DataRegistry, state: &mut BattleState, ai: &mut AiDriver) -> usize {
    let mut assigned = 0;
    ai.plan_round_with(reg, state, |d| {
        assigned += d
            .events
            .iter()
            .filter(|e| matches!(e, BattleEvent::MissionAssigned { .. }))
            .count();
    });
    let _ = state.apply(reg, &Order::Commit { side: 0 });
    state.resolve_round(reg);
    assigned
}

fn pulsed_rules() -> tactics_core::data::CommandRules {
    let mut rules = command_rules(999, true, 0);
    // One round between reviews at every skill: she thinks every other
    // round, and what happens in between must wake her or wait. The cap
    // must rise with the base — delay() clamps to it, and a zero cap is
    // the every-round pulse regardless of base.
    rules.review.base_ticks = 1;
    rules.review.max_ticks = 3;
    rules
}

/// A quiet battlefield for watching the pulse itself: one commanded
/// formation, two pieces of ground worth holding, and the only enemy far
/// beyond anyone's eyes — so no contact can ever interrupt the clock.
fn pulse_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    let row = "g".repeat(40);
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "pulse_stage",
        "palette": { "g": "grass" },
        "rows": [row.clone(), row.clone(), row],
        "objectives": [
            { "id": "bridge", "name": "Bridge", "at": [[5, 1]], "value": 3 },
            { "id": "ford", "name": "Ford", "at": [[35, 1]], "value": 2 },
        ],
        "formations": [ { "id": "line", "name": "The Line", "side": 1 } ],
    }))
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
    let mut lead = unit_at([25, 1], 1, "medium_tank", "Lead");
    lead.formation = Some("line".into());
    lead.leads = true;
    let mut wing = unit_at([26, 2], 1, "medium_tank", "Wing");
    wing.formation = Some("line".into());
    let hermit = unit_at([39, 2], 0, "medium_tank", "Hermit");
    let placements = vec![lead, wing, hermit];
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

#[test]
fn a_commander_reviews_on_her_own_pulse() {
    // Between pulses the plan stands. The balanced doctrine retargets off
    // ground already taken — but only when she is actually reviewing, so
    // the ford assignment waits for her clock even though the bridge fell
    // in the first minute.
    let mut reg = registry();
    reg.command = Some(pulsed_rules());
    strip_radios(&mut reg);
    let mut state = pulse_stage(&reg, 81);
    assert!(state.fog.side(1).spotted.is_empty(), "a quiet field");
    let mut ai = AiDriver::new();
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            81,
            &reg,
        ),
    );

    assert!(
        pulse_round(&reg, &mut state, &mut ai) > 0,
        "round 1 assigns"
    );
    // The bridge falls to her side between pulses.
    state.objective_held[0] = Some(1);
    assert_eq!(
        pulse_round(&reg, &mut state, &mut ai),
        0,
        "round 2 is between pulses: the plan stands"
    );
    assert!(
        pulse_round(&reg, &mut state, &mut ai) > 0,
        "round 3 is her pulse, and she moves her people on"
    );
}

#[test]
fn a_breaking_formation_wakes_her_between_pulses() {
    // No commander sleeps through a formation breaking: the interrupt runs
    // the review early and the withdrawal goes out on the round the damage
    // is known, not on the next scheduled pulse.
    let mut reg = registry();
    reg.command = Some(pulsed_rules());
    strip_radios(&mut reg);
    let mut state = BattleState::from_map(&reg, "river_crossing", 82).unwrap();
    let mut ai = AiDriver::new();
    ai.insert(
        1,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: Some("elastic_defense".into()),
            },
            82,
            &reg,
        ),
    );

    let _ = pulse_round(&reg, &mut state, &mut ai);
    // Between pulses, the line is shot to pieces.
    let line = formation_named(&state, "valkyrie_line");
    for id in state.formations()[line.index()].members.clone() {
        maul(&reg, &mut state, id);
    }
    let mut withdrew = false;
    ai.plan_round_with(&reg, &mut state, |d| {
        withdrew |= d.events.iter().any(|e| {
            matches!(
                e,
                BattleEvent::MissionAssigned {
                    mission: Mission::Withdraw { .. },
                    ..
                }
            )
        });
    });
    assert!(
        withdrew,
        "the shock wakes her and the order goes out at once"
    );
}

// --- detachment: a personal order outranks the standing mission ------------

#[test]
fn a_hand_placed_vehicle_stays_where_her_commander_put_her() {
    // The second playtest's bug: a tank ordered by hand into a town tile
    // drifted back to the road at the next planning phase, because the
    // formation's Advance quietly reasserted itself. A direct order is the
    // commander taking personal charge — DETACHMENT — and the standing
    // mission stops applying to her until she is recalled or the formation
    // is given fresh orders.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 91).unwrap();
    let armor = formation_named(&state, "kuhlmann_armor");
    let bridge = state.scenario.objectives()[0].anchor();
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: bridge },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    let member = state.formations()[armor.index()].members[0];
    let start = state.unit(member).unwrap().pos;
    // Two hexes north, off the mission's axis: the commander's own spot.
    let post = start + tactics_core::Hex::new(0, -2);
    assert!(state.map.contains(post));
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: member,
                to: Some(post),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .unwrap();
    assert!(state.unit(member).unwrap().detached(), "she is detached");

    // Play the round out, then let her formation's executor fill the next
    // round's plans. She must not be marched back toward the bridge.
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);
    let held = state.unit(member).unwrap().pos;
    let mut ai = AiDriver::new();
    ai.insert(
        0,
        Box::new(tactics_core::ai::SideCommand::executor_only(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: None,
            },
            91,
            &reg,
        )),
    );
    ai.plan_round(&reg, &mut state);
    let unit = state.unit(member).unwrap();
    assert_eq!(
        unit.planned_destination(),
        held,
        "no threat, no recall: she stands where she was put"
    );

    // A fresh formation order collects everyone: detachment does not
    // survive the commander speaking to the whole formation again. (The
    // executor closed the side above, so this happens next round.)
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);
    let ford = state
        .scenario
        .objectives()
        .iter()
        .find(|o| o.id == "north_ford")
        .unwrap()
        .anchor();
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: ford },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    assert!(
        !state.unit(member).unwrap().detached(),
        "new orders for the formation reach her too"
    );
}

#[test]
fn a_personal_order_is_taken_back_whole_or_not_at_all() {
    // What used to be `detached`, `tasking` and `latitude` is one
    // `PersonalOrder`, and this is the property that shape buys. Both routes
    // out of a personal order are checked on both halves *together*, because
    // the failure they used to be able to produce was three assignments at
    // five sites with one of them forgotten: a recalled crew still driving
    // for her commander's destination, or a crew back under her formation's
    // mission still refusing the battle drill on an insistence nobody was
    // insisting on any more. Neither state is reachable now — there is no
    // third of an order left to stand.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 91).unwrap();
    let armor = formation_named(&state, "kuhlmann_armor");
    let bridge = state.scenario.objectives()[0].anchor();
    let ford = state
        .scenario
        .objectives()
        .iter()
        .find(|o| o.id == "north_ford")
        .unwrap()
        .anchor();
    let order = |to| Order::SetMission {
        formation: armor,
        mission: Mission::Advance { to },
        latitude: Latitude::Delegated,
    };
    state.apply(&reg, &order(bridge)).unwrap();
    let member = state.formations()[armor.index()].members[0];
    let post = state.unit(member).unwrap().pos + tactics_core::Hex::new(0, -2);
    assert!(state.map.contains(post));
    let send = |state: &mut BattleState| {
        state
            .apply(
                &reg,
                &Order::Radio {
                    unit: member,
                    to: Some(post),
                    fire: None,
                    latitude: Latitude::Binding,
                },
            )
            .unwrap();
        let unit = state.unit(member).unwrap();
        assert!(unit.detached(), "a direct order takes personal charge");
        assert_eq!(
            unit.march(),
            Some(tactics_core::battle::March {
                to: post,
                latitude: Latitude::Binding,
            }),
            "and the ground and the insistence arrive as one value"
        );
    };

    // The recall.
    send(&mut state);
    state
        .apply(&reg, &Order::ClearIntent { unit: member })
        .unwrap();
    let unit = state.unit(member).unwrap();
    assert!(
        !unit.detached(),
        "a recall puts her back under her formation"
    );
    assert_eq!(unit.march(), None, "and takes the march back with it");

    // And a fresh order to the whole formation, which collects everyone.
    send(&mut state);
    state.apply(&reg, &order(ford)).unwrap();
    let unit = state.unit(member).unwrap();
    assert!(!unit.detached(), "the formation was spoken to as a whole");
    assert_eq!(unit.march(), None, "so her personal errand is over too");
}

// --- base of fire ----------------------------------------------------------

#[test]
fn a_fires_formation_stands_base_of_fire_for_the_assault() {
    // Tier 3 of "fighting as one": coordination *between* formations, which
    // is the commander's business rather than an executor's. Kuhlmann's
    // reconnaissance section carries the battery, so a centralized brain
    // takes it out of the ground rotation entirely and points it at the
    // fight her armour is having — while the armour still gets the ground.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 11).unwrap();
    let mut ai = AiDriver::new();
    ai.insert(
        0,
        make_battle_planner(
            &AiConfig {
                planner: "command".into(),
                difficulty: 5,
                doctrine: Some("massed_armor".into()),
            },
            11,
            &reg,
        ),
    );
    ai.plan_round(&reg, &mut state);

    let recon = formation_named(&state, "kuhlmann_recon");
    let armor = formation_named(&state, "kuhlmann_armor");
    assert_eq!(
        state.formations()[recon.index()].mission,
        Some(Mission::Support {
            formation: "kuhlmann_armor".into()
        }),
        "the section with the howitzer in it shoots for the platoon"
    );
    assert!(
        matches!(
            state.formations()[armor.index()].mission,
            Some(Mission::Assault { .. })
        ),
        "and the platoon still gets its ground — massed armour presses for it: {:?}",
        state.formations()[armor.index()].mission
    );
}

/// One formation to be shot for and one to do the shooting, on an open road
/// with the only enemy far outside anybody's eyes — so a tile's score is the
/// mission and nothing else.
fn base_of_fire_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    let row = "g".repeat(46);
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "base_of_fire_stage",
        "palette": { "g": "grass" },
        "rows": [row.clone(), row.clone(), row],
        "formations": [
            { "id": "assault", "name": "The Assault", "side": 0 },
            { "id": "guns", "name": "The Guns", "side": 0 },
        ],
    }))
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
    let mut lead = unit_at([6, 1], 0, "medium_tank", "Lead");
    lead.formation = Some("assault".into());
    lead.leads = true;
    let mut battery = unit_at([20, 1], 0, "artillery", "Battery");
    battery.formation = Some("guns".into());
    battery.leads = true;
    let placements = vec![lead, battery, unit_at([45, 1], 1, "medium_tank", "Far Foe")];
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
        state.fog.side(0).spotted.is_empty(),
        "the stage needs no enemy in sight, or the attack term joins in"
    );
    state
}

#[test]
fn support_holds_her_at_overwatch_distance() {
    // A band, not a pull. The attack term already pays her for a tile with a
    // shot on it, so what the mission has to say is the distance: close
    // enough that her fire lands where theirs is needed, far enough that she
    // is not in the assault she is covering. Measured directly against both
    // failures — hugging the people she supports, and trailing the map
    // behind them.
    let reg = registry_wireless();
    let mut state = base_of_fire_stage(&reg, 4);
    let guns = formation_named(&state, "guns");
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: guns,
                mission: Mission::Support {
                    formation: "assault".into(),
                },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("shooting for a friendly formation that is not your own is legal");

    let evaluator = Evaluator::new(tactics_core::data::DoctrineDef {
        id: "band_probe".into(),
        name: String::new(),
        description: String::new(),
        aggression: 0.0,
        cover_value: 0.0,
        elevation_value: 0.0,
        // Zero, so the out-of-support penalty is the mass term's business
        // and not this test's: what separates these tiles must be the
        // mission. The crowding penalty inside two hexes is universal and
        // survives — it is drill, not doctrine — so hugging is penalised
        // twice over, which is the right answer arrived at honestly.
        concentration: 0.0,
        scouting: 0.0,
        objective_value: 1.0,
        indirect_appetite: 1.0,
        withdraw_threshold: 0.5,
        initiative: 0.5,
        delegation: 0.5,
        route_caution: 0.0,
        contest_aversion: 0.0,
        screening: 0.0,
        teaches: Vec::new(),
    });
    let score = |col: i32| {
        evaluator
            .score_tile(&reg, &state, UnitId(1), tactics_core::offset_to_hex(col, 1))
            .score
    };

    // The assault's leader stands at column 6, so these are one, three and
    // fourteen hexes off her.
    let hugging = score(7);
    let standoff = score(9);
    let straggling = score(20);
    assert!(
        standoff > hugging,
        "supporting distance beats riding along with them: {standoff} vs {hugging}"
    );
    assert!(
        standoff > straggling,
        "and beats trailing the map behind them: {standoff} vs {straggling}"
    );
}

#[test]
fn nobody_supports_the_enemy_or_herself() {
    // The three sentences that have no meaning, refused where both ends of
    // the order are in hand. A base of fire for a formation that is not
    // there would score zero forever and look exactly like a formation that
    // had decided to do nothing.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 3).unwrap();
    let armor = formation_named(&state, "kuhlmann_armor");
    let support = |formation: &str| Order::SetMission {
        formation: armor,
        mission: Mission::Support {
            formation: formation.into(),
        },
        latitude: tactics_core::battle::Latitude::Delegated,
    };

    assert_eq!(
        state.apply(&reg, &support("valkyrie_line")),
        Err(tactics_core::battle::OrderError::CannotSupportThat),
        "you do not stand base of fire for the people shooting at you"
    );
    assert_eq!(
        state.apply(&reg, &support("kuhlmann_armor")),
        Err(tactics_core::battle::OrderError::CannotSupportThat),
        "nor for yourself"
    );
    assert_eq!(
        state.apply(&reg, &support("ghost_battalion")),
        Err(tactics_core::battle::OrderError::NoSuchFormation),
        "nor for somebody who is not in this battle"
    );
    state
        .apply(&reg, &support("kuhlmann_recon"))
        .expect("her own side's other formation is the whole point");
}

#[test]
fn a_plan_may_end_in_support_but_not_continue_past_it() {
    // Support is a posture, not a leg: "advance to the ridge, then shoot for
    // the platoon" is a plan, and it ends there. Nothing follows a posture,
    // and the refusal is loud rather than a leg left sitting in a queue that
    // can never begin.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 8).unwrap();
    let recon = formation_named(&state, "kuhlmann_recon");
    let ridge = state.scenario.objectives()[0].anchor();
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: recon,
                mission: Mission::Advance { to: ridge },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    state
        .apply(
            &reg,
            &Order::QueueMission {
                formation: recon,
                mission: Mission::Support {
                    formation: "kuhlmann_armor".into(),
                },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("a plan may end in support");
    assert_eq!(
        state.apply(
            &reg,
            &Order::QueueMission {
                formation: recon,
                mission: Mission::Hold { at: None },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        ),
        Err(tactics_core::battle::OrderError::MissionIsTerminal),
        "and nothing follows it"
    );
}

// --- the crew's clock ------------------------------------------------------

/// One watcher on overwatch and one enemy who will walk out from behind a
/// forest wall mid-round. `gap` is how far from the wall the watcher stands.
fn ambush_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    // A tall forest curtain with the enemy tucked behind it: she cannot be
    // seen until she moves out from behind the trees.
    let rows = ["gggggggggggg", "ggggggffgggg", "gggggggggggg"];
    two_side_battle(
        reg,
        &rows,
        vec![
            unit_at([1, 1], 0, "medium_tank", "Watcher"),
            unit_at([8, 1], 1, "medium_tank", "Walker"),
        ],
        seed,
    )
}

#[test]
fn a_surprise_costs_reaction_time_whenever_it_arrives() {
    // The old gate waived the tax after tick two of every round: an enemy
    // appearing at tick seven was answered the same tick. The clock now
    // starts when SHE APPEARS, so the watcher's first shot comes exactly
    // reaction-delay ticks after first sight, wherever in the round the
    // surprise falls.
    let mut reg = registry();
    reg.command = None;
    let mut state = ambush_stage(&reg, 101);
    let (watcher, walker) = (UnitId(0), UnitId(1));
    assert!(
        !state.fog.side(0).spotted.contains(&walker),
        "the stage needs the walker hidden"
    );
    // The walker steps into the open; the watcher holds.
    let out = state.unit(walker).unwrap().pos + tactics_core::Hex::new(0, -1);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: walker,
                to: out,
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let (mut seen_at, mut fired_at) = (None, None);
    while state.resolving_tick().is_some() && fired_at.is_none() {
        let tick = state.resolving_tick().unwrap();
        for event in state.step_tick(&reg) {
            match event {
                BattleEvent::UnitSpotted {
                    unit, by_side: 0, ..
                } if unit == walker => {
                    seen_at.get_or_insert(tick);
                }
                BattleEvent::ShotFired {
                    attacker,
                    opportunity: true,
                    ..
                } if attacker == watcher => {
                    fired_at.get_or_insert(tick);
                }
                _ => {}
            }
        }
    }
    let seen = seen_at.expect("she steps into view during the round");
    let fired = fired_at.expect("and is engaged before it ends");
    let delay = 2; // reactions untrained on an average crew: base_ticks
    assert_eq!(
        fired,
        seen + delay,
        "noticed at tick {seen}, engaged {delay} ticks later — not instantly"
    );
}

#[test]
fn a_target_watched_across_rounds_is_not_news_twice() {
    // The other direction: the old gate re-charged the delay at the top of
    // every round. A crew that has held the same enemy in sight since last
    // round owes nothing — her gun speaks on the first tick it is ready.
    let mut reg = calm(registry());
    reg.command = None;
    // Soft guns for the same reason as `crews_report_moving_up_the_ladder`:
    // the clock across the round boundary is the thing under test, and it
    // needs both crews alive to see round two. The reload is pinned at five
    // ticks so the arithmetic discriminates: first shots at tick 2 (the
    // reaction delay), again at 7, and the barrel is ready exactly as round
    // two opens — so tick 0 proves the clock did not re-charge, while a
    // re-charged clock would show tick 2. With the gun's own reload of
    // three the carry-over happens to equal the delay and the test could
    // not tell a cooling barrel from a re-taxed crew.
    if let Some(w) = reg.weapons.get_mut("gun_75") {
        w.damage = 2;
        w.reload_ticks = Some(5);
    }
    let mut state = duel(&reg, 102);
    let watcher = UnitId(0);
    // Round one: they see each other, shots are exchanged, the clock is paid.
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "still watching him"
    );
    // Round two, both hold: the first opportunity shot must come at tick 0,
    // the moment the gun is ready — not at tick two again.
    commit_all(&reg, &mut state);
    let mut first_shot = None;
    while state.resolving_tick().is_some() && first_shot.is_none() && !state.is_over() {
        let tick = state.resolving_tick().unwrap();
        for event in state.step_tick(&reg) {
            if let BattleEvent::ShotFired {
                attacker,
                opportunity: true,
                ..
            } = event
                && attacker == watcher
            {
                first_shot.get_or_insert(tick);
            }
        }
    }
    assert_eq!(
        first_shot,
        Some(0),
        "an enemy watched across the round boundary is old news"
    );
}

// --- the crew's loop: the mid-round drill ----------------------------------

/// The ambush staged with somewhere to go: a forest stand west of the
/// watcher, the same curtain in the middle, and the walker tucked behind it.
/// The watcher sits idle on open grass — exactly the crew the mid-round
/// drill exists for.
fn open_ground_stage(reg: &DataRegistry, watcher_at: [i32; 2], seed: u64) -> BattleState {
    let rows = ["ffgggggggggg", "ffggggffgggg", "ffgggggggggg"];
    two_side_battle(
        reg,
        &rows,
        vec![
            unit_at(watcher_at, 0, "medium_tank", "Watcher"),
            unit_at([8, 1], 1, "medium_tank", "Walker"),
        ],
        seed,
    )
}

/// Play out the ambush and record when side 0 first saw the walker and when
/// (and where) the watcher broke for cover, then finish the round.
///
/// The fourth element is the battle **as it stood when that tick began**, and
/// it is there because the drill's destination can only be judged against the
/// board the drill was looking at. Both crews move inside a tick, so pricing
/// a hex against the end of the round asks a question about a walker who has
/// driven on since — which reads as "the gun expects nothing anywhere",
/// because by then it usually does.
fn spring_the_ambush(
    reg: &DataRegistry,
    state: &mut BattleState,
) -> (
    Option<u32>,
    Option<u32>,
    Option<tactics_core::Hex>,
    Option<BattleState>,
) {
    let (watcher, walker) = (UnitId(0), UnitId(1));
    assert!(
        !state.fog.side(0).spotted.contains(&walker),
        "the stage needs the walker hidden"
    );
    let out = state.unit(walker).unwrap().pos + tactics_core::Hex::new(0, -1);
    state
        .apply(
            reg,
            &Order::SetMove {
                unit: walker,
                to: out,
            },
        )
        .unwrap();
    commit_all(reg, state);

    let (mut seen_at, mut broke_at, mut broke_to, mut broke_on) = (None, None, None, None);
    while state.resolving_tick().is_some() && !state.is_over() {
        let tick = state.resolving_tick().unwrap();
        let before = state.clone();
        for event in state.step_tick(reg) {
            match event {
                BattleEvent::UnitSpotted {
                    unit, by_side: 0, ..
                } if unit == walker => {
                    seen_at.get_or_insert(tick);
                }
                BattleEvent::TookCover { unit, at } if unit == watcher => {
                    broke_at.get_or_insert(tick);
                    broke_to.get_or_insert(at);
                    broke_on.get_or_insert(before.clone());
                }
                _ => {}
            }
        }
    }
    (seen_at, broke_at, broke_to, broke_on)
}

#[test]
fn a_crew_caught_in_the_open_breaks_for_cover_before_the_round_ends() {
    // The planning-table drill covers a crew who is threatened when the
    // round is planned; this is the one who is ambushed at tick four. She
    // owes her reaction time — the same per-enemy clock her gunner pays for
    // opportunity fire — and then she owes nobody a planning phase: the
    // tracks move mid-round, toward the quietest ground in reach, and the
    // event stream says so out loud.
    //
    // **What is asserted about the destination changed in Wave 2 and the
    // rule did not.** It used to be `Some("forest")`, because the drill took
    // the reachable tile with the most terrain `cover` and the stand of trees
    // west of her is the only cover on the stage. The drill spends the
    // currency now, so the claim is the one the currency can make: the ground
    // she picked is ground on which the gun that frightened her expects to do
    // strictly less. On this stage that is the dead ground behind the
    // curtain, one hex away, rather than the trees three hexes further west —
    // and preferring the near hex is the *point* of the change, since both
    // are equally invisible to the walker and one of them can be reached
    // before she is shot at again. The test that pins the wood-versus-dead-
    // ground choice on its own is
    // `the_drill_goes_where_the_gun_cannot_see_her_not_to_the_nearest_wood`.
    let mut reg = registry_wireless();
    soften(&mut reg);
    let mut state = open_ground_stage(&reg, [3, 1], 201);
    let parked = state.unit(UnitId(0)).unwrap().pos;

    let (seen_at, broke_at, broke_to, broke_on) = spring_the_ambush(&reg, &mut state);
    let seen = seen_at.expect("she steps into view during the round");
    let broke = broke_at.expect("and the watcher does not wait for the round to end");
    let delay = 2; // reactions untrained on an average crew: base_ticks
    assert_eq!(
        broke,
        seen + delay,
        "noticed at tick {seen}, moving {delay} ticks later — not instantly"
    );
    let dest = broke_to.expect("a destination came with the event");
    let board = broke_on.expect("and the board she decided on came with it");
    let walker = UnitId(1);
    let there = tactics_core::battle::incoming_from(&reg, &board, UnitId(0), dest, &[walker]).worth;
    let here =
        tactics_core::battle::incoming_from(&reg, &board, UnitId(0), parked, &[walker]).worth;
    assert!(
        there < here,
        "she makes for ground the gun can do less on, not merely anywhere: \
         {there} at {dest:?} against {here} at {parked:?}"
    );
    assert_ne!(
        state.unit(UnitId(0)).unwrap().pos,
        parked,
        "and the tracks actually turned before the round was over"
    );
}

#[test]
fn a_crew_with_a_route_in_hand_drives_it_rather_than_flinching() {
    // The reaction-latency post-mortem's rule, applied to the drill: only
    // responses to NEW information may cost time, and by the same token only
    // a crew with nothing left to execute may improvise. A route already in
    // hand keeps being driven — interrupting ordered movement is the
    // evaluator's business at the next planning table (that is what makes an
    // Advance a movement to contact), never the engine's mid-round.
    let mut reg = registry_wireless();
    soften(&mut reg);
    let mut state = open_ground_stage(&reg, [3, 1], 202);
    let (watcher, dest) = (UnitId(0), state.unit(UnitId(0)).unwrap().pos);
    // Send her west into the trees under her own orders: the route both
    // outlasts the ambush and ends in cover, so the drill would have nothing
    // to add even if it wrongly fired after arrival.
    let ordered = dest + tactics_core::Hex::new(-3, 0);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: watcher,
                to: ordered,
            },
        )
        .unwrap();

    let (seen_at, broke_at, _, _) = spring_the_ambush(&reg, &mut state);
    assert!(seen_at.is_some(), "the ambush still happens");
    assert_eq!(
        broke_at, None,
        "a crew executing her orders is not hijacked by the drill"
    );
    assert_eq!(
        state.unit(watcher).unwrap().pos,
        ordered,
        "she drives the route she was given"
    );
}

#[test]
fn an_overwatching_crew_trusts_her_gun_over_her_tracks() {
    // A fire order is the deliberate hold-and-watch, and it outranks the
    // drill exactly as it does at the planning table: she was put there to
    // shoot, and a gun line that scatters for the trees the moment it is
    // shot back at is not a gun line.
    let mut reg = registry_wireless();
    soften(&mut reg);
    let mut state = open_ground_stage(&reg, [3, 1], 203);
    let watcher = UnitId(0);
    let parked = state.unit(watcher).unwrap().pos;
    // Area fire on an empty patch of grass: legal without a spot, and it
    // marks her intent as overwatch for the whole round. Deliberately NOT
    // the curtain's mouth — with real penetration a blind 75 that happens
    // to land on the walker as she steps out can kill her, and this test
    // is about the drill deferring to the fire order, not about luck.
    let ground = parked + tactics_core::Hex::new(1, 1);
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: watcher,
                fire: FireIntent::Area {
                    at: ground,
                    weapon: 0,
                },
            },
        )
        .unwrap();

    let (seen_at, broke_at, _, _) = spring_the_ambush(&reg, &mut state);
    assert!(seen_at.is_some(), "the ambush still happens");
    assert_eq!(broke_at, None, "overwatch stands");
    assert_eq!(state.unit(watcher).unwrap().pos, parked);
}

#[test]
fn a_crew_with_no_better_ground_in_reach_stands_where_she_is() {
    // The comparison is strict: the drill moves a crew to strictly better
    // ground or not at all, which is what keeps a crew who has already
    // reached the quietest hex in reach from dancing between two equally
    // good tiles forever.
    //
    // **The stage had to change in Wave 2 and the rule did not.** It used to
    // be a bare five-hex duel asserting that *nobody moves at all*, on the
    // reasoning that a billiard table offers no cover to prefer. That was
    // true of the terrain table and is not true of the currency: a gun loses
    // accuracy with range, so on a billiard table the far corner really is
    // quieter than the near one and a crew who has noticed the gun really
    // ought to back off. Asserting nobody moves would now be asserting that
    // the drill cannot read the arithmetic, which is the defect this wave
    // removes rather than a rule worth keeping.
    //
    // So what is checked is the strictness itself, over the same duel: every
    // dash the drill lays goes to ground where the guns she has noticed
    // expect *strictly* less than where she stood, and she never shuffles
    // for nothing. That is the property the doc comment on `run_crew_drill`
    // claims and the one that makes the drill settle instead of oscillate;
    // the old stage was one consequence of it.
    let reg = registry_wireless();
    let mut state = duel(&reg, 204);
    commit_all(&reg, &mut state);
    let mut dashes = 0;
    while state.resolving_tick().is_some() && !state.is_over() {
        // The board the drill was looking at. Both crews move inside a tick,
        // so a destination priced against the end of it is priced against
        // somebody else's battle.
        let before = state.clone();
        for event in state.step_tick(&reg) {
            if let BattleEvent::TookCover { unit, at } = event {
                let from = before
                    .unit(unit)
                    .expect("she was on the field when the tick began")
                    .pos;
                // Two crews in plain sight of each other, so "the guns she
                // has noticed" and "every gun the side has found" are the
                // same list and `incoming` may stand in for the drill's own
                // per-enemy clock.
                let there = tactics_core::battle::incoming(&reg, &before, unit, at).worth;
                let here = tactics_core::battle::incoming(&reg, &before, unit, from).worth;
                assert!(
                    there < here,
                    "the drill only dashes to strictly quieter ground: \
                     {there} at {at:?} against {here} at {from:?}"
                );
                dashes += 1;
            }
        }
    }
    // And the duel is a stage rather than a coincidence: if nothing ever
    // fired the drill this test would be asserting about an empty loop.
    assert!(
        dashes > 0,
        "the duel has to actually run the drill for its strictness to be checked"
    );
}

#[test]
fn a_mod_that_prices_no_reactions_gets_the_drill_at_the_next_tick() {
    // Difficulty is a mod: zeroing the reaction rules collapses the delay to
    // nothing and the whole pricing system disappears without an `if`. One
    // tick remains and is structural, not a price — the sighting happens
    // after this tick's movement has already resolved, so the very next
    // slice of simultaneous time is the soonest any tracks can answer it.
    let mut reg = registry_wireless();
    soften(&mut reg);
    reg.reaction.base_ticks = 0;
    reg.reaction.max_ticks = 0;
    let mut state = open_ground_stage(&reg, [3, 1], 205);

    let (seen_at, broke_at, _, _) = spring_the_ambush(&reg, &mut state);
    let seen = seen_at.expect("she steps into view during the round");
    assert_eq!(
        broke_at,
        Some(seen + 1),
        "with reactions unpriced the drill is as instant as a tick model allows"
    );
}
