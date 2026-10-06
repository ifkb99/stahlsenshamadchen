//! How much a crew's quality is worth, as data rather than divisors.
//!
//! Crew stats run 0..=5 and modify the hardware they are sitting in. The
//! original coefficients were flat divisors — `awareness / 4` bonus hexes of
//! vision, `driving / 5` bonus movement points — chosen when a vehicle saw 3
//! hexes and moved 2. The scale decision multiplied those bases by four to
//! six times and left the divisors alone, so the best scout in the school was
//! worth one hex out of twenty and the best driver one point out of seven.
//! Crew quality is the emotional core of the genre; it cannot be a rounding
//! error.
//!
//! The fix is to make the bonus a percentage of the vehicle's own base, which
//! is scale-independent by construction: retuning vision ranges or movement
//! allowances no longer silently retunes what a crew is worth. The
//! percentages themselves live here, in the `balance` block of `mod.json`,
//! because a number a modder would want to change does not belong in Rust.
//!
//! All fields are integers so the arithmetic stays exact and the simulation
//! stays bit-for-bit reproducible.

use serde::{Deserialize, Serialize};

/// Serde's default for [`Balance::detection_base`].
///
/// A function rather than a plain `#[serde(default)]`, because the neutral
/// value for this one is 100 and not zero: a mod that declares a `balance`
/// block and says nothing about detection must get the game it always had,
/// not one in which nobody can see anybody.
fn detection_base() -> i32 {
    100
}

/// Serde's default for [`Balance::detection_certain_percent`], and neutral
/// for the same reason: at 100 a crew's whole reach is the near band, so
/// nothing is ever faded in and no die is ever thrown.
fn detection_certain() -> i32 {
    100
}

/// Serde defaults for the numbers that were Rust constants until 2026-08-27.
/// Each returns exactly the constant it replaced, so a mod that declares a
/// `balance` block and says nothing about them gets the game it always had.
fn min_hit() -> i32 {
    5
}
fn max_hit() -> i32 {
    95
}
fn stalemate_rounds() -> u32 {
    8
}
fn eye_height() -> i32 {
    250
}
fn target_height() -> i32 {
    200
}

