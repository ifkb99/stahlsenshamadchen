//! The one currency: what the ground can put on her, converted to the same
//! units as a round of fire.
//!
//! What the ground can put on her is the threat term the evaluator reads;
//! suppression and cadence join the currency once a round's price is
//! rounds-per-minute and fear rather than a flat damage number; one walk,
//! one gate closes the loop by pinning `ai::threats` and `ai::threatened`
//! to the same `fire_on` the danger overlay shows a player. Three sections,
//! one exchange rate.
//!
//! - what the ground can put on her
//! - suppression and cadence join the currency
//! - one walk, one gate: the readers of the currency

use tactics_core::ai::Evaluator;
use tactics_core::battle::{
    BattleState, Event as BattleEvent, FireIntent, Latitude, Order, UnitId, reachable,
};
use tactics_core::data::{DataRegistry, ShotFelt};

mod common;
use common::{
    MARCH_TO, always, breaking, commit_all, executor_only_side, felt, marching_under_fire,
    play_round, registry, registry_wireless, seen, soften, two_side_battle, unit_at,
};

// --- what the ground can put on her ----------------------------------------

/// Two crews at the west end of a strip, two of the other side in the open
/// three hexes off, and a belt of wood at six with more open ground behind
/// it.
///
/// The wood is two columns thick and spans every row on purpose. A
/// single-column curtain is something a diagonal ray gets round, and the
/// question these tests ask — *could anything at all reach her there* — is
/// answered by the whole enemy side rather than by whichever gunner happens
/// to be directly opposite.
fn firing_positions(reg: &DataRegistry, seed: u64) -> BattleState {
    let row = "ggggggffgg";
    two_side_battle(
        reg,
        &[row, row, row],
        vec![
            unit_at([0, 0], 0, "medium_tank", "Gunner"),
            unit_at([0, 2], 0, "tank_destroyer", "Overwatch"),
            unit_at([3, 1], 1, "medium_tank", "Mark"),
            unit_at([3, 2], 1, "rifle_platoon", "Section"),
        ],
        seed,
    )
}

/// The preview asked about the ground she is already standing on is the shot
/// the resolver would take, for every pair on the field.
///
/// This is the pin that makes the symmetric parameter a rearrangement rather
/// than a rule change: `fire_on(unit, unit.pos)` may not be a *model* of the
/// danger a crew is in, it has to be the resolver's own arithmetic with the
/// real hex passed in. Checked against [`expected_damage`] and [`hit_chance`]
/// directly, and against `best_weapon_against` for who is on the list at all,
/// because a roster that quietly dropped an enemy would agree beautifully
/// with itself on the ones it kept.
#[test]
fn a_bearing_taken_at_her_own_hex_is_the_shot_the_resolver_would_take() {
    let reg = seen(registry());
    let state = firing_positions(&reg, 11);
    assert_eq!(
        (
            state.fog.side(0).spotted.len(),
            state.fog.side(1).spotted.len()
        ),
        (2, 2),
        "the stage is four crews in plain sight of each other"
    );

    let mut pairs = 0;
    for me in state.units.iter() {
        let bearings = tactics_core::battle::fire_on(&reg, &state, me.id, me.pos);

        // Who is on the list, and in what order. `known_enemies` walks the
        // units in id order, so this pins the ordering contract at the same
        // time as the membership one.
        let listed: Vec<UnitId> = bearings.iter().map(|b| b.enemy).collect();
        let armed: Vec<UnitId> = state
            .known_enemies(&reg, tactics_core::battle::Knower::Crew(me.id))
            .iter()
            .filter(|e| {
                tactics_core::ai::best_weapon_against(&reg, &state, e.id, e.pos, me, me.pos)
                    .is_some()
            })
            .map(|e| e.id)
            .collect();
        assert_eq!(
            listed, armed,
            "{} should be told about exactly the enemies who are armed against her",
            me.name
        );

        for bearing in &bearings {
            let enemy = state.unit(bearing.enemy).expect("on the field");
            let weapon = reg
                .vehicle(&enemy.vehicle)
                .and_then(|v| v.weapons.get(bearing.weapon))
                .and_then(|w| reg.weapon(w))
                .expect("the bearing names a gun she actually has");
            assert_eq!(
                bearing.expected,
                tactics_core::battle::expected_damage(
                    &reg, &state, enemy.id, enemy.pos, weapon, me.id, me.pos, false,
                ),
                "{} -> {} must be priced by the resolver and nothing else",
                enemy.name,
                me.name
            );
            assert_eq!(
                bearing.hit_percent,
                tactics_core::battle::hit_chance(
                    &reg, &state, enemy.id, enemy.pos, weapon, me.id, me.pos, false,
                ),
                "{} -> {} must be rolled by the resolver and nothing else",
                enemy.name,
                me.name
            );
            pairs += 1;
        }
    }
    assert!(
        pairs >= 4,
        "the stage should produce several pairs, got {pairs}"
    );
}

/// Where she would stand is what decides what can be put on her.
///
/// The whole point of the symmetric parameter: before it, every one of these
/// three hexes priced identically, because the arithmetic resolved against
/// the hex she was already on and the tile under discussion reached it only
/// as a distance. Now the wood costs the gunner his cover term and the far
/// side of the belt costs him the shot outright.
///
/// The open hex is one nearer than the wood, so range and cover push the same
/// way here; this test claims only that the ground reaches the arithmetic at
/// all, and `gunnery.rs` is where each term is isolated.
#[test]
fn where_she_would_stand_decides_what_can_be_put_on_her() {
    let reg = seen(registry());
    let state = firing_positions(&reg, 11);
    let mark = UnitId(2);
    let gunner = UnitId(0);

    let open = tactics_core::offset_to_hex(5, 1);
    let wood = tactics_core::offset_to_hex(6, 1);
    let behind = tactics_core::offset_to_hex(9, 1);
    assert_eq!(
        state.map.get(wood).map(|t| t.terrain.as_str()),
        Some("forest"),
        "the middle hex has to be the wood or this test is about nothing"
    );

    let from_the_gunner = |at| {
        tactics_core::battle::fire_on(&reg, &state, mark, at)
            .into_iter()
            .find(|b| b.enemy == gunner)
    };
    let in_the_open = from_the_gunner(open).expect("open ground is a clear shot");
    let in_the_wood = from_the_gunner(wood).expect("the near edge of the wood is still visible");

    assert!(
        in_the_wood.hit_percent < in_the_open.hit_percent,
        "standing in the timber has to be harder to hit: {} in the wood \
         against {} in the open",
        in_the_wood.hit_percent,
        in_the_open.hit_percent
    );
    assert!(
        in_the_wood.expected < in_the_open.expected,
        "and worth less to shoot at: {} against {}",
        in_the_wood.expected,
        in_the_open.expected
    );
    assert!(
        tactics_core::battle::fire_on(&reg, &state, mark, behind).is_empty(),
        "and the far side of the belt is out of everybody's sight, which is \
         not a smaller number but no shot at all"
    );
}

/// A crew is never told about a gun her side has not found.
///
/// The same fog rule the order system lives under: an answer that flinched
/// away from an unspotted tank would announce that the tank is there. Staged
/// so that the shot provably exists — `best_weapon_from` says the gunner is
/// armed against her, at that range, with line of sight — and only the
/// knowledge is missing.
#[test]
fn a_bearing_is_never_taken_from_an_enemy_nobody_has_found() {
    let mut reg = registry();
    // No near band and no base chance: a look never becomes an acquisition,
    // which is this rule's absence rather than a gentle version of it.
    reg.balance.detection_base = 0;
    reg.balance.detection_certain_percent = 0;
    let state = firing_positions(&reg, 11);
    let (mark, gunner) = (UnitId(2), UnitId(0));

    assert!(
        state.fog.side(1).spotted.is_empty(),
        "the stage is a side that has found nobody"
    );
    let mark_pos = state.unit(mark).expect("on the field").pos;
    let gunner_pos = state.unit(gunner).expect("on the field").pos;
    assert!(
        tactics_core::battle::best_weapon_from(&reg, &state, gunner, gunner_pos, mark, mark_pos)
            .is_some(),
        "the shot itself exists: she is in range, in the open, and in his sights"
    );

    assert!(
        tactics_core::battle::fire_on(&reg, &state, mark, mark_pos).is_empty(),
        "but her side has not found him, so nothing may be said about his gun"
    );
}

