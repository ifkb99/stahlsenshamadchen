//! Saving must preserve the future, not just the present.
//!
//! A save that restores the right positions and hit points but resumes with a
//! fresh rng would play out differently from the run that produced it. Every
//! other guarantee in this engine — replays, search-based AI, the determinism
//! baseline — rests on that not happening, so these tests compare what happens
//! *after* the reload rather than what the file contains.

use std::path::PathBuf;
use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, make_battle_planner};
use tactics_core::battle::{BattleState, Event, Latitude, Order};
use tactics_core::data::DataRegistry;
use tactics_core::overworld::{ArmyMission, OverworldOrder, OverworldState};
use tactics_core::roster::CadetStatus;
use tactics_core::save::{SAVE_VERSION, SaveGame};

fn registry() -> DataRegistry {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (registry, report) = DataRegistry::load_dir(&root).expect("mods load");
    assert!(
        report.is_ok(),
        "base mod must validate: {:?}",
        report.errors
    );
    registry
}

/// The same game with the search switched off, exactly as `engine.rs`'s twin
/// of this: a stage that puts a battery in sight of its quarry, or a crew on
/// a bridge for a round, must not also be a test of whether anybody happened
/// to find anybody on the tick it was set up. `detection_certain_percent` at
/// 100 is the rule's neutral value, so this is its absence rather than a
/// gentle version of it.
fn seen(mut reg: DataRegistry) -> DataRegistry {
    reg.balance.detection_certain_percent = 100;
    reg
}

fn planner(reg: &DataRegistry, seed: u64) -> Box<dyn AiPlanner<BattleState, Order>> {
    make_battle_planner(
        &AiConfig {
            planner: "utility".into(),
            difficulty: 3,
            doctrine: Some("massed_armor".into()),
        },
        seed,
        reg,
    )
}

/// Play a few rounds, and describe what happened.
fn play(reg: &DataRegistry, state: &mut BattleState, rounds: usize, seed: u64) -> Vec<String> {
    let mut ai = AiDriver::new();
    ai.insert(0, planner(reg, seed));
    ai.insert(1, planner(reg, seed + 1));
    let mut log = Vec::new();
    for _ in 0..rounds {
        if state.is_over() {
            break;
        }
        ai.plan_round(reg, state);
        for event in state.resolve_round(reg) {
            if !matches!(event, Event::TickStarted { .. }) {
                log.push(format!("{event:?}"));
            }
        }
    }
    log
}

#[test]
fn round_trips_and_keeps_the_future_identical() {
    let reg = registry();

    // Run a battle a few rounds in, then fork it: one copy carries straight
    // on, the other goes through a save file first.
    let mut original = BattleState::from_map(&reg, "river_crossing", 11).expect("battle");
    play(&reg, &mut original, 3, 7);

    let text = SaveGame::new(&reg, None, Some(original.clone()))
        .to_json()
        .expect("serialises");
    let mut restored = SaveGame::from_json(&reg, &text)
        .expect("deserialises")
        .0
        .battle
        .expect("battle round-trips");

    // The board matches...
    assert_eq!(restored.round, original.round);
    assert_eq!(restored.units.len(), original.units.len());
    for (a, b) in restored.units.iter().zip(&original.units) {
        assert_eq!(
            (a.id, a.pos, &a.crew_state, a.alive, a.facing),
            (b.id, b.pos, &b.crew_state, b.alive, b.facing)
        );
    }

    // ...and, the point of the exercise, so does everything that happens next.
    let mut original = original;
    let expected = play(&reg, &mut original, 5, 7);
    let actual = play(&reg, &mut restored, 5, 7);
    assert_eq!(
        actual, expected,
        "a reloaded battle must play out exactly as the unsaved one would"
    );
    assert!(
        !expected.is_empty(),
        "the test needs the battle to still be live"
    );
}

