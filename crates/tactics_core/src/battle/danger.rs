//! What the enemy could do to a crew standing on a given piece of ground.
//!
//! This is one walk and one struct, and the point of both is that there
//! is exactly one of them. "How dangerous is that hex" is asked from at
//! least three directions — the evaluator deciding where to drive, the
//! player's overlay asking why a tile is red, and anything that wants to
//! explain a decision after the fact — and every one of them must get the
//! resolver's own arithmetic rather than a model of it. The previous
//! arrangement is the argument for this one: the AI priced danger as
//! `expected_damage` against the hex she was *already* standing on, scaled
//! by `1/distance` to the tile it was actually deciding about, so cover,
//! elevation, facing and range never reached the decision at all.
//!
//! What is deliberately *not* here: doctrine weights, planner numbers,
//! distance falloffs, and any opinion about what a crew ought to do with
//! the answer. Those are judgment and belong to whoever is asking. This
//! module answers a question about the rules.

use super::{BattleState, Knower, Unit, UnitId, combat};
use crate::data::DataRegistry;
use hexx::Hex;

/// One enemy's best shot at a crew standing on a particular hex.
///
/// `hit_percent` and the expectations are the halves a reader wants kept
/// apart: a near-certain scratch and an unlikely killing blow are both
/// "some expected damage", and a player owed an explanation is owed both
/// numbers. Everything below `hit_percent` already has the hit chance in it.
///
/// **The three expectations are per shot and `shots` is the cadence.** They
/// are kept apart rather than multiplied together because the two readers of
/// this struct are asking different questions: the panel names a gun and its
/// rate, and anything pricing ground for a round multiplies. Folding cadence
/// in would leave the panel unable to say "45% for 3.0, four times a round",
/// which is the sentence that explains where the total came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bearing {
    /// Who could shoot.
    pub enemy: UnitId,
    /// Which of her weapons, as an index into her chassis' weapon list.
    pub weapon: usize,
    /// Her chance of hitting, as the resolver would roll it.
    pub hit_percent: i32,
    /// Expected damage in substance points from **one shot**: hit chance
    /// times what the round is worth against the plate it would strike.
    pub expected: f32,
    /// Expected pressure in ladder points from one shot — what arriving does
    /// to the crew's nerve whether or not it gets through. Nonzero where
    /// `expected` is flatly zero, which is the whole reason it is here: a
    /// burst against a glacis is not nothing.
    pub pressure: f32,
    /// The two above in one number, per shot:
    /// `expected + pressure * morale.point_worth`. What anything *choosing*
    /// should read.
    pub worth: f32,
    /// How many times this gun fires in a round.
    pub shots: f32,
}

impl Bearing {
    /// What this gun expects to take out of her over a whole round.
    pub fn damage_per_round(&self) -> f32 {
        self.expected * self.shots
    }

    /// What it expects to do to her nerve over a whole round.
    pub fn pressure_per_round(&self) -> f32 {
        self.pressure * self.shots
    }

    /// The two together over a whole round — the number the tint and the
    /// evaluator's threat term are both denominated in.
    pub fn worth_per_round(&self) -> f32 {
        self.worth * self.shots
    }
}

/// What the spotted enemies could put on a crew standing on a hex, over one
/// round.
///
/// **Every field is per round**, which is the unit ground is priced in now
/// that cadence is in the currency: a machine gun firing six times and an 88
/// firing three are not the same threat, and until this struct existed they
/// read alike. `substance` and `pressure` are the two currencies apart, and
/// `worth` is them together at the mod's exchange rate — the one the
/// evaluator spends.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Incoming {
    /// Expected damage in substance points, summed over every bearing, per
    /// round.
    pub substance: f32,
    /// Expected pressure in ladder points, per round.
    pub pressure: f32,
    /// `substance + pressure * morale.point_worth`, per round.
    pub worth: f32,
}

/// Every enemy `unit` knows of that could put fire on her if she stood at
/// `at`, with his best weapon for the job, in enemy id order.
///
/// Fog-honest: only enemies she knows about are listed —
/// [`BattleState::known_enemies`] for [`Knower::Crew`], her picture if she
/// can hear the net and her own eyes — so a planner reading this cannot
/// flinch away from a tank nobody has told her of and thereby tell the
/// player it is there. [`fire_on_as`] asks the same question with somebody
/// else's knowledge, which is what the player's danger panel needs: she is
/// the commander, and she knows what has been reported.
///
/// Deterministic twice over: the enemies come in id order and each one's
/// weapon is chosen by [`combat::best_weapon_from`]'s first-wins tie-break,
/// so the list is a pure function of the state and may be summed, printed
/// or compared without sorting.
///
/// `at` is hypothetical; the enemies are where they really are. That is the
/// asymmetry the question has — she is deciding where to stand, and they
/// have already parked.
pub fn fire_on(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    at: Hex,
) -> Vec<Bearing> {
    fire_on_as(registry, state, Knower::Crew(unit), unit, at)
}

