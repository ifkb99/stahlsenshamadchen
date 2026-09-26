//! The radio net: who hears an order, how late, and who is left in command
//! when the net degrades.
//!
//! Contact and order latency, the command picture, commander loss and
//! succession, the two media the net runs on, orders that wait rather than
//! die when nobody is there to hear them, mission sequences, seeing the net
//! from outside it, and who is entitled to hear what — eight sections about
//! one wire between a commander's intention and a crew's orders, and every
//! way that wire can fray without a shot being fired.
//!
//! - contact and order latency
//! - the command picture
//! - commander loss and succession
//! - the net is two media
//! - orders wait instead of dying
//! - mission sequences
//! - seeing the net
//! - who is entitled to hear what

use tactics_core::ai::{
    AiConfig, AiDriver, AiPlanner, Evaluator, UtilityPlanner, make_battle_planner,
};
use tactics_core::battle::{
    BattleState, EndReason, Event as BattleEvent, FireIntent, FormationId, Knower, Latitude,
    Mission, Order, SideState, UnitId,
};
use tactics_core::data::DataRegistry;
use tactics_core::map::{Battlefield, UnitPlacement};

mod common;
use common::{
    command_rules, commit_all, formation_named, in_formation, play_round, registry,
    registry_wireless, scripted_battle, settle, sharp_planner, strike_down, strip_radios,
    two_side_battle, unit_at,
};

// --- contact and order latency ---------------------------------------------

#[test]
fn orders_take_time_to_arrive_when_the_radio_says_so() {
    // The heart of the chunk: an order is sent when it is issued and arrives
    // later. Two ticks of latency means the formation spends the opening of
    // the round doing the last thing it heard, which is the whole point —
    // and, per the reaction-latency post-mortem, the delay is on the *new
    // information* reaching them, never on executing a plan they already had.
    let mut reg = registry();
    reg.command = Some(command_rules(999, true, 2));
    let mut state = BattleState::from_map(&reg, "river_crossing", 5).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let bridge = state.scenario.objectives()[0].anchor();

    let events = state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: bridge },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("the bridge is on the map");
    assert_eq!(
        events,
        vec![BattleEvent::MissionAssigned {
            formation: "kuhlmann_armor".into(),
            mission: Mission::Advance { to: bridge },
        }],
        "the order is sent the moment it is given, and said out loud"
    );
    assert!(
        state.formations()[armor.index()].mission.is_none(),
        "but the platoon plans this round without having heard it"
    );

    commit_all(&reg, &mut state);
    let first = state.step_tick(&reg);
    assert!(
        !first
            .iter()
            .any(|e| matches!(e, BattleEvent::MissionReceived { .. })),
        "still in the air after one tick"
    );
    assert!(state.formations()[armor.index()].mission.is_none());

    let second = state.step_tick(&reg);
    assert!(
        second.iter().any(|e| matches!(
            e,
            BattleEvent::MissionReceived { formation, mission }
                if formation == "kuhlmann_armor" && *mission == Mission::Advance { to: bridge }
        )),
        "it lands on the second tick, and the log says so: {second:?}"
    );
    assert_eq!(
        state.formations()[armor.index()].mission,
        Some(Mission::Advance { to: bridge }),
        "and only then is it what the platoon is doing"
    );
    assert!(
        state.formations()[armor.index()].incoming.is_none(),
        "nothing is left travelling"
    );
}

#[test]
fn a_command_block_with_zero_coefficients_is_the_game_without_one() {
    // The additivity pin, and the one this chunk most needs: the first
    // reaction-latency attempt died because it broke tests at *any* setting of
    // its knob, which is the tell that a model is wrong rather than mistuned.
    // So the rules are declared at their most generous — everyone in radio
    // contact of everyone, orders that arrive instantly — and the battle they
    // produce must be the same battle, decision for decision, as one whose
    // registry has no `command` block at all. A command-planner side, so
    // missions are actually being issued and could actually go astray.
    //
    // Leaders die in this window — on `river_crossing` a platoon leader dies
    // in the first round of nearly every seed — and the pin holds anyway,
    // because a cut-off crew soldiers on the orders she was carrying rather
    // than dropping them. What a block is *allowed* to add at zero
    // coefficients is words, not deeds: the wire events (out of contact,
    // restored, reports reaching the commander) are the system's information
    // surface and exist whenever it does. So the comparison is exact after
    // setting those three aside, and then requires that nothing else was set
    // aside — a behaviour drift hiding among the wire events fails the
    // second assertion instead of slipping through the first.
    let wire = |line: &String| {
        line.starts_with("OutOfContact")
            || line.starts_with("ContactRestored")
            || line.starts_with("ContactReported")
    };
    let run = |rules: Option<tactics_core::data::CommandRules>| -> Vec<String> {
        let mut reg = registry();
        reg.command = rules;
        // The pin pins the RULES coefficients, so the vehicles' own radio
        // sets are stripped: hardware at 8 hexes would cap the "everyone in
        // contact" net the zeroed block declares, and hardware is content,
        // not a coefficient. Stripped identically in both runs.
        for vehicle in reg.vehicles.values_mut() {
            vehicle.radio = None;
        }
        let mut state = BattleState::from_map(&reg, "river_crossing", 21).unwrap();
        let mut ai = AiDriver::new();
        ai.insert(
            0,
            make_battle_planner(
                &AiConfig {
                    planner: "command".into(),
                    difficulty: 5,
                    doctrine: Some("massed_armor".into()),
                },
                21,
                &reg,
            ),
        );
        ai.insert(1, sharp_planner(&reg, 22, "elastic_defense"));
        let mut log = Vec::new();
        for _ in 0..6 {
            if state.is_over() {
                break;
            }
            // Planning-phase events too: a mission going astray in transit
            // would show up here first.
            ai.plan_round_with(&reg, &mut state, |d| {
                log.extend(d.events.iter().map(|e| format!("{e:?}")));
            });
            log.extend(state.resolve_round(&reg).iter().map(|e| format!("{e:?}")));
        }
        log
    };
    let without = run(None);
    assert!(!without.is_empty(), "the battle should do something");
    assert!(
        without.iter().any(|line| line.contains("MissionAssigned")),
        "and it should be issuing missions, or this proves nothing"
    );
    assert!(
        without.iter().any(|line| line.contains("ShotHit")),
        "and fighting, rather than driving about out of contact"
    );
    assert!(
        !without.iter().any(wire),
        "no block, no wires: the None run must contain no wire events at all"
    );
    let zeroed = run(Some(tactics_core::data::CommandRules {
        radius: 999,
        radius_per_signals: 0,
        relay: true,
        visual_range: 0,
        overworld_radius: 999,
        review: tactics_core::data::ReactionRules {
            skill: "command".into(),
            base_ticks: 0,
            levels_per_tick: 0,
            max_ticks: 0,
        },
        latency: tactics_core::data::ReactionRules {
            skill: "command".into(),
            base_ticks: 0,
            levels_per_tick: 0,
            max_ticks: 0,
        },
    }));
    let (spoken, deeds): (Vec<String>, Vec<String>) = zeroed.into_iter().partition(wire);
    assert_eq!(
        deeds, without,
        "a command block at zero coefficients must change words, never deeds"
    );
    assert!(
        spoken.iter().all(wire),
        "and everything set aside really was wire traffic"
    );
}

/// Play the opening round of `river_crossing` under these command rules with
/// nobody ordered to do anything, so that contact is computed against the
/// deployment as declared. Returns the battle in its second planning phase and
/// everything the round said.
fn quiet_round(
    reg: &DataRegistry,
    mission: Option<(FormationId, Mission)>,
) -> (BattleState, Vec<BattleEvent>) {
    let mut state = BattleState::from_map(reg, "river_crossing", 5).expect("battle");
    if let Some((formation, mission)) = mission {
        state
            .apply(
                reg,
                &Order::SetMission {
                    formation,
                    mission,
                    latitude: tactics_core::battle::Latitude::Delegated,
                },
            )
            .expect("a legal mission");
    }
    commit_all(reg, &mut state);
    let events = state.resolve_round(reg);
    (state, events)
}

/// Plan exactly one unit, by handing every other unit on her side a
/// hold-fire order first so the planner has a single decision left to make.
///
/// That isolation is the point: the evaluator's mass term reads what her
/// neighbours are *planning*, so planning a whole side would let a difference
/// in somebody else's orders leak into hers and make a comparison meaningless.
fn plan_one(
    reg: &DataRegistry,
    state: &mut BattleState,
    unit: UnitId,
    seed: u64,
) -> tactics_core::Hex {
    let side = state.unit(unit).expect("she is alive").side;
    let others: Vec<UnitId> = state
        .side_units(side)
        .map(|u| u.id)
        .filter(|id| *id != unit)
        .collect();
    for id in others {
        state
            .apply(
                reg,
                &Order::SetFire {
                    unit: id,
                    fire: FireIntent::Hold,
                },
            )
            .expect("holding fire is always legal");
    }
    let mut planner = UtilityPlanner::new(
        Evaluator::new(reg.doctrine("massed_armor").expect("base doctrine").clone()),
        5,
        seed,
    );
    loop {
        let order = planner.next_order(reg, state, side);
        if matches!(order, Order::Commit { .. }) {
            break;
        }
        let _ = state.apply(reg, &order);
    }
    state
        .unit(unit)
        .expect("planning harms nobody")
        .planned_destination()
}