/// The sight grid is left out of the file on purpose, so loading has to put it
/// back. An empty one answers every line-of-sight question wrongly rather than
/// failing, which is the sort of bug that would surface as "the AI has gone
/// blind" three systems away.
#[test]
fn loading_rebuilds_the_sight_grid_that_the_save_leaves_out() {
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 3).expect("battle");
    assert!(!state.sight.is_empty(), "a fresh battle has its sight grid");

    let text = SaveGame::new(&reg, None, Some(state.clone()))
        .to_json()
        .unwrap();
    // The sight grid serialises to an empty object because its only field is
    // skipped. (Checking for "tiles" would not work: `HexMap` has a field by
    // that name which genuinely is saved.)
    assert!(
        text.contains("\"sight\": {}"),
        "the sight grid should be empty in the file, not carried"
    );

    let restored = SaveGame::from_json(&reg, &text).unwrap().0.battle.unwrap();
    assert!(!restored.sight.is_empty(), "loading must rebuild it");

    // And it must be the *same* grid, not merely a non-empty one. Sampled
    // over a spread of pairs rather than every pair, which would be a million
    // line-of-sight tests on a 1261-tile map.
    let mut hexes: Vec<_> = restored.map.iter().map(|(h, _)| h).collect();
    hexes.sort_by_key(|h| (h.x, h.y));
    let mut checked = 0;
    for from in hexes.iter().step_by(37) {
        for to in hexes.iter().step_by(53) {
            assert_eq!(
                restored.sight.clear(*from, *to),
                state.sight.clear(*from, *to),
                "sight differs at {from:?} -> {to:?}"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 500,
        "sampled too little to mean anything: {checked}"
    );
}

#[test]
fn a_save_written_before_objectives_existed_still_opens() {
    // A content patch must not cost a player their campaign, so the two
    // fields objectives added carry `#[serde(default)]` — and `rehydrate`
    // sizes them from the map afterwards, because scoring indexes through
    // them and a short vector would panic rather than simply score nothing.
    let reg = registry();
    let state = BattleState::from_map(&reg, "river_crossing", 7).expect("battle");
    let text = SaveGame::new(&reg, None, Some(state)).to_json().unwrap();

    let mut old: serde_json::Value = serde_json::from_str(&text).unwrap();
    let battle = old["battle"].as_object_mut().unwrap();
    battle.remove("objective_held").expect("field is saved");
    battle.remove("score").expect("field is saved");

    let restored = SaveGame::from_json(&reg, &old.to_string())
        .expect("a save from before objectives must still load")
        .0
        .battle
        .expect("battle survives");
    assert_eq!(
        restored.objective_held.len(),
        restored.map.objectives().len(),
        "control must be sized against the map it was loaded with"
    );
    assert_eq!(restored.score.len(), restored.sides.len());
    assert!(restored.leader().is_none(), "and nobody has scored yet");
}

#[test]
fn a_battle_saved_mid_fight_remembers_who_holds_the_ground() {
    let reg = seen(registry());
    let mut state = BattleState::from_map(&reg, "river_crossing", 4).expect("battle");
    // Put someone on the bridge and let a round pay out, so there is control
    // and a score to lose rather than two empty vectors.
    let bridge = state.map.objectives()[0].anchor();
    let unit = state.units[0].id;
    state.unit_mut(unit).unwrap().pos = bridge;
    play(&reg, &mut state, 1, 4);
    assert_eq!(state.objective_held[0], Some(state.units[0].side));
    assert!(state.score(state.units[0].side) > 0, "a round paid out");

    let text = SaveGame::new(&reg, None, Some(state.clone()))
        .to_json()
        .unwrap();
    let restored = SaveGame::from_json(&reg, &text).unwrap().0.battle.unwrap();
    assert_eq!(restored.objective_held, state.objective_held);
    assert_eq!(restored.score, state.score);
}

/// The chain of command is state, not a cache, so unlike the sight grid it
/// has to survive in the file itself — and unlike objective control it needs
/// nothing put back on load, because it is plain data with no map-sized index
/// behind it. Both halves of that are checked here: it round-trips exactly,
/// and a save written before formations existed still opens.
#[test]
fn a_saved_battle_remembers_who_answers_to_whom() {
    let reg = registry();
    let mut original = BattleState::from_map(&reg, "river_crossing", 12).expect("battle");
    assert!(
        !original.formations().is_empty(),
        "river_crossing declares formations, or this test proves nothing"
    );
    // Mid-fight rather than at setup: a save is taken from a battle in
    // progress, and command state must not quietly depend on being fresh.
    play(&reg, &mut original, 2, 9);

    let text = SaveGame::new(&reg, None, Some(original.clone()))
        .to_json()
        .expect("serialises");
    let restored = SaveGame::from_json(&reg, &text)
        .expect("deserialises")
        .0
        .battle
        .expect("battle round-trips");
    assert_eq!(
        restored.command, original.command,
        "every formation, leader and member must come back exactly"
    );

    // And a save from before any of this existed loads as what it was: one
    // flat pool per side, with nothing for `rehydrate` to rebuild.
    let mut old: serde_json::Value = serde_json::from_str(&text).unwrap();
    old["battle"]
        .as_object_mut()
        .unwrap()
        .remove("command")
        .expect("field is saved");
    let older = SaveGame::from_json(&reg, &old.to_string())
        .expect("a save from before formations must still load")
        .0
        .battle
        .expect("battle survives");
    assert!(
        older.formations().is_empty(),
        "no chain of command is a legal state, not a broken one"
    );
}

/// A mission is the longest-lived thing a side ever says: it survives the
/// round it was given in, so it had better survive the save taken during that
/// round too. It needs nothing put back on load — plain data, no map-sized
/// index behind it — which is precisely the claim this checks, since a field
/// that silently came back `None` would look exactly like a formation nobody
/// had ordered yet.
#[test]
fn a_standing_mission_survives_being_saved_and_reloaded() {
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 15).expect("battle");
    let recon = tactics_core::battle::FormationId(
        state
            .formations()
            .iter()
            .position(|f| f.id == "kuhlmann_recon")
            .expect("river_crossing declares it") as u32,
    );
    let bridge = state.map.objectives()[0].anchor();
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: recon,
                mission: tactics_core::battle::Mission::Advance { to: bridge },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("the bridge is on the map");
    // Mid-fight, and past the round the order was given in: a save taken two
    // rounds later must still know what the platoon was told.
    play(&reg, &mut state, 2, 15);

    let text = SaveGame::new(&reg, None, Some(state.clone()))
        .to_json()
        .expect("serialises");
    let restored = SaveGame::from_json(&reg, &text)
        .expect("deserialises")
        .0
        .battle
        .expect("battle round-trips");
    assert_eq!(
        restored.formations()[recon.index()].mission,
        Some(tactics_core::battle::Mission::Advance { to: bridge }),
        "a reloaded formation is still under orders"
    );
    assert_eq!(restored.command, state.command);

    // And a save written before missions existed opens as a formation that
    // simply has not been told anything, which is a legal state.
    let mut old: serde_json::Value = serde_json::from_str(&text).unwrap();
    for formation in old["battle"]["command"]["formations"]
        .as_array_mut()
        .expect("command state is saved")
    {
        formation
            .as_object_mut()
            .unwrap()
            .remove("mission")
            .expect("field is saved");
    }
    let older = SaveGame::from_json(&reg, &old.to_string())
        .expect("a save from before missions must still load")
        .0
        .battle
        .expect("battle survives");
    assert!(
        older.formations().iter().all(|f| f.mission.is_none()),
        "no orders is a state, not a broken file"
    );
}

