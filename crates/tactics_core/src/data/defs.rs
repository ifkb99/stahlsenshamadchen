//! Serde definitions for everything a mod can declare.
//!
//! Every definition carries a string `id` that other definitions reference.
//! Mods loaded later (in dependency order) may override earlier definitions
//! by re-declaring the same id.

use super::Scale;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// How a unit traverses terrain. Terrain definitions price movement per
/// class; a missing entry means that class cannot enter the tile at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovementClass {
    Foot,
    Wheeled,
    Tracked,
    Boat,
    Air,
}

impl MovementClass {
    /// Every class there is, so a per-class table can be built by walking
    /// them rather than by remembering to add a row.
    pub const ALL: [MovementClass; 5] = [
        MovementClass::Foot,
        MovementClass::Wheeled,
        MovementClass::Tracked,
        MovementClass::Boat,
        MovementClass::Air,
    ];

    /// This class's slot in such a table.
    ///
    /// An exhaustive match rather than a cast, so adding a class fails to
    /// compile here instead of quietly indexing past the end of somebody's
    /// array. [`Self::ALL`] is checked against it in the tests below.
    pub const fn index(self) -> usize {
        match self {
            MovementClass::Foot => 0,
            MovementClass::Wheeled => 1,
            MovementClass::Tracked => 2,
            MovementClass::Boat => 3,
            MovementClass::Air => 4,
        }
    }
}

/// A person. Rides in vehicles, has a face and a name.
///
/// Characters are people; vehicles are hardware, and a unit on the battlefield
/// is always a crew inside one. What she can *do* is not written here — see
/// [`crate::data::SkillDef`] — only who she is and what she has been taught.
/// `Default` is a nameless cadet who is average at everything, which is what
/// the battle spawns into seats a scenario left empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CharacterDef {
    pub id: String,
    pub name: String,
    /// Temperament, by core id. Anything unnamed sits at
    /// [`crate::data::AVERAGE`], so a definition states only what makes her
    /// unusual.
    #[serde(default)]
    pub cores: std::collections::HashMap<String, i32>,
    /// What she has been taught, by skill id. A skill she is not listed for is
    /// untrained and falls back to her cores at a penalty — not to zero.
    #[serde(default)]
    pub skills: std::collections::HashMap<String, i32>,
    /// Trait ids she starts with. The rest she earns.
    #[serde(default)]
    pub traits: Vec<String>,
    /// Asset-relative path to a portrait image, e.g. `mods/base/portraits/anka.png`.
    #[serde(default)]
    pub portrait: Option<String>,
    #[serde(default)]
    pub bio: String,
}

/// Directional armor. Which facing an incoming shot strikes is derived from
/// the attack direction relative to the target's facing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArmorSpec {
    pub front: i32,
    pub side: i32,
    pub rear: i32,
}

/// The facing arc a shot lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmorFacing {
    Front,
    Side,
    Rear,
}

impl ArmorSpec {
    pub fn value(&self, facing: ArmorFacing) -> i32 {
        match facing {
            ArmorFacing::Front => self.front,
            ArmorFacing::Side => self.side,
            ArmorFacing::Rear => self.rear,
        }
    }
}

fn default_max_climb() -> i32 {
    1
}

/// Movement capability of a vehicle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MovementSpec {
    pub class: MovementClass,
    /// Movement points per turn.
    pub points: u32,
    /// Maximum elevation difference (up or down) the vehicle can cross in a
    /// single step.
    #[serde(default = "default_max_climb")]
    pub max_climb: i32,
}