#[test]
fn a_cut_off_unit_keeps_the_orders_she_had() {
    // Out of contact is not amnesia and it is not license: a cadet who loses
    // the wire soldiers on the standing orders she was carrying when it went
    // dead. What she cannot do is hear anything new. This inverts the first
    // model this test pinned — "cut off means unmissioned" — which measured
    // badly the moment leaders started dying on first contact: a command
    // block silently deleted half the map's missions by round two, and a
    // system whose presence deletes orders is a tax, not a texture.
    let reg = registry();
    let (armor, follower, start) = {
        let probe = BattleState::from_map(&reg, "river_crossing", 5).unwrap();
        let armor = formation_named(&probe, "kuhlmann_armor");
        let formation = &probe.formations()[armor.index()];
        let leader = formation.leader.expect("the platoon has a commander");
        let follower = *formation
            .members
            .iter()
            .find(|id| **id != leader)
            .expect("and somebody to command");
        (armor, follower, probe.unit(follower).unwrap().pos)
    };
    // Ground to hold, rather than ground to take: the map's own objectives
    // already pull everyone toward the bridge, so a mission to advance on it
    // would be indistinguishable from her own judgment and this test would
    // pass while proving nothing.
    let mission = Mission::Hold { at: Some(start) };

    let cut_off_reg = {
        let mut r = registry();
        // Two hexes and no relay: the platoon deploys strung out, so the
        // second tank cannot hear her commander.
        r.command = Some(command_rules(2, false, 0));
        strip_radios(&mut r);
        r
    };
    let (cut_off, events) = quiet_round(&cut_off_reg, Some((armor, mission.clone())));
    let formation = &cut_off.formations()[armor.index()];
    assert_eq!(
        formation.mission,
        Some(mission.clone()),
        "the platoon is under orders"
    );
    let carried = formation
        .out_of_contact
        .iter()
        .map(|c| (c.unit, c.orders.clone()))
        .collect::<Vec<_>>();
    assert_eq!(
        carried,
        vec![(follower, Some(mission.clone()))],
        "she is the one out of contact, and she took the order with her"
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, BattleEvent::OutOfContact { unit } if *unit == follower))
            .count(),
        1,
        "said once, not once a tick"
    );

    // The same battle under rules that reach the whole map, so the two states
    // differ in nothing but who can hear the wire.
    let heard_reg = {
        let mut r = registry();
        r.command = Some(command_rules(999, false, 0));
        strip_radios(&mut r);
        r
    };
    let (in_contact, _) = quiet_round(&heard_reg, Some((armor, mission)));
    assert!(
        in_contact.formations()[armor.index()]
            .out_of_contact
            .is_empty(),
        "nobody is cut off when the radius covers the map"
    );

    // And the twin with no chain of command at all: what an unmissioned cadet
    // would do, which the deaf one must NOT match — she has orders.
    let mut twin = cut_off.clone();
    twin.command = Default::default();

    let mut a = cut_off.clone();
    let mut b = twin;
    let mut c = in_contact;
    let deaf = plan_one(&cut_off_reg, &mut a, follower, 77);
    let unmissioned = plan_one(&cut_off_reg, &mut b, follower, 77);
    let obedient = plan_one(&heard_reg, &mut c, follower, 77);
    assert_eq!(
        deaf, obedient,
        "cut off or not, she is executing the same standing order"
    );
    assert_ne!(
        deaf, unmissioned,
        "and it is the order steering her, not her own judgment"
    );
    assert!(
        start.distance_to(deaf) <= start.distance_to(unmissioned),
        "holding means staying: {deaf:?} against {unmissioned:?} from {start:?}"
    );
}

#[test]
fn an_order_never_heard_does_not_steer_her() {
    // The counterpart: a cadet already out of contact when the order is given
    // never receives it. The formation's standing mission changes behind her
    // back; she fights on what she knew — which was nothing.
    let reg = registry();
    let (armor, follower) = {
        let probe = BattleState::from_map(&reg, "river_crossing", 5).unwrap();
        let armor = formation_named(&probe, "kuhlmann_armor");
        let formation = &probe.formations()[armor.index()];
        let leader = formation.leader.expect("a commander");
        let follower = *formation
            .members
            .iter()
            .find(|id| **id != leader)
            .expect("a subordinate");
        (armor, follower)
    };
    let cut_off_reg = {
        let mut r = registry();
        r.command = Some(command_rules(2, false, 0));
        strip_radios(&mut r);
        r
    };
    // Round one, no mission: she goes out of contact carrying nothing.
    let (mut state, _) = quiet_round(&cut_off_reg, None);
    let start = state.unit(follower).unwrap().pos;
    // Now the order goes out. It reaches the formation — the leader can hear
    // herself — but not her.
    state
        .apply(
            &cut_off_reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Hold { at: Some(start) },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("a legal mission");
    let formation = &state.formations()[armor.index()];
    assert!(
        formation.latest_mission().is_some(),
        "the platoon has its orders"
    );
    assert_eq!(
        formation
            .out_of_contact
            .iter()
            .find(|c| c.unit == follower)
            .map(|c| c.orders.clone()),
        Some(None),
        "but she is carrying the nothing she was cut off with"
    );

    let mut twin = state.clone();
    twin.command = Default::default();
    let deaf = plan_one(&cut_off_reg, &mut state, follower, 91);
    let unmissioned = plan_one(&cut_off_reg, &mut twin, follower, 91);
    assert_eq!(
        deaf, unmissioned,
        "an order she never heard cannot steer her"
    );
}

#[test]
fn contact_lost_is_said_once_and_restored_out_loud() {
    let mut reg = registry();
    reg.command = Some(command_rules(3, false, 0));
    strip_radios(&mut reg);
    let mut state = BattleState::from_map(&reg, "river_crossing", 8).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let formation = &state.formations()[armor.index()];
    let leader = formation.leader.expect("a commander");
    let follower = *formation
        .members
        .iter()
        .find(|id| **id != leader)
        .expect("somebody to command");
    let beside = state.unit(leader).unwrap().pos + tactics_core::Hex::new(1, 0);
    let away = state.unit(leader).unwrap().pos + tactics_core::Hex::new(0, 8);
    assert!(state.world.contains(beside) && state.world.contains(away));
    assert!(state.unit_at(beside).is_none() && state.unit_at(away).is_none());

    // She starts alongside her commander, so the opening tick has nothing to
    // report about her.
    state.unit_mut(follower).unwrap().pos = beside;
    commit_all(&reg, &mut state);
    let quiet = state.step_tick(&reg);
    assert!(
        !quiet.iter().any(|e| matches!(
            e,
            BattleEvent::OutOfContact { unit } | BattleEvent::ContactRestored { unit } if *unit == follower
        )),
        "a platoon driving together says nothing: {quiet:?}"
    );

    // Then she drives out of earshot. Once.
    state.unit_mut(follower).unwrap().pos = away;
    let lost = state.step_tick(&reg);
    assert_eq!(
        lost.iter()
            .filter(|e| matches!(e, BattleEvent::OutOfContact { unit } if *unit == follower))
            .count(),
        1,
        "losing contact is news exactly once: {lost:?}"
    );
    let still = state.step_tick(&reg);
    assert!(
        !still
            .iter()
            .any(|e| matches!(e, BattleEvent::OutOfContact { .. })),
        "staying out of contact is not news again every tick: {still:?}"
    );

    // And back, which the player must also hear: a unit silently starting to
    // obey again is as confusing as one silently ignoring orders.
    state.unit_mut(follower).unwrap().pos = beside;
    let back = state.step_tick(&reg);
    assert_eq!(
        back.iter()
            .filter(|e| matches!(e, BattleEvent::ContactRestored { unit } if *unit == follower))
            .count(),
        1,
        "restored contact is said out loud: {back:?}"
    );
    assert!(
        state.formations()[armor.index()].out_of_contact.is_empty(),
        "and the state agrees with the log"
    );
}

// --- the command picture ---------------------------------------------------

/// A long open road: a leader in the west, her scout far to the east with an
/// enemy recon car beyond — inside the scout's eyes, outside everybody's
/// guns, and far outside the leader's radio unless a test says otherwise.
fn picture_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    let row = "g".repeat(60);
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "picture_stage",
        "palette": { "g": "grass" },
        "rows": [row],
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
    let mut leader = unit_at([0, 0], 0, "recon_car", "Leader");
    leader.formation = Some("net".into());
    leader.leads = true;
    let mut scout = unit_at([25, 0], 0, "recon_car", "Scout");
    scout.formation = Some("net".into());
    let enemy = unit_at([32, 0], 1, "recon_car", "Prowler");
    let placements = vec![leader, scout, enemy];
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
fn a_scout_out_of_contact_reports_nothing() {
    // Seeing is not reporting. The side's fog spots through any unit's eyes;
    // the commander's picture learns only what somebody on the net can tell
    // her. A scout beyond the radio finds the enemy and nobody knows —
    // which is recon wasted, and the whole reason the wires matter.
    let mut reg = registry();
    reg.command = Some(command_rules(8, false, 0));
    strip_radios(&mut reg);
    let mut state = picture_stage(&reg, 3);
    let (scout, enemy) = (UnitId(1), UnitId(2));

    commit_all(&reg, &mut state);
    let events = state.step_tick(&reg);
    assert!(
        state.fog.side(0).spotted.contains(&enemy),
        "the side's fog does see him, through her"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, BattleEvent::OutOfContact { unit } if *unit == scout)),
        "and she is announced as off the net"
    );
    assert!(
        !state.picture(0).iter().any(|c| c.unit == enemy),
        "but the commander has heard nothing: a cut-off scout files no report"
    );

    // March the leader east until the scout is back on the net; the report
    // goes through the moment somebody in contact can vouch for the sighting.
    if let Some(unit) = state.unit_mut(UnitId(0)) {
        unit.pos = tactics_core::offset_to_hex(20, 0);
    }
    let events = state.step_tick(&reg);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, BattleEvent::ContactRestored { unit } if *unit == scout)),
        "she is back on the net"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, BattleEvent::ContactReported { unit, .. } if *unit == enemy)),
        "and the sighting finally reaches the commander, said out loud"
    );
    let contact = state
        .picture(0)
        .iter()
        .find(|c| c.unit == enemy)
        .expect("the picture now carries him");
    assert!(contact.fresh, "freshly, because somebody can see him now");
}

