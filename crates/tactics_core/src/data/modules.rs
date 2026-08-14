//! Modules as content: what lives inside a vehicle and can be broken
//! separately from it.
//!
//! A vehicle today is a single hit-point pool, so everything that can go
//! wrong with her goes wrong at the same rate and in the same direction:
//! twelve points of Löwe, minus some. That is the model the ballistics
//! rewrite exists to end. A penetration does not "do damage", it finds
//! something — the gun, the running gear, the ammunition, the wireless set,
//! or one of the girls — and what it found decides what the rest of the
//! battle looks like for that crew. A tank whose tracks are gone still
//! shoots; a tank whose gun is gone can still drive; a tank whose racks go
//! up is not in the battle any more.
//!
//! **Effect kinds are code; module instances are data.** That split is the
//! designer's ruling and it is the whole reason this is a content type
//! rather than an enum on [`crate::data::VehicleDef`]. There are only ever
//! as many [`ModuleEffect`]s as the outcome engine has branches for, because
//! each one is a branch someone wrote. How many of each a vehicle carries,
//! how big a target each is, and how much punishment it takes are numbers a
//! mod writes — so a second machine gun, a commander's cupola or an
//! auxiliary generator is a JSON entry rather than a patch to Rust.
//!
//! **Nothing in combat reads any of this yet.** This module is the data
//! layer, in exactly the shape [`crate::data::AmmoDef`] arrived in:
//! definitions load, validate and print, units carry per-module state
//! through a save, and the resolver has not heard of it. The outcome chunk
//! is what makes a module something a shot can find, and the proof that this
//! piece changed no behaviour is that `tests/snapshots/event_stream.txt` did
//! not move by a single byte.

use serde::{Deserialize, Serialize};

/// What breaking this module does to the vehicle, and therefore which branch
/// of the outcome engine resolves a hit on it.
///
/// A behaviour selector, not a description: two modules sharing an effect are
/// resolved by the same code against different numbers, which is what lets a
/// mod add hardware without adding Rust. Adding a variant here is adding a
/// consequence to the game and is deliberately a code change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModuleEffect {
    /// The armament. Destroyed, the vehicle goes silent: she is still a
    /// crewed tank sitting on ground the enemy wants, and she cannot make
    /// anyone regret it. The most direct way to take a vehicle out of a
    /// fight without killing anybody in it, and the reason the AI's
    /// withdraw machinery will pull a gun-dead crew without a rule saying
    /// "retreat when disarmed" — a unit that prices every shot at zero is
    /// already a unit with nothing to contribute.
    Gun,
    /// The running gear. Destroyed, she is immobilised where she stands:
    /// still fighting, still dangerous down her own arc, and no longer able
    /// to choose the range or turn her front plate towards a new threat.
    /// Whether that is a reprieve or a death sentence is entirely a question
    /// of where she was standing when it happened, which is what makes it
    /// the most interesting outcome on this list.
    Mobility,
    /// The ammunition stowage. Destroyed, it rolls brew-up — and the roll is
    /// scaled by how much is still aboard, so an emptied tank is measurably
    /// harder to torch than a full one. That quietly makes the ammunition
    /// count a survival statistic as well as an economy, and it is the one
    /// outcome on this list that ends the vehicle and usually the crew.
    Ammo,
    /// The wireless set. Destroyed, she drops off the net: orders stop
    /// reaching her and her reports stop reaching her commander, so she
    /// falls back on her standing mission and the battle drill. The entire
    /// chain-of-command layer starts caring about ballistics for free here,
    /// which was flagged as a future when radio hardware landed.
    Radio,
    /// The soldiers themselves — the abstracted mass of a platoon that is not
    /// one of the named girls. The one effect on this list that is not a
    /// piece of hardware, and it is a module for exactly the reason the
    /// others are: it is a nameable thing inside the unit that a shot can
    /// find and that content decides the size of.
    ///
    /// Three meanings, all of them the infantry chunks' to implement and none
    /// of them read yet:
    ///
    /// - **Interior weight.** Casualty rolls draw from the crew stations and
    ///   the modules together, so a big `size` here is what makes a burst
    ///   find riflemen far more often than it finds the platoon commander.
    ///   "Leaders last" then falls out of arithmetic rather than a rule, and
    ///   a lucky burst can still find her early, which is where the drama is.
    /// - **Firepower.** Every weapon the unit fires scales its damage by the
    ///   troops fraction — hits remaining over [`ModuleDef::toughness`] — so a
    ///   platoon at half strength shoots half as hard and the girls alone are
    ///   nearly harmless. This is why `toughness` on a troops module is
    ///   counted in sections-worth of casualties rather than in the usual one
    ///   or two: it is a strength bar, read as a fraction.
    /// - **Remnant at zero.** At no hits remaining the platoon is not deleted;
    ///   she is a remnant, the girls still aboard the battle, pulled hard
    ///   towards withdrawal by the condition score she has already wrecked.
    ///   Like a mission-killed tank, a shattered platoon is a story rather
    ///   than a removal from the board.
    Troops,
}