/// A gun in the middle of a five-row field, a belt of wood masking the
/// northern half of it, and a scout who can see the gun and cannot touch it.
///
/// Everything about the shape is there to leave one term standing. The two
/// tiles the tests below compare — `(10, 0)` behind the wood and `(10, 4)` in
/// the open — are **the same distance from the gun**, so the closing term
/// (`-(nearest enemy) * aggression`) cannot separate them; they are the same
/// terrain, so the ground prior cannot; the scout is alone on her side, so
/// there is no spacing term; and the map names no objectives, so there is no
/// gradient to anywhere.
///
/// A recon car against a tank destroyer, and both halves of that matter. Her
/// only weapon is a machine gun with six hexes of reach, so from either tile
/// she has no shot at all and the offense term is zero on both — a crew who
/// *could* shoot from the open tile would be paid for standing there, which
/// is a real thing for the evaluator to weigh and would make this test about
/// two terms rather than one. She sees twenty hexes, which is what lets her
/// side hold a contact on a gun eleven hexes off; the gun's eighty-eight
/// reaches sixteen, so it covers both tiles and the wood is the only reason
/// one of them is safe.
fn masked_and_open(reg: &DataRegistry, seed: u64) -> BattleState {
    let wood = format!("{}ff{}", "g".repeat(4), "g".repeat(10));
    let open = "g".repeat(16);
    two_side_battle(
        reg,
        &[&wood, &wood, &open, &open, &open],
        vec![
            unit_at([2, 2], 0, "recon_car", "Scout"),
            unit_at([0, 2], 1, "tank_destroyer", "Gun"),
        ],
        seed,
    )
}

/// The ground under a spotted gun is worth less to stand on than ground the
/// same distance away that the gun cannot see.
///
/// This is Phase 2b as one assertion. Before it the threat term priced
/// `best_weapon_against(gun, gun.pos, her, her.pos)` — the shot at the hex
/// she was *already* on — and let the candidate tile in only through a
/// `1/distance` falloff behind a six-hex gate, so two tiles equidistant from
/// the gun were worth *exactly* the same whatever stood between them. Line of
/// sight, cover, elevation, facing and obliquity are all on the near side of
/// that arithmetic and none of them reached the decision about where to
/// drive.
///
/// Mutation-checked by pricing the threat at `me.pos` instead of `tile`,
/// which is the term as it stood: the two tiles then score identically and
/// the strict comparison below fails.
#[test]
fn a_crew_would_rather_stand_where_the_gun_cannot_see_her() {
    let reg = seen(registry());
    let state = masked_and_open(&reg, 19);
    let scout = UnitId(0);
    let masked = tactics_core::offset_to_hex(10, 0);
    let exposed = tactics_core::offset_to_hex(10, 4);
    let gun = state.unit(UnitId(1)).expect("on the field").pos;

    assert!(
        state.fog.side(0).spotted.contains(&UnitId(1)),
        "the stage is a gun her side has found; an unfound one may not be \
         flinched away from at all"
    );
    assert_eq!(
        (gun.distance_to(masked), gun.distance_to(exposed)),
        (11, 11),
        "the two tiles have to be the same distance from the gun, or this is \
         a test about range"
    );
    assert_eq!(
        (
            state.map.get(masked).map(|t| t.terrain.as_str()),
            state.map.get(exposed).map(|t| t.terrain.as_str())
        ),
        (Some("grass"), Some("grass")),
        "and the same ground, or it is a test about cover"
    );

    // `.worth` is what the evaluator's threat term spends, so it is what a
    // test about that term should read.
    let incoming = |at| tactics_core::battle::incoming(&reg, &state, scout, at).worth;
    assert_eq!(
        incoming(masked),
        0.0,
        "behind the wood the gun has no shot to take"
    );
    assert!(
        incoming(exposed) > 0.0,
        "and in the open it has one: {}",
        incoming(exposed)
    );

    let eval = Evaluator::new(tactics_core::data::DoctrineDef::default());
    let score = |at| eval.score_tile(&reg, &state, scout, at).score;
    assert!(
        score(masked) > score(exposed),
        "the masked tile has to be worth more to stand on: {} against {}",
        score(masked),
        score(exposed)
    );
}

/// A gun, a wood and a bare field the same distance from it, on either side
/// of its own row.
///
/// The stage for the ground prior. `(9, 0)` is timber and `(9, 2)` is grass,
/// both nine hexes from the gun at `(0, 1)` and both in its plain sight, so
/// the only thing that separates them is what cover is worth — which is
/// exactly the question 2c had to answer twice, once with the gun found and
/// once without.
fn wood_and_field(reg: &DataRegistry, seed: u64) -> BattleState {
    let north = format!("{}f{}", "g".repeat(9), "g".repeat(2));
    let plain = "g".repeat(12);
    two_side_battle(
        reg,
        &[&north, &plain, &plain],
        vec![
            unit_at([2, 1], 0, "recon_car", "Scout"),
            unit_at([0, 1], 1, "tank_destroyer", "Gun"),
        ],
        seed,
    )
}

/// A doctrine with no appetite for ground beyond the two fields under test.
///
/// `scouting` and `aggression` are zeroed so that `score_tile`'s two roaming
/// terms — walk toward the middle of an empty map, close on whoever is
/// visible — cannot separate two tiles that are not the same distance from
/// the centre or from the enemy. What is left of the sum on these stages is
/// the ground prior minus the threat, which is the pair of terms 2b and 2c
/// are about. Everything else is zero by construction: the scout has no
/// weapon that reaches, no friend to space herself against, no objective to
/// walk to and no orders.
fn taste_only() -> tactics_core::data::DoctrineDef {
    tactics_core::data::DoctrineDef {
        scouting: 0.0,
        aggression: 0.0,
        ..Default::default()
    }
}