/// A vehicle chassis: the hardware half of a battlefield unit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VehicleDef {
    pub id: String,
    pub name: String,
    /// Asset-relative sprite path; placeholder art is generated when absent.
    #[serde(default)]
    pub sprite: Option<String>,
    /// Freeform classification tag ("light_tank", "artillery", ...).
    #[serde(default)]
    pub class: String,
    /// **Deprecated and unread.** The hit-point pool this used to size was
    /// removed by the ballistics rewrite: what a vehicle can lose now is
    /// her crew and her modules. Optional so new content can omit it;
    /// tolerated so old content keeps validating. Delete it from data at
    /// leisure.
    #[serde(default)]
    pub max_hp: i32,
    pub movement: MovementSpec,
    pub armor: ArmorSpec,
    /// Base vision range in hexes, before crew bonuses.
    pub vision_range: u32,
    /// Weapon definition ids, in display order. Index 0 is the main gun.
    pub weapons: Vec<String>,
    /// Crew roles this vehicle needs ("commander", "driver", "gunner", ...).
    #[serde(default)]
    pub crew_slots: Vec<String>,
    /// The radio set this vehicle carries, by [`RadioDef`] id.
    ///
    /// `None` falls back to the command block's symmetric `radius`, so
    /// content written before radios were things keeps its game. A set that
    /// cannot send ([`RadioDef::send`] absent) is the historical
    /// receive-only fit: she hears her orders and files nothing. The field
    /// lives on the vehicle because a radio is a thing a vehicle carries,
    /// and things a vehicle carries can one day be hit.
    #[serde(default)]
    pub radio: Option<String>,
    /// How survivable this vehicle is for the cadets inside it, 0-5.
    ///
    /// Separate from armour on purpose: armour decides whether the vehicle
    /// dies, safety decides what that costs its crew. A thinly armoured car
    /// everyone can jump out of and a heavy tank whose ammunition sits under
    /// the turret floor are different problems, and the roster is where the
    /// difference shows up. See [`crate::roster::resolve_crew_fate`].
    #[serde(default = "default_safety")]
    pub safety: i32,
    /// What this vehicle carries in its racks: [`crate::data::AmmoDef`] id to rounds
    /// aboard.
    ///
    /// A `BTreeMap` rather than a `HashMap`, and not as a matter of taste.
    /// Stowage reaches validation output today and will reach the event
    /// stream the moment a shot spends a round, and this project has already
    /// been bitten once by hash iteration order leaking into events — it
    /// stayed invisible for months because a process always agrees with
    /// itself. Ordered by key, the answer cannot depend on the hash seed.
    ///
    /// Empty by default: a vehicle written before ammunition existed carries
    /// nothing, fires exactly as it always did, and validates.
    #[serde(default)]
    pub stowage: std::collections::BTreeMap<String, u32>,
    /// What is aboard that can be broken separately from the vehicle:
    /// [`crate::data::ModuleDef`] ids, in no particular order.
    ///
    /// A list rather than a map because a module is a thing a vehicle either
    /// has or does not; how much of it there is lives on the definition, as
    /// `size`. Naming the same id twice is meaningless — per-module state is
    /// keyed by id — and validation says so.
    ///
    /// **Empty means "assume the usual four", not "an empty hull."** A
    /// vehicle written before modules existed still has a gun, running gear,
    /// racks and a wireless set, and spawning her as a shell with nothing
    /// inside would make every mod in existence wrong at once. The rule is
    /// deliberately mechanical so it can be stated in one sentence: an empty
    /// list means look up [`crate::data::STANDARD_MODULES`] in the registry
    /// and take whichever of them the loaded content actually declares. A mod
    /// that declares no modules at all therefore yields units with nothing
    /// inside to hit, which is precisely today's game.
    #[serde(default)]
    pub modules: Vec<String>,
    /// How hard this unit is to see, as a percentage.
    ///
    /// Not a vision range and not cover: it scales every *spotter's*
    /// effective range against this unit, so a platoon lying in a ditch is
    /// hard to see from anywhere rather than hard to see from one direction.
    /// Standing in real cover counts it a second time, which is what makes
    /// infantry in timber nearly invisible until they move or shoot — and
    /// firing still reveals, because `reveal_to_all` is untouched. An ambush
    /// is spent by springing it.
    ///
    /// Zero by default and zero on every vehicle written so far, which is
    /// exactly today's game: a scaling factor of 1 changes no spotting
    /// decision. Nothing reads this yet — the spotting pass is the next
    /// infantry chunk's business.
    #[serde(default)]
    pub concealment: u32,
    /// How big a target she is, in percentage points of an attacker's hit
    /// chance. Negative is harder to hit.
    ///
    /// The twin of [`Self::concealment`] and easy to confuse with it, so:
    /// concealment is about being *found* and scales a spotter's range;
    /// profile is about being *hit* once found, and reaches the gunner's
    /// arithmetic. A platoon in the open has been seen and is still thirty
    /// men lying in a field, which is why the two have to be separate
    /// numbers rather than one "hard to deal with" stat.
    ///
    /// Deliberately not derived from armour, capacity or class. How big a
    /// thing is and how thick it is are different facts, and inferring one
    /// from the other would take away a mod's ability to describe a lightly
    /// armoured but enormous vehicle — which is most self-propelled
    /// artillery ever built.
    ///
    /// Zero by default and zero on every vehicle in the base mod, so armour
    /// fights armour exactly as it did before this existed; the infantry
    /// chassis are the only ones that declare it, which is what makes their
    /// effect on the balance tables attributable to this one number.
    #[serde(default)]
    pub profile: i32,
    /// How much room she takes up on a hex, against a terrain's `capacity`.
    ///
    /// One by default, so a chassis that says nothing is what it always was.
    /// A hex is 100 m across and a rifle platoon is thirty people lying in a
    /// field, so *count* is the wrong unit for crowding: three platoons in a
    /// wood is a defended wood, and three heavy tanks in it is a traffic jam.
    /// Read it through [`Self::footprint`] rather than the field, which is
    /// where the zero-means-one rule lives.
    #[serde(default)]
    pub footprint: u32,
    /// How many units she lifts, in whole units.
    ///
    /// Deliberately counted in units rather than in seats. A rifle platoon is
    /// already one piece on one hex rather than thirty soldiers, and her lift
    /// is abstracted to match: an armoured personnel carrier that says `1`
    /// carries a platoon, whatever the platoon is written to contain. Zero —
    /// the default, and every vehicle that exists today — means she is not a
    /// transport at all.
    ///
    /// Nothing reads this yet. The passenger state, the mount and dismount
    /// orders and the shared fate of a penetrated carrier are the ride
    /// chunk's, and this is only the number the content writes down.
    #[serde(default)]
    pub capacity: u32,
    /// Requisition cost on the overworld.
    #[serde(default)]
    pub cost: i32,
}

