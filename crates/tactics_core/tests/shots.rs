//! One shot, from the penetration gate to the shell that missed and is
//! still in the air.
//!
//! The penetration gate and the outcome engine are the resolver's core —
//! no hit points, a share of a plate defeated or not — and soft targets
//! and hidden ones are the same arithmetic against a target with no armour
//! to speak of. Shells in flight is the same shot before it lands, and
//! what an order promises is the vocabulary a mission's promise draws its
//! words from, kept here because it is measured the same way a shot's
//! promise is: by whether the engine delivers it.
//!
//! - the penetration gate
//! - the outcome engine: no hit points
//! - soft targets and hidden ones
//! - shells in flight
//! - what an order promises

use tactics_core::battle::{
    BattleState, Destruction, EndReason, Event as BattleEvent, FireIntent, Latitude, Mission,
    Order, UnitId, reachable,
};
use tactics_core::data::{ArmorFacing, DataRegistry, RoundPressure, ShotFelt};

mod common;
use common::{
    commit_all, duel, play_round, registry, registry_wireless, seen, settle, soften,
    two_side_battle, unit_at,
};

// --- the penetration gate --------------------------------------------------

/// A recon car's machine gun and a heavy tank, adjacent on open grass. The
/// oldest complaint in the balance tables, staged.
fn plink_stage(reg: &DataRegistry, seed: u64) -> BattleState {
    two_side_battle(
        reg,
        &["ggggg", "ggggg", "ggggg"],
        vec![
            unit_at([1, 1], 0, "recon_car", "Plinker"),
            unit_at([3, 1], 1, "heavy_tank", "Wall"),
        ],
        seed,
    )
}