/// What the side knows is two questions, and every brain and screen asks one
/// of them through `BattleState::known_enemies`.
///
/// The cut-off scout of [`a_scout_out_of_contact_reports_nothing`] has found
/// a recon car her commander has never heard of. The commander — the player's
/// seat, and the AI's mission review — does not know he is there. The scout
/// does, because she is looking at him, and her own planning, drill and
/// danger read him. Her leader, who hears the net but cannot see that far,
/// does not. Before this, every AI crew planned against the side's pooled
/// fog, so a chain of command cost the human and nobody else, and the danger
/// panel could name a gun the board did not draw.
///
/// With no `command` block both questions have the old answer: the pooled
/// fog, everybody's eyes.
#[test]
fn a_cut_off_scout_acts_on_what_she_sees_and_her_commander_never_hears_of_it() {
    let mut reg = registry();
    reg.command = Some(command_rules(8, false, 0));
    strip_radios(&mut reg);
    let mut state = picture_stage(&reg, 3);
    let (leader, scout, enemy) = (UnitId(0), UnitId(1), UnitId(2));
    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    assert!(
        state.fog.side(0).spotted.contains(&enemy) && !state.hears_orders(scout),
        "the stage is a scout off the net who has found somebody"
    );
    let ids = |who: Knower, state: &BattleState, reg: &DataRegistry| -> Vec<UnitId> {
        state.known_enemies(reg, who).iter().map(|u| u.id).collect()
    };
    assert_eq!(
        ids(Knower::Commander(0), &state, &reg),
        Vec::<UnitId>::new(),
        "nobody has told the commander"
    );
    assert_eq!(
        ids(Knower::Crew(scout), &state, &reg),
        vec![enemy],
        "the scout is looking at him"
    );
    assert_eq!(
        ids(Knower::Crew(leader), &state, &reg),
        Vec::<UnitId>::new(),
        "her leader can neither see him nor hear about him"
    );

    let mut bare = reg.clone();
    bare.command = None;
    for who in [
        Knower::Commander(0),
        Knower::Crew(scout),
        Knower::Crew(leader),
    ] {
        assert_eq!(
            ids(who, &state, &bare),
            vec![enemy],
            "{who:?}: with no chain of command, what anybody sees everybody knows"
        );
    }
}

#[test]
fn a_contact_no_longer_seen_goes_stale_not_absent() {
    // "We lost sight of it" is information; "it was never there" is a lie.
    // A contact nobody can re-report stays on the picture as a ghost at the
    // last reported position, marked stale rather than deleted.
    let mut reg = registry();
    reg.command = Some(command_rules(999, false, 0));
    strip_radios(&mut reg);
    let mut state = picture_stage(&reg, 4);
    let enemy = UnitId(2);
    let seen_at = state.unit(enemy).unwrap().pos;

    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    let contact = state
        .picture(0)
        .iter()
        .find(|c| c.unit == enemy)
        .expect("reported while seen");
    assert!(contact.fresh);
    assert_eq!(contact.at, seen_at);

    // He slips away beyond every eye on the field.
    if let Some(unit) = state.unit_mut(enemy) {
        unit.pos = tactics_core::offset_to_hex(55, 0);
    }
    state.step_tick(&reg);
    assert!(
        !state.fog.side(0).spotted.contains(&enemy),
        "nobody can see him any more"
    );
    let ghost = state
        .picture(0)
        .iter()
        .find(|c| c.unit == enemy)
        .expect("but the commander still has him on the map");
    assert!(!ghost.fresh, "as a ghost");
    assert_eq!(
        ghost.at, seen_at,
        "standing where he was last reported, not where he is"
    );
}

/// What the side can already see when the battle opens is in the picture
/// before anybody plans round one — and with no `command` block there is no
/// picture at all, which is the game without one.
///
/// Both setup paths used to compute the fog and stop, so the picture did not
/// exist until the first tick: the player's screen, which draws it, showed no
/// enemy during the first planning phase however plainly one stood in view.
/// Two tanks in the open, neither in a formation, so each answers to her own
/// side and reports what she sees.
/// A playout (`ai::plan::playout`) is fought on the board her commander
/// knows, not on what her side has spotted: the scout off the net has found
/// somebody, and he is not on her commander's board. Otherwise choosing a
/// plan by playing it out would read through the chain of command — the
/// commander would plan against an enemy nobody told her about.
#[test]
fn a_playout_is_fought_on_the_board_her_commander_knows() {
    let mut reg = registry();
    reg.command = Some(command_rules(8, false, 0));
    strip_radios(&mut reg);
    let mut state = picture_stage(&reg, 3);
    let (scout, enemy) = (UnitId(1), UnitId(2));
    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    assert!(
        state.fog.side(0).spotted.contains(&enemy) && !state.hears_orders(scout),
        "the stage is a scout off the net who has found somebody"
    );
    let world = tactics_core::ai::plan::known_world(&reg, &state, 0, 7);
    let on_board = |world: &BattleState| world.units.iter().any(|u| u.id == enemy && u.alive());
    assert!(
        !on_board(&world),
        "spotted by a crew and never reported is not on her commander's board"
    );
    assert!(
        world
            .units
            .iter()
            .filter(|u| u.side == 0)
            .all(|u| u.alive())
    );

    // Once she is told, he is.
    let mut bare = reg.clone();
    bare.command = None;
    let world = tactics_core::ai::plan::known_world(&bare, &state, 0, 7);
    assert!(on_board(&world));
}

#[test]
fn the_commander_is_told_what_is_already_in_sight_when_the_battle_opens() {
    let reg = common::seen(registry());
    assert!(
        reg.command.is_some(),
        "the base mod declares a chain of command"
    );
    let state = common::duel(&reg, 3);
    assert!(
        state.is_planning() && state.round <= 1,
        "nothing has been resolved yet"
    );
    for (side, enemy) in [(0u8, UnitId(1)), (1, UnitId(0))] {
        assert!(
            state
                .picture(side)
                .iter()
                .any(|c| c.unit == enemy && c.fresh),
            "side {side} can see {enemy:?} and was not told before round one: {:?}",
            state.picture(side)
        );
    }

    let mut bare = reg.clone();
    bare.command = None;
    let state = common::duel(&bare, 3);
    assert!(
        state.picture(0).is_empty() && state.picture(1).is_empty(),
        "with no chain of command there is nobody to tell and nothing to draw from"
    );
}

// --- commander loss and succession -----------------------------------------

