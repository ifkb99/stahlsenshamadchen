//! The seam between the campaign and a battle (`tactics_core::field`): a
//! clash staged as a field battle, and the battle handed back.
//!
//! These lived in the game crate's battle and overworld screens while the
//! code they test did, and moved with it. Contents: deployment (nobody forms
//! up on her own lane home; each army fills one of the map's formations);
//! orders inherited from the campaign; the whole seam fought headlessly into
//! the roster (every cadet accounted for, the same battle leaving the
//! campaign in the same state twice, a crew who takes an exit coming home);
//! which battlefield the ground picks; and the whole campaign played out
//! headlessly through `tactics_core::harness::campaign`.
//!
//! Everything here drives the real path — `Clash::muster`, `Clash::problem`,
//! `Clash::stage`, `Clash::inherit_missions`, `FieldBattle::report`,
//! `OverworldState::apply_battle_result` — because a parallel staging would
//! be a second answer to what an order of battle is, and the whole point of
//! testing this seam is that the two halves agree.

use std::collections::HashMap;
use tactics_core::ai::{AiConfig, AiDriver, make_battle_planner};
use tactics_core::battle::{BattleState, Fate, Mission, SideState};
use tactics_core::field::{BattleForce, Clash, FieldBattle, battlefield_for, deploy};
use tactics_core::harness::campaign::{CampaignOptions, play};
use tactics_core::map::{MapKind, ObjectiveKind};
use tactics_core::overworld::{
    ArmyId, ArmyMission, ArmyUnit, BattleReport, CrewLoss, OverworldEvent, OverworldState,
};
use tactics_core::roster::{CadetId, CadetStatus};

mod common;
use common::registry;

/// Deployment puts a side at the shallowest tiles of its own map edge —
/// which is exactly where a retreat lane lives. Without this rule the
/// attacker's leading vehicles spawn standing on their own way out and
/// drive off the map on the first tick, ending the battle before it
/// starts.
///
/// Fought over *every* shipped battlefield rather than over
/// `river_crossing` alone, and the loop asserts a second thing on the way
/// past: that each of them offers a way off it at all. The rule is a fact
/// about `deploy` rather than about one map, and the map it was written
/// against was for a long time the only one with a lane to stand on —
/// `battle_forest` declared no exit until the terrain-coverage chunk, so
/// nobody on it could withdraw and this check had nothing to say about
/// it.
#[test]
fn nobody_deploys_onto_their_own_way_off_the_map() {
    let reg = registry();
    let mut ids: Vec<&str> = reg
        .maps
        .values()
        .filter(|m| m.kind == MapKind::Battle)
        .map(|m| m.id.as_str())
        .collect();
    ids.sort_unstable();
    assert!(ids.len() >= 5, "the base mod ships five battlefields");

    let forces: Vec<BattleForce> = [0u8, 1]
        .iter()
        .map(|side| BattleForce {
            army: ArmyId(*side as u32),
            side: *side,
            units: (0..4)
                .map(|_| ArmyUnit {
                    vehicle: "medium_tank".into(),
                    crew: Vec::new(),
                    name: None,
                })
                .collect(),
            mission: None,
        })
        .collect();

    for id in ids {
        let file = reg.map(id).expect("shipped battle map");
        let map = tactics_core::map::Battlefield::from_map_file(file).expect("map parses");
        assert!(
            map.scenario
                .objectives()
                .iter()
                .any(|o| o.kind == ObjectiveKind::Exit),
            "battlefield `{id}` declares no exit, so nobody fighting on it can withdraw"
        );

        let (placements, _, _) =
            deploy(&reg, &tactics_core::roster::Roster::new(), &map, &forces, 0);
        assert_eq!(placements.len(), 8, "everyone was placed on `{id}`");
        for placement in &placements {
            let hex = tactics_core::offset_to_hex(placement.at[0], placement.at[1]);
            for objective in map.scenario.objectives() {
                assert!(
                    !(objective.kind == ObjectiveKind::Exit
                        && objective.open_to(placement.side)
                        && objective.contains(hex)),
                    "on `{id}` side {} deployed onto exit `{}` at {:?}",
                    placement.side,
                    objective.id,
                    placement.at
                );
            }
        }
    }
}