#[test]
fn an_ordered_shot_that_cannot_penetrate_bounces_and_does_nothing() {
    // The floor is dead. A machine gun ORDERED onto a heavy tank still
    // obeys — the rounds go downrange — but what comes of them is a bounce
    // event and nothing else: no chip and no hit points. Grinding a heavy
    // tank down with an MG was the balance instrument's oldest "worth a
    // look" line, and this is its tombstone.
    //
    // **What it no longer says is that nothing at all happens.** Wave 1 put
    // the crew's nerve in the ledger, so a belt that declares `suppression`
    // takes nothing off her plate and does rattle the people behind it. That
    // is the designer's ruling, not a leak: *firing at an impenetrable plate
    // may not hurt but has tactical value.* The rule this test still defends
    // is the one that matters — the damage ledger has no floor — and it
    // defends it in both directions now, one battle each. Both belts are
    // staged rather than inherited from the base mod, so the test says what
    // the *rule* does and a content edit cannot quietly retire half of it.
    //
    // The designer's later ruling (2026-09-23) is that the proper equipment
    // is needed to degrade armour *and its crew's nerve*, and the base mod
    // now lets none of a bullet's suppression through plate
    // (`morale.through_plate_percent: 0`). The loud belt below therefore
    // stages the old reading, 100, and the shipped reading is a third battle
    // at the end: the same belt, the same bounces, and nobody frayed.
    let quiet = {
        let mut reg = registry_wireless();
        reg.ammo
            .get_mut("ball_mg")
            .expect("the base mod ships a belt")
            .suppression = 0;
        reg
    };
    let reg = {
        let mut reg = registry_wireless();
        reg.ammo.get_mut("ball_mg").expect("shipped").suppression = 2;
        reg.morale.through_plate_percent = 100;
        reg
    };
    let mut state = plink_stage(&reg, 301);
    let (plinker, wall) = (UnitId(0), UnitId(1));
    let wall_before = state.substance(&reg, state.unit(wall).unwrap());
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: plinker,
                fire: FireIntent::Target {
                    target: wall,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let mut bounced = 0;
    let mut hit = 0;
    while state.resolving_tick().is_some() && !state.is_over() {
        for event in state.step_tick(&reg) {
            match event {
                BattleEvent::ShotBounced {
                    target, rattled, ..
                } if target == wall => {
                    assert!(!rattled, "small arms do not rattle a tank crew");
                    bounced += 1;
                }
                BattleEvent::ShotHit { target, .. } if target == wall => hit += 1,
                _ => {}
            }
        }
    }
    assert!(
        bounced > 0,
        "the bursts that struck were announced as bounces"
    );
    assert_eq!(hit, 0, "and not one of them counted as a hit");
    let rattled = state.unit(wall).unwrap().pressure;
    assert_eq!(
        state.substance(&reg, state.unit(wall).unwrap()),
        wall_before,
        "armor that holds costs nothing"
    );
    assert!(
        rattled > 0,
        "and a belt that declares suppression says so out loud: bullets on \
         plate take nothing off her and are still not a quiet afternoon"
    );

    // The same battle under a mod that declares no suppression, which is the
    // additivity half and the old assertion word for word. A belt that says
    // nothing frays nobody, because `bounced` is not charged for small arms
    // and there is nothing else to charge.
    let mut silent = plink_stage(&quiet, 301);
    silent
        .apply(
            &quiet,
            &Order::SetFire {
                unit: plinker,
                fire: FireIntent::Target {
                    target: wall,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&quiet, &mut silent);
    while silent.resolving_tick().is_some() && !silent.is_over() {
        silent.step_tick(&quiet);
    }
    assert_eq!(
        silent.unit(wall).unwrap().pressure,
        0,
        "plinking with a belt that declares nothing does not fray anyone's nerves"
    );

    // And the shipped reading: the loud belt again, against a mod that lets
    // none of a bullet's suppression through plate. The belt still declares
    // it; the crew behind the glacis does not feel it.
    let shipped = {
        let mut reg = registry_wireless();
        reg.ammo.get_mut("ball_mg").expect("shipped").suppression = 2;
        reg.morale.through_plate_percent = 0;
        reg
    };
    let mut buttoned = plink_stage(&shipped, 301);
    buttoned
        .apply(
            &shipped,
            &Order::SetFire {
                unit: plinker,
                fire: FireIntent::Target {
                    target: wall,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&shipped, &mut buttoned);
    while buttoned.resolving_tick().is_some() && !buttoned.is_over() {
        buttoned.step_tick(&shipped);
    }
    assert_eq!(
        buttoned.unit(wall).unwrap().pressure,
        0,
        "the proper equipment is needed: a belt does not pin a crew behind plate"
    );
}

#[test]
fn a_gun_that_cannot_hurt_what_it_sees_holds_its_fire() {
    // The same scene with nobody ordering anything: opportunity fire prices
    // the shot at zero and the crew keeps her gun quiet and her position
    // secret. Before the gate this was impossible — the floor made every
    // shot worth something, so every gun in range always spoke.
    //
    // "Cannot hurt" is a statement about the currency the mod declares, and
    // Wave 1 gave the currency a second half. Under the base mod's belt this
    // crew opens up, and should: `suppression: 2` is the designer saying a
    // burst on a glacis is worth firing, and
    // `the_loader_will_fire_a_belt_at_plate_she_cannot_beat_when_fear_is_worth_something`
    // is that half of the ruling as its own test. The discipline this one
    // defends is the other half and is unchanged: a shot worth *nothing* is
    // not taken. So the stage is a mod that declines both — a silent belt, and
    // fear priced at nothing — which is the game before suppression existed,
    // and the assertion is the one it always made.
    let reg = {
        let mut reg = registry_wireless();
        // And a third price since 2026-09-23 — closing the hull up
        // (`balance.buttoning_worth`) — declined with the other two.
        reg.balance.buttoning_worth = 0;
        reg.ammo
            .get_mut("ball_mg")
            .expect("the base mod ships a belt")
            .suppression = 0;
        reg.morale.point_worth = 0.0;
        reg
    };
    let mut state = plink_stage(&reg, 302);
    let plinker = UnitId(0);
    commit_all(&reg, &mut state);
    while state.resolving_tick().is_some() && !state.is_over() {
        for event in state.step_tick(&reg) {
            if let BattleEvent::ShotFired { attacker, .. } = event {
                assert_ne!(
                    attacker, plinker,
                    "nothing aboard can hurt a heavy tank in a currency that \
                     prices only damage, so she holds fire"
                );
            }
        }
    }
}

#[test]
fn a_kinetic_round_that_beats_a_plate_up_close_fades_at_the_end_of_its_reach() {
    // Velocity, cashed out: the same gun against the same plate penetrates
    // at arm's length and bounces at the end of its reach, because a solid
    // shot arrives with whatever speed the air has left it. Scatter is
    // zeroed so the gate is a hard threshold and the test is arithmetic,
    // not luck; the interpolation endpoints are set so the plate sits
    // between them.
    let mut reg = registry_wireless();
    reg.balance.pen_scatter = 0;
    // Two things the stage has to hold still, both of them new, and both of
    // them the rest of this wave working rather than failing.
    //
    // The coaxial is quiet. It cannot beat this plate either, but it now has
    // a reason to fire at it — `suppression: 2` — and it fires on every tick
    // the main gun is reloading, so every burst draws from the same rng
    // stream and the solid shot's four rolls stop being the four rolls this
    // seed was chosen for. `a_burst_that_cannot_get_through_still_counts_for_what_it_does_to_her_nerve`
    // is where the coaxial's new job is pinned; here it is noise.
    //
    // And the rack holds only the round under test. The loader prices what
    // she chambers in the same currency as everybody else, so at the far end
    // of the reach — where the solid shot is exactly what this test says it
    // is, a bounce — she reaches for high explosive instead, because HE at
    // least rattles them. That is `best_round_against` making the right call
    // and the whole reason `round_worth` carries the pressure term; a test
    // about a kinetic round's falloff simply must not leave her the choice.
    if let Some(belt) = reg.ammo.get_mut("ball_mg") {
        belt.suppression = 0;
    }
    if let Some(ammo) = reg.ammo.get_mut("ap_75") {
        ammo.penetration = [7, 4]; // medium front plate is 5: beaten near, safe far
    }
    if let Some(w) = reg.weapons.get_mut("gun_75") {
        w.range = [1, 4];
        w.ammo = vec!["ap_75".into()];
    }
    let shoot = |reg: &DataRegistry, dist: i32, seed: u64| -> (u32, u32) {
        let row = "g".repeat(8);
        let mut state = two_side_battle(
            reg,
            &[&row, &row, &row],
            vec![
                unit_at([1, 1], 0, "medium_tank", "Gunner"),
                unit_at([1 + dist, 1], 1, "medium_tank", "Plate"),
            ],
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
        commit_all(reg, &mut state);
        let (mut hits, mut bounces) = (0, 0);
        while state.resolving_tick().is_some() && !state.is_over() {
            for event in state.step_tick(reg) {
                // The solid shot and nothing else. This used to count every
                // arrival at the Plate, which was the same thing while the
                // gunner's coaxial had no reason to fire; since suppression
                // joined the currency her machine gun opens up too and
                // bounces off the same front plate, so an unfiltered count
                // is a count of two guns. Filtering on the round is exactly
                // what `ShotHit`/`ShotBounced` gained an `ammo` field for.
                let ap = |a: &Option<String>| a.as_deref() == Some("ap_75");
                match event {
                    BattleEvent::ShotHit {
                        target: UnitId(1),
                        ref ammo,
                        ..
                    } if ap(ammo) => hits += 1,
                    BattleEvent::ShotBounced {
                        target: UnitId(1),
                        ref ammo,
                        ..
                    } if ap(ammo) => bounces += 1,
                    _ => {}
                }
            }
        }
        (hits, bounces)
    };

    let (near_hits, near_bounces) = shoot(&reg, 1, 303);
    assert!(near_hits > 0, "point blank, the round goes through");
    assert_eq!(near_bounces, 0, "every time");

    let (far_hits, far_bounces) = shoot(&reg, 4, 304);
    assert_eq!(far_hits, 0, "at the end of its reach it cannot");
    assert!(far_bounces > 0, "and the plate says so out loud");
}

#[test]
fn no_seam_of_the_hull_is_impenetrable() {
    // The regression that shaped the obliquity model, pinned. An earlier
    // draft measured impact angle against the armor ARC's central normal;
    // the front arc spans ninety degrees of incoming ray, so its edges
    // read as near-parallel strikes and tripled the plate — a tank went
    // impenetrable from directions her side armor was plainly facing, and
    // a duel's return fire went silent for a tick until the hulls turned.
    // The hull is a hexagonal prism: the struck face's own normal is never
    // more than thirty degrees off the shot, so a round that comfortably
    // beats every plate gets in from every bearing there is.
    let mut reg = registry_wireless();
    reg.balance.pen_scatter = 0;
    let row = "g".repeat(9);
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row, &row, &row],
        vec![
            unit_at([1, 1], 0, "medium_tank", "Gunner"),
            unit_at([4, 2], 1, "medium_tank", "Hull"),
        ],
        305,
    );
    let (gunner, hull) = (UnitId(0), UnitId(1));
    let center = state.unit(hull).unwrap().pos;
    let ring: Vec<tactics_core::Hex> = center
        .all_neighbors()
        .into_iter()
        .chain(center.all_neighbors().into_iter().map(|n| n + (n - center)))
        .collect();
    for post in ring {
        if state.map.get(post).is_none() || state.unit_at(post).is_some() {
            continue;
        }
        state.units[gunner.index()].pos = post;
        let preview = tactics_core::battle::preview_attack(&reg, &state, gunner, 0, hull, false)
            .expect("both stand on the field");
        assert_eq!(
            preview.pen_chance, 100,
            "a 75 that beats every plate of a medium gets in from {post:?} too"
        );
    }
}

#[test]
fn the_racks_run_dry_and_the_gun_falls_silent() {
    // One armor-piercing round left and no high explosive at all: she
    // fires it, the moment is announced, and the gun says nothing for the
    // rest of the battle — silence the player was told about rather than
    // an order the game ate.
    let reg = registry_wireless();
    let mut state = duel(&reg, 306);
    let shooter = UnitId(0);
    state.units[shooter.index()].ammo = [("ap_75".to_string(), 1), ("he_75".to_string(), 0)]
        .into_iter()
        .collect();
    commit_all(&reg, &mut state);

    let (mut fired, mut dry_said) = (0, false);
    while state.resolving_tick().is_some() && !state.is_over() {
        for event in state.step_tick(&reg) {
            match event {
                BattleEvent::ShotFired { attacker, .. } if attacker == shooter => fired += 1,
                BattleEvent::WeaponDry { unit, .. } if unit == shooter => dry_said = true,
                _ => {}
            }
        }
    }
    assert_eq!(
        fired, 1,
        "the last round goes downrange and nothing follows it"
    );
    assert!(dry_said, "and running dry is said out loud");
    assert_eq!(
        state.unit(shooter).map(|u| u.ammo["ap_75"]),
        Some(0),
        "the rack is empty"
    );
}

#[test]
fn a_mod_without_ammunition_still_fights_with_its_guns_own_numbers() {
    // Additivity, read strictly: ammunition is content a mod may decline.
    // Stripping every weapon's ammo list drops combat onto the legacy
    // path — the gun's own damage and penetration through the same gate,
    // nothing counted, nothing spent — so a pre-ballistics mod keeps the
    // relationships its author tuned, forever, with infinite rounds.
    let mut reg = registry_wireless();
    for weapon in reg.weapons.values_mut() {
        weapon.ammo.clear();
    }
    let mut state = duel(&reg, 307);
    let racks: Vec<_> = state.units.iter().map(|u| u.ammo.clone()).collect();
    commit_all(&reg, &mut state);

    let mut hit = false;
    while state.resolving_tick().is_some() && !state.is_over() {
        for event in state.step_tick(&reg) {
            if matches!(event, BattleEvent::ShotHit { .. }) {
                hit = true;
            }
        }
    }
    assert!(hit, "a 75 still beats a medium's plate on its own numbers");
    for (unit, before) in state.units.iter().zip(racks) {
        assert_eq!(
            unit.ammo, before,
            "uncounted rounds are infinite ones: nothing was spent"
        );
    }
}

// --- the hull and the gun --------------------------------------------------

/// A gun crew on open grass with a recon car off her flank: the car's
/// machine gun cannot hurt either chassis these tests stage, so the only
/// thing that can happen is the subject answering it. Her hull is turned to
/// face east, which puts the car — due north of her — outside her frontal
/// arc. Unit 0 is the subject, unit 1 the car.
fn flanked(reg: &DataRegistry, vehicle: &str) -> BattleState {
    let row = "ggggggg";
    let mut state = common::two_side_battle(
        reg,
        &[row, row, row, row, row],
        vec![
            common::unit_at([3, 4], 0, vehicle, "Subject"),
            common::unit_at([2, 0], 1, "recon_car", "Flanker"),
        ],
        5,
    );
    let east = {
        let me = state.unit(UnitId(0)).unwrap().pos;
        me.main_direction_to(me + tactics_core::Hex::new(1, 0))
    };
    state.unit_mut(UnitId(0)).unwrap().facing = east;
    let (me, car) = (
        state.unit(UnitId(0)).unwrap(),
        state.unit(UnitId(1)).unwrap(),
    );
    assert_ne!(
        tactics_core::battle::struck_facing(me.pos, me.facing, car.pos),
        ArmorFacing::Front,
        "the stage needs the flanker outside her frontal arc"
    );
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "and in her sight"
    );
    state
}

/// Order the subject onto the flanker and play ticks until she has fired or
/// `ticks` have gone by. The events of every tick played.
fn engage(reg: &DataRegistry, state: &mut BattleState, ticks: u32) -> Vec<BattleEvent> {
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
        .expect("an ordered shot at a spotted enemy");
    common::commit_all(reg, state);
    let mut events = Vec::new();
    for _ in 0..ticks {
        let tick = state.step_tick(reg);
        let fired = tick.iter().any(
            |e| matches!(e, BattleEvent::ShotFired { attacker, .. } if *attacker == UnitId(0)),
        );
        events.extend(tick);
        if fired {
            break;
        }
    }
    events
}

/// A turret answers a flank without turning the hull, so she is still
/// showing the side she was caught on. Before hulls and turrets were told
/// apart every vehicle swung her whole hull onto whatever she shot at, for
/// nothing: a flank was worth one shot, and 87–90% of hits in every sample
/// struck front plate.
#[test]
fn a_turret_answers_a_flank_without_turning_the_hull() {
    let reg = seen(registry());
    assert!(reg.vehicle("medium_tank").unwrap().turret);
    let mut state = flanked(&reg, "medium_tank");
    let before = state.unit(UnitId(0)).unwrap().facing;
    let events = engage(&reg, &mut state, 6);
    assert!(
        events.iter().any(
            |e| matches!(e, BattleEvent::ShotFired { attacker, .. } if *attacker == UnitId(0))
        ),
        "she answers: {events:?}"
    );
    assert_eq!(
        state.unit(UnitId(0)).unwrap().facing,
        before,
        "and her hull is where she left it"
    );
}

/// A gun that does not traverse swings the whole hull round first, says so,
/// and pays `balance.pivot_ticks` before the shot — the price of a casemate's
/// low hull and heavy front. No round is spent on the pivot.
#[test]
fn a_casemate_swings_her_hull_round_and_pays_for_it() {
    let reg = seen(registry());
    assert!(!reg.vehicle("tank_destroyer").unwrap().turret);
    let pivot = reg.balance.pivot_ticks;
    assert!(pivot > 0, "the base mod prices the pivot");
    let mut state = flanked(&reg, "tank_destroyer");
    let rounds_before: u32 = state.unit(UnitId(0)).unwrap().ammo.values().sum();
    let events = engage(&reg, &mut state, 1);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, BattleEvent::Pivoted { unit, .. } if *unit == UnitId(0))),
        "the pivot is announced: {events:?}"
    );
    assert!(
        !events.iter().any(
            |e| matches!(e, BattleEvent::ShotFired { attacker, .. } if *attacker == UnitId(0))
        ),
        "and there is no shot on the tick she spends turning"
    );
    let me = state.unit(UnitId(0)).unwrap();
    let car = state.unit(UnitId(1)).unwrap();
    assert_eq!(
        tactics_core::battle::struck_facing(me.pos, me.facing, car.pos),
        ArmorFacing::Front,
        "her front is on the flanker now"
    );
    assert_eq!(
        me.ammo.values().sum::<u32>(),
        rounds_before,
        "turning costs time, never a shell"
    );
    assert!(
        me.cooldowns.iter().all(|cd| *cd >= pivot - 1),
        "every gun on the hull waits out the swing: {:?}",
        me.cooldowns
    );
}

