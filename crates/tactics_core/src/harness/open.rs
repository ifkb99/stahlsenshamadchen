//! Battles on open ground: a window onto a whole generated world, with the
//! two forces starting kilometres apart (REIMAGINED.md, *Flanks*).
//!
//! `harness::generated` fights on one battle map, and its arena deployment
//! puts the forces 18 hexes apart — inside the heavy gun's reach and a
//! round's drive from the 75's — so a battle is decided by the shooting
//! before a manoeuvre could be worth the minutes it costs. Here the world
//! is several campaign hexes across and the battle is fought on a window
//! onto it ([`World::window_onto`](crate::world::World::window_onto)), which
//! loads the ground round wherever the crews are and forgets the rest, so
//! nothing confines them to one map. Each world draws how far apart the
//! forces start, which way they face each other, and where between them
//! the fight is centred; the forces keep the arena's frontage, moved apart
//! along their axis.
//!
//! What is worth fighting for is read off the ground by the rule
//! `generated` uses, scaled to the distance: the highest ground near the
//! middle, and the highest on each flank in the band between the two
//! forces — never beside one force's start line, or the objective is a gift.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::arena::Arena;
use super::generated::{deploy, scenario_between};
use crate::Hex;
use crate::data::DataRegistry;
use crate::map::Scenario;
use crate::world::chunk_centre;
use crate::worldgen::GeneratedWorld;

/// How big the world is and how far apart the forces start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenLayout {
    /// The world's radius in campaign hexes (each one battle map across).
    pub world_radius: u32,
    /// The least distance between the two forces' start lines, in hexes.
    pub apart_min: u32,
    /// The most.
    pub apart_max: u32,
    /// How many fields (each fought twice, once a seat) are laid on one
    /// world before the next is made. Making a world is most of the cost of
    /// a battle on open ground — a quarter of a rollout's time at one field
    /// a world — and the start lines, the bearing and the middle are drawn
    /// per field, so a world fought over sixteen times is sixteen different
    /// problems on the same country.
    pub fields_per_world: u32,
}

impl Default for OpenLayout {
    /// Three to six kilometres apart on a world seven campaign hexes
    /// across: past every gun's reach at the start (the 88 reaches 16), so
    /// the approach is a phase of its own.
    fn default() -> Self {
        Self {
            world_radius: 3,
            apart_min: 30,
            apart_max: 60,
            fields_per_world: 16,
        }
    }
}

/// A world, what is worth fighting for on it, and where the forces start.
pub struct OpenField {
    pub world: Arc<GeneratedWorld>,
    pub scenario: Scenario,
    /// Pairs of (side 0's hex, side 1's hex), in the arena's order.
    pub deployment: Vec<(Hex, Hex)>,
    /// How far apart the two start lines were drawn, in hexes.
    pub apart: u32,
}

/// SplitMix64, written out so a world's draws never depend on `std::hash`.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The worlds open fields are laid on, each made once and shared by every
/// field that asks for it, from any thread: the first to ask makes it and
/// the rest wait for that one rather than making their own.
#[derive(Default)]
pub struct Worlds {
    made: Mutex<BTreeMap<u64, Arc<WorldSlot>>>,
}

/// One world: made once, by whoever asks first, or `None` if it cannot be.
type WorldSlot = OnceLock<Option<Arc<GeneratedWorld>>>;

impl Worlds {
    /// World `seed` at `layout`'s size, made on first asking.
    pub fn get(
        &self,
        reg: &DataRegistry,
        layout: OpenLayout,
        seed: u64,
    ) -> Option<Arc<GeneratedWorld>> {
        let slot = self
            .made
            .lock()
            .expect("nobody panics holding the list")
            .entry(seed)
            .or_default()
            .clone();
        slot.get_or_init(|| {
            let rules = crate::data::WorldGen {
                radius: layout.world_radius,
                ..reg.worldgen.clone()?
            };
            let chunk_radius = reg.scale.battle_map_radius();
            GeneratedWorld::with_rules(rules, chunk_radius, seed)
                .ok()
                .map(Arc::new)
        })
        .clone()
    }
}

/// Open field number `index`, with `arena`'s forces laid out on it by
/// `layout`: on world `index / fields_per_world`, from `worlds`. `None` when
/// the registry has no world generator, the world cannot be made, or a
/// start hex finds no ground a tank can stand on.
pub fn field(
    reg: &DataRegistry,
    arena: &Arena,
    worlds: &Worlds,
    index: u64,
    layout: OpenLayout,
) -> Option<OpenField> {
    let world = worlds.get(reg, layout, index / layout.fields_per_world.max(1) as u64)?;
    let chunk_radius = world.chunk_radius;
    let seed = index;

    // The draws, per field: how far apart, which way, and where the middle
    // is (within two battle maps' radius of the world's centre, so both
    // start lines and the ground between them are well inside the world,
    // and the fields on one world spread over most of it).
    let span = layout.apart_max.saturating_sub(layout.apart_min) as u64 + 1;
    let apart = layout.apart_min + (mix(seed) % span) as u32;
    let turns = (mix(seed ^ 0x5EED_0001) % 6) as u32;
    let offsets: Vec<Hex> = Hex::ZERO.range(2 * chunk_radius).collect();
    let offset = offsets[(mix(seed ^ 0x5EED_0002) % offsets.len() as u64) as usize];
    let middle = chunk_centre(Hex::ZERO, chunk_radius) + offset;

    // The arena's deployment, each side moved bodily along the axis so the
    // start lines are `apart` hexes apart and the frontage is the arena's.
    let centre = arena.centre_hex();
    let pairs = arena.deployment();
    let n = pairs.len().max(1) as i32;
    let west = pairs.iter().fold(Hex::ZERO, |a, (w, _)| a + *w - centre);
    let west = Hex::new(west.x / n, west.y / n);
    let was = (2 * west.ulength() as i32).max(1);
    let stretch = |v: i32| (v * (apart as i32 - was)).div_euclid(was);
    let push = Hex::new(stretch(west.x), stretch(west.y));
    let lay = |hex: Hex, push: Hex| middle + (hex - centre + push).rotate_cw(turns);
    let wants: Vec<(Hex, Hex)> = pairs
        .iter()
        .map(|(w, e)| (lay(*w, push), lay(*e, -push)))
        .collect();
    let ground = |h: Hex| world.tile(h);
    let deployment = deploy(reg, &ground, middle, wants)?;

    // What is worth having: the crest near the middle, the flanks in the
    // band between the two start lines.
    let mean = |side: fn(&(Hex, Hex)) -> Hex| -> Hex {
        let sum = deployment.iter().map(side).fold(Hex::ZERO, |a, b| a + b);
        Hex::new(sum.x / n, sum.y / n)
    };
    let (ours, theirs) = (mean(|p| p.0), mean(|p| p.1));
    let half = apart / 2;
    let candidates: Vec<Hex> = middle.range(half).collect();
    let scenario = scenario_between(
        reg,
        &ground,
        &candidates,
        middle,
        &deployment,
        (apart / 6).max(4),
        &|h| (h.distance_to(ours) - h.distance_to(theirs)).unsigned_abs() <= half,
    )?;
    Some(OpenField {
        world,
        scenario,
        deployment,
        apart,
    })
}