/// Per-point value of each crew stat.
///
/// Three different kinds of number live here, and which kind a new field is
/// decides how it must be written. [`Self::vision_per_observation`] and
/// [`Self::speed_per_driving`] are *percentages of the vehicle's base*, since
/// what a sharp-eyed commander buys you depends on what they are looking
/// through. [`Self::accuracy_per_gunnery`], [`Self::downhill_bonus`] and
/// [`Self::blind_penalty`] are *percentage points of hit chance*, because hit
/// chance is already a 0..=100 quantity with no base to scale against. And
/// [`Self::cover_to_hit_percent`] is a *percentage of a rating declared
/// elsewhere* — the terrain's own `cover` — which is the kind to reach for
/// when a number's job is to say how much of somebody else's number counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Balance {
    /// Percent of the vehicle's base vision range added per point of the
    /// crew's best awareness. At the default 5, a gifted scout (awareness 5)
    /// sees a quarter further than the same car with a novice aboard.
    pub vision_per_observation: i32,
    /// Percent of the vehicle's base movement allowance added per point of
    /// the crew's best driving. Because movement points are a speed, this
    /// reads directly: +25% on a 30 km/h medium tank is 36 km/h.
    pub speed_per_driving: i32,
    /// Percentage points of hit chance added per point of the crew's best
    /// gunnery. This one was never devalued by the scale change — hit chance
    /// has no base to be a fraction of — but it lived as a bare `* 3` in
    /// `combat.rs`, which is the same design smell.
    pub accuracy_per_gunnery: i32,
    /// `speed_per_athletics`, retired the day it shipped and kept here only
    /// so that a mod still declaring it can be told.
    ///
    /// It was meant to be `speed_per_driving`'s twin for a chassis that
    /// walks, and it cannot express anything: a foot unit has **one**
    /// movement point, and one hex a round already *is* six kilometres an
    /// hour, which is walking pace and correct. No percentage of one rounds
    /// to anything else below 80 a point, where it stops being a slope and
    /// becomes a switch between one hex and two. Measured before it was
    /// retired: bit-identical over 180 battles at 5, at 20 and at 40. The
    /// scale is the constraint and no multiplier fixes it, so `athletics`
    /// means [`Self::athletics_per_climb_level`] and nothing else — steep
    /// ground, which is a thing infantry can have and no chassis can buy.
    ///
    /// What a foot unit walks at is now her chassis's listed allowance and
    /// nobody's skill. She is still not asked for `driving`: that was the
    /// defect this field was written to fix, and dropping the field does not
    /// bring it back.
    ///
    /// Deserialised under its old name, never serialised, and warned about
    /// rather than refused, exactly as `planner.mission_weight` is.
    #[serde(default, rename = "speed_per_athletics", skip_serializing)]
    pub retired_speed_per_athletics: Option<i32>,
    /// Percent of a weapon's listed reload taken off per point of the crew's
    /// `loading` above average.
    ///
    /// The last of the six skills nothing read, and the one whose wiring had
    /// a trap in it: a reload reaches the game twice, once as the cooldown
    /// the resolver sets after a shot and once as the *cadence* every price
    /// in the currency is quoted per round of. Wiring only the cooldown would
    /// have left the evaluator, `danger::fire_on` and the player's danger
    /// overlay all pricing a gun nobody fires at that rate. Both go through
    /// one function for that reason, the way `edge_cost` and `MoveGrid::cost`
    /// share `step_cost`.
    ///
    /// Zero is the rule's absence. A quick loader can never reach a reload of
    /// nothing: [`Self::scaled`]'s floor of one tick is exactly right here,
    /// because a gun that reloads in no time fires infinitely often.
    pub reload_per_loading: i32,
    /// Chance in 100, per round, that a crew gets one broken module working
    /// again, before `maintenance` is added to it.
    ///
    /// The designer's ruling of 2026-09-11 on what `maintenance` should
    /// reach: there was no breakdown rule and no repair rule anywhere in the
    /// engine, so wiring the skill meant inventing one, and a crew getting a
    /// thrown track back on under fire is the version you can watch happen.
    ///
    /// **Zero is the rule's absence down to the rng stream** — no die is
    /// thrown at all — which is the same contract
    /// [`Self::detection_certain_percent`] keeps and what lets the
    /// determinism baseline tell a mod that declines this rule from the game
    /// before it existed.
    pub field_repair_percent: i32,
    /// Ticks a gun that does not traverse ([`crate::data::VehicleDef::turret`]
    /// false) spends swinging her hull onto a target outside her frontal
    /// arc before she can fire at it.
    ///
    /// **Zero is the game before hulls and turrets were told apart**: she
    /// turns and fires in the same tick, as every vehicle always did.
    /// Above zero, a casemate caught from the side answers late, which is
    /// the price of her low hull and heavy front.
    pub pivot_ticks: u32,
    /// What a hex in sight of an enemy she knows of costs a crew driving
    /// somewhere, in movement points, by the order she is driving under.
    /// See [`RouteExposure`].
    pub route_exposure: RouteExposure,
    /// Ticks a roofed armoured crew stays closed up after small-arms fire
    /// finds her — a bullet off her plate, or a belt aimed at her going
    /// past. Re-armed by every such shot.
    ///
    /// The designer's ruling (2026-09-23) and the historical record: rifle
    /// and machine-gun fire does not break a tank crew's nerve, it makes her
    /// button up, and a buttoned-up crew is nearly blind to infantry close
    /// in — which is what infantry fired at tanks *for*, so the anti-tank
    /// team could close (German and US doctrine alike; a closed-down crew
    /// "cannot see anything within 10 meters", FM 3-23.25). Buttoned up she
    /// finds a concealed enemy at [`Self::buttoned_search_percent`] of her
    /// chance and reacts [`Self::buttoned_reaction_ticks`] later; her nerve
    /// and her aim are untouched. **Zero is the game before**: nobody ever
    /// closes up.
    pub buttoned_ticks: u32,
    /// Percent of her detection chance a buttoned-up crew keeps against a
    /// target that has any concealment — infantry, a scout team — at any
    /// range. 100 is the neutral value. Vehicles in the open are found as
    /// well through periscopes as over the hatch rim, which is what the one
    /// study of closed-hatch acquisition found, so this reads only a target
    /// who is hard to see in the first place.
    pub buttoned_search_percent: i32,
    /// Ticks added to her reaction delay while she is buttoned up.
    pub buttoned_reaction_ticks: u32,
    /// What closing a roofed hull up is worth to the gun that does it, in
    /// substance points a round — the price that lets a crew who can do no
    /// damage to a tank want to fire at her anyway. Spent once a round
    /// (spread over the gun's shots), and only against a hull that is not
    /// already closed up. Whole substance points, because `Balance` is
    /// compared exactly. Zero is the game before: a belt that can do nothing
    /// to a tank is not fired at one.
    pub buttoning_worth: u32,
    /// Points added to [`Self::field_repair_percent`] per point of the
    /// crew's `maintenance` above average.
    ///
    /// The seat that answers for `maintenance` is the driver's, so a tank
    /// that has lost her driver is also a tank nobody can get the tracks back
    /// on — and a stand-in pays [`Self::substitution_penalty`] for the
    /// attempt, exactly as she does for laying the gun.
    pub repair_per_maintenance: i32,
    /// Points of `athletics` above average that buy a foot unit one more
    /// level of climb.
    ///
    /// The designer's ruling of 2026-09-11, and the reason it is a threshold
    /// rather than a percentage: a level is a level, and a foot unit's
    /// allowance is one movement point, so there is nothing here for a
    /// percentage to land on. What it buys is the thing tanks can never have
    /// — a fit platoon goes over the face the rest of the army drives round.
    ///
    /// Foot only, deliberately. What a tank can climb is a fact about her
    /// suspension; what a platoon can climb is a fact about the platoon.
    /// And it only ever *adds*: a clumsy section still gets whatever her
    /// chassis allows, because ground that is passable is passable.
    ///
    /// Zero is the rule's absence.
    pub athletics_per_climb_level: i32,
    /// Percent of her chassis's `concealment` added per point of the crew's
    /// `fieldcraft`.
    ///
    /// A percentage of somebody else's number, like
    /// [`Self::cover_to_hit_percent`]: what hides a platoon is lying still in
    /// the right ground, and the chassis says how much there is to be had.
    /// A crew with no concealment to multiply — every armoured chassis in the
    /// base mod declares zero — gets nothing for it, which is the honest
    /// reading rather than an oversight: fieldcraft does not make a tank
    /// smaller.
    ///
    /// Zero is the rule's absence. It also keeps `search`'s fast path exactly
    /// as fast for a crew hiding behind nothing.
    pub concealment_per_fieldcraft: i32,
    /// Percent of a platoon's damage added per point of the crew's
    /// `small_arms`.
    ///
    /// Charged where the platoon's own strength already is, in `mustered`, so
    /// a section that has lost half its riflemen and a section that shoots
    /// badly arrive at the fire she actually puts out by the same route.
    /// Deliberately not applied to suppression: what frightens a crew is
    /// volume of fire rather than marksmanship, and the volume is the
    /// fraction still standing.
    ///
    /// Zero is the rule's absence.
    pub troops_per_small_arms: i32,
    /// How much worse someone is at a job that is not hers.
    ///
    /// Crews are short-handed far more often than they are complete — the
    /// school has ten cadets and its tanks have four seats each — so somebody
    /// covering an empty gunner's seat is the normal case, not an edge one.
    /// A penalty rather than nothing, because a commander can lay a gun; she
    /// is just not the gunner.
    ///
    /// **Six, the designer's ruling of 2026-09-10**, and the first time this
    /// number was ever asked a question. It is also charged to a cadet
    /// working her own station hurt, so it is the whole price of a crew that
    /// is not what it should be. The `seats` table swept it: at 0 a crew a
    /// seat short wins 50.0% against a control of 50.0, at 2 — the first
    /// draft, which nothing had ever measured — 49.0 against 49.0, at 6
    /// 41.0 against 49.7, at 14 27.8. The two substance points an empty seat
    /// takes with it are worth nothing at all, so this is the *entire* cost
    /// of a short crew, and below 6 the wound system has no teeth on a
    /// battlefield.
    pub substitution_penalty: i32,
    /// How big a target one cadet is when a penetration rolls what it found
    /// inside, on the same scale as a module's `size`. At the default 2
    /// with the standard module set (sizes 3+4+2+1), a four-cadet crew is a
    /// little under half of what there is to hit — which is the CM-shaped
    /// truth of it: most of the inside of a tank is people.
    pub crew_weight: i32,
    /// Ledger points of behind-armor damage per effect roll, rounding up.
    /// At 4, a machine-gun burst into a soft car rolls once and an 88
    /// through a glacis rolls three or four times.
    pub points_per_effect: i32,
    /// Chance an ammunition-rack hit sets the racks off, as a percent,
    /// scaled down by how empty they are: the full figure with full racks,
    /// none with none. Zero is the gentle game where nothing ever burns —
    /// difficulty is a mod.
    pub brewup_percent: i32,
    /// Percentage points of brew-up chance shaved per point of the
    /// vehicle's `safety` stat — wet stowage, in one number. Safety was
    /// already the stat for "what a knocked-out vehicle costs the cadets
    /// inside" at the campaign's fate rolls; this makes it matter while
    /// the shooting is still happening, so a vehicle designed around her
    /// crew is measurably harder to torch, not merely gentler afterwards.
    pub brew_safety_percent: i32,
    /// Percentage points of hit chance gained for shooting downhill.
    ///
    /// One number rather than a per-level slope because what height buys a
    /// gunner is mostly the first level of it: she can see the whole
    /// vehicle instead of whatever the intervening ground has left of it.
    /// Stacking it per elevation step would make a mountain battery
    /// unmissable, which is not what standing above somebody is worth.
    pub downhill_bonus: i32,
    /// What fraction of a terrain's `cover` rating is subtracted from hit
    /// chance, as a percent.
    ///
    /// Cover is declared once per terrain and spends itself in two places:
    /// here, and in how hard the tile is to see into. At the default 50 a
    /// town's cover of 40 is twenty points of accuracy, which is the number
    /// the game has always used — it simply used to be a `/ 2` in
    /// `combat.rs`, where no modder could reach it.
    pub cover_to_hit_percent: i32,
    /// Percentage points of hit chance lost **per hex** the target has
    /// crossed this round.
    ///
    /// A crossing target is the classic gunnery problem — the lead is a
    /// guess and the guess is wrong — and until this existed a vehicle that
    /// spent the round crossing open ground was exactly as hard to hit as
    /// one sitting hull-down and still.
    ///
    /// Per hex rather than a flat charge for having moved at all, and the
    /// first draft was flat. At this game's scale that reads wrong twice
    /// over: a hex is 100 m and a round 60 s, so one hex is a walking pace
    /// and five is thirty km/h, and a flat penalty prices them the same. It
    /// also throws away the only thing that makes a fast chassis' speed a
    /// defence rather than merely a way of arriving sooner — which is the
    /// question the recon car has been losing sixty times a run.
    pub moving_target_per_hex: i32,
    /// Percentage points of hit chance lost **per hex** the attacker has
    /// crossed this round.
    ///
    /// Larger than [`Self::moving_target_per_hex`], and deliberately: laying
    /// a gun from a moving vehicle is harder than tracking a moving one from
    /// a still one, in a period where almost nothing is stabilised. This is
    /// the number that makes halting to shoot a decision — a crew who wants
    /// her round to count stops for it, and pays for the stop in the time
    /// she spends visible on the same piece of ground.
    ///
    /// Uncapped, and the clamp to [`Self::min_hit`] is what stops
    /// it running away: a vehicle at a full gallop can technically still
    /// fire, and technically still will not hit anything.
    pub firing_on_the_move_per_hex: i32,
    /// Percentage points of hit chance lost firing at a tile rather than at
    /// a unit anybody can see.
    ///
    /// Deliberately large. Blind fire is shelling a map reference, and the
    /// only reason it is worth doing at all is that a shell landing on
    /// ground is still a shell landing on ground — which is why artillery,
    /// whose whole trade is exactly that, is the weapon this number is
    /// really about.
    pub blind_penalty: i32,

    /// The floor and ceiling a hit chance is clamped to, as percentages.
    ///
    /// There is no such thing as a certainty or an impossibility on a
    /// battlefield: the best-laid shot can be spoiled and the wildest one can
    /// connect. These were `MIN_HIT`/`MAX_HIT` in `combat.rs`, which made the
    /// one pair of numbers deciding whether luck exists at all the only
    /// gunnery numbers a mod could not touch.
    #[serde(default = "min_hit")]
    pub min_hit: i32,
    #[serde(default = "max_hit")]
    pub max_hit: i32,
    /// Rounds of nothing happening before a battle is called off.
    ///
    /// "Nothing" is accomplishment, not effort — hits, breakages, burnings,
    /// deaths and departures reset the clock and a bounce does not — so this
    /// is how long two forces may fail to hurt each other before the fight is
    /// declared over.
    #[serde(default = "stalemate_rounds")]
    pub stalemate_rounds: u32,
    /// Observer eye height above her own tile surface, in centimetres: a
    /// commander's cupola.
    ///
    /// Centimetres because this block is deliberately all-integer — see the
    /// module doc — so that the arithmetic stays exact and the simulation
    /// stays bit-for-bit reproducible. 250 is 2.5 m, and 250/100.0 is exact
    /// in `f32`.
    #[serde(default = "eye_height")]
    pub eye_height_cm: i32,
    /// How far above the target tile's surface must be visible for the target
    /// to count as seen, in centimetres.
    ///
    /// Deliberately lower than [`Self::eye_height_cm`], so looking out is
    /// fractionally easier than being looked at — which is the whole of what
    /// makes a reverse slope worth taking. **The rule lives in the ratio**, so
    /// a mod raising one should think about the other.
    #[serde(default = "target_height")]
    pub target_height_cm: i32,
    /// The overmatch, as a percent of the armor beaten, at which a
    /// penetration delivers everything it has.
    ///
    /// The pipeline was always written with three outcomes — clean
    /// penetration, partial penetration, bounce — and the outcome engine
    /// shipped with two, so a round that scraped through the plate spent
    /// exactly the same budget inside as one that vastly overmatched it.
    /// That is the flattening the whole no-hit-points model exists to
    /// avoid, one layer further in: it makes the *margin* of a penetration
    /// mean nothing, which is most of what distinguishes a gun that can
    /// just about manage a target from one that eats it.
    ///
    /// At the default 130 a round needs three tenths in hand to do its
    /// worst; below that it is breaking up on the plate and paying
    /// [`Self::partial_penetration_percent`] of its budget, interpolated so
    /// there is no step for a modder to tune against. Setting it to 100 is
    /// the game before this existed — every penetration is clean — which is
    /// the additivity contract.
    pub clean_penetration_percent: i32,
    /// What a round that only just gets through spends inside, as a percent
    /// of its full budget.
    ///
    /// Spall and fragments rather than the whole energy dump: something came
    /// through and it is still dangerous, but the crew are being hit by
    /// pieces of their own armour rather than by the round. Read at exactly
    /// the point of penetration and interpolated up to full at
    /// [`Self::clean_penetration_percent`]. 100 disables the band.
    pub partial_penetration_percent: i32,
    /// The chance, per hundred points of a bystander's `presence`, that a shot
    /// which missed what it was aimed at finds *her* instead.
    ///
    /// A hex is 100 m across and can hold more than one crew. Until stacking
    /// existed a miss was simply a miss, because there was never anybody else
    /// standing there; now that a platoon can share a wood with the carrier
    /// that brought it, a round that goes past the carrier has somewhere else
    /// to end up. The gunner still *aims* — this is not a lottery over the
    /// occupants, it is what happens after her aimed shot has already failed.
    ///
    /// Rolled once per bystander in id order, each at
    /// `stray_percent * presence / 100`, so several bystanders make a stray
    /// likelier without any one of them ever making it certain, and the
    /// arithmetic never needs a cap. **Zero is the game before stacking**: no
    /// rolls happen, a miss is a miss, and the additivity rule is kept without
    /// an `if` in Rust.
    #[serde(default)]
    pub stray_percent: i32,
    /// Chance in 100 that a crew who has an enemy in her field of view
    /// actually picks her out of it, per tick, before anything is subtracted
    /// for the ground or added for the enemy's driving.
    ///
    /// **100 is the game before detection rolls existed** — being looked at
    /// was being seen, and a hard `vision_range` cutoff was the only thing on
    /// the battlefield keeping anything hidden. That is the additivity
    /// contract for this whole rule: at 100, with no terrain declaring
    /// [`crate::data::TerrainDef::concealment`], not one die is rolled and
    /// not one spot moves.
    ///
    /// Deliberately *not* scaled by the spotter's `observation`. What a sharp
    /// crew buys is already priced once, in [`Self::vision`], and a skill
    /// that bought both range and speed of acquisition would be paid for
    /// twice — while the range it bought already shortens
    /// [`Self::detection_at_range_percent`]'s bite at any given distance,
    /// which is the same benefit arriving honestly.
    #[serde(default = "detection_base")]
    pub detection_base: i32,
    /// The share of a crew's reach inside which she simply sees what she is
    /// looking at: no search, no die, no ground worth hiding in.
    ///
    /// **100 is the neutral value**, and it is the second half of the
    /// additivity contract — at 100 nothing is ever faded in, so
    /// [`Self::detection_at_range_percent`] and every terrain's
    /// `concealment` are worth nothing and the spotting pass never reaches
    /// the rng at all.
    ///
    /// It exists because the first draft had no near band and was wrong in a
    /// way that showed up as two dozen failing tests: two tanks three hexes
    /// apart on open grass had an 82% chance of noticing each other, per
    /// tick, which reads as "she probably sees the tank 300 m away in the
    /// open field". Nobody has to *search* for that. What a crew searches is
    /// the far part of her own reach, and everything concealment is worth is
    /// worth out there — a wood at 200 m hides nobody, and the same wood at
    /// 1.5 km hides a company.
    ///
    /// So the whole reduction fades in together, from nothing at this share
    /// of her reach to all of it at the limit. One curve rather than one per
    /// term, because they are all answers to the same question: how much of
    /// what is in front of her can she actually resolve at this distance.
    #[serde(default = "detection_certain")]
    pub detection_certain_percent: i32,
    /// Percentage points of that chance lost at the far edge of the
    /// spotter's reach, faded in from [`Self::detection_certain_percent`].
    ///
    /// This is the number the whole item is about. A hex is 100 m, so a
    /// commander with 20 hexes of vision is claiming to see two kilometres —
    /// which she can, and which is exactly why a cutoff at twenty and
    /// certainty at nineteen reads as a wall rather than as eyesight. Spend
    /// the outer part of a sight radius on *time to find* instead and the
    /// same range becomes a gradient: what is close is known, what is far is
    /// suspected.
    ///
    /// Zero is the flat game where a crew at the limit of her vision finds an
    /// enemy exactly as fast as one at point-blank range.
    #[serde(default)]
    pub detection_at_range_percent: i32,
    /// Percentage points of that chance gained **per hex** the target has
    /// crossed this round.
    ///
    /// Movement is what gives a hidden crew away, and per hex rather than a
    /// flat charge for the same reason [`Self::moving_target_per_hex`] is:
    /// one hex a round is a walking pace and five is thirty km/h, and a flat
    /// term prices them identically. It reads the same `Unit::moved` counter
    /// the gunnery terms do, so a crew who dashed for cover pays for the dash
    /// in being seen as well as in being hit.
    ///
    /// The round's first look therefore sees a field on which nobody has
    /// driven yet, because `moved` is zeroed in `begin_round`. That is the
    /// truth of it rather than an oversight: what she did last round is not
    /// still kicking up dust, and anyone who was found while doing it is
    /// found already — a search buys a *contact*, and keeping one costs
    /// nothing.
    #[serde(default)]
    pub detection_per_hex_moved: i32,
    /// Round-to-round penetration variance, as a percent.
    ///
    /// No two shells leave the same barrel identically, and armor plate is
    /// not uniform either: a fired round's penetration is scaled by a
    /// uniformly random factor in `100 ± pen_scatter` percent before it
    /// meets the plate. This is what makes a marginal shot *marginal* —
    /// sometimes through, sometimes a bounce — rather than a foregone
    /// conclusion the AI can price as certainty. Zero collapses every
    /// matchup to a hard threshold, which is both a legitimate mod choice
    /// and what the deterministic tests set. An integer percent rather than
    /// a float because the analytic chance the AI reads and the roll the
    /// resolver makes must count the same finite outcomes, exactly.
    pub pen_scatter: i32,
}