/// With nobody found, the arithmetic has nothing to say and a doctrine's
/// taste for cover decides the ground.
///
/// The other half of 2c, and the reason the terrain term did not simply go
/// away when the threat term learned to read cover for itself. `incoming` is
/// a statement about the enemies this side has *found*; on an approach march
/// — first contact falls in round 3.5 of a 13-round battle — it is exactly
/// zero, and a crew with an empty list needs some reason to prefer a wood to
/// a field. That reason is `cover_value`, which is read here and nowhere else
/// in the engine, scaled by `planner.cover_prior`.
///
/// Mutation-checked from both ends, because the term has two halves to lose:
/// a doctrine with no opinion about cover and a mod that prices the prior at
/// nothing must both leave the two tiles exactly level.
#[test]
fn with_nobody_found_a_doctrines_taste_for_cover_decides_the_ground() {
    let mut reg = registry();
    // No near band and no base chance: a look never becomes an acquisition,
    // so this is the rule's absence rather than a gentle version of it.
    reg.balance.detection_base = 0;
    reg.balance.detection_certain_percent = 0;
    let state = wood_and_field(&reg, 19);
    let scout = UnitId(0);
    let wood = tactics_core::offset_to_hex(9, 0);
    let field = tactics_core::offset_to_hex(9, 2);

    assert!(
        state.fog.side(0).spotted.is_empty(),
        "the stage is a side that has found nobody"
    );
    assert_eq!(
        (
            state.map.get(wood).map(|t| t.terrain.as_str()),
            state.map.get(field).map(|t| t.terrain.as_str())
        ),
        (Some("forest"), Some("grass")),
        "one tile has to be the timber and the other the open ground"
    );
    for at in [wood, field] {
        assert_eq!(
            tactics_core::battle::incoming(&reg, &state, scout, at).worth,
            0.0,
            "and nothing may be said about a gun nobody has found"
        );
    }

    let gap = |reg: &DataRegistry, doctrine: tactics_core::data::DoctrineDef| {
        let eval = Evaluator::new(doctrine);
        eval.score_tile(reg, &state, scout, wood).score
            - eval.score_tile(reg, &state, scout, field).score
    };
    let balanced = gap(&reg, taste_only());
    assert!(
        balanced > 0.0,
        "with no arithmetic to go on she takes the timber, by {balanced}"
    );

    let mut keen = taste_only();
    keen.cover_value *= 3.0;
    let keener = gap(&reg, keen);
    assert!(
        keener > balanced,
        "a doctrine that likes cover three times as much wants it more: \
         {keener} against {balanced}"
    );

    let mut indifferent = taste_only();
    indifferent.cover_value = 0.0;
    assert_eq!(
        gap(&reg, indifferent),
        0.0,
        "a doctrine with no opinion about cover has none, and this term is \
         the only thing in the engine that reads the field"
    );

    let mut plain = reg.clone();
    plain.planner.cover_prior = 0.0;
    assert_eq!(
        gap(&plain, taste_only()),
        0.0,
        "and so does a mod that prices the prior at nothing"
    );
}

/// The prior stands down where the resolver has already answered.
///
/// The rule 2c settled on, and what keeps cover from being paid for twice:
/// cover is *already* in the threat term, per gun and per bearing, through
/// `balance.cover_against_accuracy`. So the flat bonus is paid on ground no
/// found gun can reach and withheld on ground one can. Same two tiles as the
/// test above, same doctrine; the only difference is whether the gun has been
/// spotted.
///
/// Pinned as an equality rather than as an inequality, which is what makes it
/// a check on the gate itself: with the gun found, `planner.cover_prior` at
/// its shipped value and at zero must give **the same number**, because the
/// term is not being consulted at all. Remove the gate and the two differ by
/// the bonus, which is the mutation.
///
/// Note what this is not. It is not "cover stops mattering under fire" — the
/// threat term says the timber is safer and says so in substance points, and
/// says it far louder than the bonus ever did: 3.5 points of avoided fire
/// against the 0.9 the flat term was paying. That gap is the whole argument
/// for Phase 2 in two numbers, and it is why the bonus can stand down without
/// a crew forgetting what a wood is for.
#[test]
fn the_ground_prior_stands_down_where_the_arithmetic_speaks() {
    let mut hidden = registry();
    hidden.balance.detection_base = 0;
    hidden.balance.detection_certain_percent = 0;
    let found = seen(registry());
    let scout = UnitId(0);
    let wood = tactics_core::offset_to_hex(9, 0);
    let field = tactics_core::offset_to_hex(9, 2);

    let read = |reg: &DataRegistry| {
        let state = wood_and_field(reg, 19);
        let eval = Evaluator::new(taste_only());
        let gap = eval.score_tile(reg, &state, scout, wood).score
            - eval.score_tile(reg, &state, scout, field).score;
        let incoming = |at| tactics_core::battle::incoming(reg, &state, scout, at).worth;
        (gap, incoming(wood), incoming(field))
    };
    let without_the_prior = |reg: &DataRegistry| {
        let mut plain = reg.clone();
        plain.planner.cover_prior = 0.0;
        read(&plain).0
    };

    let (unseen_gap, no_wood, no_field) = read(&hidden);
    assert_eq!(
        (no_wood, no_field),
        (0.0, 0.0),
        "with the gun unfound there is no arithmetic about either tile"
    );
    assert!(
        unseen_gap > without_the_prior(&hidden),
        "so the prior speaks: {unseen_gap} against {} with it priced at \
         nothing",
        without_the_prior(&hidden)
    );

    let (found_gap, hit_wood, hit_field) = read(&found);
    assert!(
        hit_wood > 0.0 && hit_field > 0.0,
        "with the gun found it covers both tiles: {hit_wood} and {hit_field}"
    );
    assert!(
        hit_wood < hit_field,
        "and the resolver already knows the timber is the safer of the two: \
         {hit_wood} against {hit_field}"
    );
    assert_eq!(
        found_gap,
        without_the_prior(&found),
        "so under the gun the bonus is not consulted at all, and pricing it \
         at nothing changes nothing"
    );
    assert!(
        found_gap > unseen_gap,
        "which costs a crew nothing, because what the arithmetic pays for the \
         same timber is larger than what the opinion did: {found_gap} against \
         {unseen_gap}"
    );
}

/// Breaking off is announced, and insisting stops it happening.
///
/// The autonomy line DIRECTION.md draws, as a test rather than as a
/// paragraph: *friction the player can predict and price is drama; friction
/// she cannot see is a bug report.* Phase 2 made the threat term larger and
/// sharper — a crew now prices the ground she is being sent across in the
/// resolver's own arithmetic, so an ordinary order gets set aside more
/// readily than it used to — and the two things that keep that legible rather
/// than infuriating are already built: the deviation raises `Decision::drill`
/// so the log can say *"Anka Weiss breaks off her march and takes cover"*,
/// and `Latitude::Binding` is the player's answer to it.
///
/// The same stage, seed and gun as
/// `a_binding_march_presses_on_where_an_ordinary_one_takes_cover`, which
/// pins the *behaviour*; this pins that the behaviour is visible and
/// overridable. Both halves matter and neither implies the other: an
/// unannounced break-off reads as the game malfunctioning, and an announced
/// one the player cannot countermand reads as the game arguing with her.
#[test]
fn a_crew_who_breaks_off_says_so_and_one_who_was_meant_does_not() {
    let reg = seen(registry_wireless());

    // The march she was given, fought for a round so that there is a march
    // under way for the drill to preempt: `radio()` plans her herself on the
    // round the order lands, so nothing can deviate until the second one.
    // The seed is hunted here rather than written down. It used to be a
    // `const 4`, and the crew it was chosen to bruise was killed outright the
    // moment loaders started reaching their guns — which is what a hand-picked
    // seed always does, eventually, to whoever is next. Anything that leaves
    // her alive under fire will do.
    let survivable = |seed: u64| {
        let (mut state, crew) = marching_under_fire(&reg, Latitude::Delegated, seed);
        executor_only_side(&reg, seed).plan_round(&reg, &mut state);
        let _ = state.apply(&reg, &Order::Commit { side: 1 });
        state.resolve_round(&reg);
        state.unit(crew).is_some_and(|u| u.alive())
    };
    let seed = (0u64..40)
        .find(|seed| survivable(*seed))
        .expect("some seed leaves her alive to use her judgment");

    let deviations = |latitude: Latitude| {
        let (mut state, crew) = marching_under_fire(&reg, latitude, seed);
        executor_only_side(&reg, seed).plan_round(&reg, &mut state);
        let _ = state.apply(&reg, &Order::Commit { side: 1 });
        state.resolve_round(&reg);
        assert!(
            state.unit(crew).is_some_and(|u| u.alive()),
            "the stage is meant to bruise, not to kill"
        );
        let mut hers = Vec::new();
        executor_only_side(&reg, seed).plan_round_with(&reg, &mut state, |decision| {
            let about = match &decision.order {
                Order::SetMove { unit, .. } => Some(*unit),
                _ => None,
            };
            if about == Some(crew) {
                hers.push(decision.drill);
            }
        });
        (hers, state.unit(crew).unwrap().planned_destination())
    };

    let (delegated, delegated_to) = deviations(Latitude::Delegated);
    assert!(
        delegated.iter().any(|drill| *drill),
        "the crew who was given her judgment used it, and the order that did \
         it has to carry the flag that lets the log say so: {delegated:?}, \
         heading for {delegated_to:?}"
    );

    let (binding, binding_to) = deviations(Latitude::Binding);
    assert!(
        binding.iter().all(|drill| !*drill),
        "and the crew who was told her commander meant it never reaches the \
         drill at all, so there is nothing to announce: {binding:?}, heading \
         for {binding_to:?}"
    );
    assert!(
        MARCH_TO.distance_to(binding_to) < MARCH_TO.distance_to(delegated_to),
        "which is the same comparison the behaviour test makes, restated so \
         that a stage that stopped deviating could not pass this quietly: \
         binding -> {binding_to:?}, delegated -> {delegated_to:?}"
    );
}