/// With no turret declared and no pivot priced, every hull swings onto its
/// target and fires in the same tick — the game before this rule, which is
/// what a mod that says nothing gets.
#[test]
fn with_no_turret_and_no_pivot_every_hull_swings_onto_its_target_as_it_always_did() {
    let mut reg = seen(registry());
    reg.balance.pivot_ticks = 0;
    for v in reg.vehicles.values_mut() {
        v.turret = false;
    }
    let mut state = flanked(&reg, "medium_tank");
    let events = engage(&reg, &mut state, 6);
    assert!(
        events.iter().any(
            |e| matches!(e, BattleEvent::ShotFired { attacker, .. } if *attacker == UnitId(0))
        ),
        "she fires: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, BattleEvent::Pivoted { .. })),
        "without a price nobody announces a pivot"
    );
    let me = state.unit(UnitId(0)).unwrap();
    let car = state.unit(UnitId(1)).unwrap();
    assert_eq!(
        tactics_core::battle::struck_facing(me.pos, me.facing, car.pos),
        ArmorFacing::Front,
        "and her whole hull turned onto the car"
    );
}

// --- the outcome engine: no hit points -------------------------------------

#[test]
fn a_penetration_names_the_girl_it_hurt() {
    // Permadeath without a name is just a number going down. Every crew hit
    // carries the cadet it found, she is really aboard the vehicle it names,
    // and the seat she sits in is marked — the state and the story must be
    // the same fact.
    let mut reg = registry_wireless();
    soften(&mut reg); // interiors are cadets only: every pen finds one
    let mut state = duel(&reg, 401);
    let (west, east) = (UnitId(0), UnitId(1));
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: west,
                fire: FireIntent::Target {
                    target: east,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let mut named = Vec::new();
    while state.resolving_tick().is_some() && !state.is_over() {
        for event in state.step_tick(&reg) {
            if let BattleEvent::CrewHit { unit, cadet, .. } = event
                && unit == east
            {
                named.push(cadet);
            }
        }
    }
    assert!(!named.is_empty(), "a softened 75 wounds rather than breaks");
    let hull = state.unit(east).unwrap();
    for cadet in named {
        let seat = hull
            .crew
            .iter()
            .position(|g| *g == cadet)
            .expect("the cadet the event names is aboard the vehicle it names");
        assert_ne!(
            hull.crew_state.get(seat).copied().unwrap_or_default(),
            tactics_core::battle::CrewCondition::Fine,
            "and her seat is marked"
        );
    }
}

#[test]
fn a_destroyed_gun_module_silences_the_primary_weapon_only() {
    // The module maps to the mount: main gun dead means the 75 never speaks
    // again, while the coaxial stays mechanically ready — it merely has
    // nothing worth shooting at in this scene, which is the gate's own
    // discipline, not the module's.
    let reg = registry_wireless();
    let mut state = duel(&reg, 402);
    let (west, east) = (UnitId(0), UnitId(1));
    state.units[west.index()]
        .modules
        .insert("main_gun".into(), 0);
    assert!(
        !tactics_core::battle::weapon_ready(&reg, &state, west, 0),
        "a destroyed gun module is not a gun"
    );
    assert!(
        tactics_core::battle::weapon_ready(&reg, &state, west, 1),
        "the machine gun is its own mount and still answers ready"
    );
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: west,
                fire: FireIntent::Target {
                    target: east,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);
    let mut west_fired_gun = false;
    while state.resolving_tick().is_some() && !state.is_over() {
        for event in state.step_tick(&reg) {
            if let BattleEvent::ShotFired {
                attacker, weapon, ..
            } = &event
                && *attacker == west
                && weapon == "gun_75"
            {
                west_fired_gun = true;
            }
        }
    }
    assert!(!west_fired_gun, "a destroyed gun does not obey the order");
}

#[test]
fn a_mobility_kill_stops_her_where_she_stands() {
    // Tracks are the module with a middle state: damaged running gear
    // halves her speed, destroyed stops her on the spot, and talented
    // driving buys back neither.
    let reg = registry_wireless();
    let mut state = duel(&reg, 403);
    let west = UnitId(0);
    let whole = {
        let u = state.unit(west).unwrap();
        tactics_core::battle::move_points(&reg, &state.roster, u, state.terrain_at(u.pos))
    };
    assert!(whole > 0);

    state.units[west.index()].modules.insert("tracks".into(), 1);
    let limping = {
        let u = state.unit(west).unwrap();
        tactics_core::battle::move_points(&reg, &state.roster, u, state.terrain_at(u.pos))
    };
    assert_eq!(limping, whole / 2, "one thrown track halves her");

    state.units[west.index()].modules.insert("tracks".into(), 0);
    let stopped = {
        let u = state.unit(west).unwrap();
        tactics_core::battle::move_points(&reg, &state.roster, u, state.terrain_at(u.pos))
    };
    assert_eq!(stopped, 0, "both gone stops her where she stands");
    assert_eq!(
        reachable(&reg, &state, west).len(),
        1,
        "the move overlay is her own tile and nothing else"
    );
}

#[test]
fn a_dead_radio_drops_her_off_the_net() {
    // The radio module dying is the chain-of-command layer's stake in
    // ballistics: six hexes from her leader — inside the set's reach, past
    // flag range — she is on the net right up until the set is wreckage,
    // and then she is a cadet driving on standing orders.
    let reg = registry();
    let mut state = BattleState::from_map(&reg, "river_crossing", 8).unwrap();
    let formation = state.formations()[0].clone();
    let leader = formation.leader.expect("the formation has a leader");
    let stray = *formation
        .members
        .iter()
        .find(|m| **m != leader)
        .expect("a formation of one tests nothing");
    let post = state.unit(leader).expect("leader").pos + tactics_core::Hex::new(6, 0);
    state.units[stray.index()].pos = post;
    settle(&reg, &mut state);
    assert!(
        state.formations()[0].in_contact(stray),
        "six hexes out, the set carries her leader's voice"
    );

    state.units[stray.index()]
        .modules
        .insert("radio_set".into(), 0);
    settle(&reg, &mut state);
    assert!(
        !state.formations()[0].in_contact(stray),
        "the same six hexes with a wrecked set is silence"
    );
}

#[test]
fn a_one_rung_ladder_never_abandons_anything() {
    // Difficulty is a mod, read against the bail-out: abandoning rides the
    // rung the pressure ladder puts a crew on, so a ladder with one steady
    // rung produces crews that stay with the tank whatever comes through
    // the armor — the gentle game, with no `if` in Rust to switch.
    let mut reg = registry_wireless();
    reg.morale.rungs = vec![tactics_core::data::MoraleRung {
        id: "steady".into(),
        name: "Steady".into(),
        at_pressure: 0,
        obeys: true,
        accuracy: 0,
        pinned: false,
    }];
    let mut state = duel(&reg, 405);
    commit_all(&reg, &mut state);
    for _ in 0..6 {
        if state.is_over() {
            break;
        }
        for event in play_round(&reg, &mut state) {
            assert!(
                !matches!(event, BattleEvent::Abandoned { .. }),
                "nobody jumps off a one-rung ladder"
            );
        }
        commit_all(&reg, &mut state);
    }
}

#[test]
fn a_bounced_shell_wrecks_no_plate_it_never_touched() {
    // The B5 instrument's first finding, cured and pinned. Overpressure
    // used to consult the hull's THINNEST plate, so a 105 bouncing off a
    // heavy tank's glacis (blast 6 against a rear plate of 3, doubled)
    // wrecked her through armor the burst never faced — artillery needed
    // 1.8 shells per heavy while its penetration table read zero. The
    // plate consulted now is the one the burst arrives on: a frontal
    // bounce is a frontal problem, external module rattles at worst, and
    // the heavy tank drives away from a plunging barrage that never finds
    // anything but her glacis. (With the base mod's numbers the direct-hit
    // overmatch path only fires through adjacent splash onto genuinely
    // soft skins — `neighbors_of_a_shellburst_feel_half_the_blast` pins
    // that half.)
    let mut reg = registry_wireless();
    reg.balance.pen_scatter = 0;
    let row = "g".repeat(8);
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([0, 1], 0, "artillery", "Battery"),
            unit_at([4, 1], 1, "heavy_tank", "Wall"),
        ],
        406,
    );
    let (battery, wall) = (UnitId(0), UnitId(1));
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: battery,
                fire: FireIntent::Target {
                    target: wall,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let (mut bounces, mut pens) = (0, 0);
    for _ in 0..4 {
        if state.is_over() {
            break;
        }
        for event in play_round(&reg, &mut state) {
            match event {
                BattleEvent::ShotBounced { target, .. } if target == wall => bounces += 1,
                BattleEvent::ShotHit { target, .. } if target == wall => pens += 1,
                _ => {}
            }
        }
        commit_all(&reg, &mut state);
    }
    assert!(bounces > 0, "the shells arrive and the glacis holds");
    assert_eq!(pens, 0, "a 105 cannot beat a heavy tank's front");
    assert!(
        state
            .unit(wall)
            .is_some_and(|u| u.alive() && u.destruction() != Some(Destruction::Crushed)),
        "and she is not wrecked through a plate the bursts never touched"
    );
}