/// An army arriving on a battlefield is a formation on it, because
/// otherwise nothing the campaign decided has anybody to say it to: a
/// mission is given to a formation, and a field battle of flat pools
/// could inherit no orders at all.
#[test]
fn each_army_fills_one_of_the_maps_formations() {
    let reg = registry();
    let file = reg.map("river_crossing").expect("shipped battle map");
    let map = tactics_core::map::Battlefield::from_map_file(file).expect("map parses");
    let forces: Vec<BattleForce> = (0..4)
        .map(|i| BattleForce {
            army: ArmyId(i),
            side: (i % 2) as u8,
            units: (0..2)
                .map(|_| ArmyUnit {
                    vehicle: "medium_tank".into(),
                    crew: Vec::new(),
                    name: None,
                })
                .collect(),
            mission: None,
        })
        .collect();

    let (placements, _, _) = deploy(&reg, &tactics_core::roster::Roster::new(), &map, &forces, 0);
    let named: Vec<&str> = placements
        .iter()
        .filter_map(|p| p.formation.as_deref())
        .collect();
    assert_eq!(named.len(), placements.len(), "nobody is left unattached");
    for def in map.scenario.formations() {
        assert_eq!(
            named.iter().filter(|id| **id == def.id).count(),
            2,
            "one army of two vehicles per declared formation: {}",
            def.id
        );
        let leaders = placements
            .iter()
            .filter(|p| p.formation.as_deref() == Some(def.id.as_str()) && p.leads)
            .count();
        assert_eq!(leaders, 1, "exactly one leader in {}", def.id);
    }
}

/// An army arriving on a battlefield is led by its senior cadet — the
/// vehicle carrying the highest rank — and with no ranks declared by its
/// first vehicle, as it always was.
#[test]
fn an_army_is_led_onto_the_field_by_its_senior_cadet() {
    let file = registry()
        .map("river_crossing")
        .expect("shipped battle map")
        .clone();
    let map = tactics_core::map::Battlefield::from_map_file(&file).expect("map parses");
    for (captain, leader) in [(Some("mina"), 2usize), (None, 0)] {
        let mut reg = registry();
        for character in reg.characters.values_mut() {
            character.rank = None;
        }
        reg.ranks = vec![tactics_core::data::RankDef {
            id: "captain".into(),
            name: "Captain".into(),
        }];
        if let Some(who) = captain {
            reg.characters.get_mut(who).unwrap().rank = Some("captain".into());
        }
        let mut roster = tactics_core::roster::Roster::new();
        let units: Vec<ArmyUnit> = ["anka", "juno", "mina"]
            .iter()
            .map(|who| ArmyUnit {
                vehicle: "medium_tank".into(),
                crew: vec![roster.enlist(0, reg.character(who).unwrap(), &reg)],
                name: None,
            })
            .collect();
        let forces = vec![BattleForce {
            army: ArmyId(0),
            side: 0,
            units,
            mission: None,
        }];
        let (placements, _, _) = deploy(&reg, &roster, &map, &forces, 0);
        let leads: Vec<usize> = placements
            .iter()
            .enumerate()
            .filter(|(_, p)| p.leads)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            leads,
            vec![leader],
            "with {captain:?} a captain, vehicle {leader} leads the army"
        );
    }
}