// --- suppression and cadence join the currency ------------------------------

//
// Two facts the resolver already knew and the pricing never read. **Pressure**:
// fire that cannot beat a plate expects zero damage, so a machine gun looking
// at a heavy tank was invisible to every chooser including the shooter's — the
// designer's `threatened` note in DIRECTION.md, and the reason nobody in this
// game had ever fired a belt at armour. **Cadence**: every term was per shot,
// so an MG at six shots a round and an 88 at three read alike.
//
// The additivity contract these tests are here to keep: `ammo.suppression`
// defaults to 0 and `morale.point_worth` to 0.0, and at those values every
// number in the game is the number it was.

/// A heavy tank at four hexes with a machine gun looking at her glacis, and
/// nothing else on the field.
///
/// The machine gun is the whole point: `ball_mg` penetrates 1 against front
/// armour 8, so the damage half of every price is *exactly* zero and anything
/// the arithmetic says about this pairing is the pressure half saying it.
///
/// The heavy tank's racks are emptied, which is the stage rather than a
/// dodge. An 88 at four hexes ends a recon car in one shot, and a test that
/// wants to watch a belt play against plate for forty rounds cannot also be a
/// test of how long the car survives. A gun with an ammunition list and
/// nothing on it is silent by the engine's own documented rule, so this needs
/// no special case anywhere.
fn a_belt_at_a_glacis(reg: &DataRegistry, seed: u64) -> BattleState {
    let mut state = two_side_battle(
        reg,
        &["gggggggggg", "gggggggggg", "gggggggggg"],
        vec![
            unit_at([0, 1], 0, "heavy_tank", "Plate"),
            unit_at([4, 1], 1, "recon_car", "Belt"),
        ],
        seed,
    );
    if let Some(plate) = state.unit_mut(UnitId(0)) {
        for aboard in plate.ammo.values_mut() {
            *aboard = 0;
        }
    }
    state
}

/// The one gun the recon car has, for the tests that need to name it.
fn her_machine_gun(reg: &DataRegistry) -> &tactics_core::data::WeaponDef {
    reg.weapon("mg").expect("the base mod ships a machine gun")
}

/// A burst that cannot get through still counts for what it does to her
/// nerve.
///
/// The designer's sentence, made arithmetic: *even if your IFV is immune to
/// 50 cal from the front, getting hit by it is not a fun time.* Before this
/// the machine gun expected zero against a glacis and was therefore not a
/// weapon against her at all — `best_weapon_from`'s third gate threw it out,
/// so she did not appear on the danger list, the crew never fired, and no
/// planner ever weighed the ground the gun covered.
///
/// Both directions are asserted, because the interesting claim is the
/// additive one: at `point_worth: 0` the burst is worth nothing to anybody
/// weighing it, which is the game exactly as it was, and the crew still feels
/// it.
#[test]
fn a_burst_that_cannot_get_through_still_counts_for_what_it_does_to_her_nerve() {
    let mut reg = seen(registry());
    // A belt that says something about what it is like to be under it. The
    // number is the stage's, not the base mod's: this test is about the
    // mechanism, and the shipped value is chosen by sweep elsewhere.
    reg.ammo
        .get_mut("ball_mg")
        .expect("the base mod ships a belt")
        .suppression = 2;
    let state = a_belt_at_a_glacis(&reg, 31);
    let (plate, belt) = (UnitId(0), UnitId(1));
    let (plate_pos, belt_pos) = (
        state.unit(plate).expect("staged").pos,
        state.unit(belt).expect("staged").pos,
    );

    // The stage's own premise: no damage whatsoever gets through, so
    // everything below is the pressure half and cannot be the damage half
    // wearing a disguise.
    let damage = tactics_core::battle::expected_damage(
        &reg,
        &state,
        belt,
        belt_pos,
        her_machine_gun(&reg),
        plate,
        plate_pos,
        false,
    );
    assert_eq!(
        damage, 0.0,
        "a belt against front-8 armour must expect nothing, or this test is \
         measuring the wrong half"
    );
    let pressure = tactics_core::battle::expected_pressure(
        &reg,
        &state,
        belt,
        belt_pos,
        her_machine_gun(&reg),
        plate,
        plate_pos,
        false,
    );
    assert!(
        pressure > 0.0,
        "and it must expect some pressure, or there is nothing here to price: \
         {pressure}"
    );

    // Fear priced at nothing is the game before this existed, down to the
    // gate: no bearing at all, because a gun worth zero is not a weapon
    // against her.
    reg.morale.point_worth = 0.0;
    assert!(
        tactics_core::battle::fire_on(&reg, &state, plate, plate_pos).is_empty(),
        "with fear worth nothing the burst is not a threat, which is exactly \
         what every planner believed before this chunk"
    );

    // Fear priced at something, and she is on the list with a worth that is
    // all pressure.
    reg.morale.point_worth = 1.0;
    let bearings = tactics_core::battle::fire_on(&reg, &state, plate, plate_pos);
    assert_eq!(
        bearings.len(),
        1,
        "with fear worth something the burst is a threat: {bearings:?}"
    );
    let burst = bearings[0];
    assert_eq!(burst.expected, 0.0, "and still no damage at all");
    assert!(burst.worth > 0.0, "and a worth made entirely of pressure");
    assert_eq!(
        burst.worth,
        burst.expected + burst.pressure * reg.morale.point_worth,
        "worth is the two currencies at the mod's exchange rate and nothing else"
    );
}