#[test]
fn an_emptied_rack_is_harder_to_torch() {
    // Brew-up chance rides the fraction of ammunition still aboard, so
    // shooting your racks empty is quietly a survival strategy. Staged so
    // every effect roll finds the rack: full racks at certainty burn on the
    // first penetration; the same tank with empty racks takes the same hit
    // and does not.
    let mut reg = registry_wireless();
    reg.balance.pen_scatter = 0;
    reg.balance.crew_weight = 0;
    reg.balance.brewup_percent = 100;
    for module in reg.modules.values_mut() {
        if module.effect != tactics_core::data::ModuleEffect::Ammo {
            module.size = 0;
        }
    }

    let torch = |reg: &DataRegistry, empty: bool, seed: u64| -> bool {
        let mut state = duel(reg, seed);
        let (west, east) = (UnitId(0), UnitId(1));
        if empty {
            for count in state.units[east.index()].ammo.values_mut() {
                *count = 0;
            }
        }
        state
            .apply(
                reg,
                &Order::SetFire {
                    unit: west,
                    fire: FireIntent::Target {
                        target: east,
                        weapon: 0,
                    },
                },
            )
            .unwrap();
        commit_all(reg, &mut state);
        let mut brewed = false;
        while state.resolving_tick().is_some() && !state.is_over() {
            for event in state.step_tick(reg) {
                if matches!(event, BattleEvent::BrewedUp { unit } if unit == east) {
                    brewed = true;
                }
            }
        }
        brewed
    };

    assert!(torch(&reg, false, 407), "full racks at certainty burn");
    assert!(!torch(&reg, true, 408), "empty racks cannot");
}

// --- soft targets and hidden ones ------------------------------------------

#[test]
fn a_platoon_in_the_trees_is_invisible_until_the_scout_closes() {
    // Concealment: standing on ground somebody can see is not being seen.
    // A rifle platoon at concealment 40 doubles to 80 in the treeline, so
    // the recon car that sees twenty hexes of open ground spots her at
    // four — while a tank on the same tile is spotted at the full twenty,
    // which is the additivity half of the claim.
    let reg = registry_wireless();
    let row = format!("gggggg{}g", "f");
    let stage = |vehicle: &str, dist: i32| -> BattleState {
        two_side_battle(
            &reg,
            &[&row, &row, &row],
            vec![
                unit_at([6 - dist.min(6), 1], 0, "recon_car", "Scout"),
                unit_at([6, 1], 1, vehicle, "Quarry"),
            ],
            501,
        )
    };

    let far = stage("rifle_platoon", 6);
    assert!(
        !far.fog.side(0).spotted.contains(&UnitId(1)),
        "six hexes out, the treeline keeps her"
    );
    let near = stage("rifle_platoon", 3);
    assert!(
        near.fog.side(0).spotted.contains(&UnitId(1)),
        "three hexes out, even trees are not enough"
    );
    let control = stage("medium_tank", 6);
    assert!(
        control.fog.side(0).spotted.contains(&UnitId(1)),
        "a tank on the same tile hides from nobody"
    );
}

#[test]
fn springing_the_ambush_spends_it() {
    // The other half of concealment: firing reveals, unconditionally. The
    // platoon the carrier could not see kills it from three hexes — and is
    // seen by everyone from the muzzle flash on.
    let reg = registry_wireless();
    let rows = ["ggggg", "ggfgg", "ggggg"];
    let mut state = two_side_battle(
        &reg,
        &rows,
        vec![
            unit_at([2, 1], 0, "rifle_platoon", "Ambush"),
            unit_at([4, 1], 1, "apc", "Taxi"),
        ],
        502,
    );
    let (platoon, taxi) = (UnitId(0), UnitId(1));
    assert!(
        !state.fog.side(1).spotted.contains(&platoon),
        "the taxi drives past a treeline it cannot read"
    );
    assert!(
        state.fog.side(0).spotted.contains(&taxi),
        "while the platoon has watched her come the whole way"
    );
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: platoon,
                fire: FireIntent::Target {
                    target: taxi,
                    weapon: 1,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);
    let events = state.resolve_round(&reg);
    assert!(
        events.iter().any(|e| matches!(
            e,
            BattleEvent::ShotFired { attacker, weapon, .. }
                if *attacker == platoon && weapon == "rpg"
        )),
        "the rocket goes out"
    );
    assert!(
        state.fog.side(1).spotted.contains(&platoon) || state.unit(taxi).is_none_or(|u| !u.alive()),
        "and the ambush is spent: sprung means seen"
    );
}