/// The campaign's decision reaches the battlefield: an army that was
/// falling back fights toward the way out, without the player having to
/// order every platoon out again by hand.
#[test]
fn a_withdrawing_army_hands_its_formations_the_way_out() {
    let reg = registry();
    let forces: Vec<BattleForce> = [0u8, 1]
        .iter()
        .map(|side| BattleForce {
            army: ArmyId(*side as u32),
            side: *side,
            units: (0..2)
                .map(|_| ArmyUnit {
                    vehicle: "medium_tank".into(),
                    crew: Vec::new(),
                    name: None,
                })
                .collect(),
            // Only the defender was pulling back.
            mission: (*side == 1).then_some(ArmyMission::Withdraw {
                to: tactics_core::offset_to_hex(13, 4),
            }),
        })
        .collect();

    let clash = Clash {
        map_id: "river_crossing".into(),
        roster: std::sync::Arc::new(tactics_core::roster::Roster::new()),
        attacker: ArmyId(0),
        defender: ArmyId(1),
        sides: ["A", "B"]
            .map(|name| SideState {
                name: name.into(),
                ai: None,
            })
            .to_vec(),
        attacker_side: 0,
        forces,
        seed: 9,
    };
    let (mut state, _) = clash
        .stage(&reg, clash.seed)
        .expect("the staged placements are content the base mod ships");
    let lines = clash.inherit_missions(&reg, &mut state);
    // One army per side, so one formation per side exists: a declaration
    // nobody joined is dropped rather than carried empty.
    assert_eq!(lines.len(), 1, "side 1's formation was told");

    for formation in state.formations() {
        let ordered = formation.latest_mission();
        if formation.side == 1 {
            assert!(
                matches!(ordered, Some(Mission::Withdraw { via }) if via == "east_road"),
                "{} should be leaving by its own lane, got {ordered:?}",
                formation.id
            );
        } else {
            assert!(
                ordered.is_none(),
                "{} was told nothing on the map and must be told nothing here",
                formation.id
            );
        }
    }
}

/// The campaign the base mod ships, with all forty-nine named cadets
/// enlisted into the roster its armies are crewed from.
fn campaign(reg: &tactics_core::data::DataRegistry) -> OverworldState {
    OverworldState::from_map(reg, "frontier", 11).expect("the shipped campaign map builds")
}

/// A battle that has been fought to a finish, with the bookkeeping that
/// says which army each hull marched in with.
struct Fought {
    state: BattleState,
    outcome: BattleReport,
    forces: Vec<BattleForce>,
    field: FieldBattle,
    rounds: usize,
}

impl Fought {
    /// The army a unit marched in with.
    fn field_origin(&self, unit: tactics_core::battle::UnitId) -> Option<ArmyId> {
        self.field.origin(unit)
    }
}

/// Stage the clash the campaign would stage, fight it out with nobody
/// watching, and do the accounting.
///
/// Both sides are given a planner: the campaign hands side 0 to the
/// player and a headless test has no player, so her seat is filled the
/// same way `STAHL_AUTOPLAY` fills it.
fn fight(
    reg: &tactics_core::data::DataRegistry,
    campaign: &OverworldState,
    map_id: &str,
    attacker: ArmyId,
    defender: ArmyId,
    seed: u64,
) -> Fought {
    // Through the campaign's own muster, with nobody joining and nobody
    // called up, exactly as `launch_battle` builds it.
    let clash = Clash::muster(campaign, attacker, defender, &[], &[], map_id.to_string());

    // The campaign asks before it commits; so does this.
    assert!(
        clash.problem(reg).is_none(),
        "the shipped campaign cannot stage its own opening clash on {map_id}"
    );
    let (mut state, field) = clash
        .stage(reg, seed)
        .expect("the clash the campaign just approved");
    clash.inherit_missions(reg, &mut state);

    let mut ai = AiDriver::new();
    for side in 0..state.sides.len() as u8 {
        let cfg = state.sides[side as usize].ai.clone().unwrap_or(AiConfig {
            planner: "utility".into(),
            difficulty: 3,
            doctrine: Some("bounding_overwatch".into()),
        });
        ai.insert(side, make_battle_planner(&cfg, seed ^ side as u64, reg));
    }

    let mut rounds = 0;
    while !state.is_over() && rounds < 60 {
        ai.plan_round(reg, &mut state);
        state.resolve_round(reg);
        rounds += 1;
    }
    let outcome = field.report(reg, &state);
    Fought {
        state,
        outcome,
        forces: clash.forces,
        field,
        rounds,
    }
}

