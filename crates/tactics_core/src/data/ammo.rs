//! Ammunition as content: what a gun puts downrange, rather than a number
//! baked into the gun.
//!
//! A weapon today carries one `damage`, one `penetration` and one
//! `damage_type`, which makes it a single unchanging thing that always does
//! the same to everything. Real gunnery is a choice made by a loader with
//! seconds to make it — armour-piercing at the tank, high explosive at the
//! gun crew — and that choice is the interesting one. So the round becomes a
//! nameable piece of content with its own behaviour, the gun becomes a list
//! of rounds it can chamber, and the vehicle becomes a rack with a finite
//! number of each aboard.
//!
//! **Nothing in combat reads any of this yet.** This module is the data layer
//! of the ballistics rewrite and is deliberately inert: [`AmmoDef`]s load,
//! validate and print, vehicles carry counts through saves, and
//! [`crate::battle::combat`] still resolves every shot from the weapon's own
//! `damage`/`penetration`/`damage_type` exactly as it did before. The
//! penetration pipeline that spends these is the next piece of work, and the
//! proof that this piece changed no behaviour is that
//! `tests/snapshots/event_stream.txt` did not move by a single byte.

use serde::{Deserialize, Serialize};

/// What kind of thing arrives at the target, and therefore which branch of
/// the penetration pipeline resolves it.
///
/// This is a behaviour selector rather than a description: two rounds sharing
/// a class are resolved by the same arithmetic on different numbers, which is
/// what lets a mod add a round without adding Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmmoClass {
    /// A solid shot that defeats armour with its own momentum, and therefore
    /// loses penetration as the air takes its velocity away. The classic
    /// armour-piercing shell, and the only class whose two penetration
    /// entries are normally different.
    Kinetic,
    /// A shaped charge: the jet is formed on impact, so how fast the round
    /// was travelling when it arrived barely matters. HEAT and its
    /// descendants — deadly at any range its gunner can hit at, which is a
    /// very different weapon from a solid shot even at identical paper
    /// penetration.
    Chemical,
    /// A bursting charge. Poor against armour by design and dangerous to
    /// everything that is not armoured, which is what [`AmmoDef::blast`] is
    /// for.
    Explosive,
    /// Bullets. Effectively no armour penetration, fired in bursts rather
    /// than shots, and the reason an unarmoured crew is not safe merely
    /// because the tank facing them has run out of shells.
    SmallArms,
}

impl AmmoClass {
    /// The spelling a mod writes in JSON, so a tool that prints a roster
    /// prints what the content actually says rather than a Rust identifier.
    /// Kept in step with the `snake_case` rename above by
    /// `every_ammo_class_prints_the_name_a_mod_writes`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kinetic => "kinetic",
            Self::Chemical => "chemical",
            Self::Explosive => "explosive",
            Self::SmallArms => "small_arms",
        }
    }
}

/// One kind of round, by name.
///
/// # Units
///
/// **[`Self::penetration`] is in the same abstract units as
/// [`crate::data::ArmorSpec`]** — the small integers already written on every
/// vehicle in the base mod, where the Panther's glacis is `5` and the Löwe's
/// is `8`. It is emphatically *not* millimetres of rolled homogeneous armour,
/// and nothing in this chunk rescales armour to make it so. A round that says
/// `penetration: [6, 5]` is claiming to beat a front-5 Panther at close range
/// and to be marginal against it at the end of its reach, which is the same
/// arithmetic `WeaponDef::penetration` feeds today. Whoever eventually moves
/// this to millimetres has to move `ArmorSpec` in the same commit or every
/// number in the game becomes a lie at once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AmmoDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Which branch of the pipeline resolves a hit by this round.
    pub class: AmmoClass,
    /// Penetration at the firing weapon's minimum range and at its maximum,
    /// linearly interpolated in between by [`Self::penetration_at`].
    ///
    /// Two entries rather than a value and a decay coefficient because the
    /// pair is what a gunnery table actually prints, and because it lets a
    /// round that does not care about range say so in content — a
    /// [`AmmoClass::Chemical`] round declares the same number twice and the
    /// same interpolation then yields a flat curve, with no branch in Rust.
    /// Validation warns when a chemical round declares two different numbers,
    /// since that is a typo far more often than it is a design decision.
    pub penetration: [i32; 2],
    /// How much the round does *after* it is through, as a multiplier.
    ///
    /// Penetrating is not the same as killing: a rifle bullet that finds a
    /// vision slot and a 105 mm shell that gets inside are both penetrations
    /// and only one of them ends the crew. Spent by the pipeline chunk;
    /// nothing reads it yet.
    #[serde(default = "default_post_pen")]
    pub post_pen: f32,
    /// Effect on things the round did not have to penetrate — infantry in the
    /// open, gun crews, the wheels of a soft vehicle. Zero for a solid shot,
    /// which is exactly why an army of tank destroyers cannot hold ground.
    /// Spent by the pipeline chunk; nothing reads it yet.
    #[serde(default)]
    pub blast: i32,
    /// Muzzle velocity in metres per second.
    ///
    /// Data for flight time, which at this scale is invisible for direct fire
    /// — a hex is 100 m and a tick is 5 s, so an 800 m/s shell crosses a
    /// sixteen-hex battlefield in under two seconds and lands inside the tick
    /// that fired it. It is written down now because indirect fire, where a
    /// shell is genuinely in the air across ticks, is the case that will need
    /// it, and because the number is part of what a round *is*.
    pub velocity: u32,
}

/// A round that says nothing about behind-armour effect is ordinary: getting
/// through is the achievement, and the multiplier changes nothing.
fn default_post_pen() -> f32 {
    1.0
}

impl AmmoDef {
    /// Penetration of this round fired `hexes` from the muzzle by a weapon
    /// whose reach is `range`, interpolated linearly between the two declared
    /// entries.
    ///
    /// Clamped at both ends, so a point-blank shot from a weapon with a
    /// minimum range gets the near figure rather than an extrapolated one.
    /// Integer arithmetic throughout: this is on the road to the combat
    /// resolver, and the simulation is bit-for-bit reproducible or it is
    /// nothing.
    pub fn penetration_at(&self, hexes: u32, range: [u32; 2]) -> i32 {
        let [near, far] = self.penetration;
        let span = range[1].saturating_sub(range[0]);
        if span == 0 {
            return near;
        }
        let travelled = hexes.clamp(range[0], range[1]) - range[0];
        near + (far - near) * travelled as i32 / span as i32
    }
}