#[test]
fn a_thinned_platoon_shoots_at_half_strength() {
    // The troops module's firepower meaning: every weapon the platoon
    // fires scales by the riflemen still standing. Half the sections is
    // half the fire, through the same preview the player reads.
    let reg = registry_wireless();
    let row = "g".repeat(6);
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([1, 1], 0, "rifle_platoon", "Platoon"),
            unit_at([3, 1], 1, "scout_section", "Target"),
        ],
        503,
    );
    let (platoon, target) = (UnitId(0), UnitId(1));
    let full = tactics_core::battle::preview_attack(&reg, &state, platoon, 0, target, false)
        .expect("both stand")
        .damage;
    assert!(full > 0, "a whole platoon's rifles are worth something");

    state.units[platoon.index()]
        .modules
        .insert("rifle_sections".into(), 3);
    let half = tactics_core::battle::preview_attack(&reg, &state, platoon, 0, target, false)
        .expect("both stand")
        .damage;
    assert_eq!(
        half,
        full * 3 / 6,
        "three of six sections left is half the fire"
    );
}

#[test]
fn a_shellburst_beside_a_platoon_is_attrition_not_erasure() {
    // The plate-zero carve-out: a dispersed platoon has no hull for blast
    // overmatch to crush, so splash converts to casualty rolls. Shell after
    // shell lands next door and the platoon bleeds — and is still a platoon,
    // never a single-event deletion.
    let reg = registry_wireless();
    let row = "g".repeat(9);
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([0, 1], 0, "artillery", "Battery"),
            unit_at([5, 1], 1, "rifle_platoon", "Platoon"),
        ],
        504,
    );
    let (_, platoon) = (UnitId(0), UnitId(1));
    let beside = state.unit(platoon).unwrap().pos + tactics_core::Hex::new(1, 0);
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: UnitId(0),
                fire: FireIntent::Area {
                    at: beside,
                    weapon: 0,
                },
            },
        )
        .unwrap();
    commit_all(&reg, &mut state);

    let mut bled = false;
    for _ in 0..4 {
        if state.is_over() {
            break;
        }
        for event in play_round(&reg, &mut state) {
            if matches!(
                event,
                BattleEvent::ModuleHit { unit, .. } | BattleEvent::CrewHit { unit, .. }
                    if unit == platoon
            ) {
                bled = true;
            }
        }
        commit_all(&reg, &mut state);
    }
    assert!(bled, "four volleys next door draw blood");
    let unit = state.units[platoon.index()].clone();
    assert!(
        unit.destruction() != Some(Destruction::Crushed),
        "but a spread-out platoon is not a hull to crush"
    );
}

#[test]
fn a_remnant_platoon_is_a_story_not_a_gun() {
    // Troops at zero: the cadets are alive, the platoon is finished. Her
    // rifles are worth nothing, she holds fire even with an enemy in her
    // lap, and her condition says what the withdraw machinery needs to
    // hear.
    //
    // "Worth nothing" is a statement about the currency, and Wave 1 gave it
    // a second half, so the stage now says which currency it means: fear
    // priced at nothing, which is the game before suppression. Two things
    // were found by running it the other way and both are recorded here
    // because they are the interesting part.
    //
    // The first is a fix. `mustered` scaled a remnant's *damage* by the
    // riflemen still standing and not her round's `suppression`, so two
    // cadets could pin a tank as hard as a full platoon; it scales both now,
    // which is the same sentence this test is named for said in the other
    // currency.
    //
    // The second was the lead's to rule on, and was ruled on (2026-09-09):
    // the ladder now charges a penetration by the share of its listed
    // budget it spent, so a platoon whose damage has been mustered to
    // nothing no longer expects the full `hit + penetrated` a shot. What she
    // still expects is the price of the one point the resolver floors every
    // penetration's spend at — a third of a rifle's — which is why this
    // stage still says which currency it means, and why
    // `a_remnant_platoon_frightens_by_the_one_point_her_bullet_still_spends`
    // records the residue rather than hiding it.
    let reg = {
        let mut reg = registry_wireless();
        reg.morale.point_worth = 0.0;
        reg
    };
    let row = "g".repeat(5);
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([1, 1], 0, "rifle_platoon", "Remnant"),
            unit_at([3, 1], 1, "scout_section", "Enemy"),
        ],
        505,
    );
    let platoon = UnitId(0);
    state.units[platoon.index()]
        .modules
        .insert("rifle_sections".into(), 0);
    assert!(
        state.condition(&reg, state.unit(platoon).unwrap()) < 0.5,
        "a shattered platoon reads as one"
    );
    commit_all(&reg, &mut state);
    for event in state.resolve_round(&reg) {
        assert!(
            !matches!(event, BattleEvent::ShotFired { attacker, .. } if attacker == platoon),
            "two cadets and no riflemen fire nothing worth firing"
        );
    }
    assert!(
        state.units[platoon.index()].alive(),
        "and she is a story still on the field, not a deletion"
    );
}

/// The ladder charges a shell for what it spent, not for the fact of its
/// arrival.
///
/// The designer's ruling on Wave 1's open question, read straight off the
/// price list: a clean penetration costs `hit + penetrated`, one that got a
/// quarter of itself through costs a quarter of that, and one that spent
/// nothing costs nothing beyond what the round declares as suppression. The
/// bounce price is untouched — a bounce spends nothing by definition and is
/// priced on the ring, not the spend. `spent_share` is the one reading of
/// "how much of its budget", with the resolver's floor (a penetration always
/// puts one point inside) and a clamp at the whole of it.
#[test]
fn the_ladder_charges_a_shell_for_what_it_spent() {
    let mut reg = registry();
    // The outcome and the round's own suppression; knowing she is the
    // target is a flat price on top, tested in `fire_and_movement.rs`.
    reg.morale.targeted = 0;
    let rules = &reg.morale;
    let felt = RoundPressure {
        small_arms: false,
        suppression: 2,
        armoured: false,
        open_top: false,
        aimed: true,
    };
    let outcome = (rules.hit + rules.penetrated) as f32;
    assert_eq!(
        rules.pressure_for(ShotFelt::Penetrated { spent: 1.0 }, felt),
        outcome + 2.0,
        "a clean penetration is the whole ladder price plus the round's own"
    );
    assert_eq!(
        rules.pressure_for(ShotFelt::Penetrated { spent: 0.25 }, felt),
        outcome * 0.25 + 2.0,
        "a shell that scraped a quarter of itself through costs a quarter"
    );
    assert_eq!(
        rules.pressure_for(ShotFelt::Penetrated { spent: 0.0 }, felt),
        2.0,
        "and one that spent nothing costs only what the round declares"
    );
    assert_eq!(
        rules.pressure_for(ShotFelt::Bounced, felt),
        rules.bounced as f32 + 2.0,
        "a bounce is priced on the ring, and the spend does not enter into it"
    );
    use tactics_core::battle::spent_share;
    assert_eq!(spent_share(6, 6), 1.0);
    assert_eq!(spent_share(2, 8), 0.25);
    assert_eq!(
        spent_share(0, 3),
        spent_share(1, 3),
        "the resolver floors every penetration's spend at one point, and so does this"
    );
    assert_eq!(
        spent_share(9, 3),
        1.0,
        "and nothing spends more than it has"
    );
}