impl VehicleDef {
    /// How much room she takes up. Zero in the file means one, so a chassis
    /// that says nothing is the size it always was and nobody has to write
    /// `"footprint": 1` on every vehicle in a mod.
    pub fn footprint(&self) -> u32 {
        self.footprint.max(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DamageType {
    /// Armor-piercing shells: full armor interaction.
    Kinetic,
    /// High explosive: armor facing counts half.
    Explosive,
    /// Small arms / MG: poor against any armor.
    SmallArms,
}

/// Middling protection, for content written before `safety` existed.
fn default_safety() -> i32 {
    3
}

/// A weapon mounted on a vehicle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WeaponDef {
    pub id: String,
    pub name: String,
    /// Base damage on the AW-style 10-HP scale.
    pub damage: i32,
    /// Penetration vs armor; damage scales by `pen / (pen + armor)`.
    pub penetration: i32,
    pub damage_type: DamageType,
    /// `[min, max]` range in hexes. A min of 2+ means no point-blank shots
    /// (typical for artillery).
    pub range: [u32; 2],
    /// Base hit chance percentage at closest range.
    pub accuracy: i32,
    /// Hit chance lost per hex of distance beyond the first.
    #[serde(default)]
    pub accuracy_falloff: i32,
    /// Indirect-fire weapons may fire without line of sight if a friendly
    /// unit spots the target.
    #[serde(default)]
    pub indirect: bool,
    /// How far a shell from this piece can land from where it was aimed, as
    /// a percentage of the distance it flew.
    ///
    /// Dispersion, and it is a different fact from the flight time beside
    /// it — which is the trap this field has to be read carefully to avoid.
    /// Flight time models the *target* being somewhere else by the time the
    /// round arrives; it says nothing about a target who is standing still,
    /// and until this existed a shell aimed at a parked vehicle hit it with
    /// certainty. This is the gun's own error: the piece is laid on a map
    /// reference by a crew reading a plotting board, and the round goes
    /// where the barrel and the charge send it.
    ///
    /// A percentage of the range flown rather than a flat radius, because
    /// dispersion grows with range, which is the whole reason a battery
    /// registers on a target before firing for effect. At 3% a shell sent
    /// three kilometres can be a hex out.
    ///
    /// Zero by default and only read on the indirect path, so a mod that
    /// says nothing shells exactly as it did — the same additivity contract
    /// [`Self::indirect`] and the ammunition list already keep. Note this is
    /// deliberately **not** a hit roll: a shell that comes down on an
    /// occupied hex still hits what is on it, and charging a blind-fire
    /// accuracy penalty as well would price the same scatter twice.
    #[serde(default)]
    pub dispersion: u32,
    /// Ticks before this weapon can fire again, as a practical aimed rate of
    /// fire rather than a mechanical reload — at the default scale a tick is
    /// 5 s, so 4 is a shot every 20 s.
    ///
    /// `None` means a full round, so a weapon that says nothing keeps the
    /// one-shot-per-round cadence; anything faster fires several times while
    /// a round plays out. This is an `Option` rather than a serde default
    /// because "a full round" is now [`Scale::ticks_per_round`], which lives
    /// in the mod being loaded and is not reachable from a `fn() -> u32`.
    /// Read it through [`Self::reload`].
    #[serde(default)]
    pub reload_ticks: Option<u32>,
    /// [`crate::data::AmmoDef`] ids this weapon can chamber, in the order a loader would
    /// reach for them — index 0 is what she loads when nobody has said
    /// otherwise.
    ///
    /// **This is what combat reads, where it is not empty.** The penetration
    /// pipeline resolves a shot from the round the loader chose; the fields
    /// above are the fallback, and a weapon naming no ammunition is still a
    /// perfectly ordinary weapon that fires from its own
    /// `damage`/`penetration`/`damage_type` rather than one that cannot fire.
    /// That fallback is what made ammunition an additive change to every mod
    /// already written, and it is why it is still here.
    #[serde(default)]
    pub ammo: Vec<String>,
}

impl WeaponDef {
    /// Ticks between shots, resolving the "a full round" default against the
    /// scale in force.
    pub fn reload(&self, scale: &Scale) -> u32 {
        self.reload_ticks.unwrap_or(scale.ticks_per_round).max(1)
    }

    /// How many times this weapon fires while a round plays out.
    ///
    /// Cadence, and the reason it is a number rather than a comparison: every
    /// price in this engine used to be per *shot*, so a machine gun at a shot
    /// every 10 s and an 88 at one every 20 s read identically to a crew
    /// deciding where to stand. They are not identical — the machine gun
    /// fires six times a round and the 88 three — and anything pricing
    /// *ground for a round* has to multiply by this or it is pricing a
    /// different question than it thinks it is.
    ///
    /// Fractional on purpose: a piece that takes longer than a round to load
    /// gets less than one shot, which is the honest reading of a howitzer
    /// firing every 30 s on a 60 s round, and rounding it to zero would make
    /// heavy artillery free to stand in front of.
    ///
    /// Reads [`Self::reload`] rather than the `reload_ticks` field, for the
    /// reason the accessor exists: "a full round" is a fact about the scale
    /// in force and serde's default cannot see the mod being loaded.
    pub fn shots_per_round(&self, scale: &Scale) -> f32 {
        scale.ticks_per_round as f32 / self.reload(scale) as f32
    }
}

/// A radio set: the hardware a vehicle carries onto the net.
///
/// Content rather than a number on the vehicle, because everything the
/// future wants — damage naming a component, refit as a requisition
/// decision, interception caring what model transmits — hangs off a
/// *nameable thing*. The historical pattern this encodes is the receive-only
/// set: early-war line tanks carried receivers (FuG 2, SCR-538) under
/// transmitting leaders, heard everything, and answered with their tracks
/// or a flag. Every set receives; a vehicle that names no set at all falls
/// back to the command block's symmetric radius, so content written before
/// radios were things keeps its game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RadioDef {
    pub id: String,
    pub name: String,
    /// Transmit range in hexes, before the crew's `signals` skill works the
    /// set better or worse. `None` is a receive-only set: she hears the net
    /// and cannot speak on it.
    #[serde(default)]
    pub send: Option<u32>,
    #[serde(default)]
    pub description: String,
}

/// How a side fights, as opposed to how well it thinks.
///
/// Doctrine is the identity knob: two academies running the same planner at
/// the same difficulty should still be recognisable opponents, one massing
/// armour and trading fire while the other cedes ground and shoots from
/// cover. Planners read these weights instead of hard-coding coefficients,
/// which is what lets a mod add a new fighting style without Rust.
///
/// Every field is defaulted, so a doctrine file may set only what it cares
/// about. Weights are multipliers around 1.0 unless noted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DoctrineDef {
    pub id: String,
    pub name: String,
    pub description: String,
    /// 0..=1: how much expected damage outweighs self-preservation.
    pub aggression: f32,
    /// Worth of terrain cover when choosing where to sit.
    pub cover_value: f32,
    /// Worth of high ground.
    pub elevation_value: f32,
    /// Preference for staying near friends and concentrating fire, rather
    /// than spreading across the map.
    pub concentration: f32,
    /// Appetite for advancing into ground nobody has scouted.
    pub scouting: f32,
    /// How much the ground a map declares worth taking is worth to this
    /// doctrine, as a multiplier on each objective's own value.
    ///
    /// Defaulted rather than required because a doctrine written before
    /// objectives existed must not silently become one that ignores them —
    /// a planner that does not care about the objectives loses on points
    /// without ever noticing there was anything to lose.
    #[serde(default = "default_objective_value")]
    pub objective_value: f32,
    /// Willingness to spend indirect fire rather than hold it.
    pub indirect_appetite: f32,
    /// Fraction of starting strength lost before the side looks for a way
    /// out. Reserved until morale and withdrawal exist.
    pub withdraw_threshold: f32,
    /// How much of her own judgment a subordinate acts on while carrying out
    /// an order.
    ///
    /// Read by the goal chooser and nowhere else. A doctrine with initiative
    /// puts the tile her own sweep picked on the candidate list beside the
    /// ground she was sent to, and pays
    /// [`PlannerRules::deviation_cost`](super::PlannerRules::deviation_cost)
    /// `* (1 - initiative)` for taking it — so at 1 she weighs her orders as
    /// one option among two and at 0 there is no second option to weigh, and
    /// the game is the one before subordinate initiative existed.
    ///
    /// **It governs *how* she carries out an order, never *whether* she
    /// believes it.** She may fight from ground she chose short of the map
    /// reference; she may not decide the far objective was the better idea.
    /// That distinction is not a nicety — pricing her own objectives as an
    /// unordered crew prices them makes an order systematically cheaper than
    /// terrain, and it is `Unit::detached` that excuses a unit from her
    /// formation's mission, which is a chain-of-command decision rather than
    /// an evaluator one.
    ///
    /// The field's original note read "acting without orders when out of
    /// contact with a commander", which is the obvious next rule and
    /// deliberately not this one: a cut-off crew keeps the orders she had, and
    /// scaling her initiative by having lost the wire is a second rule with
    /// its own measurement.
    ///
    /// At the shipped `deviation_cost` the base mod's doctrines straddle the
    /// threshold — massed armour at 0.3 drives at the hex she was given while
    /// elastic defence at 0.7 and recon pull at 0.9 stop to fight from ground
    /// of their own — which is what makes the number mean something rather
    /// than sitting outside every value in the mod.
    pub initiative: f32,
    /// Reserved for chain of command: how much a commander devolves
    /// decisions to subordinates.
    pub delegation: f32,
    /// What a round of the march spent in front of an enemy gun is worth
    /// avoiding, in the same points a piece of ground is scored in.
    ///
    /// The goal chooser prices the *road* to a goal, not only the goal: how
    /// long the drive takes over real terrain, and what share of it is walked
    /// where something that can shoot her can see her. This is the price of
    /// that share. Set beside `IMPATIENCE` (0.35 a round) rather than beside
    /// an objective's worth: at 0.8 an elastic-defence crew will spend two
    /// extra rounds going the covered way round, and at 0.2 massed armour
    /// takes the direct road and expects to trade.
    ///
    /// Defaulted to a competent value rather than to zero, for the same
    /// reason [`Self::objective_value`] is: a doctrine written before the
    /// chooser could see a road must not silently become one that marches
    /// down the open one. The off switch for this whole family of terms is
    /// difficulty — at difficulty 1 a commander's foresight is zero and none
    /// of them is read at all, which is exactly the chooser as it stood.
    pub route_caution: f32,
    /// What arriving a round later than the enemy is worth, per round late,
    /// when choosing which ground to march for.
    ///
    /// The cheapest possible answer to "what will the enemy do about it": if
    /// somebody who can already be seen is nearer to that bridge than she is,
    /// she will not have it to herself when she gets there. It is deliberately
    /// a discount and never a veto — a doctrine that refused every contested
    /// objective would be a doctrine that never fights for anything, which is
    /// precisely the stalemate the objectives were introduced to end.
    ///
    /// Fog-honest: only spotted enemies count, so this cannot tell a crew
    /// about ground she has no business knowing is threatened.
    ///
    /// Defaulted like [`Self::route_caution`], and turned off by the same
    /// switch: a commander with no foresight never asks the question.
    pub contest_aversion: f32,
    /// How readily this commander gives a formation a screen instead of a
    /// place in the line.
    ///
    /// The chassis decides *who could* — a formation is eyes when its
    /// vehicles see [`PlannerRules::eyes_ratio_percent`] further than they
    /// shoot — and this decides *whether she would*. A formation is sent on
    /// `Recon` when the share of it that is eyes exceeds `1 - screening`, so
    /// a cautious number wants a formation that is nothing but scouts and a
    /// generous one will screen with anything that has a pair of eyes in it.
    ///
    /// **Zero is the rule's absence**, and it was every doctrine's value
    /// until 2026-09-11, which is why `Mission::Recon` was code the
    /// commander could not issue: the executor drove it, `pull_under_fire`
    /// gated on it and `Formation::latitude_for` handled it, and nothing
    /// ever produced one.
    #[serde(default)]
    pub screening: f32,
}