impl ModuleEffect {
    /// The spelling a mod writes in JSON, so a tool that prints a roster
    /// prints what the content actually says rather than a Rust identifier.
    /// Kept in step with the `snake_case` rename above by
    /// `every_module_effect_prints_the_name_a_mod_writes`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gun => "gun",
            Self::Mobility => "mobility",
            Self::Ammo => "ammo",
            Self::Radio => "radio",
            Self::Troops => "troops",
        }
    }
}

/// One piece of hardware a vehicle carries, by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// What losing it costs, and which branch of the outcome engine says so.
    pub effect: ModuleEffect,
    /// Relative weight when a penetration rolls what it found inside.
    ///
    /// Not a volume in litres and not a hit box: a share of the lottery a
    /// round entering the fighting compartment draws from. Bigger is hit more
    /// often, and the ratios between the numbers are the only thing that
    /// matters — doubling every module's size on a vehicle changes nothing.
    ///
    /// **Crew stations are weighted alongside these in the same roll**, by
    /// the outcome chunk rather than by this one. That shared draw is the
    /// reason this is a plain number rather than a probability: the odds a
    /// module is what the round found cannot be written down until it is
    /// known who else is standing in the way, and that is a property of the
    /// vehicle and her crew rather than of the module.
    pub size: u32,
    /// How many hits it takes to finish this module off.
    ///
    /// One is the ordinary case: found is broken. A `2` means the first hit
    /// damages and the second destroys, which is how a mod says "this is
    /// redundant, or big enough to keep working with a hole in it" — a
    /// track that has lost a road wheel still turns. What *damaged* costs
    /// versus *destroyed* is the outcome chunk's to decide; this number only
    /// says how much punishment it takes to get there.
    #[serde(default = "default_toughness")]
    pub toughness: u32,
}

/// Most hardware is finished by the hit that finds it. Content written
/// without a `toughness` means the ordinary case, not an indestructible one.
fn default_toughness() -> u32 {
    1
}

/// The module ids the engine assumes when a vehicle declares none of her own.
///
/// A deliberately narrow piece of Rust knowledge, and worth being precise
/// about what it is: these are *ids to look up*, never definitions. The
/// engine has no built-in idea of what a track is or what losing it costs —
/// it only knows that a vehicle written before modules existed almost
/// certainly has these four, and that guessing them is kinder than spawning
/// her as a hull with nothing inside. Every id here that no mod declares is
/// simply skipped, so a mod that ships no modules at all gets units with
/// empty module maps and exactly today's game. See
/// [`crate::data::DataRegistry::modules_for`], which is the only reader.
pub const STANDARD_MODULES: [&str; 4] = ["main_gun", "tracks", "ammo_rack", "radio_set"];