#[test]
fn command_passes_to_the_next_girl_in_the_order_of_battle() {
    // Succession is formation machinery, not wire machinery, so this runs on
    // a registry with no `command` block at all: who is in charge of a platoon
    // is a fact about the platoon, and a mod that never priced a radio still
    // has one cadet senior to another. Seniority is the order the map author
    // wrote her formation down in — lowest living unit id — which is the same
    // authorable rule `leads` follows for the first leader.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 3).expect("battle");
    let armor = formation_named(&state, "kuhlmann_armor");
    let (leader, heir) = {
        let formation = &state.formations()[armor.index()];
        let leader = formation.leader.expect("a commander");
        let heir = *formation
            .members
            .iter()
            .find(|id| **id != leader)
            .expect("and somebody to inherit");
        (leader, heir)
    };
    assert_eq!(
        state.formations()[armor.index()].founding_leader,
        Some(leader),
        "the map's commander is on record from the first tick"
    );

    commit_all(&reg, &mut state);
    let quiet = state.step_tick(&reg);
    assert!(
        !quiet
            .iter()
            .any(|e| matches!(e, BattleEvent::CommandPassed { .. })),
        "nobody is promoted while she is alive"
    );

    strike_down(&mut state, leader);
    let events = state.step_tick(&reg);

    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::CommandPassed { formation, from, to }
                if formation == "kuhlmann_armor" && *from == leader && *to == heir
        )),
        "command passes, and the log names both ends of it: {events:?}"
    );
    let formation = &state.formations()[armor.index()];
    assert_eq!(formation.leader, Some(heir), "she has the platoon now");
    assert_eq!(
        formation.founding_leader,
        Some(leader),
        "but the cadet the map put in charge is not rewritten by her own death \
         — a scenario's loss condition asks about her, not her successor"
    );
    assert!(
        !state
            .step_tick(&reg)
            .iter()
            .any(|e| matches!(e, BattleEvent::CommandPassed { .. })),
        "and it is said once, not once a tick"
    );
}

/// A formation strung out along a road: the commander at the west end, three
/// more in a huddle twenty hexes east of her, and an enemy far beyond
/// everybody's guns. With a short radius nobody but the commander is on the
/// net, which is what makes the succession visible in the contact graph.
fn strung_out_platoon(reg: &DataRegistry) -> BattleState {
    let row = "g".repeat(60);
    let placements = vec![
        in_formation(unit_at([0, 0], 0, "recon_car", "Commander"), "column", true),
        in_formation(unit_at([20, 0], 0, "recon_car", "Heir"), "column", false),
        in_formation(
            unit_at([22, 0], 0, "recon_car", "Neighbour"),
            "column",
            false,
        ),
        in_formation(
            unit_at([40, 0], 0, "recon_car", "Straggler"),
            "column",
            false,
        ),
        unit_at([59, 0], 1, "recon_car", "Prowler"),
    ];
    scripted_battle(
        reg,
        serde_json::json!({
            "id": "strung_out",
            "palette": { "g": "grass" },
            "rows": [row],
            "formations": [ { "id": "column", "name": "The Column", "side": 0 } ],
        }),
        placements,
    )
}

#[test]
fn a_successor_leads_a_formation_back_into_contact() {
    // This inverts a rule an earlier chunk pinned: a dead leader used to
    // strand her whole formation out of contact for the rest of the battle,
    // because the net was anchored on a cadet who was no longer there. She is
    // replaced within the tick now, and the net re-forms around wherever her
    // successor is standing — which is not where the commander was, so who is
    // in contact genuinely changes hands with the command.
    let mut reg = registry();
    // Five hexes and no relay: the column is too long for one voice, so the
    // three easterners are cut off while the commander is alive.
    reg.command = Some(command_rules(5, false, 0));
    strip_radios(&mut reg);
    let mut state = strung_out_platoon(&reg);
    let column = formation_named(&state, "column");
    let (commander, heir, neighbour, straggler) = (UnitId(0), UnitId(1), UnitId(2), UnitId(3));

    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    assert_eq!(
        state.formations()[column.index()]
            .out_of_contact
            .iter()
            .map(|c| c.unit)
            .collect::<Vec<_>>(),
        vec![heir, neighbour, straggler],
        "nobody down the road can hear her"
    );

    strike_down(&mut state, commander);
    let events = state.step_tick(&reg);

    let formation = &state.formations()[column.index()];
    assert_eq!(formation.leader, Some(heir), "the next cadet has it");
    assert!(
        formation.in_contact(heir) && formation.in_contact(neighbour),
        "and the net re-forms around her: {:?}",
        formation.out_of_contact
    );
    assert!(
        !formation.in_contact(straggler),
        "twenty hexes further on is still twenty hexes further on"
    );
    for unit in [heir, neighbour] {
        assert!(
            events
                .iter()
                .any(|e| matches!(e, BattleEvent::ContactRestored { unit: u } if *u == unit)),
            "coming back onto the net is said out loud: {events:?}"
        );
    }
}

#[test]
fn losing_a_commander_shakes_her_formation() {
    // The formation takes it hard, and the whole formation does — unlike
    // watching a friend burn, which only reaches the crews who could see it,
    // this is news that travels the chain of command. It rides the same
    // ladder and the same one place that turns a tick's events into fear.
    let mut reg = registry_wireless();
    // A distinctive number, so what arrives can only have come from here.
    reg.morale.leader_lost = 5;
    let mut state = strung_out_platoon(&reg);
    let column = formation_named(&state, "column");
    let commander = UnitId(0);

    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    assert!(
        state.alive_units().all(|u| u.pressure == 0),
        "nothing has happened to anybody yet"
    );

    strike_down(&mut state, commander);
    state.step_tick(&reg);

    let members = state.formations()[column.index()].members.clone();
    for id in members.iter().filter(|id| **id != commander) {
        assert_eq!(
            state.unit(*id).expect("still on the field").pressure,
            5,
            "every cadet in the column felt it, however far down the road she is"
        );
    }
    assert_eq!(
        state.unit(UnitId(4)).expect("the enemy is fine").pressure,
        0,
        "and nobody outside the formation felt anything at all"
    );
}

/// A decapitation stage: two crews of side 0 in one formation behind a forest
/// curtain, one enemy on the far side of it, and a lane home in the west. The
/// curtain is the same one the objective tests use — these rules are about
/// who is left, and a firefight would decide the battle before the
/// bookkeeping could be watched.
fn decapitation_battle(reg: &DataRegistry, loss_conditions: serde_json::Value) -> BattleState {
    scripted_battle(
        reg,
        serde_json::json!({
            "id": "decapitation_map",
            "palette": { "g": "grass", "f": "forest" },
            "rows": ["gggggfggggg"],
            "objectives": [{
                "id": "west_road", "at": [[0, 0], [1, 0]], "value": 1,
                "kind": "exit", "side": 0
            }],
            "formations": [ { "id": "staff", "name": "Staff Group", "side": 0 } ],
            "loss_conditions": loss_conditions,
        }),
        vec![
            in_formation(unit_at([3, 0], 0, "medium_tank", "Kuhlmann"), "staff", true),
            in_formation(
                unit_at([4, 0], 0, "medium_tank", "Adjutant"),
                "staff",
                false,
            ),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
    )
}

#[test]
fn a_map_may_declare_that_losing_the_command_formation_loses_the_battle() {
    // Decapitation is map data. The engine always degrades a formation that
    // loses its commander; whether the *battle* is over because of it is a
    // question about what this battle was for, and only the scenario knows.
    let reg = registry();
    let mut state = decapitation_battle(
        &reg,
        serde_json::json!([{ "side": 0, "formation": "staff", "when": "leader_lost" }]),
    );
    let commander = state.formations()[0]
        .founding_leader
        .expect("the map named one");

    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    assert!(!state.is_over(), "the battle is ordinary until she is hit");

    strike_down(&mut state, commander);
    let events = state.step_tick(&reg);
    assert_eq!(
        state.over.map(|r| (r.winner, r.reason)),
        Some((Some(1), EndReason::Decapitated)),
        "her side has lost, whatever is still on the field: {events:?}"
    );
    assert!(
        state.side_units(0).next().is_some(),
        "and it really is a decapitation rather than an elimination — side 0 \
         still has a tank"
    );

    // The negative, and the additivity rule in one line: the same battle, the
    // same dead commander, with the declaration taken out.
    let reg = registry();
    let mut plain = decapitation_battle(&reg, serde_json::json!([]));
    let commander = plain.formations()[0].founding_leader.expect("a commander");
    commit_all(&reg, &mut plain);
    plain.step_tick(&reg);
    strike_down(&mut plain, commander);
    plain.step_tick(&reg);
    assert!(
        !plain.is_over(),
        "a map that says nothing fights on with a new commander"
    );
}

#[test]
fn a_formation_that_withdrew_intact_is_not_a_decapitation() {
    // `wiped` asks whether a formation was destroyed, and driving off the map
    // by a lane your own map wrote down is not being destroyed. Reading
    // `!alive` here — the mistake exits exist to prevent — would end the
    // battle against the side that carried out its withdrawal perfectly.
    let reg = registry();
    let wiped = serde_json::json!([{ "side": 0, "formation": "staff", "when": "wiped" }]);
    let mut state = decapitation_battle(&reg, wiped.clone());
    for (unit, to) in [(UnitId(0), [0, 0]), (UnitId(1), [1, 0])] {
        state
            .apply(
                &reg,
                &Order::SetMove {
                    unit,
                    to: tactics_core::offset_to_hex(to[0], to[1]),
                },
            )
            .expect("the road home is walkable");
    }
    play_round(&reg, &mut state);

    assert!(
        state.units[0].exited() && state.units[1].exited(),
        "the whole staff group got away"
    );
    assert_ne!(
        state.over.map(|r| r.reason),
        Some(EndReason::Decapitated),
        "and leaving is not losing"
    );

    // The other half of the same rule: a formation that leaves a vehicle
    // burning behind it *has* been wiped out, and the condition fires.
    let mut caught = decapitation_battle(&reg, wiped);
    strike_down(&mut caught, UnitId(1));
    caught
        .apply(
            &reg,
            &Order::SetMove {
                unit: UnitId(0),
                to: tactics_core::offset_to_hex(0, 0),
            },
        )
        .expect("the survivor runs for the road");
    play_round(&reg, &mut caught);
    assert_eq!(
        caught.over.map(|r| (r.winner, r.reason)),
        Some((Some(1), EndReason::Decapitated)),
        "one of them died, so the formation was destroyed rather than withdrawn"
    );
}

#[test]
fn a_loss_condition_must_name_a_formation_of_its_own_side() {
    // A loss condition decides a battle, so a typo in one does not look
    // wrong — it quietly makes a scenario unwinnable, or unlosable. Both of
    // these are errors for that reason.
    let reg = registry();
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "misplaced_stakes",
        "palette": { "g": "grass" },
        "rows": ["gg"],
        "shape": "free",
        "sides": [{ "name": "West" }, { "name": "East" }],
        "formations": [ { "id": "staff", "side": 0 } ],
        "units": [
            { "at": [0, 0], "side": 0, "vehicle": "medium_tank", "formation": "staff" },
        ],
        "loss_conditions": [
            { "side": 0, "formation": "ghost_staff", "when": "leader_lost" },
            { "side": 1, "formation": "staff", "when": "wiped" },
        ],
    }))
    .unwrap();
    let mut report = tactics_core::data::ValidationReport::default();
    file.validate_into(&reg, &mut report);
    let errors = report.errors.join("\n");

    assert!(
        errors.contains("`ghost_staff`, which the map does not declare"),
        "a stake on a formation that does not exist can never be settled: {errors}"
    );
    assert!(
        errors.contains("but that formation belongs to side 0"),
        "and a side cannot stake the battle on somebody else's cadets: {errors}"
    );
}