fn default_objective_value() -> f32 {
    1.0
}

impl Default for DoctrineDef {
    /// The balanced doctrine, used when a side names none. These are the
    /// coefficients the utility planner used before doctrine existed, so
    /// behaviour without a doctrine file is unchanged.
    fn default() -> Self {
        Self {
            id: "balanced".into(),
            name: "Balanced".into(),
            description: "No pronounced style.".into(),
            aggression: 0.6,
            cover_value: 1.0,
            elevation_value: 1.0,
            concentration: 1.0,
            scouting: 1.0,
            objective_value: default_objective_value(),
            indirect_appetite: 1.0,
            withdraw_threshold: 0.7,
            initiative: 0.5,
            delegation: 0.5,
            // Competent, not zero. A doctrine that says nothing about the
            // road is a doctrine with no opinion about it, and the opinion
            // every real one holds is that being shot at on the way there is
            // worth going round for. Set beside `IMPATIENCE` (0.35 a round):
            // at 0.6 the balanced doctrine will spend most of an extra round
            // to take a covered approach, and half a round to reach ground
            // before the enemy does.
            route_caution: 0.6,
            contest_aversion: 0.3,
            screening: 0.0,
        }
    }
}

/// A terrain type occupying one hex tile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainDef {
    pub id: String,
    pub name: String,
    /// Default palette glyph, used by map generators. Map files may remap
    /// glyphs freely in their own palettes.
    #[serde(default)]
    pub glyph: Option<char>,
    /// Asset-relative sprite path; placeholder art is generated when absent.
    #[serde(default)]
    pub sprite: Option<String>,
    /// Placeholder-art color as `#rrggbb`.
    #[serde(default = "default_terrain_color")]
    pub color: String,
    /// Movement point cost to enter, per movement class. Missing class =
    /// impassable for that class.
    #[serde(default)]
    pub move_cost: HashMap<MovementClass, u32>,
    /// Percentage damage reduction granted to occupants.
    #[serde(default)]
    pub cover: i32,
    /// Extra height (in elevation steps) this terrain adds when blocking
    /// line of sight, e.g. forests and buildings.
    #[serde(default)]
    pub vision_block: i32,
    /// How much harder this ground is to *find* somebody in, in percentage
    /// points off a searching crew's chance each tick.
    ///
    /// The twin of [`VehicleDef::concealment`] and deliberately a separate
    /// number from both of its neighbours here. `vision_block` is geometry:
    /// a wood stands between two hexes and the sight line stops. `cover` is
    /// what the ground is worth once the shooting starts. This is neither —
    /// it is how long a crew already inside somebody's field of view can
    /// keep from being picked out of it, which is the difference between
    /// eyesight and detection and the reason a still tank in a wheatfield is
    /// not the same problem as one on a road.
    ///
    /// Not derived from `cover`, because the two come apart in both
    /// directions: a standing crop conceals and stops nothing, a low wall
    /// covers and hides no one. Zero by default, and zero means the ground
    /// gives her nothing — with [`crate::data::Balance::detection_base`] at
    /// 100 that is the game before detection rolls existed, where being
    /// looked at *was* being seen.
    #[serde(default)]
    pub concealment: i32,
    /// On the overworld, armies inside concealing terrain are hidden from
    /// enemies unless adjacent.
    #[serde(default)]
    pub concealing: bool,
    /// Overworld: what holding this ground is worth to a campaign commander,
    /// on the campaign planner's own scale (a capturable tile is worth
    /// `4 + value / 2` against an enemy army's 6). Read by
    /// [`crate::overworld::SimpleOverworldPlanner`] and nothing else.
    ///
    /// It was `income`, and it did two jobs: it paid funds into a treasury
    /// nothing ever spent, and it told the campaign planner which ground to
    /// want. The treasury is gone (2026-09-23); the second job was live, so
    /// it kept its numbers under the name of what it does.
    #[serde(default)]
    pub value: i32,
    /// Retired 2026-09-23, and kept only to be warned about: funds are gone,
    /// and what the campaign planner wants from this ground is
    /// [`Self::value`].
    #[serde(default, rename = "income", skip_serializing)]
    pub retired_income: Option<i32>,
    /// Overworld: whether an army can capture this tile as an objective.
    #[serde(default)]
    pub capturable: bool,
    /// How much [`VehicleDef::footprint`] this hex will hold, or `None` for
    /// the rule this game had before stacking existed: one unit, whatever
    /// size it is.
    ///
    /// `None` rather than `1` on purpose. A terrain that declares nothing has
    /// to behave exactly as it always did, and "one unit of any size" is not
    /// the same statement as "one footprint" — the latter would refuse a
    /// medium tank onto grass the moment anything declared a footprint of 2,
    /// which is an additivity break disguised as a default. So stacking is
    /// opt-in per terrain, and a mod that never mentions capacity never gets
    /// it.
    #[serde(default)]
    pub capacity: Option<u32>,
    /// The battlefield a field battle on this ground is fought on.
    ///
    /// The campaign map is one hex per battle map, so when two armies meet
    /// somewhere the game has to turn "they met in the mountains" into a
    /// battlefield. That used to be the naming convention alone —
    /// `battle_<terrain>` — which is a fine default and a poor contract: it
    /// says a terrain may only ever have one battlefield, that the two must
    /// share a name, and it fails *silently*, dropping the fight onto
    /// whichever battle map iterated first. `deep_forest` did not even reach
    /// `battle_forest`.
    ///
    /// Saying it out loud lets several terrains share ground (a city and a
    /// factory are both fought over the same town), lets one be renamed
    /// without the link going quiet, and — the reason it is worth a field —
    /// makes a missing link something `validate-mods` can name. The
    /// convention still answers for a mod that declares nothing, so this is
    /// additive: a mod written before the field behaves exactly as it did.
    #[serde(default)]
    pub battlefield: Option<String>,
}

fn default_terrain_color() -> String {
    "#808080".to_string()
}

impl TerrainDef {
    /// Movement cost for the class, `None` if impassable.
    pub fn cost_for(&self, class: MovementClass) -> Option<u32> {
        self.move_cost.get(&class).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_movement_class_has_its_own_slot_in_a_per_class_table() {
        // `MovementClass::index` is an exhaustive match, so adding a class
        // fails to compile there; what nothing else checks is that `ALL`
        // lists every one of them and that no two share a slot. A table
        // sized from `ALL` with a gap in it is an array index past the end
        // waiting to happen.
        let mut slots: Vec<usize> = MovementClass::ALL.iter().map(|c| c.index()).collect();
        slots.sort_unstable();
        assert_eq!(slots, (0..MovementClass::ALL.len()).collect::<Vec<_>>());
    }
}