/// How much the order a crew is driving under wants dead ground: movement
/// points added to every hex of her route that an enemy she knows of can see
/// (`ground::watched` — his sight line, inside his vision range).
///
/// The designer's ruling (2026-09-23): the route is part of the order. A
/// crew sent to *find* the enemy goes the long way round in dead ground,
/// because being found first is the one way to fail a reconnaissance; a crew
/// sent to *take* ground through fire goes straight, because the time a
/// detour costs is time the defender uses. The price is paid by the path
/// finder that drives her (`movement::path_to`) and by the step a long march
/// takes each round (`movement::step_toward`), so the road a crew is ordered
/// down is the road she drives.
///
/// Keyed by the mission's verb (`Mission::verb`), and `march` for a crew on
/// a personal march order. **In `balance`**, not `planner`, because it
/// reaches a human's crew: the driver of a player's scout car picks her own
/// route, and picks it by the same rule as a machine's. Every field defaults
/// to zero, which is the shortest route whatever the order — the game before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RouteExposure {
    pub advance: u32,
    pub assault: u32,
    pub hold: u32,
    pub reconnoitre: u32,
    pub withdraw: u32,
    pub support: u32,
    /// A crew detached on a march of her own (`PersonalOrder::Marching`).
    pub march: u32,
}

impl RouteExposure {
    /// The price for an order named by its verb, or for a personal march.
    /// An unknown verb prices nothing.
    pub fn for_verb(&self, verb: &str) -> u32 {
        match verb {
            "advance" => self.advance,
            "assault" => self.assault,
            "hold" => self.hold,
            "reconnoitre" => self.reconnoitre,
            "withdraw" => self.withdraw,
            "support" => self.support,
            "march" => self.march,
            _ => 0,
        }
    }
}