/// Every cadet the battle was handed comes back out of it exactly once.
///
/// The seam between a battle and a campaign is arithmetic nobody watches:
/// a survivor list built off unit indices, a loss list built off two
/// different questions ("did her vehicle come home" and "was she hurt in
/// the seat"), and a roster written from both. A cadet dropped here is a
/// person who quietly stops existing, and a cadet counted twice is one
/// whose wound is rolled for twice; neither shows up as a crash.
///
/// The one deliberate overlap is a cadet hurt at her station in a vehicle
/// that came home: she is *both* aboard a survivor and reported, and
/// `CrewLoss::found` is what says so. A cadet pulled out of a wreck
/// (`found: None`) must never be both.
#[test]
fn every_cadet_who_marched_into_a_field_battle_is_accounted_for_when_it_ends() {
    let reg = registry();
    let mut base = campaign(&reg);
    let (attacker, defender) = (ArmyId(0), ArmyId(2));
    // One vehicle nobody was assigned to, which is ordinary campaign
    // state — an army can hold a chassis it has no cadets for. The
    // battle crews it anonymously out of its *own* copy of the roster,
    // so this is what makes the "the campaign never enlisted her" check
    // below ask a real question rather than an empty one.
    base.army_mut(attacker)
        .expect("the attacker exists")
        .units
        .push(ArmyUnit {
            vehicle: "light_tank".into(),
            crew: Vec::new(),
            name: Some("Spare".into()),
        });

    let mut winners = Vec::new();
    let mut station_wounds = 0;
    let mut exits = 0;
    // Scanned rather than hand-picked. Four hunted seeds used to stand
    // here and they stopped producing a station wound the moment the
    // field-repair rule moved every battle downstream of it — which is
    // what a re-hunted fixture always does, twice a year, silently. The
    // loop asks for what the test is actually about (a win for each side
    // and two cadets carried home hurt) and stops when it has it, so the
    // next rule that moves a battle costs nobody an afternoon.
    for seed in 0u64..24 {
        if winners.contains(&Some(0)) && winners.contains(&Some(1)) && station_wounds >= 2 {
            break;
        }
        let fought = fight(&reg, &base, "battle_plains", attacker, defender, seed);
        assert!(
            fought.state.is_over(),
            "seed {seed} was still being fought after {} rounds",
            fought.rounds
        );
        winners.push(fought.outcome.winner);

        let marched: Vec<CadetId> = fought
            .forces
            .iter()
            .flat_map(|f| f.units.iter().flat_map(|u| u.crew.iter().copied()))
            .collect();
        let hulls: usize = fought.forces.iter().map(|f| f.units.len()).sum();
        assert_eq!(
            fought.state.units.len(),
            hulls,
            "seed {seed}: somebody was left in the assembly area"
        );

        // The vehicle count is conserved: a hull is either a survivor or
        // a wreck, and there is no third place for one to go.
        let survivors: usize = fought.outcome.survivors.iter().map(|(_, u)| u.len()).sum();
        let lost = fought.state.lost_units().count();
        assert_eq!(
            survivors + lost,
            hulls,
            "seed {seed}: {survivors} survivors + {lost} wrecks is not the {hulls} that marched in"
        );

        // A crew that drove off the map by an exit came home. This is the
        // case that was wrong once — `alive_units` here would have handed
        // the campaign a withdrawal as a burnt-out vehicle.
        for unit in fought.state.units.iter() {
            if !matches!(unit.fate, Fate::Exited) {
                continue;
            }
            exits += 1;
            for cadet in &unit.crew {
                assert!(
                    fought
                        .outcome
                        .survivors
                        .iter()
                        .any(|(_, units)| units.iter().any(|u| u.crew.contains(cadet))),
                    "seed {seed}: {cadet:?} took an exit and was not reported home"
                );
                assert!(
                    !fought
                        .outcome
                        .losses
                        .iter()
                        .any(|l| l.cadet == *cadet && l.found.is_none()),
                    "seed {seed}: {cadet:?} drove off the map and was written off as a wreck"
                );
            }
        }

        // Now hand it to the campaign, which is where it becomes state
        // somebody has to live with.
        let mut after = base.clone();
        let before: HashMap<CadetId, u32> = after
            .roster
            .iter()
            .map(|cadet| (cadet.id, cadet.battles))
            .collect();
        after.commit_to_battle(&[]);
        let events = after.apply_battle_result(&reg, &fought.outcome);

        for cadet in &marched {
            let aboard = after
                .armies
                .iter()
                .any(|a| a.units.iter().any(|u| u.crew.contains(cadet)));
            let reported: Vec<&CrewLoss> = fought
                .outcome
                .losses
                .iter()
                .filter(|l| l.cadet == *cadet)
                .collect();
            assert!(
                aboard || !reported.is_empty(),
                "{cadet:?} marched out at seed {seed} and is in nobody's account"
            );
            assert!(
                reported.len() <= 1,
                "{cadet:?} was reported {} times at seed {seed}",
                reported.len()
            );
            if let Some(loss) = reported.first() {
                if loss.found.is_none() {
                    assert!(
                        !aboard,
                        "{cadet:?} was pulled out of a wreck and is still crewing at seed {seed}"
                    );
                } else {
                    // Hurt at her station in a vehicle that came home:
                    // the one cadet who is legitimately in both lists,
                    // and the wound has to outlive the battle.
                    station_wounds += 1;
                    assert!(
                        aboard,
                        "{cadet:?} came home in her own tank and left the army"
                    );
                    let status = after.roster.get(*cadet).expect("she is on the roll").status;
                    assert!(
                        !status.is_ready(),
                        "{cadet:?} was found {:?} at her station and the campaign says she is fine",
                        loss.found
                    );
                    assert!(
                        matches!(status, CadetStatus::Wounded { .. } | CadetStatus::Dead),
                        "a station casualty is treated, not adrift: {status:?}"
                    );
                }
                assert!(
                    events.iter().any(|e| matches!(
                        e,
                        OverworldEvent::CrewCasualty { cadet: c, .. } if c == cadet
                    )),
                    "{cadet:?} was a casualty at seed {seed} and nobody was told"
                );
            }
        }

        // Every survivor has one more battle behind her, and nobody else
        // does: a crew that did not fight cannot be credited with it.
        for (_, units) in &fought.outcome.survivors {
            for unit in units {
                for cadet in &unit.crew {
                    if let Some(now) = after.roster.get(*cadet) {
                        assert_eq!(
                            now.battles,
                            before[cadet] + 1,
                            "{cadet:?} survived seed {seed} and was not credited with it"
                        );
                    }
                }
            }
        }
        for unit in fought.state.lost_units() {
            for cadet in &unit.crew {
                if let Some(now) = after.roster.get(*cadet) {
                    assert_eq!(
                        now.battles, before[cadet],
                        "{cadet:?} did not come home from seed {seed} and was credited with it"
                    );
                }
            }
        }
        for cadet in after.roster.iter() {
            if !marched.contains(&cadet.id) {
                assert_eq!(
                    cadet.battles, before[&cadet.id],
                    "{:?} stayed at the academy and was credited with a battle",
                    cadet.id
                );
            }
        }

        // The academy's rolls are the academy's: an anonymous crew
        // enlisted into the battle's own copy of the roster must not come
        // back holding a handle the campaign cannot resolve.
        for army in &after.armies {
            for unit in &army.units {
                for cadet in &unit.crew {
                    assert!(
                        after.roster.get(*cadet).is_some(),
                        "{} holds {cadet:?}, whom the campaign never enlisted",
                        army.name
                    );
                }
            }
        }

        // An army with nothing left is destroyed, and said to be.
        for army in &after.armies {
            if army.units.is_empty() && [attacker, defender].contains(&army.id) {
                assert!(
                    !army.alive,
                    "{} lost every vehicle and is still on the map",
                    army.name
                );
                assert!(
                    events.iter().any(
                        |e| matches!(e, OverworldEvent::ArmyDestroyed { army: a } if *a == army.id)
                    ),
                    "{} was wiped out at seed {seed} and nobody was told",
                    army.name
                );
            }
        }
    }
    // Not invariants of the seam but of this test being worth running.
    // Four seeds on `battle_plains` are fought out because one battle is
    // one shape of ending: these four are 5 wrecks / 7 / 9 / 6 with two
    // won by each side, so the accounting is checked against a rout in
    // both directions rather than against one lucky afternoon.
    assert!(
        winners.contains(&Some(0)) && winners.contains(&Some(1)),
        "every seed was won by the same side, so a defeat's accounting went unchecked: {winners:?}"
    );
    assert!(
        station_wounds >= 2,
        "no cadet was hurt at her station and carried home; the half of the \
         loss list that is not a wreck went untested (exits seen: {exits})"
    );
}