/// [`fire_on`], asked with `who`'s knowledge of the enemy rather than her
/// own.
///
/// The player's danger panel and overlay pass [`Knower::Commander`]: what
/// she is shown on the board is her picture, and advice that named a gun
/// the board draws as a ghost — or not at all — would be the fog leaking
/// through the one panel built to be honest about it.
pub fn fire_on_as(
    registry: &DataRegistry,
    state: &BattleState,
    who: Knower,
    unit: UnitId,
    at: Hex,
) -> Vec<Bearing> {
    let mut bearings = Vec::new();
    guns_bearing_on(
        registry,
        state,
        who,
        unit,
        at,
        None,
        |enemy, weapon, value| {
            // The gun is known to exist by the walk above; asking the chassis
            // for it again only to name it would be a second lookup for a
            // number we would then have to keep in step.
            let hit_percent = registry
                .vehicle(&enemy.vehicle)
                .and_then(|v| v.weapons.get(weapon))
                .and_then(|w| registry.weapon(w))
                .map(|w| {
                    combat::hit_chance(registry, state, enemy.id, enemy.pos, w, unit, at, false)
                })
                .unwrap_or(0);
            bearings.push(Bearing {
                enemy: enemy.id,
                weapon,
                hit_percent,
                expected: value.expected,
                pressure: value.pressure,
                worth: value.worth,
                shots: value.shots,
            });
        },
    );
    bearings
}

/// The same question, summed over a round: what the spotted enemies could put
/// on `unit` if she stood at `at`, in both currencies and in the one that
/// combines them.
///
/// **Per round, not per shot.** Every term in this engine used to be per
/// trigger pull, which priced a machine gun firing six times a round and an
/// 88 firing three identically; the multiplication happens here because this
/// is the answer to *how bad is that ground*, and ground is held for rounds.
///
/// This is what the evaluator's threat term spends, and it exists beside
/// [`fire_on`] rather than as `fire_on(..).iter().sum()` for one reason,
/// which is cost. `Evaluator::score_tile` is the hottest function the AI
/// has and it asks this once per candidate tile per crew per round, so a
/// `Vec` allocated and thrown away each time — and, worse, a second
/// [`combat::hit_chance`] per enemy for a `hit_percent` the evaluator never
/// reads — is paid for on every tile of every sweep. Both callers walk the
/// same [`guns_bearing_on`], so there is still exactly one answer to *who
/// can shoot her there and with what*; they differ only in what they do
/// with it.
pub fn incoming(registry: &DataRegistry, state: &BattleState, unit: UnitId, at: Hex) -> Incoming {
    sum_bearings(registry, state, unit, at, None)
}

/// [`incoming`], restricted to a named list of enemies.
///
/// The same arithmetic and the same walk, asked about *some* of the guns
/// rather than all of them. It exists for the mid-round battle drill, which
/// has to price ground against the threats this crew has actually caught up
/// with: her `reactions` delay is a per-enemy clock, so a gun that appeared
/// out of a treeline this tick is one she does not know about yet, and
/// letting it into the arithmetic would let her flinch from something she has
/// not seen — the same fog dishonesty [`fire_on`] refuses at the level of the
/// side's picture, one clock further in.
///
/// `only` is a list of enemy ids; anybody not on it is skipped. It is walked
/// in `state.units` order like everything else here, so the result does not
/// depend on how the caller sorted its list.
pub fn incoming_from(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    at: Hex,
    only: &[UnitId],
) -> Incoming {
    sum_bearings(registry, state, unit, at, Some(only))
}

fn sum_bearings(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    at: Hex,
    only: Option<&[UnitId]>,
) -> Incoming {
    let mut total = Incoming::default();
    guns_bearing_on(
        registry,
        state,
        Knower::Crew(unit),
        unit,
        at,
        only,
        |_, _, value| {
            total.substance += value.expected * value.shots;
            total.pressure += value.pressure * value.shots;
            total.worth += value.worth_per_round();
        },
    );
    total
}