impl Default for Balance {
    fn default() -> Self {
        Self {
            min_hit: min_hit(),
            max_hit: max_hit(),
            stalemate_rounds: stalemate_rounds(),
            eye_height_cm: eye_height(),
            target_height_cm: target_height(),
            vision_per_observation: 5,
            speed_per_driving: 5,
            reload_per_loading: 0,
            field_repair_percent: 0,
            pivot_ticks: 0,
            route_exposure: RouteExposure::default(),
            buttoned_ticks: 0,
            buttoned_search_percent: 100,
            buttoned_reaction_ticks: 0,
            buttoning_worth: 0,
            repair_per_maintenance: 0,
            retired_speed_per_athletics: None,
            athletics_per_climb_level: 0,
            concealment_per_fieldcraft: 0,
            troops_per_small_arms: 0,
            accuracy_per_gunnery: 3,
            substitution_penalty: 6,
            crew_weight: 2,
            points_per_effect: 4,
            brewup_percent: 60,
            brew_safety_percent: 12,
            downhill_bonus: 10,
            cover_to_hit_percent: 50,
            moving_target_per_hex: 5,
            firing_on_the_move_per_hex: 8,
            clean_penetration_percent: 130,
            partial_penetration_percent: 55,
            blind_penalty: 40,
            pen_scatter: 15,
            // 100, zero, zero: a crew who can see the ground somebody is
            // standing on has found her, at any range, driving or parked.
            // That is precisely the spotting this engine shipped with, which
            // is what makes every detection number below opt-in data rather
            // than a rule Rust imposes.
            detection_base: detection_base(),
            detection_certain_percent: detection_certain(),
            detection_at_range_percent: 0,
            detection_per_hex_moved: 0,
            // Zero, so the Rust default is the game before a hex could hold
            // two crews. The base mod turns it on; a mod that says nothing
            // about stacking never meets the rule.
            stray_percent: 0,
        }
    }
}