/// Fear is priced by the same arithmetic that charges it.
///
/// `MoraleRules::pressure_for` is the one price list, and this is the check
/// that the analytic twin and the resolver read it the same way — the damage
/// side's "the preview is the resolver" property, restated for pressure. The
/// method is the one the hit-chance tests use: fire a great many shots at a
/// staged pairing, average what the ladder actually charged, and require it to
/// land on what `expected_pressure` promised.
///
/// The margin is a fifth either way, because the sample is a few hundred shots
/// of a Bernoulli mixture; what it is pinning is that the two are the *same*
/// arithmetic, and a second copy of the price list would be out by whole
/// ladder points rather than by a rounding. Measured on this stage the ratio
/// lands within a couple of percent of one.
#[test]
fn fear_is_priced_by_the_same_arithmetic_that_charges_it() {
    let mut reg = seen(registry());
    reg.ammo
        .get_mut("ball_mg")
        .expect("the base mod ships a belt")
        .suppression = 3;
    // A belt against a glacis accomplishes nothing by design, and the
    // stalemate clock counts accomplishment — so the battle this test needs
    // is precisely the one the clock exists to stop. Held off for long
    // enough to take a sample; the rule is untouched.
    reg.balance.stalemate_rounds = 1_000;
    let state = a_belt_at_a_glacis(&reg, 77);
    let (plate, belt) = (UnitId(0), UnitId(1));
    let (plate_pos, belt_pos) = (
        state.unit(plate).expect("staged").pos,
        state.unit(belt).expect("staged").pos,
    );
    let promised = tactics_core::battle::expected_pressure(
        &reg,
        &state,
        belt,
        belt_pos,
        her_machine_gun(&reg),
        plate,
        plate_pos,
        false,
    );
    assert!(promised > 0.0, "the stage has to expect something");
    let aboard = state
        .unit(belt)
        .and_then(|u| u.ammo.get("ball_mg").copied())
        .expect("she came with a belt");

    // Fire the belt over and over and read what the ladder charged. Pressure
    // is shed between rounds, so the count is taken off the events rather
    // than off her `pressure` field — the question is what was *charged*.
    let mut state = state.clone();
    let order = Order::SetFire {
        unit: belt,
        fire: FireIntent::Target {
            target: plate,
            weapon: 0,
        },
    };
    // Whole points, rounded the way the ledger rounds them, because what is
    // being averaged is what the crew was actually charged.
    let mut charged = 0.0f32;
    for _ in 0..40 {
        state.apply(&reg, &order).expect("she is ordered to shoot");
        for event in play_round(&reg, &mut state) {
            match &event {
                BattleEvent::ShotHit {
                    target,
                    ammo,
                    damage,
                    budget,
                    ..
                } if *target == plate => {
                    let spent = tactics_core::battle::spent_share(*damage, *budget);
                    charged += reg
                        .morale
                        .pressure_for(ShotFelt::Penetrated { spent }, felt(&reg, ammo, false))
                        .round();
                }
                BattleEvent::ShotBounced {
                    target,
                    ammo,
                    rattled,
                    ..
                } if *target == plate => {
                    charged += reg
                        .morale
                        .pressure_for(ShotFelt::Bounced, felt(&reg, ammo, !rattled))
                        .round();
                }
                _ => {}
            }
        }
    }

    // `expected_pressure` is per shot *fired*, hit chance included, so the
    // comparison is charged-per-shot-fired. Counting only the arrivals would
    // compare a conditional mean against an unconditional one and be wrong by
    // exactly the hit chance.
    let fired = aboard
        - state
            .unit(belt)
            .and_then(|u| u.ammo.get("ball_mg").copied())
            .expect("she is still on the field");
    assert!(
        fired >= 100,
        "the sample has to be big enough to average, got {fired} shots"
    );
    let mean = charged / fired as f32;
    let ratio = mean / promised;
    assert!(
        (0.8..=1.2).contains(&ratio),
        "the ladder charged {mean:.3} a shot over {fired} shots and the \
         arithmetic promised {promised:.3} — a factor of {ratio:.3}, which is \
         two price lists rather than one"
    );
}

/// A shell that only scrapes through is priced by the same arithmetic too.
///
/// The belt-at-a-glacis twin above never penetrates, so it proves the two
/// price lists agree on the bounce arm and says nothing about the arm the
/// designer's ruling changed. This stage is an 88 at a plate exactly as
/// thick as its penetration — the heavy tank's glacis thickened from 8 to
/// 9 for the purpose, so the roll sits at parity, the shell gets through
/// about half the time and spends about two thirds of itself when it does.
/// Fought as four hundred single rounds from fresh stages rather than one
/// long battle, because a tank an 88 can penetrate does not survive a long
/// battle. What is compared is what the ladder charged per shot fired
/// against what `expected_pressure` promised — to a tenth here rather than
/// the glacis test's fifth, because the sample is twelve hundred shots and
/// the fifth could not tell the arithmetic from its own mutation.
///
/// Measured: 1200 shots, 64.8% arriving against a promised 65%, 49.0%
/// penetrating against 51.6%, a mean spend of 8.62 of the 13 listed, and a
/// ratio of **0.954**. The residue is the twin rounding its expected spend
/// before dividing (nine of thirteen where the shots average 8.6) and the
/// ledger rounding each charge, both deliberate. The mutation — charge
/// `hit + penetrated` whatever the shell spent, on the charging side of
/// this test — reads **1.27** on the same sample. At sixty stages the real
/// ratio wandered to 0.84 on a two-sigma draw of the hit roll, which is why
/// the sample is what it is.
///
/// The stage is at parity rather than at the 120 per cent a shipped pairing
/// offers because the first draft was, and it did not discriminate: at 120
/// per cent the partial band spends seven eighths of the shell and charging
/// the whole of it instead was inside the band. At parity the shell spends
/// two thirds, which is the whole reason the plate is thickened.
#[test]
fn a_shell_that_only_scrapes_through_is_priced_by_the_same_arithmetic_too() {
    let mut reg = seen(registry());
    reg.vehicles
        .get_mut("heavy_tank")
        .expect("the base mod ships a heavy tank")
        .armor
        .front = 9;
    let (gun, plate) = (UnitId(0), UnitId(1));
    let eighty_eight = reg.weapon("gun_88").expect("the base mod ships an 88");
    assert_eq!(
        eighty_eight
            .ammo
            .first()
            .and_then(|id| reg.ammo(id))
            .map(|a| a.penetration[0]),
        Some(9),
        "the stage is built on the 88 meeting its own penetration in the plate"
    );
    let (mut charged, mut fired, mut promised, mut partials) = (0.0f32, 0u32, 0.0f32, 0u32);
    for seed in 0..400 {
        let mut state = two_side_battle(
            &reg,
            &["gggggggggg", "gggggggggg", "gggggggggg"],
            vec![
                unit_at([0, 1], 0, "heavy_tank", "Gun"),
                unit_at([1, 1], 1, "heavy_tank", "Plate"),
            ],
            900 + seed,
        );
        let (gun_pos, plate_pos) = (state.unit(gun).unwrap().pos, state.unit(plate).unwrap().pos);
        // The plate does not shoot back, as on the glacis stage: a gun crew
        // under fire climbs the ladder and her rung's accuracy takes the hit
        // chance below the one the promise was made at, which is a stage
        // artefact and not a price list. Her fire order stands anyway — it is
        // deliberate overwatch, and the drill leaves her where she is, so the
        // shot the arithmetic priced is the shot that is fired.
        if let Some(plate) = state.unit_mut(plate) {
            for aboard in plate.ammo.values_mut() {
                *aboard = 0;
            }
        }
        for (unit, target) in [(gun, plate), (plate, gun)] {
            state
                .apply(
                    &reg,
                    &Order::SetFire {
                        unit,
                        fire: FireIntent::Target { target, weapon: 0 },
                    },
                )
                .expect("both are in plain sight");
        }
        let expected = tactics_core::battle::expected_pressure(
            &reg,
            &state,
            gun,
            gun_pos,
            eighty_eight,
            plate,
            plate_pos,
            false,
        );
        let mut shots = 0u32;
        // The 88 and nothing else: a heavy tank carries a coaxial belt too,
        // and its bounces are charged the belt's suppression, which the
        // promise above never priced — so the count and the charge are both
        // filtered on the round, which is what the events carry it for.
        let ap = |a: &Option<String>| a.as_deref() == Some("ap_88");
        for event in play_round(&reg, &mut state) {
            match &event {
                // Off the events rather than off her racks, because on some
                // seeds the plate's own 88 finishes her inside the round.
                BattleEvent::ShotFired {
                    attacker, weapon, ..
                } if *attacker == gun && weapon == "gun_88" => shots += 1,
                BattleEvent::ShotHit {
                    target,
                    ammo,
                    damage,
                    budget,
                    ..
                } if *target == plate && ap(ammo) => {
                    if damage < budget {
                        partials += 1;
                    }
                    let spent = tactics_core::battle::spent_share(*damage, *budget);
                    charged += reg
                        .morale
                        .pressure_for(ShotFelt::Penetrated { spent }, felt(&reg, ammo, false))
                        .round();
                }
                BattleEvent::ShotBounced {
                    target,
                    ammo,
                    rattled,
                    ..
                } if *target == plate && ap(ammo) => {
                    charged += reg
                        .morale
                        .pressure_for(ShotFelt::Bounced, felt(&reg, ammo, !rattled))
                        .round();
                }
                _ => {}
            }
        }
        fired += shots;
        promised += expected * shots as f32;
    }
    assert!(
        fired >= 100,
        "the sample has to be big enough to average, got {fired} shots"
    );
    assert!(
        partials >= 100,
        "the stage has to produce partial penetrations, or it is the glacis test again: {partials}"
    );
    let ratio = charged / promised;
    assert!(
        (0.9..=1.1).contains(&ratio),
        "the ladder charged {charged:.0} over {fired} shots where the arithmetic promised \
         {promised:.0} — a factor of {ratio:.3}, which is two price lists rather than one"
    );
}

