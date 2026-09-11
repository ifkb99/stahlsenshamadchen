//! What a battle costs the cadets who fought it, as data rather than as
//! numbers scattered through `roster.rs`.
//!
//! The fate rolls were the last big table of bare integers left in Rust: how
//! likely a kinetic penetration is to hurt somebody, what a point of a
//! vehicle's `safety` is worth against that, how long a wound keeps a cadet
//! out. Every one of them is a thing a designer wants to try three values of
//! — this is the dial between "an armoured skirmish costs nobody anything"
//! and "half the school is in the infirmary by Tuesday" — and the repo's own
//! rule is that a number a modder would want to change does not belong in
//! Rust.
//!
//! Being data also makes the harsh version a mod. A campaign that wants
//! attrition to bite ships a `casualties` block and changes nothing else; the
//! base game's block *is* the gentle default, in the same shape as
//! difficulty.
//!
//! All fields are integers and all day ranges are inclusive `[low, high]`
//! pairs, so the rolls stay exact and the campaign stays reproducible from
//! its seed.

use serde::{Deserialize, Serialize};

/// The casualty table: chances in 100 and recovery times in campaign days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Casualties {
    /// Whether a campaign built from this content is willing to kill its
    /// characters, which is what [`crate::roster::CadetStatus::Dead`] needs
    /// before it is reachable at all.
    ///
    /// Here rather than only in [`crate::roster::CasualtyRules`] because of
    /// the rule this whole file exists to keep: **every harsh system is an
    /// additive rule whose absence is the gentle game**. The base mod ships
    /// this `true` — the shipped campaign kills, which is the designer's
    /// ruling of 2026-09-10 — and a mod that says nothing gets a campaign
    /// where the worst a cadet suffers is a long recovery, exactly as before.
    /// Turning the stakes off is therefore a `casualties` block and not a
    /// line of Rust, which is the whole test.
    ///
    /// [`crate::overworld::OverworldState::from_map`] copies it onto the live
    /// rule, which stays where it was: a campaign holds its own answer and
    /// saves it, so a settings screen can override what the content proposed
    /// without editing anybody's mod.
    pub permadeath: bool,
    /// Chance in 100 that a cadet is hurt at all when her vehicle is
    /// destroyed by a kinetic penetration, before `safety` is applied. The
    /// worst of the three: a long rod through the fighting compartment
    /// sprays the inside of the hull with hot metal.
    pub harm_kinetic: i32,
    /// The same for high explosive, which is likelier to disable the vehicle
    /// than the people in it.
    pub harm_explosive: i32,
    /// The same for small arms, which have barely touched the crew at all by
    /// the time they finish off a vehicle.
    pub harm_small_arms: i32,
    /// The same when the battle recorded no cause — burned out, abandoned,
    /// or a source the sim did not attribute. The middling case on purpose:
    /// an unattributed loss should be neither a free pass nor the worst.
    pub harm_unattributed: i32,
    /// Points of harm chance taken off per point of the vehicle's `safety` —
    /// hatches, layout, where the ammunition lives. At the default 8 a
    /// safety-0 deathtrap is meaningfully worse than a safety-5 one without
    /// either ever reaching certainty.
    pub harm_per_safety: i32,
    /// Floor and ceiling on the harm chance after safety, so no vehicle is
    /// ever perfectly safe and none is ever a certain grave.
    pub harm_floor: i32,
    pub harm_ceiling: i32,
    /// Chance in 100 that a cadet who got out unhurt got out on the wrong
    /// side of the fighting and has to walk back.
    pub adrift_percent: i32,
    /// How many days that walk takes, inclusive.
    pub adrift_days: [u32; 2],
    /// Chance in 100 that a wound is bad enough to be fatal where the
    /// campaign allows it. With permadeath off this is the long-recovery
    /// case instead — the same roll, a gentler consequence.
    ///
    /// This one prices a cadet pulled out of a vehicle that did not come
    /// home. What happens to one found hurt in a seat that did is
    /// [`Self::carried_fatal_percent`], and they were the same number until
    /// the attrition table could ask what each of them cost.
    pub severe_percent: i32,
    /// Chance in 100 that a cadet carried home out of the fight — knocked
    /// out at her station in a vehicle that survived — dies of it, where the
    /// campaign allows anyone to.
    ///
    /// Its own number rather than [`Self::severe_percent`] because the two
    /// price situations the rest of this file is at pains to keep apart. The
    /// wreck case has to guess what became of her from what killed the
    /// vehicle; this one is a cadet somebody carried to a doctor within the
    /// hour, in a tank that drove home. Sharing one probability said those
    /// were equally survivable, which nothing else in the model believes —
    /// and, being one number, it could not be argued with: no sweep could
    /// separate what a wreck costs from what a homecoming does.
    ///
    /// Defaults to the value it was sharing, so content that says nothing is
    /// the game exactly as it was. Zero is the rule's absence: a cadet whose
    /// vehicle came home is never killed by the campaign, whatever else it
    /// allows.
    #[serde(default = "default_carried_fatal")]
    pub carried_fatal_percent: i32,
    /// Points off [`Self::severe_percent`] per point of the best `first_aid`
    /// still working aboard her vehicle, above average.
    ///
    /// **Her crewmates', not her own** (the designer's ruling, 2026-09-11):
    /// she is the one bleeding, so what decides whether a wound is the kind
    /// that buries her is whether anybody beside her knows what to do about
    /// it. It is what finally gives the loader's seat a reason to be filled —
    /// `loading` and `first_aid` are the only two skills that seat answers
    /// for, and until this neither was read by any rule, so leaving a loader
    /// behind was free.
    ///
    /// Severity rather than days, also the designer's: a medic decides
    /// whether a cadet is buried or out for a fortnight, which is the stake
    /// the muster screen was given teeth for. Zero is the rule's absence.
    pub severe_per_aid: i32,
    /// The same, for [`Self::carried_fatal_percent`]: what her crewmates buy
    /// a cadet carried home in a vehicle that came back.
    ///
    /// Two numbers rather than one for the reason the two chances they reduce
    /// are two numbers — "dragged out of a fire" and "carried home" are not
    /// one situation, and a mod should be able to say that a doctor within
    /// the hour is worth more than a friend with a field dressing.
    pub carried_fatal_per_aid: i32,
    /// Days out for a severe wound, inclusive.
    pub severe_days: [u32; 2],
    /// Days out for an ordinary one.
    pub light_days: [u32; 2],
    /// Days out for a cadet carried home out of the fight — knocked out at
    /// her station in a vehicle that survived. Worse than an ordinary wound
    /// and better than a severe one, and *never* fatal without permadeath,
    /// for the reason the whole distinction exists: her tank came home and
    /// somebody got her to a doctor.
    pub carried_days: [u32; 2],
    /// Days out for a cadet who was hurt at her station and kept working.
    /// The lightest case in the game, and the one that used to be thrown
    /// away at the end of every battle.
    pub grazed_days: [u32; 2],
}