/// The campaign is the half a player would actually mind losing: a cadet with
/// nine battles behind her and a wound that has three days left on it.
#[test]
fn a_campaign_keeps_its_girls_and_their_scars() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 5).expect("overworld");
    let cadet = state.side_armies(0).next().unwrap().units[0].crew[0];
    state.roster.get_mut(cadet).unwrap().battles = 9;
    state.roster.get_mut(cadet).unwrap().xp = 250;
    state.roster.get_mut(cadet).unwrap().status = CadetStatus::Wounded { days: 3 };

    let text = SaveGame::new(&reg, Some(state.clone()), None)
        .to_json()
        .unwrap();
    let mut restored = SaveGame::from_json(&reg, &text)
        .unwrap()
        .0
        .overworld
        .unwrap();

    let back = restored
        .roster
        .get(cadet)
        .expect("she is still on the roster");
    assert_eq!(back.battles, 9);
    assert_eq!(back.xp, 250);
    assert_eq!(back.status, CadetStatus::Wounded { days: 3 });
    assert_eq!(back.owner, 0, "and still belongs to her academy");

    // Armies still point at her, rather than at a dangling handle.
    assert_eq!(
        restored.side_armies(0).next().unwrap().units[0].crew[0],
        cadet
    );

    // The recovery clock carries on from where it was, not from the start.
    restored.roster.advance_day();
    assert_eq!(
        restored.roster.get(cadet).unwrap().status,
        CadetStatus::Wounded { days: 2 }
    );
}

/// Standing orders are the campaign's version of the property missions already
/// have on the battlefield: they outlive the turn they were given in, which is
/// only true if they outlive the save file too. The wire has to survive with
/// them — an army restored into contact it does not have would accept an order
/// nobody could deliver.
#[test]
fn a_campaign_keeps_its_standing_orders_and_its_silences() {
    let reg = registry();
    let mut state = OverworldState::from_map(&reg, "frontier", 3).expect("overworld");
    let army = state.senior_army(0).expect("side 0 has armies");
    let to = tactics_core::offset_to_hex(6, 2);
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army,
                mission: ArmyMission::Advance { to },
            },
        )
        .expect("her own army, within her own net");
    let cut_off = state.out_of_contact.clone();
    assert!(
        !cut_off.is_empty(),
        "frontier's junior companies start outside the four-hex net, which is \
         what makes this test worth writing"
    );
    // And an order for one of those companies, which headquarters is holding
    // until it can be transmitted. A save that dropped it would leave the
    // player waiting on an order that no longer exists anywhere.
    let deaf = cut_off[0];
    state
        .apply(
            &reg,
            &OverworldOrder::SetMission {
                army: deaf,
                mission: ArmyMission::Hold,
            },
        )
        .expect("accepted, and waiting for a wire");
    assert_eq!(state.waiting_missions, vec![(deaf, ArmyMission::Hold)]);

    let text = SaveGame::new(&reg, Some(state), None).to_json().unwrap();
    let restored = SaveGame::from_json(&reg, &text)
        .unwrap()
        .0
        .overworld
        .expect("campaign survives");

    assert_eq!(
        restored.army(army).unwrap().mission,
        Some(ArmyMission::Advance { to }),
        "she is still advancing on the same place"
    );
    assert_eq!(
        restored.out_of_contact, cut_off,
        "and the same companies are still off the net"
    );
    assert_eq!(
        restored.waiting_missions,
        vec![(deaf, ArmyMission::Hold)],
        "with the same order still in the tray"
    );
    for id in &cut_off {
        assert!(!restored.in_contact(*id));
    }

    // A campaign saved before any of this existed opens as one with no orders
    // and nobody cut off, which is what it was.
    let mut older: serde_json::Value = serde_json::from_str(&text).unwrap();
    let overworld = older
        .get_mut("overworld")
        .and_then(|o| o.as_object_mut())
        .expect("the campaign is in there");
    overworld.remove("out_of_contact").expect("field is saved");
    overworld
        .remove("waiting_missions")
        .expect("field is saved");
    for army in overworld
        .get_mut("armies")
        .and_then(|a| a.as_array_mut())
        .expect("armies are a list")
    {
        army.as_object_mut().unwrap().remove("mission");
    }
    let older = SaveGame::from_json(&reg, &older.to_string())
        .expect("a save from before campaign missions must still load")
        .0
        .overworld
        .expect("campaign survives");
    assert!(older.armies.iter().all(|a| a.mission.is_none()));
    assert!(older.out_of_contact.is_empty());
    assert!(older.waiting_missions.is_empty());
}