// --- rank, and who commands ----------------------------------------------

/// A registry with a ladder of rank and these characters holding these
/// ranks. Radios stripped, so the command block's symmetric radius is every
/// set's reach and a test can place leaders in or out of it by distance.
fn ranked(radius: Option<u32>, holders: &[(&str, &str)]) -> DataRegistry {
    let mut reg = common::seen(registry());
    // Only the holders this test names: the base mod ranks cadets of its own,
    // and a test about who outranks whom must not inherit them.
    for character in reg.characters.values_mut() {
        character.rank = None;
    }
    reg.ranks = ["sergeant", "lieutenant", "captain"]
        .iter()
        .map(|id| tactics_core::data::RankDef {
            id: id.to_string(),
            name: id.to_string(),
        })
        .collect();
    for (character, rank) in holders {
        reg.characters
            .get_mut(*character)
            .expect("a shipped cadet")
            .rank = Some(rank.to_string());
    }
    reg.command = radius.map(|r| command_rules(r, true, 0));
    strip_radios(&mut reg);
    reg
}

/// A crewed placement: one named cadet, in a formation.
fn crewed(at: [i32; 2], who: &str, formation: &str, leads: bool) -> UnitPlacement {
    let mut placement = in_formation(unit_at(at, 0, "medium_tank", who), formation, leads);
    placement.crew = vec![who.to_string()];
    placement
}

fn chain_stage(
    reg: &DataRegistry,
    placements: Vec<UnitPlacement>,
    formations: &[&str],
) -> BattleState {
    let formations: Vec<serde_json::Value> = formations
        .iter()
        .map(|id| serde_json::json!({ "id": id, "name": id, "side": 0 }))
        .collect();
    scripted_battle(
        reg,
        serde_json::json!({
            "id": "chain",
            "palette": { "g": "grass" },
            "rows": ["g".repeat(60)],
            "formations": formations,
        }),
        placements,
    )
}

/// When a leader falls, the highest rank still fighting takes her place —
/// not the next vehicle in the order of battle. Rank governs who gives the
/// orders (the designer's ruling, 2026-09-23); among equals, and on a mod
/// with no ranks at all, command passes by arrival order as it always did.
#[test]
fn the_highest_rank_takes_over_not_the_next_in_line() {
    let placements = || {
        vec![
            crewed([1, 0], "anka", "platoon", true),
            crewed([2, 0], "juno", "platoon", false),
            crewed([3, 0], "mina", "platoon", false),
        ]
    };
    for (holders, heir) in [
        (vec![("anka", "captain"), ("mina", "lieutenant")], UnitId(2)),
        (vec![], UnitId(1)),
    ] {
        let reg = ranked(None, &holders);
        let mut state = chain_stage(&reg, placements(), &["platoon"]);
        commit_all(&reg, &mut state);
        strike_down(&mut state, UnitId(0));
        state.step_tick(&reg);
        assert_eq!(
            state.formations()[0].leader,
            Some(heir),
            "with ranks {holders:?}, command passes to {heir:?}"
        );
    }
}

/// A connected army is one command under its senior officer, whichever
/// formation she happens to lead and wherever the map wrote it down.
#[test]
fn a_connected_army_is_one_command_under_its_senior_officer() {
    let reg = ranked(Some(99), &[("anka", "lieutenant"), ("mina", "captain")]);
    let state = chain_stage(
        &reg,
        vec![
            crewed([1, 0], "anka", "first", true),
            crewed([8, 0], "juno", "second", true),
            crewed([15, 0], "mina", "third", true),
        ],
        &["first", "second", "third"],
    );
    let commands = state.operational_commands(&reg, 0);
    assert_eq!(commands.len(), 1, "everybody can hear everybody");
    assert_eq!(commands[0].commander, UnitId(2), "the captain commands");
    assert_eq!(commands[0].formations, vec![0, 1, 2]);
}

/// A column the net cannot reach has a commander of its own — its own senior
/// — and a junior who can reach the rest of the army does not command a
/// senior who cannot: they are two commands, each under the senior present.
#[test]
fn a_column_cut_off_has_its_own_senior_in_command() {
    let reg = ranked(Some(6), &[("anka", "captain"), ("juno", "lieutenant")]);
    let state = chain_stage(
        &reg,
        vec![
            // The captain, alone and far away.
            crewed([1, 0], "anka", "detached", true),
            // A lieutenant and a sergeant's column, within reach of each other.
            crewed([30, 0], "juno", "main", true),
            crewed([34, 0], "mina", "rear", true),
        ],
        &["detached", "main", "rear"],
    );
    let commands = state.operational_commands(&reg, 0);
    let shape: Vec<(UnitId, Vec<usize>)> = commands
        .iter()
        .map(|c| (c.commander, c.formations.clone()))
        .collect();
    assert_eq!(
        shape,
        vec![(UnitId(0), vec![0]), (UnitId(1), vec![1, 2])],
        "the captain commands herself; the lieutenant commands the column"
    );
}

/// With no chain of command priced, a side is one command under its most
/// senior leader — the game before, where one brain spoke for the side.
#[test]
fn with_no_chain_of_command_a_side_is_one_command() {
    let reg = ranked(None, &[("juno", "captain")]);
    let state = chain_stage(
        &reg,
        vec![
            crewed([1, 0], "anka", "first", true),
            crewed([50, 0], "juno", "second", true),
        ],
        &["first", "second"],
    );
    let commands = state.operational_commands(&reg, 0);
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].commander, UnitId(1));
}

/// A rank the ladder does not declare is an error, not a silent demotion to
/// the bottom; and the campaign's cadet carries her rank from her
/// definition, because promotion is something a campaign does to a person.
#[test]
fn a_rank_is_declared_before_it_is_held_and_a_cadet_carries_hers() {
    let mut reg = ranked(None, &[("anka", "captain")]);
    let mut report = tactics_core::data::ValidationReport::default();
    reg.validate_into(&mut report);
    assert!(
        report.is_ok(),
        "declared ranks validate: {:?}",
        report.errors
    );

    let mut roster = tactics_core::roster::Roster::new();
    let id = roster.enlist(0, reg.character("anka").unwrap(), &reg);
    assert_eq!(roster.get(id).unwrap().rank.as_deref(), Some("captain"));

    reg.characters.get_mut("juno").unwrap().rank = Some("admiral".into());
    let mut report = tactics_core::data::ValidationReport::default();
    reg.validate_into(&mut report);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("juno") && e.contains("admiral")),
        "an undeclared rank is refused: {:?}",
        report.errors
    );
}

// --- the net is two media --------------------------------------------------