/// A remnant platoon frightens by the one point her bullet still spends.
///
/// The residue the ruling leaves, recorded rather than hidden. `mustered`
/// scales a platoon's damage by the riflemen she has left, so a platoon
/// with none rounds to a budget of zero; `resolve_impact` then floors every
/// penetration's spend at one point, so her bullet still puts one inside,
/// and the ladder — reading the same floor through `spent_share` — charges
/// her a third of a rifle's price for it, exactly, where before the ruling
/// it charged the whole. That is what "scale by damage spent" says when the
/// resolver says a point was spent. Whether a platoon with no riflemen
/// should be spending one at all is a question about the floor, and it is
/// the designer's.
///
/// Mutation-checked by reading the muster fraction instead of the floored
/// spend (zero rather than a third): the ratio then reads 0 and the
/// equality fails.
#[test]
fn a_remnant_platoon_frightens_by_the_one_point_her_bullet_still_spends() {
    let mut reg = seen(registry_wireless());
    // What her bullet spends, alone. Knowing she is the target is charged
    // whatever the bullet does, so a remnant and a full platoon share it
    // and it would blur the ratio this test reads — as would a rifle round
    // that declares suppression of its own, or a miss priced at all: each
    // is a constant both platoons pay.
    reg.morale.targeted = 0;
    reg.morale.near_miss_percent = 0;
    reg.ammo.get_mut("rifle_ball").expect("shipped").suppression = 0;
    let row = "g".repeat(5);
    let mut state = two_side_battle(
        &reg,
        &[&row, &row, &row],
        vec![
            unit_at([1, 1], 0, "rifle_platoon", "Platoon"),
            unit_at([3, 1], 1, "scout_section", "Enemy"),
        ],
        506,
    );
    let (platoon, enemy) = (UnitId(0), UnitId(1));
    let (from, at) = (
        state.unit(platoon).unwrap().pos,
        state.unit(enemy).unwrap().pos,
    );
    let rifles = reg.weapon("rifles").expect("the base mod ships rifles");
    let expects = |state: &BattleState| {
        tactics_core::battle::expected_pressure(
            &reg, state, platoon, from, rifles, enemy, at, false,
        )
    };
    let full = expects(&state);
    assert!(
        full > 0.0,
        "a full platoon's rifles frighten a scout section"
    );
    state.units[platoon.index()]
        .modules
        .insert("rifle_sections".into(), 0);
    let remnant = expects(&state);
    let floor = tactics_core::battle::spent_share(0, rifles.damage);
    assert!(
        (remnant / full - floor).abs() < 1e-3,
        "a remnant expects the floored point's share of a full platoon's fear, \
         {floor:.3}, and reads {remnant:.3} against {full:.3}"
    );
    assert!(
        remnant > 0.0,
        "which is not nothing — the residue this test exists to record"
    );
}

#[test]
fn an_unseen_crew_holds_her_rockets_for_the_killing_shot() {
    // Ambush discipline: the enemy has not seen her, and that advantage
    // is not spent on a mediocre shot. A tank destroyer at the rocket's
    // full reach is a coin flip through the front plate — the unseen
    // platoon lets him pass. A taxi she has let close to two hexes is the
    // decisive shot the rockets were carried for, and the same platoon
    // fires without being told. (The ranges differ because the stage must
    // keep her unseen: the destroyer's better glass would find her at
    // two.) Ordered fire never consults any of this: the commander's word
    // outranks the ambusher's patience.
    let reg = registry_wireless();
    let rows = ["ggggg", "gfggg", "ggggg"];
    let watch = |target_vehicle: &str, dist: i32, seed: u64| -> bool {
        let mut state = two_side_battle(
            &reg,
            &rows,
            vec![
                unit_at([1, 1], 0, "rifle_platoon", "Ambush"),
                unit_at([1 + dist, 1], 1, target_vehicle, "Passerby"),
            ],
            seed,
        );
        let platoon = UnitId(0);
        assert!(
            !state.fog.side(1).spotted.contains(&platoon),
            "the stage needs her unseen"
        );
        commit_all(&reg, &mut state);
        state
            .resolve_round(&reg)
            .iter()
            .any(|e| matches!(e, BattleEvent::ShotFired { attacker, .. } if *attacker == platoon))
    };

    assert!(
        !watch("tank_destroyer", 3, 601),
        "a coin-flip shot is not worth the ambush"
    );
    assert!(
        watch("apc", 2, 602),
        "a taxi allowed to close is the shot the rockets were carried for"
    );
}

// --- shells in flight ------------------------------------------------------

/// A battery west, a target east, and nothing but grass between them.
///
/// Grass everywhere is deliberate: the mid-round drill only moves an idle
/// crew to *strictly better* cover, so on a uniform field nobody bolts and a
/// test about where a shell lands is not also a test about who flinched.
/// The target sits seven hexes out, which is inside the howitzer's reach and
/// inside the battery's own eyes (800 m) but outside a scout car's machine
/// gun (600 m) — so the only gun that speaks in these tests is the one being
/// tested.
fn battery_stage(reg: &DataRegistry, target: &str, seed: u64) -> BattleState {
    let row = "g".repeat(16);
    let state = two_side_battle(
        reg,
        &[&row, &row, &row],
        vec![
            unit_at([0, 1], 0, "artillery", "Battery"),
            unit_at([7, 1], 1, target, "Quarry"),
        ],
        seed,
    );
    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the battery has to be able to see what it is laying on"
    );
    state
}

/// Play one round tick by tick, pairing every event with the absolute tick it
/// happened on — which is the clock `ShellInFlight::lands` is written in and
/// therefore the only honest way to assert that a shell took time.
fn ticked_round(reg: &DataRegistry, state: &mut BattleState) -> Vec<(u64, BattleEvent)> {
    commit_all(reg, state);
    let mut log = Vec::new();
    while state.resolving_tick().is_some() && !state.is_over() {
        let now = state.absolute_tick(reg);
        log.extend(state.step_tick(reg).into_iter().map(|e| (now, e)));
    }
    log
}

#[test]
fn a_shell_takes_time_to_arrive_and_lands_on_the_hex_not_the_unit() {
    // The artillery rework in one scene. The battery is ordered onto a unit,
    // and what it actually fires at is the *ground she is standing on* at the
    // moment the lanyard is pulled. The shell is then in the air for real
    // ticks, and when it comes down she has driven out of the beaten zone: no
    // hit, no bounce, no scratch — a hole in the field where she used to be.
    //
    // The round is slowed to 10 m/s so the flight is fourteen ticks rather
    // than one, and the shell outlives the round that fired it. That is not a
    // fudge of the model, it is the model at a scale a three-row test map can
    // show: `flight_ticks` is the same function the engine used to time this
    // shell, and at the shipped 470 m/s the same sentence needs kilometres of
    // ground to be true on — which is exactly the range artillery is fired at
    // and exactly why the balance table for it moved.
    let mut reg = seen(registry_wireless());
    if let Some(ammo) = reg.ammo.get_mut("he_105") {
        ammo.velocity = 10;
    }
    let mut state = battery_stage(&reg, "recon_car", 501);
    let (battery, quarry) = (UnitId(0), UnitId(1));
    let aim = state.unit(quarry).unwrap().pos;
    let before = state.substance(&reg, state.unit(quarry).unwrap());

    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: battery,
                fire: FireIntent::Target {
                    target: quarry,
                    weapon: 0,
                },
            },
        )
        .unwrap();

    // East, away from the battery, for as long as it takes the shell to come
    // down. A round's intent is cleared when the round is, so she is told
    // again each time — three hexes is what a scout car's wheels buy her on
    // grass, and the point is only that she does not stay put.
    let mut log = Vec::new();
    for _ in 0..3 {
        let [col, row] = tactics_core::hex_to_offset(state.unit(quarry).unwrap().pos);
        state
            .apply(
                &reg,
                &Order::SetMove {
                    unit: quarry,
                    to: tactics_core::offset_to_hex(col + 3, row),
                },
            )
            .unwrap();
        log.extend(ticked_round(&reg, &mut state));
        if log
            .iter()
            .any(|(_, e)| matches!(e, BattleEvent::ShellLanded { .. }))
        {
            break;
        }
    }

    let fired = log
        .iter()
        .find_map(|(tick, e)| match e {
            BattleEvent::ShotFired { attacker, at, .. } if *attacker == battery => {
                Some((*tick, *at))
            }
            _ => None,
        })
        .expect("the battery fires");
    assert_eq!(
        fired.1, aim,
        "she lays the gun on the ground under the unit"
    );

    let landed = log
        .iter()
        .find_map(|(tick, e)| match e {
            BattleEvent::ShellLanded { at, .. } => Some((*tick, *at)),
            _ => None,
        })
        .expect("and the shell eventually arrives");
    let flight = tactics_core::battle::flight_ticks(&reg.scale, 10, 7);
    assert!(
        flight > 1,
        "the test needs a shell that is genuinely in the air"
    );
    assert_eq!(
        landed.0,
        fired.0 + flight,
        "it arrives exactly the flight time later, not in the tick that fired it"
    );
    assert_eq!(landed.1, aim, "on the hex it was aimed at");

    // And nobody was home.
    assert!(
        state.unit_at(aim).is_none(),
        "she drove out of the beaten zone"
    );
    for (_, event) in &log {
        match event {
            BattleEvent::ShotHit { target, .. } | BattleEvent::ShotBounced { target, .. } => {
                assert_ne!(*target, quarry, "a shell cannot strike a unit that left")
            }
            BattleEvent::UnitDestroyed { unit, .. } => {
                assert_ne!(*unit, quarry, "nor kill her from a hex away")
            }
            _ => {}
        }
    }
    assert_eq!(
        state.substance(&reg, state.unit(quarry).unwrap()),
        before,
        "she comes through it untouched"
    );
}