impl Default for Casualties {
    fn default() -> Self {
        Self {
            permadeath: false,
            harm_kinetic: 55,
            harm_explosive: 40,
            harm_small_arms: 20,
            harm_unattributed: 40,
            harm_per_safety: 8,
            harm_floor: 5,
            harm_ceiling: 95,
            adrift_percent: 25,
            adrift_days: [1, 3],
            severe_percent: 25,
            carried_fatal_percent: default_carried_fatal(),
            severe_per_aid: 0,
            carried_fatal_per_aid: 0,
            severe_days: [5, 10],
            light_days: [1, 4],
            carried_days: [3, 8],
            grazed_days: [1, 4],
        }
    }
}

/// What [`Casualties::carried_fatal_percent`] was worth before it was its own
/// field: the same roll a wreck's wound is priced by.
fn default_carried_fatal() -> i32 {
    25
}

impl Casualties {
    /// An inclusive day range as a usable one, tolerating a block that names
    /// its bounds the wrong way round rather than panicking on it. A mod
    /// with `[4, 2]` means four to two days and gets two to four; a mod file
    /// is content, and content should be forgiven where forgiving it cannot
    /// be ambiguous.
    pub fn days(range: [u32; 2]) -> std::ops::RangeInclusive<u32> {
        let low = range[0].min(range[1]).max(1);
        let high = range[0].max(range[1]).max(low);
        low..=high
    }
}