/// Two side-0 formations on a road: Alpha's leader far west, her one member
/// far east beyond any radio — but two hexes from Bravo's leader, who is on
/// the net by definition. With `forest`, a wall of trees stands between that
/// member and Bravo, so nobody can see a flag.
fn signal_stage(reg: &DataRegistry, forest: bool, seed: u64) -> BattleState {
    let mut row: Vec<char> = std::iter::repeat_n('g', 40).collect();
    if forest {
        row[11] = 'f';
    }
    let row: String = row.into_iter().collect();
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "signal_stage",
        "palette": { "g": "grass", "f": "forest" },
        "rows": [row],
        "formations": [
            { "id": "alpha", "name": "Alpha", "side": 0 },
            { "id": "bravo", "name": "Bravo", "side": 0 },
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
    let mut alpha_lead = unit_at([0, 0], 0, "recon_car", "Alpha Lead");
    alpha_lead.formation = Some("alpha".into());
    alpha_lead.leads = true;
    let mut stray = unit_at([12, 0], 0, "recon_car", "Stray");
    stray.formation = Some("alpha".into());
    let mut bravo_lead = unit_at([10, 0], 0, "recon_car", "Bravo Lead");
    bravo_lead.formation = Some("bravo".into());
    bravo_lead.leads = true;
    let enemy = unit_at([38, 0], 1, "recon_car", "Far Foe");
    let placements = vec![alpha_lead, stray, bravo_lead, enemy];
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
fn a_flag_carries_between_formations_where_no_radio_does() {
    // Radio follows the chain of command, but formation membership is
    // irrelevant to seeing a signal flag: a stray twelve hexes from her own
    // leader is on the net through the neighbouring platoon's commander two
    // hexes away — unless a forest stands where the flag would have to be
    // seen, or the mod declares no visual signalling at all.
    let rules = |visual: u32| {
        let mut r = registry();
        let mut rules = command_rules(4, true, 0);
        rules.visual_range = visual;
        r.command = Some(rules);
        strip_radios(&mut r);
        r
    };
    let stray = UnitId(1);
    let contact_of = |reg: &DataRegistry, forest: bool| -> bool {
        let mut state = signal_stage(reg, forest, 6);
        commit_all(reg, &mut state);
        state.step_tick(reg);
        state
            .formations()
            .iter()
            .find(|f| f.id == "alpha")
            .expect("alpha exists")
            .in_contact(stray)
    };

    let flags = rules(2);
    assert!(
        contact_of(&flags, false),
        "the flag reaches her through Bravo's commander"
    );
    assert!(
        !contact_of(&flags, true),
        "but not through a forest: a signal has to be seen"
    );
    let silent = rules(0);
    assert!(
        !contact_of(&silent, false),
        "and a mod that declares no visual medium has none"
    );
}

// --- orders wait instead of dying ------------------------------------------

/// A leader and one crew on an open road, with an enemy parked far enough
/// east to be nobody's business. Under a two-hex radio with nobody relaying,
/// the crew's ten hexes leave her stone deaf, and a test can drive her back
/// onto the net by hand.
fn radio_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
        "id": "radio_stage",
        "palette": { "g": "grass" },
        "rows": ["g".repeat(30)],
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
    let mut leader = unit_at([0, 0], 0, "recon_car", "Leader");
    leader.formation = Some("net".into());
    leader.leads = true;
    let mut crew = unit_at([10, 0], 0, "recon_car", "Stray");
    crew.formation = Some("net".into());
    let enemy = unit_at([29, 0], 1, "recon_car", "Far Foe");
    let placements = vec![leader, crew, enemy];
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

/// A two-hex radio, nobody relaying, no flags: the narrowest net there is, so
/// a cadet ten hexes out is out for a reason a test can state in one line.
fn radio_rules() -> DataRegistry {
    let mut reg = registry();
    reg.command = Some(command_rules(2, false, 0));
    strip_radios(&mut reg);
    reg
}

#[test]
fn an_order_to_a_cut_off_unit_waits_at_the_radio() {
    // The heart of the chunk: an order to a cadet who cannot hear it is
    // *accepted* and held, not refused. Refusing was the old model, and it
    // made the player's only recourse "remember to click again", which is
    // bookkeeping rather than command.
    let reg = radio_rules();
    let mut state = radio_stage(&reg, 4);
    let crew = UnitId(1);
    settle(&reg, &mut state);
    assert!(!state.hears_orders(crew), "ten hexes on a two-hex radio");

    let first = tactics_core::offset_to_hex(13, 0);
    let events = state
        .apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: Some(first),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("accepted, not refused");
    assert_eq!(
        events,
        vec![BattleEvent::OrdersWaiting { unit: crew }],
        "and said out loud once: an order silently parked is as illegible as \
         one silently dropped"
    );
    let hers = state.unit(crew).expect("she is alive");
    assert!(
        hers.intent.path.is_empty() && !hers.planned,
        "not a step of it reached her"
    );
    assert_eq!(
        state
            .command
            .waiting_for(crew)
            .expect("it is at the radio")
            .march
            .map(|m| m.to),
        Some(first),
        "the destination is what is held — never a path, which she will \
         recompute from wherever she actually is"
    );

    // Countermanding something that never went out replaces it rather than
    // queueing behind it. Two orders in the same tray is not a state anybody
    // could act on, exactly as it is not for a formation's mission.
    let second = tactics_core::offset_to_hex(7, 0);
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: Some(second),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("accepted too");
    assert_eq!(
        state.command.waiting_for(crew).unwrap().march.map(|m| m.to),
        Some(second)
    );
    assert_eq!(state.command.waiting().len(), 1, "one slot, one cadet");
}

#[test]
fn waiting_orders_arrive_with_contact_and_are_repathed() {
    // Delivery is at the planning phase — WEGO's bargain is that resolution
    // plays out what was planned — and what is delivered is the destination,
    // re-pathed. She has driven eight hexes since it was given; a route
    // computed back then would walk her through hexes she is nowhere near.
    let reg = radio_rules();
    let mut state = radio_stage(&reg, 4);
    let crew = UnitId(1);
    settle(&reg, &mut state);
    let was = state.unit(crew).expect("alive").pos;

    // Three hexes of grass is a wheeled car's round, and the destination has
    // to be affordable from where she will *be*: nothing about the order was
    // pathable from where she was when it was given, which is the point.
    let to = tactics_core::offset_to_hex(4, 0);
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: Some(to),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("accepted");

    // She closes up on her commander overnight, which is what the whole
    // system is waiting for.
    let beside = tactics_core::offset_to_hex(1, 0);
    state.unit_mut(crew).unwrap().pos = beside;
    let events = settle(&reg, &mut state);
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, BattleEvent::OrdersDelivered { unit } if *unit == crew))
            .count(),
        1,
        "the order lands, once, and says so: {events:?}"
    );
    assert!(
        state.command.waiting().is_empty(),
        "and is not delivered again tomorrow"
    );

    let path = &state.unit(crew).expect("alive").intent.path;
    assert_eq!(
        path.last().copied(),
        Some(to),
        "she is going where she was told"
    );
    assert_eq!(
        path.first().expect("a route").distance_to(beside),
        1,
        "and the first step is from where she is standing now"
    );
    assert!(
        path.first().expect("a route").distance_to(was) > 1,
        "which is nowhere near where she was when it was given ({was:?})"
    );
}

#[test]
fn clearing_reaches_the_radio_but_not_the_girl() {
    // Not sending is free, so taking back an order that never went out needs
    // no contact. Stopping *her* is a different thing entirely: she is
    // driving on her last orders down a wire that is dead, and clearing her
    // intent from here would be the commander countermanding into silence.
    let reg = radio_rules();
    let mut state = radio_stage(&reg, 4);
    let crew = UnitId(1);
    settle(&reg, &mut state);
    assert!(!state.hears_orders(crew));

    // Her own judgment about her own tank, which is what a planner issues and
    // what needs no radio at all.
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: crew,
                to: tactics_core::offset_to_hex(13, 0),
            },
        )
        .expect("a crew decides her own route");
    let hers = state.unit(crew).expect("alive").intent.clone();
    assert!(!hers.path.is_empty(), "she is going somewhere");

    state
        .apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: Some(tactics_core::offset_to_hex(4, 0)),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("accepted, and waiting");
    assert!(state.command.waiting_for(crew).is_some());

    state
        .apply(&reg, &Order::ClearIntent { unit: crew })
        .expect("clearing needs no wire");
    assert!(
        state.command.waiting_for(crew).is_none(),
        "the message never leaves the radio"
    );
    assert_eq!(
        state.unit(crew).expect("alive").intent,
        hers,
        "and she carries on: you cannot reach her to stop her"
    );
}

#[test]
fn a_dead_girl_takes_no_delivery() {
    // An order for somebody who is not coming back is not news, it is an
    // epitaph. The slot goes quietly.
    let reg = radio_rules();
    let mut state = radio_stage(&reg, 4);
    let crew = UnitId(1);
    settle(&reg, &mut state);
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: Some(tactics_core::offset_to_hex(13, 0)),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("accepted");

    strike_down(&mut state, crew);
    let events = settle(&reg, &mut state);
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::OrdersDelivered { .. })),
        "nothing is delivered to a wreck: {events:?}"
    );
    assert!(
        state.command.waiting().is_empty(),
        "and the queue does not carry her for the rest of the battle"
    );
}