#[test]
fn a_save_from_another_version_is_refused_rather_than_misread() {
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 1).expect("overworld");
    let text = SaveGame::new(&reg, Some(state), None)
        .to_json()
        .unwrap()
        .replace(
            &format!("\"version\": {SAVE_VERSION}"),
            &format!("\"version\": {}", SAVE_VERSION + 1),
        );
    assert!(
        SaveGame::from_json(&reg, &text).is_err(),
        "a future save must be refused, not silently half-read"
    );
}

/// Difficulty is a mod in this project — whether crews bail out, whether a
/// cadet can refuse an order, whether death is permanent. So a save has to
/// remember which rules it was played under, or a campaign started gentle
/// could come back lethal without anyone being told.
#[test]
fn a_save_remembers_which_mods_were_playing() {
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 4).expect("overworld");
    let text = SaveGame::new(&reg, Some(state), None).to_json().unwrap();

    let (save, warnings) = SaveGame::from_json(&reg, &text).expect("its own mods load fine");
    assert!(warnings.is_empty(), "nothing has changed: {warnings:?}");
    assert!(
        save.mods.iter().any(|m| m.id == "base"),
        "the base mod should be stamped: {:?}",
        save.mods
    );

    // A save from a game that also had a difficulty mod loaded is refused,
    // because the rules it was played under are not the rules now.
    let harsher = text.replace(
        "\"mods\": [",
        "\"mods\": [\n    { \"id\": \"ironman\", \"version\": \"1.0.0\" },",
    );
    let err = SaveGame::from_json(&reg, &harsher).expect_err("should refuse");
    assert!(
        format!("{err}").contains("ironman"),
        "the error should name what is missing: {err}"
    );
}

/// A content patch must not break saves, or every balance tweak costs the
/// player their campaign.
#[test]
fn a_version_bump_warns_rather_than_refusing() {
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 6).expect("overworld");
    let text = SaveGame::new(&reg, Some(state), None)
        .to_json()
        .unwrap()
        .replace("\"version\": \"0.1.0\"", "\"version\": \"0.0.9\"");

    let (save, warnings) = SaveGame::from_json(&reg, &text).expect("still loads");
    assert!(save.overworld.is_some());
    assert_eq!(warnings.len(), 1, "should say so: {warnings:?}");
    assert!(warnings[0].contains("base"), "{warnings:?}");
}

/// Saves written before mods were stamped cannot be checked, and refusing them
/// would be worse than trusting them.
#[test]
fn a_save_from_before_this_existed_is_still_accepted() {
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 8).expect("overworld");
    let text = SaveGame::new(&reg, Some(state), None).to_json().unwrap();
    let stripped: serde_json::Value = {
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v.as_object_mut().unwrap().remove("mods");
        v
    };
    let (save, warnings) =
        SaveGame::from_json(&reg, &stripped.to_string()).expect("older saves still load");
    assert!(save.overworld.is_some());
    assert!(warnings.is_empty());
}