/// A campaign that fights the same battle twice comes out of it in the
/// same place.
///
/// The casualty rolls run on the campaign's own rng, and the seam sorts
/// the losses by cadet id before spending it for exactly this reason: the
/// battle reports them in whatever order its units happen to sit in, and
/// a replay that drew them in that order would diverge from the day it
/// replays.
#[test]
fn the_same_battle_leaves_the_campaign_in_the_same_state_twice() {
    let reg = registry();
    let base = campaign(&reg);
    let apply = |seed: u64| -> String {
        let fought = fight(&reg, &base, "battle_plains", ArmyId(0), ArmyId(2), seed);
        let mut after = base.clone();
        after.apply_battle_result(&reg, &fought.outcome);
        serde_json::to_string(&after).expect("a campaign serialises")
    };
    for seed in [0u64, 6] {
        assert_eq!(
            apply(seed),
            apply(seed),
            "seed {seed} left the campaign somewhere else the second time"
        );
    }
}

/// An army caught pulling back gets its crews home rather than losing
/// them: a vehicle that takes an exit it is entitled to is a survivor,
/// and the army that owns her is not destroyed for having left.
#[test]
fn a_crew_that_drives_off_the_map_comes_home_to_her_army() {
    let reg = registry();
    let mut base = campaign(&reg);
    // Through the campaign's own order, so the mission is one an army
    // could really be carrying when it is caught.
    let falling_back = ArmyId(0);
    base.apply(
        &reg,
        &tactics_core::overworld::OverworldOrder::SetMission {
            army: falling_back,
            mission: ArmyMission::Withdraw {
                to: tactics_core::offset_to_hex(0, 1),
            },
        },
    )
    .expect("her own headquarters can reach her on day one");

    // Seeds until one of them catches a crew leaving by the road, rather
    // than three somebody once hunted down: which battle produces a
    // withdrawal moves whenever a rule does, and a fixture that has to be
    // re-hunted every time a crew fights differently is a fixture that
    // ends up deleted. It stops at the first, so the usual cost is one
    // battle.
    let mut exited = 0;
    for seed in 0u64..48 {
        let fought = fight(&reg, &base, "river_crossing", ArmyId(2), falling_back, seed);
        for unit in fought.state.units.iter() {
            if !matches!(unit.fate, Fate::Exited) {
                continue;
            }
            exited += 1;
            let mut after = base.clone();
            let home = fought
                .outcome
                .survivors
                .iter()
                .find(|(id, _)| *id == falling_back)
                .map(|(_, units)| units.len())
                .unwrap_or(0);
            assert!(
                home > 0,
                "seed {seed}: a crew took the exit and her army came home empty"
            );
            // The report says who withdrew, and it means the whole army:
            // every vehicle she still has left by the road, none is on
            // the field. That is the campaign's cue to move her a hex
            // back, so it has to be right in both directions — an army
            // with one tank still standing on the ground has not gone.
            let on_field = fought
                .state
                .units
                .iter()
                .filter(|u| fought.field_origin(u.id) == Some(falling_back))
                .filter(|u| matches!(u.fate, Fate::Fighting { .. }))
                .count();
            assert_eq!(
                fought.outcome.withdrew.contains(&falling_back),
                on_field == 0,
                "seed {seed}: withdrew {:?} with {on_field} vehicle(s) still on the field",
                fought.outcome.withdrew
            );
            let before = base.army(falling_back).unwrap().pos;
            let events = after.apply_battle_result(&reg, &fought.outcome);
            assert!(
                !events.iter().any(
                    |e| matches!(e, OverworldEvent::ArmyDestroyed { army } if *army == falling_back)
                ),
                "seed {seed}: an army that withdrew was written off as destroyed"
            );
            if on_field == 0 {
                let after_pos = after.army(falling_back).unwrap().pos;
                assert_eq!(
                    after_pos.distance_to(before),
                    1,
                    "seed {seed}: an army that withdrew arrives a hex back, not nowhere"
                );
            }
            for cadet in &unit.crew {
                assert!(
                    !fought
                        .outcome
                        .losses
                        .iter()
                        .any(|l| l.cadet == *cadet && l.found.is_none()),
                    "seed {seed}: {cadet:?} left by the road and was counted as a casualty"
                );
            }
            break;
        }
        if exited > 0 {
            break;
        }
    }
    assert!(
        exited > 0,
        "no crew took an exit in forty-eight seeds; this test proved nothing"
    );
}
/// The campaign can stage the fights its own armies would cause.
///
/// `launch_battle` now asks before it commits anybody, so a campaign whose
/// content has gone missing declines one clash instead of taking the run
/// down inside `spawn_unit`. The check is only worth having if the shipped
/// content passes it, and this is where that is said: every pair of
/// hostile armies on the campaign map, on the map their ground would pick,
/// with the campaign's own roster.
#[test]
fn every_clash_the_campaign_map_can_produce_can_be_staged() {
    let reg = registry();
    let state = OverworldState::from_map(&reg, "frontier", 5).expect("overworld");
    let terrain = state
        .map
        .get(state.armies[0].pos)
        .map(|t| t.terrain)
        .unwrap_or_default();
    let map_id = battlefield_for(&reg, terrain).expect("the base mod ships a battle map");

    let mut clashes = 0;
    for attacker in &state.armies {
        for defender in state.armies.iter().filter(|d| d.side != attacker.side) {
            let clash = Clash::muster(&state, attacker.id, defender.id, &[], &[], map_id.clone());
            assert_eq!(
                clash.problem(&reg),
                None,
                "{} attacking {} cannot be staged",
                attacker.name,
                defender.name
            );
            clashes += 1;
        }
    }
    assert!(clashes > 0, "this test is meaningless without two sides");
}