/// A gun that fires six times a round is priced six times.
///
/// Cadence, and the shape of the claim matters: `Bearing` reports per *shot*
/// so the panel can name a gun and its rate, and `incoming` reports per
/// *round* because that is the unit ground is held in. The two have to agree
/// exactly, or the player's overlay and the AI's threat term are reading
/// different games off the same call.
#[test]
fn a_gun_that_fires_six_times_a_round_is_priced_six_times() {
    let reg = seen(registry());
    let state = firing_positions(&reg, 11);
    let mark = UnitId(2);
    let mark_pos = state.unit(mark).expect("staged").pos;

    let bearings = tactics_core::battle::fire_on(&reg, &state, mark, mark_pos);
    assert!(
        bearings.len() >= 2,
        "the stage needs several guns bearing on her: {bearings:?}"
    );
    // Cadence is a fact about the weapon, and this is the join: the number on
    // the bearing has to be the one the scale contract computes.
    for bearing in &bearings {
        let enemy = state.unit(bearing.enemy).expect("on the field");
        let weapon = reg
            .vehicle(&enemy.vehicle)
            .and_then(|v| v.weapons.get(bearing.weapon))
            .and_then(|w| reg.weapon(w))
            .expect("the bearing names a gun she has");
        assert_eq!(
            bearing.shots,
            reg.scale.ticks_per_round as f32
                / tactics_core::battle::crewed_reload(&reg, &state, bearing.enemy, weapon) as f32,
            "{}'s cadence must be the rate her own loader achieves, not the \
             datasheet's — the panel, the evaluator and the resolver all read \
             one answer",
            enemy.name
        );
        assert!(
            bearing.shots > 0.0,
            "and every gun on the field fires at least sometimes"
        );
    }

    let total = tactics_core::battle::incoming(&reg, &state, mark, mark_pos);
    assert_eq!(
        total.substance,
        bearings.iter().map(|b| b.expected * b.shots).sum::<f32>(),
        "a round of incoming is each gun's shot times how often it fires"
    );
    assert_eq!(
        total.pressure,
        bearings.iter().map(|b| b.pressure * b.shots).sum::<f32>(),
        "and so is the fear"
    );
    assert_eq!(
        total.worth,
        bearings.iter().map(|b| b.worth * b.shots).sum::<f32>(),
        "and so is the worth the evaluator spends"
    );

    // And it is genuinely bigger than the per-shot sum it replaced, or the
    // whole change is decoration. Every gun in the base mod fires more than
    // once in a sixty-second round.
    let one_each: f32 = bearings.iter().map(|b| b.expected).sum();
    assert!(
        total.substance > one_each,
        "a round of fire has to be worse than one shot each: {} against {one_each}",
        total.substance
    );
}

/// A mod that says nothing about suppression plays the game it always played.
///
/// The additivity rule, stated over both new numbers at once: with
/// `ammo.suppression` at zero everywhere and `morale.point_worth` at zero,
/// every bearing's `worth` is its `expected` to the last bit, so nothing that
/// chooses can tell the two currencies apart. This is the property that lets
/// a mod written before this chunk keep its balance.
#[test]
fn a_mod_that_says_nothing_about_suppression_plays_the_game_before() {
    let mut reg = seen(registry());
    for ammo in reg.ammo.values_mut() {
        ammo.suppression = 0;
    }
    reg.morale.point_worth = 0.0;
    let state = firing_positions(&reg, 11);

    let mut bearings_seen = 0;
    for me in state.units.iter() {
        for at in [me.pos, tactics_core::offset_to_hex(5, 1)] {
            for bearing in tactics_core::battle::fire_on(&reg, &state, me.id, at) {
                assert_eq!(
                    bearing.worth, bearing.expected,
                    "worth must be exactly expected damage, so nothing that \
                     chooses can tell this game from the one before it"
                );
                bearings_seen += 1;
            }
        }
    }
    assert!(
        bearings_seen >= 4,
        "the stage has to produce bearings to say anything, got {bearings_seen}"
    );

    // Note the one thing that is *not* zero even here: the ladder's own
    // `bounced` and `penetrated` prices are still charged, and they reach
    // `expected_pressure` because it is the same price list. That is not a
    // leak — it is the analytic twin telling the truth about a rule that
    // already existed — and it is invisible to every chooser while
    // `point_worth` is zero, which is what the assertions above pin. Small
    // arms on plate are the one case the ladder itself prices at nothing, so
    // a belt with no suppression frightens nobody at all.
    let plate_state = a_belt_at_a_glacis(&reg, 31);
    let (plate, belt) = (UnitId(0), UnitId(1));
    assert_eq!(
        tactics_core::battle::expected_pressure(
            &reg,
            &plate_state,
            belt,
            plate_state.unit(belt).expect("staged").pos,
            her_machine_gun(&reg),
            plate,
            plate_state.unit(plate).expect("staged").pos,
            false,
        ),
        0.0,
        "a belt that declares no suppression frightens nobody, which is the \
         plinking rule exactly as it stood"
    );
}

/// The loader will fire a belt at plate she cannot beat when fear is worth
/// something.
///
/// The end-to-end version of the first test in this section, and the one that
/// says the change reached the game rather than only the arithmetic:
/// `best_opportunity_shot` refuses a worthless shot, and until worth carried
/// pressure a burst against a glacis was worthless by definition. Nobody in
/// this engine had ever fired a machine gun at a tank.
#[test]
fn the_loader_will_fire_a_belt_at_plate_she_cannot_beat_when_fear_is_worth_something() {
    let mut reg = seen(registry());
    reg.ammo
        .get_mut("ball_mg")
        .expect("the base mod ships a belt")
        .suppression = 2;

    let belt_fires = |reg: &DataRegistry| {
        let mut state = a_belt_at_a_glacis(reg, 5);
        // Nobody is ordered to do anything, so every shot below is the crew's
        // own opportunity fire — which is the decision under test.
        let mut fired = 0;
        for _ in 0..3 {
            for event in play_round(reg, &mut state) {
                if matches!(
                    event,
                    BattleEvent::ShotFired {
                        attacker: UnitId(1),
                        ..
                    }
                ) {
                    fired += 1;
                }
            }
        }
        fired
    };

    reg.morale.point_worth = 0.0;
    assert_eq!(
        belt_fires(&reg),
        0,
        "with fear worth nothing she holds her fire, keeps her position quiet, \
         and that is the discipline this engine has always had"
    );

    reg.morale.point_worth = 1.0;
    let with_fear = belt_fires(&reg);
    assert!(
        with_fear > 0,
        "and with fear worth something she opens up: {with_fear} bursts"
    );
}