/// An order in transit is the most fragile thing command state carries: it is
/// a countdown, and a save that restored the mission but forgot how far it had
/// travelled would either deliver it twice or never. So this is the
/// fork-and-compare property again, aimed at the clock rather than the board —
/// the restored battle has to hear the order on the same tick the unsaved one
/// does.
#[test]
fn a_mission_in_transit_survives_a_save() {
    let mut reg = registry();
    // Declared here rather than in the base mod: this chunk is the machinery,
    // and the shipped game switches it on a chunk later. `levels_per_tick: 0`
    // so the delay is the stated three ticks whoever is commanding.
    reg.command = Some(tactics_core::data::CommandRules {
        radius: 999,
        visual_range: 0,
        radius_per_signals: 0,
        relay: true,
        overworld_radius: 999,
        review: tactics_core::data::ReactionRules {
            skill: "command".into(),
            base_ticks: 0,
            levels_per_tick: 0,
            max_ticks: 0,
        },
        latency: tactics_core::data::ReactionRules {
            skill: "command".into(),
            base_ticks: 3,
            levels_per_tick: 0,
            max_ticks: 5,
        },
    });
    let mut original = BattleState::from_map(&reg, "river_crossing", 19).expect("battle");
    let armor = tactics_core::battle::FormationId(
        original
            .formations()
            .iter()
            .position(|f| f.id == "kuhlmann_armor")
            .expect("river_crossing declares it") as u32,
    );
    let bridge = original.map.objectives()[0].anchor();
    original
        .apply(
            &reg,
            &Order::SetMission {
                formation: armor,
                mission: tactics_core::battle::Mission::Advance { to: bridge },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("the bridge is on the map");

    // One tick in: the order is still travelling, which is the state worth
    // saving.
    for side in original.living_sides() {
        if !original.has_committed(side) {
            original
                .apply(&reg, &Order::Commit { side })
                .expect("commit");
        }
    }
    original.step_tick(&reg);
    assert!(
        original.formations()[armor.index()].mission.is_none(),
        "it has not landed yet, or this test is about nothing"
    );
    let in_flight = original.formations()[armor.index()]
        .incoming
        .clone()
        .expect("a mission in the air");

    let text = SaveGame::new(&reg, None, Some(original.clone()))
        .to_json()
        .expect("serialises");
    let mut restored = SaveGame::from_json(&reg, &text)
        .expect("deserialises")
        .0
        .battle
        .expect("battle round-trips");
    assert_eq!(
        restored.formations()[armor.index()].incoming,
        Some(in_flight),
        "the same mission with the same ticks left on it"
    );

    // And it lands on schedule in both, which is the property that matters:
    // two ticks later, in the same tick, for the same reason.
    let landed = |state: &mut BattleState| -> Vec<u32> {
        let mut ticks = Vec::new();
        for tick in 0..4u32 {
            let events = state.step_tick(&reg);
            if events
                .iter()
                .any(|e| matches!(e, Event::MissionReceived { .. }))
            {
                ticks.push(tick);
            }
        }
        ticks
    };
    let expected = landed(&mut original);
    assert_eq!(expected, vec![1], "two ticks of the three had already run");
    assert_eq!(
        landed(&mut restored),
        expected,
        "a reloaded order arrives exactly when the unsaved one would"
    );
    assert_eq!(
        restored.formations()[armor.index()].mission,
        original.formations()[armor.index()].mission,
        "and leaves both platoons under the same standing order"
    );
}

/// An order held at the radio is the other thing command state carries that
/// nobody has said out loud yet: the formation panel shows it, the next
/// planning phase delivers it, and a save that forgot it would leave the
/// player waiting for an order that no longer exists anywhere.
#[test]
fn an_order_waiting_at_the_radio_survives_a_save() {
    let mut reg = registry();
    // A one-hex net with nobody relaying, so somebody in a platoon that
    // deploys strung out is certainly deaf. Declared here rather than in the
    // base mod for the same reason the latency test does it: this is about the
    // machinery, not about what the shipped game switches on.
    reg.command = Some(tactics_core::data::CommandRules {
        radius: 1,
        visual_range: 0,
        radius_per_signals: 0,
        relay: false,
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
            max_ticks: 5,
        },
    });
    for vehicle in reg.vehicles.values_mut() {
        vehicle.radio = None;
    }
    let mut state = BattleState::from_map(&reg, "river_crossing", 21).expect("battle");
    // One quiet round, because contact is computed during resolution.
    for side in state.living_sides() {
        state.apply(&reg, &Order::Commit { side }).expect("commit");
    }
    state.resolve_round(&reg);

    let deaf = state
        .units
        .iter()
        .filter(|u| u.alive && u.side == 0)
        .map(|u| u.id)
        .find(|id| !state.hears_orders(*id))
        .expect("a one-hex net leaves somebody outside it");
    let bridge = state.map.objectives()[0].anchor();
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: deaf,
                to: Some(bridge),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("accepted, and waiting for a wire");
    assert!(state.command.waiting_for(deaf).is_some());

    let text = SaveGame::new(&reg, None, Some(state.clone()))
        .to_json()
        .expect("serialises");
    let restored = SaveGame::from_json(&reg, &text)
        .expect("deserialises")
        .0
        .battle
        .expect("battle round-trips");
    assert_eq!(
        restored.command, state.command,
        "the queue comes back with everything else the wire knows"
    );
    assert_eq!(
        restored
            .command
            .waiting_for(deaf)
            .expect("still at the radio")
            .destination,
        Some(bridge),
    );

    // And a save written before orders could wait opens as a battle in which
    // none are, which is what it was.
    let mut old: serde_json::Value = serde_json::from_str(&text).unwrap();
    old["battle"]["command"]
        .as_object_mut()
        .unwrap()
        .remove("waiting")
        .expect("field is saved");
    let older = SaveGame::from_json(&reg, &old.to_string())
        .expect("a save from before the queue must still load")
        .0
        .battle
        .expect("battle survives");
    assert!(older.command.waiting().is_empty());
}

// --- everything the wire carries, in one save ------------------------------

fn unit_at(at: [i32; 2], side: u8, vehicle: &str, name: &str) -> tactics_core::map::UnitPlacement {
    tactics_core::map::UnitPlacement {
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

fn in_formation(
    mut placement: tactics_core::map::UnitPlacement,
    formation: &str,
    leads: bool,
) -> tactics_core::map::UnitPlacement {
    placement.formation = Some(formation.into());
    placement.leads = leads;
    placement
}

fn scripted_battle(
    reg: &DataRegistry,
    file: serde_json::Value,
    placements: Vec<tactics_core::map::UnitPlacement>,
) -> BattleState {
    let file: tactics_core::map::MapFile = serde_json::from_value(file).unwrap();
    let map = tactics_core::map::HexMap::from_map_file(&file).unwrap();
    let sides = vec![
        tactics_core::battle::SideState {
            name: "West".into(),
            ai: None,
        },
        tactics_core::battle::SideState {
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
        44,
    )
}

fn commit_all(reg: &DataRegistry, state: &mut BattleState) {
    for side in state.living_sides() {
        if !state.has_committed(side) {
            state.apply(reg, &Order::Commit { side }).expect("commit");
        }
    }
}

/// An eight-hex net with nobody relaying and no flags, and three ticks of
/// transit on every order: narrow enough that a cadet can be driven off the
/// wire and slow enough that an order can be caught in the air.
fn strung_out_net() -> DataRegistry {
    let mut reg = registry();
    for vehicle in reg.vehicles.values_mut() {
        vehicle.radio = None;
    }
    reg.command = Some(tactics_core::data::CommandRules {
        radius: 8,
        radius_per_signals: 0,
        relay: false,
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
            base_ticks: 3,
            levels_per_tick: 0,
            max_ticks: 5,
        },
    });
    reg
}

/// One road, two formations, and two enemies parked beyond every gun on the
/// field but inside a scout car's eyes — so a report can be filed, and then
/// lost, without a shot being fired at anybody.
fn tangled_wire(reg: &DataRegistry) -> BattleState {
    scripted_battle(
        reg,
        serde_json::json!({
            "id": "tangled_wire",
            "palette": { "g": "grass" },
            "rows": ["g".repeat(41)],
            "objectives": [
                { "id": "crossroads", "name": "The Crossroads", "at": [[25, 0]], "value": 1 },
            ],
            "formations": [
                { "id": "alpha", "name": "Alpha", "side": 0 },
                { "id": "bravo", "name": "Bravo", "side": 0 },
            ],
        }),
        vec![
            in_formation(unit_at([19, 0], 0, "recon_car", "Leader"), "alpha", true),
            in_formation(unit_at([15, 0], 0, "recon_car", "Scout"), "alpha", false),
            in_formation(unit_at([12, 0], 0, "recon_car", "Stray"), "alpha", false),
            in_formation(unit_at([17, 0], 0, "medium_tank", "Boss"), "bravo", true),
            in_formation(unit_at([16, 0], 0, "medium_tank", "Mate"), "bravo", false),
            unit_at([39, 0], 1, "recon_car", "Far Prowler"),
            unit_at([37, 0], 1, "recon_car", "Near Prowler"),
        ],
    )
}

#[test]
fn a_battle_carrying_everything_the_wire_knows_forks_identically() {
    // The command layer landed in eight chunks, and each one added state that
    // was round-tripped on its own. Nothing had ever held all of it at once,
    // which is the arrangement a real save is: a mission still in the air, a
    // leg queued behind the standing one, an order held at the radio, a crew
    // under personal tasking, a member cut off and soldiering on a snapshot,
    // a ghost on the commander's picture, and the spotting clocks that decide
    // when a gun answers. This builds exactly that and then holds the line
    // the whole save module holds: not that the fields come back, but that
    // the FUTURE does.
    let reg = strung_out_net();
    let mut state = tangled_wire(&reg);
    let named = |id: &str| {
        tactics_core::battle::FormationId(
            state
                .formations()
                .iter()
                .position(|f| f.id == id)
                .expect("declared above") as u32,
        )
    };
    let (alpha, bravo) = (named("alpha"), named("bravo"));
    let axis = tactics_core::offset_to_hex(30, 0);
    let anchor = tactics_core::offset_to_hex(25, 0);
    let stray = tactics_core::battle::UnitId(2);
    let mate = tactics_core::battle::UnitId(4);

    // Day one: both formations get their standing orders, which arrive three
    // ticks into the round.
    for formation in [alpha, bravo] {
        state
            .apply(
                &reg,
                &Order::SetMission {
                    formation,
                    mission: tactics_core::battle::Mission::Advance { to: axis },
                    latitude: tactics_core::battle::Latitude::Delegated,
                },
            )
            .expect("the axis is on the map");
    }
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);

    // Day two: a leg queued behind alpha's advance; the leader eases back a
    // hex so the far prowler drops off every eye on the field and her report
    // of him goes stale; and the stray drives out of the net, snapshotting
    // the orders she is carrying as the wire dies.
    state
        .apply(
            &reg,
            &Order::QueueMission {
                formation: alpha,
                mission: tactics_core::battle::Mission::Hold { at: Some(anchor) },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("nothing terminal to queue behind");
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: tactics_core::battle::UnitId(0),
                to: tactics_core::offset_to_hex(18, 0),
            },
        )
        .expect("one hex west");
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: stray,
                to: tactics_core::offset_to_hex(9, 0),
            },
        )
        .expect("three hexes west, and off the wire");
    commit_all(&reg, &mut state);
    state.resolve_round(&reg);

    // Day three, and the fork: an order still in the air, one held at the
    // radio for a cadet who cannot hear it, and one that reached its cadet and
    // took her off her formation's tasking.
    state
        .apply(
            &reg,
            &Order::SetMission {
                formation: bravo,
                mission: tactics_core::battle::Mission::Hold { at: Some(anchor) },
                latitude: tactics_core::battle::Latitude::Delegated,
            },
        )
        .expect("a countermand");
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: stray,
                to: Some(anchor),
                fire: None,
                latitude: Latitude::Delegated,
            },
        )
        .expect("accepted, and waiting for a wire");
    state
        .apply(
            &reg,
            &Order::Radio {
                unit: mate,
                to: Some(tactics_core::offset_to_hex(14, 0)),
                fire: Some(tactics_core::battle::FireIntent::Hold),
                latitude: Latitude::Delegated,
            },
        )
        .expect("she can hear it");

    // Stated rather than assumed: if any of these were absent the comparison
    // below would pass without testing what it claims to.
    assert!(
        state.formations()[bravo.index()].incoming.is_some(),
        "an order in the air"
    );
    assert!(
        !state.formations()[alpha.index()].plan.is_empty(),
        "a leg queued behind the standing one"
    );
    assert!(
        state.command.waiting_for(stray).is_some(),
        "an order held at the radio"
    );
    assert!(state.units[mate.index()].detached, "a crew under tasking");
    assert!(
        state.formations()[alpha.index()]
            .out_of_contact
            .iter()
            .any(|cut| cut.orders.is_some()),
        "a cut-off crew soldiering on a snapshot"
    );
    assert!(
        state.picture(0).iter().any(|c| !c.fresh),
        "a ghost on the picture"
    );
    assert!(
        state.picture(0).iter().any(|c| c.fresh),
        "and a contact still being reported"
    );
    assert!(
        !state.fog.side(0).spotted_since.is_empty(),
        "and a spotting clock running"
    );

    let text = SaveGame::new(&reg, None, Some(state.clone()))
        .to_json()
        .expect("serialises");
    let restored = SaveGame::from_json(&reg, &text)
        .expect("deserialises")
        .0
        .battle
        .expect("battle round-trips");
    assert_eq!(
        restored.command, state.command,
        "everything the chain of command knows comes back in one piece"
    );

    let play = |state: &mut BattleState| -> Vec<String> {
        let mut log = Vec::new();
        let mut ai = AiDriver::new();
        ai.insert(0, planner(&reg, 900));
        ai.insert(1, planner(&reg, 901));
        for _ in 0..5 {
            if state.is_over() {
                break;
            }
            ai.plan_round_with(&reg, state, |d| {
                log.push(format!("{} {:?} {:?}", d.side, d.order, d.rejected));
                log.extend(d.events.iter().map(|e| format!("{e:?}")));
            });
            log.extend(
                state
                    .resolve_round(&reg)
                    .iter()
                    .filter(|e| !matches!(e, Event::TickStarted { .. }))
                    .map(|e| format!("{e:?}")),
            );
        }
        log
    };
    let mut unsaved = state;
    let mut reloaded = restored;
    let expected = play(&mut unsaved);
    assert!(
        expected.len() > 40,
        "the fork has to actually play out something: {} steps",
        expected.len()
    );
    assert_eq!(
        play(&mut reloaded),
        expected,
        "five rounds after the reload must be the five rounds that would have happened"
    );
}

