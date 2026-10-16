//! A defended sector on a generated world, for training on attack and
//! defence (ASSAULT.md).
//!
//! The defender holds a few zones of key ground spread across a frontage —
//! a village, a crossroads, a rise, a wood's edge, read off the tiles and
//! never declared — and starts in control of them, dug in if the world
//! draws a prepared defence, behind mines, wire and an anti-tank ditch laid
//! on the ground the attacker must cross. The attacker starts kilometres
//! off on the other side and needs most of the zones. Nothing in the layout
//! says how to attack or defend them: the frontage is wider than the
//! defender can cover, the zones are spread so a defender has to choose
//! between breadth and a reserve, and an attacker has to choose where to
//! mass. That is the whole of the brief; the tactics are the learner's.
//!
//! Three sizes, so the training sees platoon fights and company fights
//! ([`SectorSize`]). Both sides buy to the same budget: a doctrine is what
//! a side brings, and whether a force built to attack can defend is one of
//! the things this measures.
//!
//! **Side 0 attacks, side 1 defends**, on every field; a rollout seats the
//! learner on each side in turn. The ground is not mirrored and need not
//! be: the two seatings of one field are the same problem from each end.
//!
//! The draws are SplitMix64 of the field's index, written out, and every
//! choice the ground makes ties to the field's own lot, never a coordinate.

use std::sync::Arc;

use super::open::{OpenLayout, Worlds};
use crate::Hex;
use crate::data::{DataRegistry, ForceDef, MovementClass};
use crate::map::{Minefield, Objective, ObjectiveKind, Scenario};
use crate::world::chunk_centre;
use crate::worldgen::GeneratedWorld;

/// How big a fight: a platoon or two a side, a company, or a reinforced
/// company across a battalion's frontage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectorSize {
    Small,
    Medium,
    Large,
}

impl SectorSize {
    /// Every size, smallest first.
    pub const ALL: [SectorSize; 3] = [SectorSize::Small, SectorSize::Medium, SectorSize::Large];

    /// The size a `--sizes` list names: `small`, `medium` or `large`.
    pub fn named(name: &str) -> Option<Self> {
        match name {
            "small" => Some(Self::Small),
            "medium" => Some(Self::Medium),
            "large" => Some(Self::Large),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }

    /// The shape of a sector this size.
    pub fn shape(self) -> SectorShape {
        match self {
            // About a platoon a side on a kilometre and a bit: the
            // German platoon frontage of 220–550 yd, twice over.
            Self::Small => SectorShape {
                frontage: 12,
                depth: 6,
                zones: (2, 3),
                standoff: (14, 20),
                budget: (40, 60),
                max_vehicles: 8,
                minefields: (1, 2),
                wire: (0, 1),
                ditches: (0, 0),
            },
            // A company: two kilometres, its frontage in the TM-E 30-451
            // band (440–1,100 yd) for the defender stretched to cover the
            // zones it has to.
            Self::Medium => SectorShape {
                frontage: 20,
                depth: 10,
                zones: (3, 4),
                standoff: (18, 26),
                budget: (70, 100),
                max_vehicles: 14,
                minefields: (2, 3),
                wire: (1, 2),
                ditches: (0, 1),
            },
            // A reinforced company on a battalion's three kilometres.
            Self::Large => SectorShape {
                frontage: 32,
                depth: 14,
                zones: (4, 5),
                standoff: (22, 32),
                budget: (120, 170),
                max_vehicles: 26,
                minefields: (3, 5),
                wire: (2, 3),
                ditches: (1, 1),
            },
        }
    }
}

/// The numbers a sector of one size is drawn from. Each pair is a range,
/// drawn per field, both ends included.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectorShape {
    /// How wide the defended front is, in hexes.
    pub frontage: u32,
    /// How deep behind the front its zones may lie, in hexes.
    pub depth: u32,
    /// How many zones.
    pub zones: (u32, u32),
    /// How far in front of the defended front the attacker starts.
    pub standoff: (u32, u32),
    /// Points each side buys with (the same for both).
    pub budget: (i32, i32),
    /// The most vehicles a side may field.
    pub max_vehicles: u32,
    /// Minefields, wire belts and ditches laid by a prepared defender.
    pub minefields: (u32, u32),
    pub wire: (u32, u32),
    pub ditches: (u32, u32),
}