#[test]
fn a_shell_that_catches_her_standing_still_hits_without_a_die_roll() {
    // The other half of the bargain. Artillery has no to-hit roll any more:
    // the scatter that used to be a die is now the flight time, and a crew
    // who spends it parked is simply hit. So the shell lands and the ordinary
    // pipeline runs in the same tick — the gate, and whatever it finds — with
    // no `ShotMissed` anywhere in the stream.
    let reg = registry_wireless();
    let mut state = battery_stage(&reg, "recon_car", 502);
    let (battery, quarry) = (UnitId(0), UnitId(1));
    let aim = state.unit(quarry).unwrap().pos;
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: battery,
                fire: FireIntent::Target {
                    target: quarry,
                    weapon: 0,
                },
            },
        )
        .unwrap();

    let log = ticked_round(&reg, &mut state);
    let landed = log
        .iter()
        .find_map(|(tick, e)| match e {
            BattleEvent::ShellLanded { at, .. } if *at == aim => Some(*tick),
            _ => None,
        })
        .expect("the shell comes down on her");
    let struck = log.iter().any(|(tick, e)| {
        *tick == landed
            && matches!(
                e,
                BattleEvent::ShotHit { target, .. } | BattleEvent::ShotBounced { target, .. }
                    if *target == quarry
            )
    });
    assert!(
        struck,
        "a shell that arrives on an occupied hex resolves against her there and then"
    );
    assert!(
        !log.iter().any(
            |(_, e)| matches!(e, BattleEvent::ShotMissed { attacker, .. } if *attacker == battery)
        ),
        "and no die is thrown for it: artillery misses by being aimed at the wrong hex, not by missing"
    );
}

#[test]
fn neighbors_of_a_shellburst_feel_half_the_blast() {
    // A hundred metres is not far enough away from a 105. The battery shells
    // empty ground next door to a scout car and never touches her hex, but
    // half of a blast of six against her one-inch plate is still overmatch,
    // and overmatch does not consult the penetration gate. No round ever
    // struck her, and she is a wreck.
    let mut reg = registry_wireless();
    reg.balance.pen_scatter = 0;
    let mut state = battery_stage(&reg, "recon_car", 503);
    let (battery, quarry) = (UnitId(0), UnitId(1));
    let next_door = tactics_core::offset_to_hex(6, 1);
    assert_eq!(
        state.unit(quarry).unwrap().pos.distance_to(next_door),
        1,
        "the burst has to be one hex off her, not on her"
    );
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: battery,
                fire: FireIntent::Area {
                    at: next_door,
                    weapon: 0,
                },
            },
        )
        .unwrap();

    let log = ticked_round(&reg, &mut state);
    assert!(
        log.iter()
            .any(|(_, e)| matches!(e, BattleEvent::ShellLanded { at, .. } if *at == next_door)),
        "the shell lands where it was sent"
    );
    assert!(
        !log.iter().any(|(_, e)| matches!(
            e,
            BattleEvent::ShotHit { .. } | BattleEvent::ShotBounced { .. }
        )),
        "nothing was ever struck: this is blast, not gunnery"
    );
    assert!(
        log.iter()
            .any(|(_, e)| matches!(e, BattleEvent::UnitDestroyed { unit, .. } if *unit == quarry)),
        "and the car beside it is finished"
    );
}

#[test]
fn a_shell_is_priced_against_the_plate_it_will_strike() {
    // The playthrough review's headline defect, stated as a rule. Blast used
    // to be priced at a flat fraction of its rating no matter what it landed
    // on, while `overpressure` has always read the struck plate — so the one
    // value function behind the loader's choice and the AI's shot pricing
    // disagreed with the resolver about the most common shell in the game.
    //
    // A 105 against a tank destroyer is the sharpest case there is. Her
    // glacis is six and blast six does not overmatch it; her back plate is
    // one, and the same shell arriving there does not need the penetration
    // gate's permission at all. Same gun, same round, same range: only the
    // arc differs, and the price has to differ with it.
    //
    // The gun is stripped of its penetration first, and that is the whole
    // reason this test is worth anything. The penetration half of the price
    // has ALWAYS read the plate, so a howitzer with its own numbers scores
    // the back of a tank destroyer higher than the front no matter which
    // version of the code is running, and a test that merely compared the
    // two arcs would pass against the defect it was written for. With pen at
    // zero the only term left is blast, which is the term that was flat.
    let mut reg = registry_wireless();
    for weapon in reg.weapons.values_mut() {
        weapon.penetration = 0;
    }
    for ammo in reg.ammo.values_mut() {
        ammo.penetration = [0, 0];
    }
    let mut state = battery_stage(&reg, "tank_destroyer", 811);
    let (battery, quarry) = (UnitId(0), UnitId(1));
    let howitzer = reg
        .weapon(
            &reg.vehicle("artillery")
                .expect("she is in the base mod")
                .weapons[0],
        )
        .expect("the battery has a gun")
        .clone();
    let from = state.unit(battery).expect("on the field").pos;

    let facing_the_guns = state.unit(quarry).expect("on the field").pos;
    state.unit_mut(quarry).expect("on the field").facing = facing_the_guns.main_direction_to(from);
    let front = tactics_core::battle::expected_damage(
        &reg,
        &state,
        battery,
        from,
        &howitzer,
        quarry,
        facing_the_guns,
        false,
    );

    state.unit_mut(quarry).expect("on the field").facing = from.main_direction_to(facing_the_guns);
    let rear = tactics_core::battle::expected_damage(
        &reg,
        &state,
        battery,
        from,
        &howitzer,
        quarry,
        facing_the_guns,
        false,
    );

    assert!(
        rear > front * 3.0,
        "a burst on the back plate is worth far more than the same burst on \
         the glacis, and was worth exactly the same before: front {front}, rear {rear}"
    );
    let whole = state
        .substance(&reg, state.unit(quarry).expect("on the field"))
        .0 as f32;
    assert!(
        rear >= whole * 0.5,
        "and it is priced as what it is — a wreck, not a scratch: {rear} against \
         {whole} of tank destroyer"
    );
}