/// Every enemy `who` knows of who could put fire on `unit` at `at`, handed to
/// `each` as (enemy, weapon index, what one shot is worth) in enemy id
/// order.
///
/// The shared half of this module: [`fire_on`], [`incoming`] and
/// [`incoming_from`] are three readings of one walk, and the walk is here so
/// they cannot disagree about membership, gun choice or order. Deliberately
/// not public — what a caller wants to know is "what could be put on her",
/// and the shapes above are the useful answers to it.
fn guns_bearing_on(
    registry: &DataRegistry,
    state: &BattleState,
    who: Knower,
    unit: UnitId,
    at: Hex,
    only: Option<&[UnitId]>,
    mut each: impl FnMut(&Unit, usize, combat::ShotValue),
) {
    for enemy in state.known_enemies(registry, who) {
        if only.is_some_and(|ids| !ids.contains(&enemy.id)) {
            continue;
        }
        if let Some((weapon, value)) =
            combat::best_weapon_from(registry, state, enemy.id, enemy.pos, unit, at)
        {
            each(enemy, weapon, value);
        }
    }
}

// --- the battle drill ------------------------------------------------------
//
// `threatened` and `threats` lived in `ai::` while the engine's own reflex
// and rout called them from `battle::orders`, so a rule of the battlefield
// depended on the AI module. They are questions about fire, answered by this
// module's walk, and they live beside it.

/// Whether anything the side can see could put fire on this unit where she
/// stands. Fog-honest (spotted enemies only) and deterministic, because "was
/// she in danger" must answer the same on every machine.
///
/// Six very different things ask it, and they must ask it the same way or the
/// game contradicts itself: the battle drill at the planning table, which is
/// what an unordered crew does when nobody has told her anything; the
/// evaluator, where being under fire is what suspends a movement to contact;
/// the taxi rules, where nobody mounts up or waits at a tailgate under fire;
/// the dismount reflex; and the engine's mid-round drill and its rout, where
/// the same danger noticed at tick four is what sends an idle crew scrambling.
/// All of them are the same sentence — *is somebody shooting at me* — so all
/// of them read the same predicate rather than formulas that can drift apart.
///
/// **It is [`crate::battle::incoming`] and nothing else**, which is the whole
/// of Wave 2's first part: this used to be a second walk over the visible
/// enemies asking [`best_weapon_against`] the same question
/// [`crate::battle::danger`] asks, with its own gate. Two walks are two
/// answers waiting to happen — a gun added to one and not the other, or a
/// gate reworded in one place — and the currency has exactly one answer to
/// *who can shoot her there*. The two are arithmetically identical today
/// (`best_weapon_from` admits a gun precisely when its `worth` is positive,
/// and the sum of positive terms is positive), so this is a refactor with a
/// test on it rather than a behaviour change; what it buys is that it stays
/// identical.
pub fn threatened(registry: &DataRegistry, state: &BattleState, unit: UnitId) -> bool {
    let Some(me) = state.unit(unit) else {
        return false;
    };
    incoming(registry, state, unit, me.pos).worth > 0.0
}

/// The enemies she knows of that could put fire on this unit where she stands, in
/// id order. The list form of [`threatened`], for the two callers — the
/// mid-round drill and the rout — that must weigh each threat against when
/// she first laid eyes on it, or run away from where it is standing.
///
/// [`crate::battle::fire_on`]'s membership, and deliberately nothing more:
/// the bearings carry a weapon, a hit chance and a cadence that neither
/// caller wants, but taking the ids off the one walk is what keeps "who can
/// shoot her" from having two answers.
pub fn threats(registry: &DataRegistry, state: &BattleState, unit: UnitId) -> Vec<UnitId> {
    let Some(me) = state.unit(unit) else {
        return Vec::new();
    };
    fire_on(registry, state, unit, me.pos)
        .into_iter()
        .map(|bearing| bearing.enemy)
        .collect()
}