impl Balance {
    /// Apply a percent-of-base crew bonus to a vehicle stat.
    ///
    /// Rounds to nearest so a small vehicle still feels the bonus eventually
    /// rather than truncating it away every time, and floors at 1 because a
    /// stat of zero means "cannot see" or "cannot move", which no crew should
    /// be able to inflict on their own vehicle. `points` is signed so that a
    /// future wound model can pass a penalty through the same path.
    pub fn scaled(base: u32, percent_per_point: i32, points: i32) -> u32 {
        let percent = 100 + percent_per_point as i64 * points as i64;
        let scaled = base as i64 * percent;
        // Round half away from zero; `i64` division truncates toward zero, so
        // negatives need the nudge in the other direction.
        let rounded = if scaled >= 0 {
            (scaled + 50) / 100
        } else {
            (scaled - 50) / 100
        };
        rounded.max(1) as u32
    }

    /// How far a skill level sits from ordinary.
    ///
    /// Every crew effect is measured from [`crate::data::AVERAGE`] rather than
    /// from zero, and this is the change of meaning that matters most in the
    /// stat rework: an ordinary crew now changes nothing, and a *poor* one is
    /// a penalty. Under the old 0-5 stats every crew member could only help,
    /// so nobody was ever a liability and it never mattered who rode in which
    /// tank.
    fn margin(level: i32) -> i32 {
        level - crate::data::AVERAGE
    }