#[test]
fn a_gun_with_nothing_left_to_break_expects_nothing() {
    // Game 1 of the review, in eight lines. A howitzer put thirty-six shells
    // into one tank destroyer's front; by the seventh round her tracks and
    // her antenna — everything a burst can reach from outside a plate it
    // cannot beat — were already destroyed, and the remaining twenty-nine
    // shells were spent on a vehicle the shell provably could not touch.
    //
    // The AI was not being stubborn. It was reading a number that never
    // consulted the hull, so there was nothing in the arithmetic to change
    // its mind. Now the price of that shot is zero and `best_weapon_against`
    // — the predicate every planner in the game prices shots with — refuses
    // to call it a weapon at all, which is what puts the gun back on a
    // target worth having.
    let reg = seen(registry_wireless());
    let mut state = battery_stage(&reg, "tank_destroyer", 812);
    let (battery, quarry) = (UnitId(0), UnitId(1));
    let howitzer = reg
        .weapon(
            &reg.vehicle("artillery")
                .expect("she is in the base mod")
                .weapons[0],
        )
        .expect("the battery has a gun")
        .clone();
    let from = state.unit(battery).expect("on the field").pos;
    let hull = state.unit(quarry).expect("on the field").pos;
    state.unit_mut(quarry).expect("on the field").facing = hull.main_direction_to(from);

    assert!(
        tactics_core::battle::expected_damage(
            &reg, &state, battery, from, &howitzer, quarry, hull, false
        ) > 0.0,
        "while her running gear and her radio are intact the harassment is worth something"
    );

    let outside: Vec<String> = reg
        .modules
        .iter()
        .filter(|(_, m)| {
            matches!(
                m.effect,
                tactics_core::data::ModuleEffect::Mobility
                    | tactics_core::data::ModuleEffect::Radio
            )
        })
        .map(|(id, _)| id.clone())
        .collect();
    let u = state.unit_mut(quarry).expect("on the field");
    for (id, hits) in u.modules.iter_mut() {
        if outside.contains(id) {
            *hits = 0;
        }
    }

    assert_eq!(
        tactics_core::battle::expected_damage(
            &reg, &state, battery, from, &howitzer, quarry, hull, false
        ),
        0.0,
        "with both of them gone the shell has nothing left to reach"
    );
    // ...and with only damage in the ledger she is not a target at all. That
    // is what this test was written to pin and it is pinned here under the
    // mod that prices only damage, because the shipped one no longer does:
    // `he_105` declares `suppression: 2`, so shelling a crippled tank is
    // still worth doing for what it does to the crew inside, which is the
    // designer's ruling and is a different sentence from "the shell can
    // still break something". The defect this test was written against — a
    // 105 putting thirty-six shells into a hull with nothing left to reach,
    // because the pricing said 1.8 every time — is the `expected_damage`
    // assertion above, and it is untouched.
    let damage_only = {
        let mut reg = reg.clone();
        reg.morale.point_worth = 0.0;
        reg
    };
    assert!(
        tactics_core::ai::best_weapon_against(
            &damage_only,
            &state,
            battery,
            from,
            state.unit(quarry).expect("on the field"),
            hull,
        )
        .is_none(),
        "so the battery is not armed against her at all, and stops firing"
    );
}

#[test]
fn a_bounce_that_achieves_nothing_does_not_hold_the_battle_open() {
    // The chain reaction behind the same barrage, and the reason one AI
    // mispricing cost two things rather than one. A bounce used to reset the
    // stalemate clock on the reading that the guns were still trying — so
    // shells that could not hurt anybody kept a decided battle breathing for
    // eight more rounds of wandering.
    //
    // Trying is not progress. What holds a battle open now is a gun
    // *accomplishing* something, and a bounce that accomplishes something
    // says so in its own event: `ModuleHit` and `CrewHit` are both still on
    // the list, and overpressure raises them from outside the plate. So the
    // livelock this clock was written against — two crews neither can kill,
    // staring at each other forever — is still shut out.
    let mut reg = registry_wireless();
    // Guns that strike and never get through, on hulls where blast has
    // nothing to break: every shot from here to the bell is a bare bounce.
    for weapon in reg.weapons.values_mut() {
        weapon.penetration = 0;
        weapon.ammo.clear();
    }
    for module in reg.modules.values_mut() {
        module.size = 0;
    }
    let mut state = duel(&reg, 813);
    let (west, east) = (UnitId(0), UnitId(1));
    for (shooter, target) in [(west, east), (east, west)] {
        state
            .apply(
                &reg,
                &Order::SetFire {
                    unit: shooter,
                    fire: FireIntent::Target { target, weapon: 0 },
                },
            )
            .unwrap();
    }

    let mut bounces = 0;
    for _ in 0..reg.balance.stalemate_rounds * 3 {
        if state.is_over() {
            break;
        }
        for (_, event) in ticked_round(&reg, &mut state) {
            match event {
                BattleEvent::ShotBounced { .. } => bounces += 1,
                BattleEvent::CrewHit { .. } | BattleEvent::ModuleHit { .. } => {
                    panic!("this scene has to be bounces and nothing else")
                }
                _ => {}
            }
        }
        for (shooter, target) in [(west, east), (east, west)] {
            let _ = state.apply(
                &reg,
                &Order::SetFire {
                    unit: shooter,
                    fire: FireIntent::Target { target, weapon: 0 },
                },
            );
        }
    }

    assert!(
        bounces > 0,
        "the guns really are firing and really are bouncing"
    );
    assert!(
        state.is_over(),
        "and the battle ends anyway: nothing either crew did changed anything"
    );
    assert_eq!(
        state.over.map(|r| r.reason),
        Some(EndReason::Stalemate),
        "by the clock rather than by anybody winning it"
    );
}

#[test]
fn a_mod_without_ammunition_keeps_instant_artillery() {
    // Additivity, read as strictly as the gate reads it. Flight time is a
    // rule, but it is a rule about *rounds*, and a mod that declines to
    // describe its ammunition has no rounds — only guns with numbers on them.
    // That mod must get the game it shipped with, in which a howitzer
    // resolves in the tick it fires, so nothing here ever goes up in the air.
    let mut reg = registry_wireless();
    for weapon in reg.weapons.values_mut() {
        weapon.ammo.clear();
    }
    let mut state = battery_stage(&reg, "recon_car", 504);
    let (battery, quarry) = (UnitId(0), UnitId(1));
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: battery,
                fire: FireIntent::Target {
                    target: quarry,
                    weapon: 0,
                },
            },
        )
        .unwrap();

    let log = ticked_round(&reg, &mut state);
    assert!(
        !log.iter()
            .any(|(_, e)| matches!(e, BattleEvent::ShellLanded { .. })),
        "a legacy howitzer puts nothing in the air"
    );
    assert!(
        state.shells.is_empty(),
        "and leaves nothing behind it either"
    );
    let fired = log
        .iter()
        .find_map(|(tick, e)| match e {
            BattleEvent::ShotFired { attacker, .. } if *attacker == battery => Some(*tick),
            _ => None,
        })
        .expect("she still fires");
    assert!(
        log.iter().any(|(tick, e)| *tick == fired
            && matches!(
                e,
                BattleEvent::ShotHit { .. }
                    | BattleEvent::ShotBounced { .. }
                    | BattleEvent::ShotMissed { .. }
            )),
        "and the shot is over in the tick that fired it, exactly as it always was"
    );
}

// --- what an order promises ------------------------------------------------

/// Every order the player can give says what it commits her to, and no two
/// of them say the same thing.
///
/// The complaint this defends against is not hypothetical: `Advance` and
/// `Assault` move a platoon toward the same hex and score identically, and
/// the *only* difference between them is one the player could not read
/// anywhere in the game. A promise that came back empty, or that read the
/// same for both, would put the game straight back where it was — so this
/// asserts the property rather than the wording.
#[test]
fn every_order_says_what_it_commits_the_platoon_to() {
    let to = tactics_core::Hex::new(1, 1);
    let all = [
        Mission::Advance { to },
        Mission::Assault { to },
        Mission::Hold { at: Some(to) },
        Mission::Recon { toward: to },
        Mission::Withdraw {
            via: "east_road".into(),
        },
        Mission::Support {
            formation: "second".into(),
        },
    ];
    let mut seen: Vec<&str> = Vec::new();
    for mission in &all {
        let promise = mission.promise();
        assert!(
            !promise.is_empty(),
            "{:?} promises nothing at all",
            mission.verb()
        );
        assert!(
            !seen.contains(&promise),
            "two orders make the same promise: {promise}"
        );
        seen.push(promise);
    }
    // The pair the whole step exists for, named explicitly: an advance stops
    // for a fight and an assault does not, and a player choosing between the
    // keys must be able to see that before she presses one.
    assert_ne!(
        Mission::Advance { to }.promise(),
        Mission::Assault { to }.promise(),
        "the two orders that differ only under fire must not read alike"
    );
    // Every verb the menu can list is in the shared table, which is what
    // stops a UI from inventing a promise the rules never made.
    for mission in &all {
        assert!(
            Mission::vocabulary()
                .iter()
                .any(|(verb, promise)| *verb == mission.verb() && *promise == mission.promise()),
            "{} is missing from the shared vocabulary",
            mission.verb()
        );
    }
}

/// The per-unit twin says the same kind of thing, because it is the same
/// decision at a different scale: an ordinary march may break off for cover
/// and a binding one may not, and both of those are promises.
#[test]
fn insisting_on_a_march_promises_something_an_ordinary_one_does_not() {
    assert_ne!(
        Latitude::Delegated.promise(),
        Latitude::Binding.promise(),
        "the whole value of insisting is that it means something different"
    );
    assert!(!Latitude::Delegated.promise().is_empty());
    assert!(!Latitude::Binding.promise().is_empty());
}