#[test]
fn a_reloaded_crew_reacts_on_the_clock_she_was_already_running() {
    // `spotted_since` is the newest thing on `SideFog` and the only piece of
    // battle state whose whole purpose is to remember a moment. It is serde'd
    // with everything else, but `rehydrate` empties two neighbouring caches
    // on the way past, and a clock that came back empty would not fail
    // loudly: `best_opportunity_shot` reads a missing entry as "she has known
    // about him all along" and fires at once. So the assertion is about the
    // TICK the gun speaks on, not about the field — an ambush saved halfway
    // through must be answered on the same tick either way.
    let mut reg = registry();
    reg.command = None;
    let mut state = scripted_battle(
        &reg,
        serde_json::json!({
            "id": "ambush",
            "palette": { "g": "grass", "f": "forest" },
            "rows": ["gggggggggggg", "ggggggffgggg", "gggggggggggg"],
        }),
        vec![
            unit_at([1, 1], 0, "medium_tank", "Watcher"),
            unit_at([8, 1], 1, "medium_tank", "Walker"),
        ],
    );
    let (watcher, walker) = (
        tactics_core::battle::UnitId(0),
        tactics_core::battle::UnitId(1),
    );
    assert!(
        !state.fog.side(0).spotted.contains(&walker),
        "the curtain has to hide her first"
    );
    let into_the_open = state.unit(walker).unwrap().pos + tactics_core::Hex::new(0, -1);
    state
        .apply(
            &reg,
            &Order::SetMove {
                unit: walker,
                to: into_the_open,
            },
        )
        .expect("one step out from the trees");
    commit_all(&reg, &mut state);

    // Stop on the tick she is first seen — mid-round, with the clock running
    // and the watcher's reaction time not yet spent.
    let mut seen = None;
    while state.resolving_tick().is_some() && seen.is_none() {
        let tick = state.resolving_tick().expect("resolving");
        for event in state.step_tick(&reg) {
            if let Event::UnitSpotted {
                unit, by_side: 0, ..
            } = event
                && unit == walker
            {
                seen = Some(tick);
            }
        }
    }
    assert!(seen.is_some(), "she steps into view during the round");
    assert!(
        !state.fog.side(0).spotted_since.is_empty(),
        "and the watcher's clock for her has started"
    );

    let text = SaveGame::new(&reg, None, Some(state.clone()))
        .to_json()
        .expect("serialises");
    let mut reloaded = SaveGame::from_json(&reg, &text)
        .expect("deserialises")
        .0
        .battle
        .expect("battle round-trips");
    assert_eq!(
        reloaded.fog.side(0).spotted_since,
        state.fog.side(0).spotted_since,
        "rehydrate empties the vision caches beside this one and must not empty it"
    );

    let first_shot = |state: &mut BattleState| -> Option<u32> {
        let mut fired = None;
        while state.resolving_tick().is_some() && fired.is_none() && !state.is_over() {
            let tick = state.resolving_tick().expect("resolving");
            for event in state.step_tick(&reg) {
                if let Event::ShotFired {
                    attacker,
                    opportunity: true,
                    ..
                } = event
                    && attacker == watcher
                {
                    fired.get_or_insert(tick);
                }
            }
        }
        fired
    };
    let mut unsaved = state;
    let expected = first_shot(&mut unsaved);
    assert!(expected.is_some(), "she is engaged before the round ends");
    assert_eq!(
        first_shot(&mut reloaded),
        expected,
        "a reloaded crew answers the ambush on the tick she would have answered it"
    );
}