/// Both new numbers are read, and both are addressable by the name the json
/// uses.
///
/// The "read at all" check the `planner` block's tests make, applied to the
/// two fields this chunk adds. The failure it guards against is not a wrong
/// number but a declared one nothing consults — which is what
/// `Scale::elevation_meters` was for months and what two fields of the old
/// `CrewStats` were for years.
///
/// Mutation-checked: `AmmoDef::suppression` forced to 0 in `Round::loaded`
/// fails the first pair, and `point_worth` forced to 0.0 in `round_worth`
/// fails the second.
#[test]
fn suppression_and_what_fear_is_worth_are_data_and_are_read() {
    let base = seen(registry());
    let state = a_belt_at_a_glacis(&base, 31);
    let (plate, belt) = (UnitId(0), UnitId(1));
    let (plate_pos, belt_pos) = (
        state.unit(plate).expect("staged").pos,
        state.unit(belt).expect("staged").pos,
    );
    let pressure_of = |reg: &DataRegistry| {
        tactics_core::battle::expected_pressure(
            reg,
            &state,
            belt,
            belt_pos,
            her_machine_gun(reg),
            plate,
            plate_pos,
            false,
        )
    };
    let worth_of = |reg: &DataRegistry| {
        tactics_core::battle::fire_on(reg, &state, plate, plate_pos)
            .first()
            .map(|b| b.worth)
            .unwrap_or(0.0)
    };

    // `ammo.<id>.suppression`: a belt that says nothing frightens nobody, and
    // one that says something frightens them by exactly what it says. The
    // base mod's belt *does* say something now, so the silent half is staged
    // rather than inherited.
    let mut quiet = base.clone();
    quiet.morale.point_worth = 1.0;
    quiet.ammo.get_mut("ball_mg").expect("shipped").suppression = 0;
    let mut loud = quiet.clone();
    loud.ammo.get_mut("ball_mg").expect("shipped").suppression = 4;
    assert_eq!(pressure_of(&quiet), 0.0, "a silent belt is read as silent");
    assert!(
        pressure_of(&loud) > 0.0,
        "and a loud one is read at all: {}",
        pressure_of(&loud)
    );

    // `morale.point_worth`: the exchange rate, and it is what turns the
    // pressure above into something a chooser can see.
    let mut free = loud.clone();
    free.morale.point_worth = 0.0;
    assert_eq!(
        worth_of(&free),
        0.0,
        "fear priced at nothing is worth nothing to anybody weighing a shot"
    );
    assert!(
        worth_of(&loud) > worth_of(&free),
        "and fear priced at something is worth something: {} against {}",
        worth_of(&loud),
        worth_of(&free)
    );

    // ...and both reach the registry through the same serde round trip
    // `--set` and `--sweep` use, addressed by the name the json writes. A
    // number that can only be changed from Rust is not content.
    let mut swept = base.clone();
    for (path, value) in [
        ("ammo.ball_mg.suppression", "3"),
        ("morale.point_worth", "0.75"),
    ] {
        let ov = tactics_core::harness::overrides::Override::parse(&format!("{path}={value}"))
            .expect("a well-formed override");
        tactics_core::harness::overrides::apply_override(&mut swept, &ov)
            .unwrap_or_else(|e| panic!("{path} should be addressable: {e}"));
    }
    assert_eq!(swept.ammo["ball_mg"].suppression, 3);
    assert_eq!(swept.morale.point_worth, 0.75);
}

// --- one walk, one gate: the readers of the currency ------------------------

//
// Wave 2's first half. `ai::threats` and `ai::threatened` used to be a second
// walk over the visible enemies with a gate of their own, and the two places
// that decide *where a crew stands when she is frightened* — the mid-round
// battle drill and the rout underneath it — read terrain `cover`, a model of
// what cover is for sitting beside a resolver that answers the same question
// exactly. Both are the currency now.
//
// The three tests below pin, in order: that there is one answer to who can
// shoot her; that the orderly reflex goes where the gun cannot see her rather
// than to the nearest trees; and that the orderly reflex and the rout are
// still two different things, which is the designer's own split.

/// A gun, a crew idle in front of it, a wood the gun is looking straight
/// into, and bare ground behind the wood that it cannot see at all.
///
/// The curtain spans every row and is one column thick, and both halves of
/// that matter. Spanning every row is what makes everything east of it dead
/// ground rather than merely awkward to see. Being one column thick is what
/// makes that dead ground *bare*: a two-column belt would put a hex that is
/// both wooded and unseen inside her reach, and she would take it — correctly,
/// but the test would then be unable to say which of the two facts she was
/// acting on.
///
/// The wood is *nearer* to her than the dead ground and carries the only
/// terrain `cover` on the map, so the rule this stage separates is exactly
/// the one that changed: under the old drill she went to the trees because
/// they scored 30, and the gun could see her sitting in them.
fn wood_and_dead_ground(reg: &DataRegistry, seed: u64) -> BattleState {
    let row = "ggggfggggg";
    two_side_battle(
        reg,
        &[row, row, row],
        vec![
            unit_at([3, 1], 0, "medium_tank", "Watcher"),
            unit_at([0, 1], 1, "medium_tank", "Gun"),
        ],
        seed,
    )
}

/// Play the round out and report where (and on what board) the drill sent
/// her.
fn drill_destination(
    reg: &DataRegistry,
    state: &mut BattleState,
    watcher: UnitId,
) -> Option<(tactics_core::Hex, BattleState)> {
    commit_all(reg, state);
    let mut found = None;
    while state.resolving_tick().is_some() && !state.is_over() {
        let before = state.clone();
        for event in state.step_tick(reg) {
            if let BattleEvent::TookCover { unit, at } = event
                && unit == watcher
                && found.is_none()
            {
                found = Some((at, before.clone()));
            }
        }
    }
    found
}

/// There is one answer to who can shoot her.
///
/// `ai::threats` is `fire_on`'s membership and `ai::threatened` is
/// `incoming(..).worth > 0`, rather than a second walk with a gate of its
/// own. The two were arithmetically identical when they were joined — a gun
/// is admitted by `best_weapon_from` precisely when one shot from it is worth
/// something, and a sum of positive terms is positive — so this test is what
/// keeps them identical: a gate reworded in one place and not the other would
/// leave a crew whom the drill thinks is safe and the overlay paints red.
///
/// Asked of every crew on the stage, including the ones with nothing bearing
/// on them, because "nobody is shooting at me" is the answer that matters for
/// the parking lot.
///
/// Mutation-checked by putting a threshold back into `threats`: the seven
/// bearings this stage produces run from 1.27 to 8.99 substance points a
/// round, so any gate above 1.27 drops the tank destroyer's rifle section and
/// fails the equality. The margin is worth recording because it is the whole
/// content of the test — the two functions agree by construction today, and
/// what is being defended is that nobody may reintroduce a second opinion.
#[test]
fn there_is_one_answer_to_who_can_shoot_her() {
    let reg = seen(registry());
    let state = firing_positions(&reg, 11);
    let mut with_somebody = 0;
    for me in state.units.iter() {
        let listed = tactics_core::ai::threats(&reg, &state, me.id);
        let bearing: Vec<UnitId> = tactics_core::battle::fire_on(&reg, &state, me.id, me.pos)
            .into_iter()
            .map(|b| b.enemy)
            .collect();
        assert_eq!(
            listed, bearing,
            "{} is threatened by exactly the enemies who have a bearing on her",
            me.name
        );
        let worth = tactics_core::battle::incoming(&reg, &state, me.id, me.pos).worth;
        assert_eq!(
            tactics_core::ai::threatened(&reg, &state, me.id),
            worth > 0.0,
            "{} counts as under fire exactly when a round of that fire is worth something \
             ({worth})",
            me.name
        );
        if !listed.is_empty() {
            with_somebody += 1;
        }
    }
    assert!(
        with_somebody >= 2,
        "the stage has to put somebody under fire or this test asserts about nothing"
    );
}