/// The threats `unit` has *noticed*: [`threats`], less any gun she has not
/// yet caught up with on her reaction clock.
///
/// The clock is per enemy — `spotted_since` plus her `reactions` delay — and
/// the same one opportunity fire pays, so a gun that appeared out of a
/// treeline this tick is one she does not know about yet. Kept as a list
/// rather than collapsed to a boolean, because [`drill_destination`] prices
/// ground against these guns and no others: reacting to a gun she has not
/// caught up with would be the reaction-latency defect rebuilt inside the
/// reflex latency is about.
///
/// The mid-round reflex's clock. The planning table's drill does not read
/// it: planning is a pause, and nothing else planned there reads it either.
pub fn noticed_threats(registry: &DataRegistry, state: &BattleState, unit: UnitId) -> Vec<UnitId> {
    let Some(me) = state.unit(unit) else {
        return Vec::new();
    };
    let now = state.round as u64 * registry.scale.ticks_per_round as u64
        + state.resolving_tick().unwrap_or(0) as u64;
    let delay =
        super::stats::reaction_delay(registry, &state.roster, me, state.terrain_at(me.pos)) as u64;
    let fog = state.fog.side(me.side);
    threats(registry, state, unit)
        .into_iter()
        .filter(|enemy| {
            fog.spotted_since
                .get(enemy)
                .is_none_or(|since| now >= since + delay)
        })
        .collect()
}

/// Where the orderly reaction takes a crew under fire: the reachable hex
/// where the guns in `noticed` can do least to her, and only if it is
/// strictly quieter than where she stands.
///
/// **One answer for both drills.** The engine's mid-round reflex
/// (`run_crew_drill`, with the guns she has noticed on her clock) and the
/// planning table's drill for a crew nobody has ordered (`SideCommand`, with
/// every gun bearing on her) both ask this. The planning table used to run
/// the whole evaluator under a synthetic `drill` doctrine instead, so a crew
/// it sent to one hex could be moved to another by the reflex a tick after
/// she arrived — the same reaction on two models.
///
/// The currency, not the terrain table. This used to take the reachable tile
/// with the highest terrain `cover`, which is a second model of what cover
/// is for beside a resolver that answers the question exactly: a wood the gun
/// is looking straight into scores 30 and is a death trap, and the reverse
/// slope twenty metres behind it scores 0 and cannot be shot at at all.
/// [`incoming_from`] brings range, sight, elevation, facing, obliquity and the
/// gun actually bearing on her into the decision — the arithmetic the
/// evaluator spends and the player's overlay draws.
///
/// Strictly better ground only: equal danger never causes a pointless
/// shuffle, and each dash is to ground the guns can do less on, which is
/// what makes the drill settle instead of oscillate. Ties go to the cheaper
/// drive, then the coordinate, last, as the invariants allow.
pub fn drill_destination(
    registry: &DataRegistry,
    state: &BattleState,
    unit: UnitId,
    noticed: &[UnitId],
) -> Option<Hex> {
    let me = state.unit(unit)?;
    let pos = me.pos;
    let danger_at = |hex: Hex| worth_key(incoming_from(registry, state, unit, hex, noticed).worth);
    let here = danger_at(pos);
    // Ties are the ordinary case, not the exception: every hex no gun she
    // has noticed can reach is worth exactly nothing, and a crew backing out
    // of a gun's envelope has a plateau of them. They fall to the one nearest
    // her friends (`movement::apart`) before the coordinate, because the
    // coordinate alone is a compass — it sent every drilling crew west, which
    // is backwards for one end of a battlefield and forwards for the other,
    // and was worth a two-to-one lean on the mirrored `battle_hills`.
    let friends = super::movement::friends_of(state, me);
    super::movement::reachable(registry, state, unit)
        .into_iter()
        .map(|(hex, cost)| (hex, cost, danger_at(hex)))
        .filter(|&(hex, _, danger)| hex != pos && danger < here)
        .min_by_key(|&(hex, cost, danger)| {
            (
                danger,
                cost,
                super::movement::apart(&friends, hex),
                hex.x,
                hex.y,
            )
        })
        .map(|(hex, _, _)| hex)
}

/// A round of expected fire, as an integer key a tiebreak can be sorted on.
///
/// `f32` has no total order, and every "where should she stand" decision in
/// this engine has to be settled the same way on every machine — the
/// determinism snapshot is a byte comparison. Quantising to about a
/// thousandth of a substance point is also the right *behaviour*: two hexes
/// whose expected fire differs in the fourth decimal are ground the resolver
/// cannot really tell apart, and treating them as equal lets the later keys
/// (the cheapest drive, then the coordinate) settle it instead of a rounding
/// artefact. Saturating on the cast handles a non-finite worth by pinning it
/// at the ends rather than panicking.
pub(crate) fn worth_key(worth: f32) -> i64 {
    (worth * 1024.0) as i64
}