    /// Vision range for a vehicle with `base` sight and a crew observing at
    /// `observation`.
    pub fn vision(&self, base: u32, observation: i32) -> u32 {
        Self::scaled(base, self.vision_per_observation, Self::margin(observation))
    }

    /// How long this crew takes to reload a weapon whose listed reload is
    /// `base` ticks.
    ///
    /// The one place the loader reaches the gun, read by the resolver's
    /// cooldown and by the cadence the currency is quoted in, so the two
    /// cannot drift.
    pub fn reload(&self, base: u32, loading: i32) -> u32 {
        Self::scaled(base, -self.reload_per_loading, Self::margin(loading))
    }

    /// The chance in 100 that a crew this good at `maintenance` gets one
    /// broken thing working again this round, clamped to a real probability.
    pub fn repair_chance(&self, maintenance: i32) -> i32 {
        (self.field_repair_percent + self.repair_per_maintenance * Self::margin(maintenance))
            .clamp(0, 100)
    }

    /// How steep a face a foot unit whose chassis allows `base` levels and
    /// whose crew is this good at `athletics` can actually take.
    pub fn climb(&self, base: i32, athletics: i32) -> i32 {
        if self.athletics_per_climb_level <= 0 {
            return base;
        }
        base + (Self::margin(athletics) / self.athletics_per_climb_level).max(0)
    }

    /// How much of her chassis's `concealment` a crew this good at
    /// `fieldcraft` actually gets.
    pub fn concealed(&self, base: u32, fieldcraft: i32) -> u32 {
        // [`Self::scaled`] floors at one, because a vehicle with no movement
        // points or no vision is a bug rather than a vehicle. Concealment is
        // not like that: zero is what every armoured chassis declares, and it
        // has to stay zero or `fog::search`'s fast path stops firing and
        // every tank on the field starts rolling to be found. The floor is
        // right where it is and wrong here, which is why this asks first.
        if base == 0 {
            return 0;
        }
        Self::scaled(
            base,
            self.concealment_per_fieldcraft,
            Self::margin(fieldcraft),
        )
    }

    /// What a platoon's listed damage becomes in the hands of riflemen this
    /// good.
    pub fn marksmanship(&self, base: u32, small_arms: i32) -> u32 {
        // Nothing times anything is nothing: a remnant whose riflemen have
        // rounded away puts out no fire, and [`Self::scaled`]'s floor of one
        // would hand her a point back. Same reason [`Self::concealed`] asks.
        if base == 0 {
            return 0;
        }
        Self::scaled(base, self.troops_per_small_arms, Self::margin(small_arms))
    }

    /// Movement allowance for a vehicle with `base` points and a crew driving
    /// at `driving`.
    pub fn speed(&self, base: u32, driving: i32) -> u32 {
        Self::scaled(base, self.speed_per_driving, Self::margin(driving))
    }

    /// What share of its budget a penetration at this `overmatch` — the
    /// rolled penetration over the armor it beat — actually spends inside.
    ///
    /// One function, called by the resolver and by the analytic twin the AI
    /// plans on, for the same reason `penetration_chance` and
    /// `penetration_roll` enumerate one comparison: a planner that prices a
    /// marginal shot as a clean one is a planner being lied to, and this is
    /// the arithmetic both of them have to agree about.
    ///
    /// Linear between the two numbers rather than a step, so there is no
    /// cliff for a marginal shot to sit on and no threshold a modder has to
    /// discover by bisection.
    pub fn penetration_share(&self, overmatch: f32) -> f32 {
        let clean = self.clean_penetration_percent.max(100) as f32 / 100.0;
        let partial = self.partial_penetration_percent.clamp(0, 100) as f32 / 100.0;
        if overmatch >= clean || clean <= 1.0 {
            return 1.0;
        }
        if overmatch <= 1.0 {
            return partial;
        }
        partial + (1.0 - partial) * (overmatch - 1.0) / (clean - 1.0)
    }