/// A campaign with no battle map declines the fight rather than crashing.
///
/// `battlefield_for` used to `expect` its way past this on the reasoning
/// that the base mod always ships one — true of the base mod, and nothing
/// at all about the mod somebody loads on top of it.
#[test]
fn a_campaign_with_no_battlefield_to_fight_on_says_so_instead_of_panicking() {
    let reg = registry();
    assert!(
        battlefield_for(&reg, "grass").is_some(),
        "the base mod ships battle maps, so this fixture is right way up"
    );
    let bare = tactics_core::data::DataRegistry::default();
    assert_eq!(
        battlefield_for(&bare, "grass"),
        None,
        "with no maps loaded there is nowhere to fight"
    );
}

/// Every terrain the campaign map is made of names the battlefield a
/// clash on it is fought over, and does so *itself*.
///
/// The third arm of `battlefield_for` — any battle map at all — is a
/// shrug, and until the `battlefield` field existed the shipped campaign
/// reached it constantly: `deep_forest`, `city`, `factory`, `highway` and
/// `mountains` all name no `battle_<terrain>` map, so a fight in the
/// mountains was resolved on whichever battlefield the map table happened
/// to iterate first. That is not a wrong answer anybody could see, which
/// is exactly why it wants a test rather than a glance.
///
/// Written against the terrain the campaign map actually *uses* rather
/// than against the roster, because a terrain nobody has put on a map is
/// allowed to have no battlefield yet.
#[test]
fn every_terrain_the_campaign_fields_names_its_own_battlefield() {
    let reg = registry();
    let file = reg
        .map("frontier")
        .expect("the base mod ships a campaign map");
    let map = tactics_core::map::Battlefield::from_map_file(file).expect("campaign map parses");
    let mut terrains: Vec<String> = map
        .terrain
        .iter()
        .map(|(_, t)| t.terrain.to_string())
        .collect();
    terrains.sort();
    terrains.dedup();
    assert!(terrains.len() > 1, "a one-terrain campaign proves nothing");
    for terrain in &terrains {
        let def = reg
            .terrain(terrain)
            .unwrap_or_else(|| panic!("the campaign map stands on `{terrain}`"));
        let named = def.battlefield.clone().unwrap_or_else(|| {
            panic!(
                "terrain `{terrain}` is on the campaign map and names no battlefield, so a clash there falls through to whichever battle map iterates first"
            )
        });
        assert!(
            reg.maps
                .get(&named)
                .is_some_and(|m| m.kind == MapKind::Battle),
            "terrain `{terrain}` is fought on `{named}`, which is not a battle map"
        );
        assert_eq!(
            battlefield_for(&reg, terrain).as_deref(),
            Some(named.as_str()),
            "a clash on `{terrain}` must be fought where the terrain says"
        );
    }
}