/// The drill goes where the gun cannot see her, not to the nearest wood.
///
/// The wood on this stage is one hex away and carries 30 points of cover; the
/// dead ground behind it is further, bare, and cannot be shot at. The old
/// drill took the reachable tile with the most terrain `cover` and therefore
/// took the wood — cover and dead ground are the same word to a terrain
/// table, and they are opposite answers to the question the drill is asking.
///
/// Both halves are asserted, because either alone is weak: the wood really
/// was reachable and really does score more cover (so the old rule had
/// something to choose), and the ground she took really is ground the gun
/// expects nothing on.
#[test]
fn the_drill_goes_where_the_gun_cannot_see_her_not_to_the_nearest_wood() {
    let mut reg = seen(registry_wireless());
    soften(&mut reg);
    let (watcher, gun) = (UnitId(0), UnitId(1));
    let mut state = wood_and_dead_ground(&reg, 301);
    let parked = state.unit(watcher).unwrap().pos;
    // Deliberate overwatch keeps the gun where it was put: a crew with a fire
    // order is exempt from the drill, so the stage does not turn into two
    // crews reacting to each other.
    state
        .apply(
            &reg,
            &Order::SetFire {
                unit: gun,
                fire: FireIntent::Area {
                    at: parked,
                    weapon: 0,
                },
            },
        )
        .expect("area fire on a hex needs no spot");

    // The wood the old rule would have taken: reachable, adjacent, and the
    // only cover on the map.
    let wood = tactics_core::offset_to_hex(4, 1);
    assert_eq!(
        state.terrain_at(wood),
        Some("forest"),
        "the stage needs its wood where the test thinks it is"
    );
    let reach = reachable(&reg, &state, watcher);
    assert!(
        reach.contains_key(&wood),
        "and the wood has to be somewhere she could actually have gone"
    );
    let cover_of = |hex| {
        state
            .terrain_at(hex)
            .and_then(|t| reg.terrain(t))
            .map_or(0, |t| t.cover)
    };
    assert!(
        cover_of(wood) > cover_of(parked),
        "the old rule had something to choose: {} against {}",
        cover_of(wood),
        cover_of(parked)
    );

    let (dest, board) =
        drill_destination(&reg, &mut state, watcher).expect("she is under fire and idle");
    assert!(
        tactics_core::battle::incoming_from(&reg, &board, watcher, dest, &[gun]).worth <= 0.0,
        "she goes where the gun expects nothing, and {dest:?} is not that hex"
    );
    assert!(
        tactics_core::battle::incoming_from(&reg, &board, watcher, wood, &[gun]).worth > 0.0,
        "the wood is inside the gun's envelope, or this stage is not the one the test needs"
    );
    assert_ne!(
        state.terrain_at(dest),
        Some("forest"),
        "and she is not in the trees the gun is looking at"
    );
}

/// A frightened crew runs from the gun; an orderly one ducks out of its
/// sight.
///
/// The designer's split, on one stage. Both reflexes now price ground in the
/// same currency, which is exactly why the difference between them has to be
/// stated: the drill minimises the fire on her and does not care how far off
/// the gun is, and the rout takes distance first and only then asks which of
/// the hexes that tie is quietest. So on ground that offers both, the drill
/// stops at the first hex the gun cannot see and the rout keeps going to the
/// far end of the field.
#[test]
fn a_frightened_crew_runs_from_the_gun_and_an_orderly_one_ducks_out_of_its_sight() {
    let mut reg = seen(registry_wireless());
    soften(&mut reg);
    let (watcher, gun) = (UnitId(0), UnitId(1));

    // The orderly half: she is steady, so the drill decides.
    let mut steady = wood_and_dead_ground(&reg, 302);
    let parked = steady.unit(watcher).unwrap().pos;
    let enemy = steady.unit(gun).unwrap().pos;
    steady
        .apply(
            &reg,
            &Order::SetFire {
                unit: gun,
                fire: FireIntent::Area {
                    at: parked,
                    weapon: 0,
                },
            },
        )
        .expect("area fire on a hex needs no spot");
    let (ducked, board) =
        drill_destination(&reg, &mut steady, watcher).expect("she is under fire and idle");

    // The rout: the same stage, the same crew, off the end of the ladder and
    // with a temperament that runs.
    let mut broken = wood_and_dead_ground(&reg, 302);
    always(&mut reg, "flight");
    broken.units[watcher.index()].pressure = breaking(&reg);
    broken
        .apply(
            &reg,
            &Order::SetFire {
                unit: gun,
                fire: FireIntent::Area {
                    at: parked,
                    weapon: 0,
                },
            },
        )
        .expect("area fire on a hex needs no spot");
    play_round(&reg, &mut broken);
    let ran = broken.unit(watcher).expect("she survives the round").pos;

    assert!(
        ran.distance_to(enemy) > parked.distance_to(enemy),
        "the rout puts ground between her and the gun: {parked:?} -> {ran:?}"
    );
    assert!(
        tactics_core::battle::incoming_from(&reg, &board, watcher, ducked, &[gun]).worth <= 0.0,
        "and the drill puts her out of its sight: {ducked:?}"
    );
    assert!(
        ran.distance_to(enemy) > ducked.distance_to(enemy),
        "and they are two reflexes rather than one with a different name: the rout opens \
         the range further than the drill does ({ran:?} against {ducked:?}), because \
         distance is the rout's first key and the drill has no distance term at all"
    );
}

/// The mid-round reflex asks the same gate the planner's drill asks.
///
/// Latitude was read in exactly one place, the planner's, and the engine's
/// own reflex in `run_crew_drill` read nothing — so a crew whose commander
/// said "I mean it" pressed on through the planning phase and was pulled
/// into the trees by the reflex on the first tick she noticed the gun. Both
/// ask `Unit::yields_to_drill` now. Staged on the drill's own ground with
/// the crew *holding* rather than marching, because that is the case the
/// planner's gate could never have covered: an idle crew on the ground she
/// was given, and the only thing between her and the wood is whether the
/// hold carries the insistence the march did.
///
/// Mutation-checked by deleting the gate from `run_crew_drill`: the binding
/// half then reports the same dash the delegated half does.
#[test]
fn a_crew_told_to_hold_her_ground_and_meaning_it_is_not_moved_by_the_reflex() {
    let mut reg = seen(registry_wireless());
    soften(&mut reg);
    let (watcher, gun) = (UnitId(0), UnitId(1));
    let dash = |latitude: Latitude| {
        let mut state = wood_and_dead_ground(&reg, 304);
        let parked = state.unit(watcher).unwrap().pos;
        // Deliberate overwatch keeps the gun where it was put, as in every
        // test on this stage.
        state
            .apply(
                &reg,
                &Order::SetFire {
                    unit: gun,
                    fire: FireIntent::Area {
                        at: parked,
                        weapon: 0,
                    },
                },
            )
            .expect("area fire on a hex needs no spot");
        // Sent to the hex she is standing on: a march of no hexes, which is
        // the shortest way to say "hold this ground" at a latitude.
        state
            .apply(
                &reg,
                &Order::Radio {
                    unit: watcher,
                    to: Some(parked),
                    fire: None,
                    latitude,
                },
            )
            .expect("where she stands is ground she may be told to hold");
        assert!(
            state.unit(watcher).unwrap().intent.is_empty(),
            "a march of no hexes leaves her idle, which is who the reflex looks at"
        );
        drill_destination(&reg, &mut state, watcher).map(|(at, _)| at)
    };
    let delegated = dash(Latitude::Delegated);
    assert!(
        delegated.is_some(),
        "told to use her judgment, she ducks out of the gun's sight: {delegated:?}"
    );
    assert_eq!(
        dash(Latitude::Binding),
        None,
        "told she is meant, she holds the ground through the same fire"
    );
}