/// A shell is state that belongs to nobody on the board: it is not a unit, it
/// has no position that anything can see, and the crew that fired it may be
/// dead before it lands. That makes it exactly the kind of thing a save
/// forgets — and the round-trip test above cannot catch it, because in the
/// shipped scenario a 105 crosses its ground in a tick or two and no shell
/// happens to be airborne when a round ends. So the fork is taken *mid-round*,
/// with a deliberately slow round still in the air, and both copies are
/// required to bring it down on the same hex at the same tick with the same
/// consequences.
#[test]
fn a_shell_in_flight_survives_a_save() {
    let mut reg = seen(registry());
    // Ten metres a second: an absurd shell, and the cheapest way to hold one
    // in the air across a save on a map small enough to read.
    if let Some(ammo) = reg.ammo.get_mut("he_105") {
        ammo.velocity = 10;
    }
    let row = "g".repeat(16);
    let mut original = scripted_battle(
        &reg,
        serde_json::json!({
            "id": "shellfall",
            "palette": { "g": "grass" },
            "rows": [&row, &row, &row],
        }),
        vec![
            unit_at([0, 1], 0, "artillery", "Battery"),
            unit_at([7, 1], 1, "recon_car", "Quarry"),
        ],
    );
    original
        .apply(
            &reg,
            &Order::SetFire {
                unit: tactics_core::battle::UnitId(0),
                fire: tactics_core::battle::FireIntent::Target {
                    target: tactics_core::battle::UnitId(1),
                    weapon: 0,
                },
            },
        )
        .expect("the battery is given a target");
    commit_all(&reg, &mut original);
    // Far enough into the round that the gun has fired and nothing has come
    // down yet.
    original.step_tick(&reg);
    original.step_tick(&reg);
    assert_eq!(
        original.shells.len(),
        1,
        "the fork has to happen with a shell genuinely in the air"
    );

    let text = SaveGame::new(&reg, None, Some(original.clone()))
        .to_json()
        .expect("serialises");
    let mut restored = SaveGame::from_json(&reg, &text)
        .expect("deserialises")
        .0
        .battle
        .expect("battle round-trips");
    assert_eq!(
        restored.shells, original.shells,
        "the shell itself comes back: who fired it, from where, at what, and when it lands"
    );

    // The property that matters is not the field, it is the future.
    let finish = |reg: &DataRegistry, state: &mut BattleState| -> Vec<String> {
        let mut log = Vec::new();
        for _ in 0..4 {
            if state.is_over() {
                break;
            }
            if state.is_planning() {
                commit_all(reg, state);
            }
            for event in state.resolve_round(reg) {
                if !matches!(event, Event::TickStarted { .. }) {
                    log.push(format!("{event:?}"));
                }
            }
        }
        log
    };
    let expected = finish(&reg, &mut original);
    let actual = finish(&reg, &mut restored);
    assert!(
        expected.iter().any(|line| line.starts_with("ShellLanded")),
        "the shell has to actually come down in the stretch being compared"
    );
    assert_eq!(
        actual, expected,
        "a reloaded battle must bring the shell down exactly as the unsaved one would"
    );
}

#[test]
fn a_mounted_platoon_rides_through_a_save() {
    // The aboard state is battle state like any other: a platoon saved in
    // the back of her carrier steps out of the file still in the back of
    // her carrier, position mirrored, standing boarding orders intact.
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 44).expect("battle");
    // Surgery rather than content: no shipped map mounts anybody yet, and
    // this test is about the fields, not the scenario.
    let (carrier, rider) = (
        tactics_core::battle::UnitId(0),
        tactics_core::battle::UnitId(1),
    );
    let pos = state.units[carrier.index()].pos;
    state.units[rider.index()].aboard = Some(carrier);
    state.units[rider.index()].pos = pos;
    state.units[rider.index()].dismounting = true;

    let text = SaveGame::new(&reg, None, Some(state.clone()))
        .to_json()
        .expect("to json");
    let restored = SaveGame::from_json(&reg, &text)
        .expect("parse")
        .0
        .battle
        .expect("battle came back");
    let r = &restored.units[rider.index()];
    assert_eq!(r.aboard, Some(carrier), "still aboard");
    assert_eq!(r.pos, pos, "still where the carrier is");
    assert!(r.dismounting, "and still under orders to get off");
}