#[test]
fn a_radioed_order_to_a_girl_on_the_net_is_just_an_order() {
    // The ordinary case, which has to stay the ordinary code: a commander
    // talking to somebody who can hear her produces exactly what `SetMove`
    // and `SetFire` produce, with nothing held and nothing announced.
    let reg = radio_rules();
    let mut radioed = radio_stage(&reg, 4);
    let mut plain = radio_stage(&reg, 4);
    let leader = UnitId(0);
    let to = tactics_core::offset_to_hex(3, 0);
    let at = tactics_core::offset_to_hex(6, 0);

    let events = radioed
        .apply(
            &reg,
            &Order::Radio {
                unit: leader,
                to: Some(to),
                fire: Some(FireIntent::Area { at, weapon: 0 }),
                latitude: Latitude::Delegated,
            },
        )
        .expect("her own commander, on the net");
    assert!(
        events.is_empty(),
        "nothing waits and nothing is announced: {events:?}"
    );
    assert!(radioed.command.waiting().is_empty());

    plain
        .apply(&reg, &Order::SetMove { unit: leader, to })
        .expect("move");
    plain
        .apply(
            &reg,
            &Order::SetFire {
                unit: leader,
                fire: FireIntent::Area { at, weapon: 0 },
            },
        )
        .expect("fire");
    assert_eq!(
        radioed.unit(leader).unwrap().intent,
        plain.unit(leader).unwrap().intent,
        "the wire changes when an order lands, never what it says"
    );
    assert!(radioed.unit(leader).unwrap().planned);

    // And an order that was never legal is refused to the commander's face
    // whether or not anybody could hear it — the queue must never become a
    // way to smuggle a shot at a friendly past the rules.
    let mut deaf = radio_stage(&reg, 4);
    let crew = UnitId(1);
    settle(&reg, &mut deaf);
    assert!(!deaf.hears_orders(crew));
    assert_eq!(
        deaf.apply(
            &reg,
            &Order::Radio {
                unit: crew,
                to: None,
                fire: Some(FireIntent::Target {
                    target: UnitId(0),
                    weapon: 0
                }),
                latitude: Latitude::Delegated,
            },
        ),
        Err(tactics_core::battle::OrderError::FriendlyTarget)
    );
    assert!(deaf.command.waiting().is_empty(), "and nothing was queued");
}

// --- mission sequences -----------------------------------------------------