/// How sectors are drawn, beyond their size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectorRules {
    /// Percent of fields on which the defender had the hours to prepare:
    /// dug in, behind mines, wire and ditches. The rest are a hasty
    /// defence, with nothing but the ground.
    pub prepared_percent: u32,
    /// Percent of its strength the attacker can lose before the attack is
    /// called off. Lower than the defender's: every source agrees an
    /// attacker gives up at a half to a third of the losses a defender
    /// takes before she withdraws (ASSAULT.md).
    pub attacker_breaks: u32,
    /// Percent of its strength the defender can lose before she gives up
    /// the position.
    pub defender_breaks: u32,
}

impl Default for SectorRules {
    fn default() -> Self {
        Self {
            prepared_percent: 75,
            attacker_breaks: 50,
            defender_breaks: 70,
        }
    }
}

/// A sector, ready to stage: the world, the scenario on it, where each
/// side wants each vehicle, and the obstacles to dig into the ground.
pub struct Sector {
    pub world: Arc<GeneratedWorld>,
    pub scenario: Scenario,
    /// Each side's wanted hexes, in the order its force deploys.
    pub deployment: [Vec<Hex>; 2],
    /// Tiles the defender has dug or strung: `(hex, terrain id)`, at the
    /// ground's own height.
    pub obstacles: Vec<(Hex, String)>,
    pub size: SectorSize,
    /// The budget both sides bought to.
    pub budget: i32,
    pub prepared: bool,
    /// The zones' anchors, front to back as the defender sees them.
    pub zones: Vec<Hex>,
}

/// Why a field could not be laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SectorError {
    #[error("the registry has no world generator, or the world cannot be made")]
    NoWorld,
    #[error("lane {0} of the front has no ground a tank can hold")]
    NoGround(u32),
}

/// SplitMix64, written out so a sector's draws never depend on `std::hash`.
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A draw from `range`, both ends included, by stream `salt` of `seed`.
fn draw(seed: u64, salt: u64, range: (u32, u32)) -> u32 {
    let span = u64::from(range.1.saturating_sub(range.0)) + 1;
    range.0 + (mix(seed ^ salt.wrapping_mul(0xA24B_AED4_963E_E407)) % span) as u32
}

/// The cube dot product of two hex differences: twice the projection of
/// one on the other, for unit hex directions.
fn dot(a: Hex, b: Hex) -> i32 {
    let (az, bz) = (-a.x - a.y, -b.x - b.y);
    a.x * b.x + a.y * b.y + az * bz
}

/// The frame a sector is laid in: its middle, the way the attacker comes
/// from (`toward`, a unit hex direction), and the way across the front.
#[derive(Clone, Copy)]
struct Frame {
    middle: Hex,
    toward: Hex,
    /// Perpendicular to `toward`, √3 hexes long: two neighbouring
    /// directions added.
    across: Hex,
}

impl Frame {
    /// How far `hex` lies toward the attacker from the front, in hexes.
    fn along(&self, hex: Hex) -> i32 {
        dot(hex - self.middle, self.toward) / 2
    }

    /// How far across the front, in tenths of a hex (signed).
    fn lateral_tenths(&self, hex: Hex) -> i32 {
        // |across|² in the cube metric is 6 and it is √3 hexes long, so a
        // hex across is dot / 6 · √3 ≈ dot · 0.2887.
        dot(hex - self.middle, self.across) * 2887 / 1000
    }