// ---------------------------------------------------------------
// The whole campaign, with nobody watching
// ---------------------------------------------------------------

/// The shipped campaign plays out to an ending with every side a machine,
/// and every clash it causes can be staged and fought to a finish.
///
/// This is what `tactics_core::field` was moved out of the game crate for:
/// until it was, no run could get from a clash on the map through a battle
/// and back, so whether a campaign *ends* was something only a person
/// playing one could find out. Four seeds, because the first thing the
/// instrument found (2026-09-23) is that most AI-versus-AI campaigns are
/// decided on day two — which is fast enough that four is cheap.
#[test]
fn the_shipped_campaign_plays_out_to_an_ending_with_nobody_watching() {
    let reg = registry();
    let options = CampaignOptions::default();
    for seed in 0u64..4 {
        let run = play(&reg, "frontier", seed, &options).expect("the shipped campaign builds");
        assert!(
            run.end.is_some(),
            "seed {seed}: still undecided after {} days and {} battles",
            run.days,
            run.battles.len()
        );
        assert!(!run.battles.is_empty(), "seed {seed} ended without a fight");
        assert_eq!(
            run.declined, 0,
            "seed {seed}: the campaign caused a clash it could not stage"
        );
        for battle in &run.battles {
            assert!(
                !battle.cut_off,
                "seed {seed}: the day-{} battle on {} was still going after {} rounds",
                battle.day, battle.map_id, battle.rounds
            );
        }
    }
}

/// The same seed is the same war: every battle, every casualty, the same
/// ending on the same day.
///
/// The campaign's own determinism is pinned elsewhere; this is about the
/// seam, which seeds each battle from the campaign's and spends the
/// campaign's rng on the casualty rolls between them. A hash map walked
/// anywhere on that path — the last-resort battlefield choice walked one
/// until it moved into core — is a different war from one run to the next.
#[test]
fn the_same_seed_is_the_same_war() {
    let reg = registry();
    let options = CampaignOptions::default();
    for seed in [6u64, 12] {
        let first = play(&reg, "frontier", seed, &options).expect("builds");
        let second = play(&reg, "frontier", seed, &options).expect("builds");
        assert_eq!(first, second, "seed {seed} fought a different war twice");
    }
}