#[test]
fn a_plan_advances_when_its_first_leg_is_done() {
    // "Advance to the ford, then hold it." The plan is transmitted once and
    // promoted locally: when a member stands on the first leg's ground, the
    // next leg becomes the standing mission with no wire and no latency —
    // the leader has known the whole plan since it arrived.
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 45).unwrap();
    let armor = formation_named(&state, "kuhlmann_armor");
    let leader = state.formations()[armor.index()].leader.unwrap();
    // A first leg two hexes from where the leader already stands, so one
    // round of driving completes it.
    let start = state.unit(leader).unwrap().pos;
    let near = start + tactics_core::Hex::new(2, 0);
    assert!(state.world.contains(near));
    let hold_at = near;
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: near },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    state
        .apply(
            &reg,
            &Order::QueueMission {
                formation: armor,
                mission: Mission::Hold { at: Some(hold_at) },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    assert_eq!(state.formations()[armor.index()].plan.len(), 1);

    let mut ai = AiDriver::new();
    ai.insert(0, sharp_planner(&reg, 45, "massed_armor"));
    let mut log: Vec<String> = Vec::new();
    for _ in 0..4 {
        if state.is_over() {
            break;
        }
        ai.plan_round(&reg, &mut state);
        let _ = state.apply(&reg, &Order::Commit { side: 1 });
        log.extend(state.resolve_round(&reg).iter().map(|e| format!("{e:?}")));
        if state.formations()[armor.index()].mission == Some(Mission::Hold { at: Some(hold_at) }) {
            break;
        }
    }
    assert_eq!(
        state.formations()[armor.index()].mission,
        Some(Mission::Hold { at: Some(hold_at) }),
        "the second leg is standing once the first is done"
    );
    assert!(
        state.formations()[armor.index()].plan.is_empty(),
        "and the plan has been consumed"
    );
    assert!(
        log.iter().any(|l| l.starts_with("MissionCompleted")),
        "the completion was announced: {log:?}"
    );
}

#[test]
fn nothing_follows_a_stand_fast_or_a_retreat() {
    let reg = registry_wireless();
    let mut state = BattleState::from_map(&reg, "river_crossing", 46).unwrap();
    let armor = formation_named(&state, "kuhlmann_armor");
    let anywhere = state.scenario.objectives()[0].anchor();

    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Hold { at: None },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    assert_eq!(
        state.apply(
            &reg,
            &Order::QueueMission {
                formation: armor,
                mission: Mission::Advance { to: anywhere },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        ),
        Err(tactics_core::battle::OrderError::MissionIsTerminal),
        "a stand-fast has no afterwards"
    );

    // A withdrawal may end a plan — "take the bridge, then get out" is a
    // legitimate raid — but nothing may follow it.
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Advance { to: anywhere },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    state
        .apply(
            &reg,
            &Order::QueueMission {
                formation: armor,
                mission: Mission::Withdraw {
                    via: "west_road".into(),
                },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    assert_eq!(
        state.apply(
            &reg,
            &Order::QueueMission {
                formation: armor,
                mission: Mission::Advance { to: anywhere },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        ),
        Err(tactics_core::battle::OrderError::MissionIsTerminal),
        "and neither has a retreat"
    );
}

#[test]
fn an_amendment_travels_the_wire_like_any_order() {
    // A queued leg is still an order: with a command block it spends its
    // ticks in the air, and a countermand issued while it travels replaces
    // the whole plan — the wire does not care what the envelope says.
    let mut reg = registry();
    reg.command = Some(command_rules(999, true, 2));
    strip_radios(&mut reg);
    let mut state = BattleState::from_map(&reg, "river_crossing", 47).unwrap();
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
    // First order lands (2 ticks), then the amendment goes into the air.
    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    state.step_tick(&reg);
    assert!(state.formations()[armor.index()].mission.is_some());
    state.resolve_round(&reg);

    state
        .apply(
            &reg,
            &Order::QueueMission {
                formation: armor,
                mission: Mission::Recon { toward: bridge },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    let f = &state.formations()[armor.index()];
    assert!(
        matches!(
            f.incoming,
            Some((tactics_core::battle::MissionChange::Append { .. }, _))
        ),
        "the amendment is in the air, not in the plan"
    );
    assert!(f.plan.is_empty());

    // Countermanded before it lands: the replacement wins and the amendment
    // never existed.
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: Mission::Hold { at: None },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);
    state.step_tick(&reg);
    state.step_tick(&reg);
    let f = &state.formations()[armor.index()];
    assert_eq!(f.mission, Some(Mission::Hold { at: None }));
    assert!(f.plan.is_empty(), "the countermand replaced the whole plan");
}

// --- seeing the net --------------------------------------------------------

#[test]
fn the_ring_the_screen_draws_is_the_edge_the_engine_walks() {
    // `radio_reach` exists so the battle screen can draw a leader's range
    // ring without keeping its own copy of the formula. What makes it worth
    // having is that the contact graph reads the same function: set a radius
    // the ring can be counted against, and the cadet one hex inside it is on
    // the net while the one a hex outside is not.
    let mut reg = registry();
    reg.command = Some(command_rules(6, false, 0));
    strip_radios(&mut reg);
    let mut state = radio_stage(&reg, 3);

    let leader = UnitId(0);
    let stray = UnitId(1);
    assert_eq!(
        state.radio_reach(&reg, leader),
        Some(6),
        "no radio hardware and no signals coefficient: the block's own radius"
    );

    // The stray sits ten hexes out in `radio_stage`; walk her to the ring and
    // then one hex past it, and contact follows the number the ring is drawn
    // at rather than any second opinion.
    let on_the_ring = state.unit(leader).expect("leader").pos + tactics_core::Hex::new(6, 0);
    state.units[stray.index()].pos = on_the_ring;
    settle(&reg, &mut state);
    assert!(
        state.formations()[0].in_contact(stray),
        "a cadet standing on the ring hears her leader"
    );

    state.units[stray.index()].pos = on_the_ring + tactics_core::Hex::new(1, 0);
    settle(&reg, &mut state);
    assert!(
        !state.formations()[0].in_contact(stray),
        "and one hex beyond it she does not"
    );

    // Nothing to draw where nothing is priced: the same window answers `None`
    // for a mod with no chain of command, which is what keeps the display's
    // additivity story the same as the engine's.
    reg.command = None;
    assert_eq!(state.radio_reach(&reg, leader), None);
}

// --- who is entitled to hear what -------------------------------------------

//
// `Event::heard_by` spent its life in the game crate, where none of these
// could reach it: the presentation layer is not linked into the engine's test
// binaries, so the one rule deciding what the enemy is allowed to overhear was
// the only rule in the battle with no test at all.

/// Fighting happens in the open, and both sides fight the same battle.
///
/// Whether the *unit* can be seen is the fog's question and has already been
/// asked by the time an event exists. This is the other half of the rule, and
/// it is the half that must stay permissive: an over-tight audience here would
/// silently drop shots and wrecks out of the log, which reads as the game
/// freezing rather than as a fog rule working.
#[test]
fn what_happens_in_the_open_is_heard_by_both_sides() {
    let reg = registry();
    let state = two_side_battle(
        &reg,
        &["gggggfggggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "West"),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
        7,
    );
    let west = state.units[0].id;

    for event in [
        BattleEvent::RoundStarted { round: 1 },
        BattleEvent::TickStarted { tick: 0 },
        BattleEvent::BrewedUp { unit: west },
        BattleEvent::Abandoned { unit: west },
        BattleEvent::UnitDestroyed {
            unit: west,
            at: state.units[0].pos,
        },
        BattleEvent::ObjectiveTaken {
            objective: "bridge".into(),
            side: Some(0),
            at: state.units[0].pos,
        },
    ] {
        assert!(
            event.heard_by(&state, 0) && event.heard_by(&state, 1),
            "{event:?} happens in the open and belongs to nobody's net"
        );
    }
}

/// What is inside her hull, and what her radio is doing, is hers.
///
/// How much ammunition she has left is her quartermaster's secret rather than
/// something the sound of her gun gives away, and what is broken or bleeding
/// in there even more so.
#[test]
fn a_crews_own_net_is_not_read_out_to_the_enemy() {
    let reg = registry();
    let state = two_side_battle(
        &reg,
        &["gggggfggggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "West"),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
        7,
    );
    let west = state.units[0].id;
    assert_eq!(state.units[0].side, 0, "the stage puts West on side 0");

    for event in [
        BattleEvent::WeaponDry {
            unit: west,
            weapon: "gun_75".into(),
        },
        BattleEvent::ModuleHit {
            unit: west,
            module: "optics".into(),
            destroyed: true,
        },
        BattleEvent::OutOfContact { unit: west },
        BattleEvent::OrdersWaiting { unit: west },
        BattleEvent::OrdersDelivered { unit: west },
        BattleEvent::ContactRestored { unit: west },
        BattleEvent::TookCover {
            unit: west,
            at: state.units[0].pos,
        },
    ] {
        assert!(
            event.heard_by(&state, 0),
            "{event:?} is her own side's business and hers to hear"
        );
        assert!(
            !event.heard_by(&state, 1),
            "{event:?} is on her net and the enemy is not on it"
        );
    }
}

/// A spot report belongs to the crew who made it, not to the crew reported.
///
/// The unit being reported is by definition the other side's, so reading
/// `unit` here instead of `by` would invert the rule and hand every contact
/// report straight to the side being looked at.
#[test]
fn a_contact_report_belongs_to_the_crew_who_made_it() {
    let reg = registry();
    let state = two_side_battle(
        &reg,
        &["gggggfggggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "West"),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
        7,
    );
    let west = state.units[0].id;
    let east = state.units[1].id;
    let report = BattleEvent::ContactReported {
        unit: east,
        by: west,
        at: state.units[1].pos,
    };
    assert!(report.heard_by(&state, 0), "West reported it");
    assert!(
        !report.heard_by(&state, 1),
        "East does not get told she has been seen"
    );
}

/// Her nerve is inside the hull with everything else.
///
/// You can see her tank reverse out of the line and draw your own conclusion
/// — `UnitMoved` is side-blind and the sprite does it in front of you — but
/// you cannot read the rung she is standing on. `CrewHit` and `ModuleHit`
/// already worked this way; morale was the lone exception, so the player's log
/// printed "Wotan 3: Breaking — not going forward" about an enemy crew.
#[test]
fn an_enemy_crews_nerve_is_not_readable_from_across_the_field() {
    let reg = registry();
    let state = two_side_battle(
        &reg,
        &["gggggfggggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "West"),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
        7,
    );
    let west = state.units[0].id;

    for event in [
        BattleEvent::MoraleChanged {
            unit: west,
            rung: "breaking".into(),
            obeys: false,
        },
        BattleEvent::Defied {
            unit: west,
            rung: "breaking".into(),
            doing: "reversing out of it".into(),
            to: Some(state.units[0].pos),
        },
    ] {
        assert!(
            event.heard_by(&state, 0),
            "{event:?} is her own commander's to know"
        );
        assert!(
            !event.heard_by(&state, 1),
            "{event:?} is not readable from the other side of the field"
        );
    }

    // ...and the deed still is. A crew reversing out of the line is a thing
    // that visibly happens, so the enemy is not being denied the *event*, only
    // the reading of her nerve.
    let driving_off = BattleEvent::UnitMoved {
        unit: west,
        path: vec![state.units[0].pos],
    };
    assert!(
        driving_off.heard_by(&state, 0) && driving_off.heard_by(&state, 1),
        "the tank reversing is visible to anybody who can see her"
    );
}

/// A spot belongs to the side that made it, and being found is not something
/// the found party is told.
///
/// She learns it when the shooting starts. This was answered in the renderer
/// until the audience rule moved into core, and answered there by a *different
/// question* — whether the spotting side had no AI on it, rather than whether
/// it was the side being drawn for. Those agree while exactly one side is
/// human-controlled and part company as soon as two are, which is why the
/// stage below puts `ai: None` on both.
#[test]
fn being_found_is_not_something_the_found_crew_is_told() {
    let reg = registry();
    let state = two_side_battle(
        &reg,
        &["gggggfggggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "West"),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
        7,
    );
    let east = state.units[1].id;
    // West has picked East out of the ground.
    let spot = BattleEvent::UnitSpotted {
        unit: east,
        by_side: 0,
        at: state.units[1].pos,
    };
    assert!(spot.heard_by(&state, 0), "West found her and knows it");
    assert!(
        !spot.heard_by(&state, 1),
        "East is not sent a note saying she has been seen"
    );
}

/// A formation's orders are its own side's business.
///
/// Assigned, received, completed, and who is commanding it now: four events
/// keyed by formation rather than by unit, so they need the other half of the
/// lookup and would go side-blind if that half were dropped.
#[test]
fn a_formations_orders_are_not_overheard_by_the_enemy() {
    let reg = registry_wireless();
    let file = reg.map("river_crossing").expect("shipped battle map");
    let map = Battlefield::from_map_file(file).expect("map parses");
    let placement = |col: i32, side: u8, formation: &str, leads: bool| UnitPlacement {
        aboard_at: None,
        at: [col, 20],
        side,
        vehicle: "medium_tank".into(),
        crew: Vec::new(),
        name: Some(format!("{formation}-{col}")),
        facing: None,
        formation: Some(formation.into()),
        leads,
    };
    let placements = vec![
        placement(10, 0, "kuhlmann_armor", true),
        placement(11, 0, "kuhlmann_armor", false),
        placement(30, 1, "valkyrie_line", true),
    ];
    let sides = vec![
        SideState {
            name: "Kuhlmann".into(),
            ai: None,
        },
        SideState {
            name: "Valkyries".into(),
            ai: None,
        },
    ];
    let (roster, crews) = tactics_core::roster::Roster::stamp_for(&reg, &placements);
    let state = BattleState::from_placements(
        &reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        1,
    )
    .expect("the staged placements are content the base mod ships");

    let mission = Mission::Hold {
        at: Some(state.units[0].pos),
    };
    for event in [
        BattleEvent::MissionAssigned {
            formation: "kuhlmann_armor".into(),
            mission: mission.clone(),
        },
        BattleEvent::MissionReceived {
            formation: "kuhlmann_armor".into(),
            mission: mission.clone(),
        },
        BattleEvent::MissionCompleted {
            formation: "kuhlmann_armor".into(),
            mission: mission.clone(),
        },
        BattleEvent::CommandPassed {
            formation: "kuhlmann_armor".into(),
            from: state.units[0].id,
            to: state.units[1].id,
        },
    ] {
        assert!(event.heard_by(&state, 0), "{event:?} is Kuhlmann's traffic");
        assert!(
            !event.heard_by(&state, 1),
            "{event:?} is Kuhlmann's traffic and the Valkyries are not on that net"
        );
    }
}

/// An event naming a unit this battle has never heard of is heard by
/// everybody.
///
/// The safe direction for a stray: saying too much in a log is a bug somebody
/// notices and reports, and silently swallowing events because a lookup missed
/// is a bug that looks like the game having stopped.
#[test]
fn an_event_about_nobody_is_not_silently_swallowed() {
    let reg = registry();
    let state = two_side_battle(
        &reg,
        &["gggggfggggg"],
        vec![
            unit_at([0, 0], 0, "medium_tank", "West"),
            unit_at([10, 0], 1, "medium_tank", "East"),
        ],
        7,
    );
    let nobody = UnitId(9999);
    let event = BattleEvent::OutOfContact { unit: nobody };
    assert!(event.heard_by(&state, 0) && event.heard_by(&state, 1));

    let orphan = BattleEvent::MissionAssigned {
        formation: "no_such_formation".into(),
        mission: Mission::Hold {
            at: Some(state.units[0].pos),
        },
    };
    assert!(orphan.heard_by(&state, 0) && orphan.heard_by(&state, 1));
}