    /// The hex `along` hexes toward the attacker and `lateral` hexes
    /// across, on the nearest lattice point.
    fn at(&self, along: i32, lateral: i32) -> Hex {
        // Across in whole √3 steps and the half step left over taken as one
        // neighbour, which leans half a hex forward: near enough for a
        // start line or a minefield, and a pure function of the frame.
        let steps = lateral.abs() * 1000 / 1732;
        let sign = lateral.signum();
        let mut hex = self.middle + self.toward * along + self.across * (steps * sign);
        let rem = lateral.abs() * 1000 - steps * 1732;
        if rem >= 866 {
            hex += self.toward.rotate_cw(if sign >= 0 { 1 } else { 5 });
        }
        hex
    }
}

/// Whether a tank can stand on `id`.
fn tracked(reg: &DataRegistry, id: &str) -> bool {
    reg.terrain(id)
        .and_then(|t| t.cost_for(MovementClass::Tracked))
        .is_some()
}

/// How much `hex` is worth defending, read off the ground the way a
/// commander reads a map: a village to hold, a crossroads to deny, a rise
/// that looks down on its approaches, a wood's edge with a field of fire.
/// In tenths: one for bare ground a tank can stand on, so a lane of open
/// field still has something to hold, and zero for ground she cannot.
fn key_ground(reg: &DataRegistry, world: &GeneratedWorld, hex: Hex) -> i32 {
    let Some((id, level)) = world.tile(hex) else {
        return 0;
    };
    if !tracked(reg, id) {
        return 0;
    }
    let mut score = 1;
    let terrain = |h: Hex| world.tile(h).map(|(t, _)| t);
    if id == "town" {
        score += 30;
    }
    if id == "road" {
        let roads = hex
            .all_neighbors()
            .iter()
            .filter(|n| terrain(**n) == Some("road"))
            .count();
        if roads >= 3 {
            score += 20;
        }
    }
    if id == "forest" {
        let open = hex
            .all_neighbors()
            .iter()
            .filter(|n| terrain(**n).is_some_and(|t| t != "forest"))
            .count();
        if open >= 2 {
            score += 10;
        }
    }
    // Relief: how far it stands above the lowest ground within three hexes,
    // up to four levels.
    let lowest = hex
        .range(3)
        .filter_map(|h| world.tile(h).map(|(_, l)| l))
        .min()
        .unwrap_or(level);
    score += (level - lowest).clamp(0, 4) * 8;
    score
}