    /// How many percentage points of hit chance a terrain's `cover` rating
    /// takes off a shot into it.
    ///
    /// Integer arithmetic, truncating, because this feeds a hit chance that
    /// has to be reproducible bit for bit — and because the divisor it
    /// replaced truncated too, which is what let this become data without
    /// moving a single number.
    pub fn cover_against_accuracy(&self, cover: i32) -> i32 {
        cover * self.cover_to_hit_percent / 100
    }

    /// Chance in 100 that one crew picks one enemy out of her own field of
    /// view this tick.
    ///
    /// Everything the caller has already settled is geometry: the enemy is on
    /// ground this crew can see, and inside whatever range her
    /// `VehicleDef::concealment` leaves the crew. This is the other half —
    /// how long it takes to find her there — and it is three terms over one
    /// curve:
    ///
    /// - `distance` against `range` fades the curve in, from nothing at
    ///   [`Self::detection_certain_percent`] of the reach to all of it at the
    ///   limit;
    /// - what fades in is the ground's own `concealment` plus
    ///   [`Self::detection_at_range_percent`], because both are answers to
    ///   the same question — how much of what is in front of her she can
    ///   actually resolve at this distance;
    /// - `moved` is hexes crossed this round, and buys the searcher
    ///   [`Self::detection_per_hex_moved`] a hex at *any* range. Driving is
    ///   not something distance forgives.
    ///
    /// Clamped to 0..=100 rather than floored above zero. A mod whose numbers
    /// reach zero is describing ground somebody can lie in indefinitely,
    /// which is a legitimate thing to describe — she is still given away the
    /// moment she drives or fires, both of which reach this from outside.
    ///
    /// Integer arithmetic throughout, like every other number in this block,
    /// because a spot is rolled against it and replays have to agree bit for
    /// bit about what was rolled.
    pub fn detection_chance(&self, concealment: i32, distance: i32, range: u32, moved: u32) -> i32 {
        // How far out she is as a percentage of the reach, so a scout with
        // twice the eyes of a tank spends this term half as fast at the same
        // distance. A range of zero can only be looked at from zero hexes
        // away, and calling that the far edge is both harmless and the
        // conservative reading.
        let reach = match range {
            0 => 100,
            range => (distance.max(0) * 100 / range as i32).clamp(0, 100),
        };
        // The near band, in hundredths so the fade keeps its resolution
        // through integer division. At 100 there is no far band at all and
        // nothing is ever faded in, which is the neutral game.
        let certain = self.detection_certain_percent.clamp(0, 100);
        let fade = if certain >= 100 {
            0
        } else {
            ((reach - certain).max(0) * 100) / (100 - certain)
        };
        (self.detection_base - (concealment + self.detection_at_range_percent) * fade / 100
            + self.detection_per_hex_moved * moved as i32)
            .clamp(0, 100)
    }