/// Sector number `index` of `size`, on world `index / fields_per_world`
/// from `worlds`, with `forces` (attacker first) deploying on it, or why
/// it cannot be.
pub fn sector(
    reg: &DataRegistry,
    worlds: &Worlds,
    index: u64,
    size: SectorSize,
    rules: SectorRules,
    forces: [&ForceDef; 2],
) -> Result<Sector, SectorError> {
    let layout = OpenLayout::default();
    let world = worlds
        .get(
            reg,
            layout,
            index / u64::from(layout.fields_per_world.max(1)),
        )
        .ok_or(SectorError::NoWorld)?;
    let shape = size.shape();
    let seed = mix(index ^ 0x5EC7_0000);
    let chunk_radius = world.chunk_radius;

    let turns = (mix(seed ^ 1) % 6) as u32;
    let offsets: Vec<Hex> = Hex::ZERO.range(chunk_radius).collect();
    let offset = offsets[(mix(seed ^ 2) % offsets.len() as u64) as usize];
    let toward = Hex::new(1, 0).rotate_cw(turns);
    let frame = Frame {
        middle: chunk_centre(Hex::ZERO, chunk_radius) + offset,
        toward,
        across: toward.rotate_cw(1) + toward.rotate_cw(2),
    };

    // The zones: the frontage cut into lanes, one zone a lane, each the
    // best ground in its lane within the defended depth. Lanes spread them
    // across the front, which is the whole point of having several; the
    // best ground in each is wherever it happens to be in depth.
    let count = draw(seed, 3, shape.zones);
    let half = shape.frontage as i32 * 10 / 2;
    let lane = (2 * half) / count as i32;
    let area: Vec<Hex> = frame
        .middle
        .range(shape.frontage.max(shape.depth) + 4)
        .filter(|h| {
            let a = frame.along(*h);
            a <= 0 && a >= -(shape.depth as i32) && frame.lateral_tenths(*h).abs() <= half
        })
        .collect();
    let mut zones: Vec<Hex> = Vec::new();
    for i in 0..count as i32 {
        let (lo, hi) = (-half + i * lane, -half + (i + 1) * lane);
        let best = area
            .iter()
            .filter(|h| {
                let l = frame.lateral_tenths(**h);
                l >= lo && l < hi
            })
            .map(|h| (key_ground(reg, &world, *h), *h))
            .filter(|(score, _)| *score > 0)
            // The best ground; then the nearer the front, which is the
            // ground that has to be held; then the field's own lot.
            .max_by_key(|(score, h)| (*score, frame.along(*h), mix(seed ^ hex_key(*h))))
            .ok_or(SectorError::NoGround(i as u32))?;
        zones.push(best.1);
    }
    let held = |anchor: Hex| -> Vec<Hex> {
        std::iter::once(anchor)
            .chain(
                anchor
                    .all_neighbors()
                    .into_iter()
                    .filter(|h| world.tile(*h).is_some_and(|(id, _)| tracked(reg, id))),
            )
            .collect()
    };
    let needs = count - count / 3;
    let mut scenario = Scenario::default()
        .with_assault(0, needs)
        .with_break_point(0, rules.attacker_breaks)
        .with_break_point(1, rules.defender_breaks);
    for (i, anchor) in zones.iter().enumerate() {
        scenario.push_objective(Objective {
            id: format!("zone{}", i + 1),
            name: format!("zone {}", i + 1),
            hexes: held(*anchor),
            value: 1,
            kind: ObjectiveKind::Hold,
            side: None,
            held: Some(1),
        });
    }

    // The defender: her platoons over the zones in turn, a quarter of
    // them (rounded down) held back as a reserve behind the middle of the
    // line — Clausewitz's fifth to a quarter, the doctrines' third to a
    // ninth. Each vehicle wants her zone's anchor and stands on the
    // nearest free ground to it.
    let defence_platoons: Vec<u32> = forces[1]
        .units
        .iter()
        .flat_map(|u| u.platoon_sizes())
        .collect();
    let reserves = defence_platoons.len() / 4;
    let centroid = {
        let n = zones.len().max(1) as i32;
        let sum = zones.iter().fold(Hex::ZERO, |a, b| a + *b);
        Hex::new(sum.x / n, sum.y / n)
    };
    let reserve_at = centroid - toward * (shape.depth as i32 / 2).max(2);
    let mut defence: Vec<Hex> = Vec::new();
    let forward = defence_platoons.len() - reserves;
    for (p, size) in defence_platoons.iter().enumerate() {
        let at = if p < forward {
            zones[p % zones.len()]
        } else {
            reserve_at
        };
        defence.extend(std::iter::repeat_n(at, *size as usize));
    }

    // The attacker: a start line `standoff` in front of the defended
    // front, as wide as it, in rows two hexes deep.
    let standoff = draw(seed, 4, shape.standoff) as i32;
    let vehicles: usize = forces[0].units.iter().map(|u| u.count as usize).sum();
    let per_row = (shape.frontage as usize / 2).max(1);
    let mut attack: Vec<Hex> = Vec::new();
    for v in 0..vehicles {
        let (row, col) = ((v / per_row) as i32, (v % per_row) as i32);
        let lateral = -(shape.frontage as i32) / 2 + 1 + col * 2;
        attack.push(frame.at(standoff + row * 2, lateral));
    }

    // The defender's work, if she had the hours: mines across the ground
    // in front of zones, wire before the near edge of a zone, and a ditch
    // across the open ground in front of the line with the roads left as
    // its crossings.
    let prepared = mix(seed ^ 5) % 100 < u64::from(rules.prepared_percent);
    let mut obstacles: Vec<(Hex, String)> = Vec::new();
    if prepared {
        scenario = scenario.with_prepared(1);
        let open = |h: Hex| {
            world
                .tile(h)
                .is_some_and(|(id, _)| matches!(id, "grass" | "hedgerow" | "plains" | "mud"))
        };
        for m in 0..draw(seed, 6, shape.minefields) {
            let zone = zones[(mix(seed ^ 7 ^ u64::from(m)) % zones.len() as u64) as usize];
            let ahead = 2 + (mix(seed ^ 8 ^ u64::from(m)) % 3) as i32;
            let long = 3 + (mix(seed ^ 9 ^ u64::from(m)) % 3) as i32;
            let base_lateral = frame.lateral_tenths(zone) / 10;
            let base_along = frame.along(zone) + ahead;
            let hexes: Vec<Hex> = (0..long)
                .map(|k| frame.at(base_along, base_lateral - long + 2 * k))
                .filter(|h| world.tile(*h).is_some())
                .collect();
            if hexes.is_empty() {
                continue;
            }
            let anti_personnel = mix(seed ^ 10 ^ u64::from(m)) % 10 < 3;
            scenario = scenario.with_minefield(Minefield {
                side: 1,
                mine: if anti_personnel {
                    "anti_personnel_mine".into()
                } else {
                    "anti_tank_mine".into()
                },
                hexes,
                // Disrupt to turn: half to four fifths of what crosses a
                // hex of it finds a mine (FM 20-32's bands).
                density: 50 + (mix(seed ^ 11 ^ u64::from(m)) % 31) as u32,
            });
        }
        for w in 0..draw(seed, 12, shape.wire) {
            let zone = zones[(mix(seed ^ 13 ^ u64::from(w)) % zones.len() as u64) as usize];
            let lateral = frame.lateral_tenths(zone) / 10;
            let along = frame.along(zone) + 2;
            for k in -1..=1 {
                let hex = frame.at(along, lateral + 2 * k);
                if open(hex) {
                    obstacles.push((hex, "wire".into()));
                }
            }
        }
        for d in 0..draw(seed, 14, shape.ditches) {
            let along = 4 + (mix(seed ^ 15 ^ u64::from(d)) % 3) as i32;
            for lateral in -(shape.frontage as i32) / 2..=(shape.frontage as i32) / 2 {
                let hex = frame.at(along, lateral);
                if open(hex) && !obstacles.iter().any(|(h, _)| *h == hex) {
                    obstacles.push((hex, "tank_ditch".into()));
                }
            }
        }
    }

    Ok(Sector {
        world,
        scenario,
        deployment: [attack, defence],
        obstacles,
        size,
        budget: budget(size, index),
        prepared,
        zones,
    })
}

/// A hex as a number to salt a draw with: its coordinates mixed, so a tie
/// goes to the field's lot rather than to whichever way is west.
fn hex_key(hex: Hex) -> u64 {
    mix(((hex.x as u32 as u64) << 32) | hex.y as u32 as u64)
}

/// The budget a sector of `size` draws for field `index`, before the forces
/// are bought: the forces must be bought to it before [`sector`] can lay
/// out their deployment.
pub fn budget(size: SectorSize, index: u64) -> i32 {
    let shape = size.shape();
    let seed = mix(index ^ 0x5EC7_0000);
    shape.budget.0 + (mix(seed ^ 16) % (shape.budget.1 - shape.budget.0 + 1).max(1) as u64) as i32
}

/// Stage `sector` with `forces` (attacker first), the battle's dice seeded
/// by `seed`.
pub fn stage(
    reg: &DataRegistry,
    sector: &Sector,
    forces: [&ForceDef; 2],
    seed: u64,
) -> Option<crate::battle::BattleState> {
    super::forces::stage_sides_on_world(
        reg,
        sector.world.clone(),
        sector.scenario.clone(),
        &sector.deployment,
        forces,
        &sector.obstacles,
        seed,
    )
}