    /// Hit chance change, in percentage points, for a crew shooting at
    /// `gunnery`. Negative for a crew worse than ordinary.
    pub fn accuracy(&self, gunnery: i32) -> i32 {
        Self::margin(gunnery) * self.accuracy_per_gunnery
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gifted_crew_is_worth_a_quarter_of_the_vehicle() {
        let balance = Balance::default();
        // Skill 15 is five above ordinary. The recon car sees 20 and the tank
        // destroyer 10; the same scout is worth five hexes in one and three in
        // the other, which is the point of scaling against the base rather
        // than adding a flat bonus.
        assert_eq!(balance.vision(20, 15), 25);
        assert_eq!(balance.vision(10, 15), 13); // 12.5, rounded to nearest
        assert_eq!(balance.speed(5, 15), 6); // 6.25 -> 6, i.e. 30 -> 36 km/h
        assert_eq!(balance.speed(3, 15), 4); // the heavy tank finally notices
    }

    #[test]
    fn an_ordinary_crew_changes_nothing() {
        // Effects are measured from AVERAGE, so a competent-but-unremarkable
        // crew leaves the vehicle exactly as designed.
        let balance = Balance::default();
        assert_eq!(balance.vision(12, crate::data::AVERAGE), 12);
        assert_eq!(balance.speed(7, crate::data::AVERAGE), 7);
        assert_eq!(balance.accuracy(crate::data::AVERAGE), 0);
    }

    #[test]
    fn cover_costs_a_shot_exactly_what_the_old_divisor_charged() {
        // The three hit-chance constants moved out of `combat.rs` at their
        // shipped values, and this is the arithmetic half of the proof that
        // nothing moved with them: the divisor truncated toward zero and so
        // does the percentage, for every cover rating a terrain can declare.
        let balance = Balance::default();
        for cover in 0..=100 {
            assert_eq!(
                balance.cover_against_accuracy(cover),
                cover / 2,
                "cover {cover}"
            );
        }
        assert_eq!(balance.downhill_bonus, 10);
        assert_eq!(balance.blind_penalty, 40);
    }

    #[test]
    fn a_mod_that_names_no_cover_rule_still_has_one() {
        // Every field here is `#[serde(default)]` through the struct-level
        // attribute, so a mod written before these numbers existed inherits
        // the game it was tuned against rather than a zeroed one — in which
        // cover would buy nothing and blind fire would be free.
        let balance: Balance = serde_json::from_str("{}").expect("empty block");
        assert_eq!(balance, Balance::default());
    }

    #[test]
    fn a_round_with_margin_in_hand_does_its_worst_and_a_marginal_one_does_not() {
        let balance = Balance::default();
        let floor = balance.partial_penetration_percent as f32 / 100.0;
        let clean = balance.clean_penetration_percent as f32 / 100.0;

        // The two ends, and they are the whole statement: beating a plate by
        // nothing spends the floor, beating it by the clean margin spends
        // everything, and there is no way to spend more than everything.
        assert!((balance.penetration_share(1.0) - floor).abs() < 1e-6);
        assert!((balance.penetration_share(clean) - 1.0).abs() < 1e-6);
        assert!((balance.penetration_share(10.0) - 1.0).abs() < 1e-6);

        // Monotone in between, with no step for a marginal shot to sit on.
        let mut previous = 0.0;
        for step in 0..=20 {
            let overmatch = 1.0 + (clean - 1.0) * step as f32 / 20.0;
            let share = balance.penetration_share(overmatch);
            assert!(
                share >= previous - 1e-6,
                "share must never fall as the margin grows: {share} after {previous}"
            );
            assert!((floor - 1e-6..=1.0 + 1e-6).contains(&share));
            previous = share;
        }
    }

    #[test]
    fn a_band_that_starts_at_parity_is_the_game_without_one() {
        // Additivity: `clean_penetration_percent: 100` says every way through
        // is a clean way through, which is what the outcome engine did before
        // the band existed — and needs no `if` in Rust to arrange.
        let balance = Balance {
            clean_penetration_percent: 100,
            partial_penetration_percent: 55,
            ..Balance::default()
        };
        for overmatch in [1.0, 1.01, 1.5, 4.0] {
            assert_eq!(
                balance.penetration_share(overmatch),
                1.0,
                "overmatch {overmatch}"
            );
        }
    }

    #[test]
    fn a_poor_crew_is_a_liability() {
        // The property the old 0-5 stats could not express: every crew member
        // could only ever help, so it never mattered who rode in which tank.
        let balance = Balance::default();
        assert!(balance.vision(20, 6) < 20, "a bad observer sees less");
        assert!(balance.speed(6, 6) < 6, "a bad driver is slower");
        assert!(balance.accuracy(6) < 0, "a bad gunner shoots worse");
    }

    #[test]
    fn a_stat_never_scales_to_zero() {
        // A wound model will eventually pass negative points through here.
        assert_eq!(Balance::scaled(4, -50, 5), 1);
        assert_eq!(Balance::scaled(1, -100, 5), 1);
    }
}

/// How long a crew takes to act.
///
/// A round is twelve ticks, orders are fixed at the start, and the world
/// changes while it resolves. A machine begins executing at tick zero. A
/// person takes a moment — to hear the order, to understand it, to get moving
/// — and that moment is the difference between a unit and a crew.
///
/// This is a data block for a reason beyond tuning: **difficulty is a mod in
/// this project**, and somebody who wants orders that simply happen should get
/// that by loading content, not by the engine carrying a branch. Setting
/// `base_ticks` and `max_ticks` to zero makes every crew instantaneous and
/// this whole system disappears without an `if` anywhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReactionRules {
    /// Which skill is consulted. Data, so a mod can decide that reacting is a
    /// matter of drill, or of nerve, or of nothing at all.
    pub skill: String,
    /// Ticks an ordinary crew takes before acting.
    pub base_ticks: u32,
    /// How many points above average shave one tick off. Below average adds
    /// them back at the same rate.
    pub levels_per_tick: i32,
    /// The slowest anyone can be, however bad they are.
    pub max_ticks: u32,
}

impl Default for ReactionRules {
    fn default() -> Self {
        Self {
            skill: "reactions".into(),
            base_ticks: 2,
            levels_per_tick: 4,
            max_ticks: 5,
        }
    }
}

impl ReactionRules {
    /// Ticks this crew waits before acting on its orders.
    ///
    /// Saturating rather than wrapping, and clamped both ends: a brilliant
    /// crew reaches zero and stops, and a dreadful one stops at `max_ticks`
    /// rather than spending the whole round frozen.
    pub fn delay(&self, level: i32) -> u32 {
        if self.max_ticks == 0 {
            return 0;
        }
        let margin = level - crate::data::AVERAGE;
        let shaved = if self.levels_per_tick == 0 {
            0
        } else {
            margin / self.levels_per_tick
        };
        (self.base_ticks as i32 - shaved).clamp(0, self.max_ticks as i32) as u32
    }
}

#[cfg(test)]
mod reaction_tests {
    use super::*;

    #[test]
    fn a_quicker_crew_acts_sooner() {
        let rules = ReactionRules::default();
        assert_eq!(rules.delay(crate::data::AVERAGE), 2, "ordinary");
        assert!(rules.delay(18) < rules.delay(crate::data::AVERAGE));
        assert!(rules.delay(4) > rules.delay(crate::data::AVERAGE));
    }

    #[test]
    fn nobody_waits_forever_and_nobody_acts_before_they_are_told() {
        let rules = ReactionRules::default();
        assert_eq!(rules.delay(100), 0, "a genius still cannot act early");
        assert_eq!(
            rules.delay(-100),
            rules.max_ticks,
            "and a fool is not frozen"
        );
    }

    #[test]
    fn a_gentle_mod_turns_the_whole_system_off_with_data() {
        // The property that matters for difficulty-as-mods: no branch in Rust
        // is needed to get orders that simply happen.
        let rules = ReactionRules {
            base_ticks: 0,
            max_ticks: 0,
            ..ReactionRules::default()
        };
        for level in [-20, 0, crate::data::AVERAGE, 30] {
            assert_eq!(rules.delay(level), 0, "level {level} should be instant");
        }
    }
}
