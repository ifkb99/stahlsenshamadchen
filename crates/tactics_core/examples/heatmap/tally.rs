//! Folding battle logs into counts: the per-hex layers, shot and contact
//! ranges, where each side first saw the other, how often orders landed,
//! and a per-unit timeline.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use tactics_core::battle::{Event, UnitId};
use tactics_core::hex_to_offset;

use crate::log::{Round, distance, keeps_order};

pub type Cell = [i32; 2];

pub const LAYERS: [&str; 6] = ["moved", "stood", "fired from", "hurt", "seen", "pinned"];
pub const MOVED: usize = 0;
pub const STOOD: usize = 1;
pub const FIRED: usize = 2;
pub const HURT: usize = 3;
pub const SEEN: usize = 4;
pub const PINNED: usize = 5;

/// Range buckets, in hexes; the last is "this far or further".
pub const BUCKETS: usize = 13;

/// A contact: where the spotted unit stood, where the nearest of the
/// spotting side stood, and the round.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Contact {
    pub round: u32,
    pub spotted: Cell,
    pub spotter: Cell,
}

/// Everything counted, summed over battles; divide by `battles` to read.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Heat {
    pub map: String,
    pub title: String,
    pub names: [String; 2],
    /// `layers[layer][side]`: hex to count.
    pub layers: Vec<[Vec<(Cell, f64)>; 2]>,
    pub lost: [Vec<(Cell, f64)>; 2],
    pub lines: [Vec<((Cell, Cell), f64)>; 2],
    /// Shots by range, per firing side.
    pub ranges: [[u32; BUCKETS]; 2],
    /// Spots by range from the nearest of the spotting side, per spotting
    /// side.
    pub spot_ranges: [[u32; BUCKETS]; 2],
    /// The first contact each side made, per battle.
    pub first: [Vec<Contact>; 2],
    /// Intentions with an ordered ground, and how many kept it, per side.
    pub orders: [u32; 2],
    pub kept: [u32; 2],
    /// Intentions with a planned end, and how many ended within a hex of
    /// it, per side.
    pub planned: [u32; 2],
    pub arrived: [u32; 2],
    pub battles: u32,
    pub wins: [u32; 2],
    pub rounds: u32,
    /// Count only units of this chassis; the ending still counts.
    pub only: Option<String>,
}

/// The counting state, keyed for speed; turned into `Heat`'s plain lists
/// at the end so it serialises to json without map keys that are arrays.
#[derive(Default)]
pub struct Tally {
    layers: [[BTreeMap<Cell, f64>; 2]; LAYERS.len()],
    lost: [BTreeMap<Cell, f64>; 2],
    lines: [BTreeMap<(Cell, Cell), f64>; 2],
    pub heat: Heat,
}

/// One unit's row in a timeline.
pub struct Lane {
    pub name: String,
    pub side: u8,
    /// Per round: activity glyph and morale rung initial.
    pub cells: Vec<(char, char)>,
}

impl Tally {
    pub fn new(only: Option<String>) -> Self {
        Tally {
            heat: Heat {
                only,
                ..Heat::default()
            },
            ..Tally::default()
        }
    }

    fn add(&mut self, layer: usize, side: u8, at: Cell) {
        *self.layers[layer][side as usize].entry(at).or_default() += 1.0;
    }

    /// Fold one battle's rounds in. `rungs` are the morale ladder's (id,
    /// name) pairs, lowest first, for the timeline; returns that battle's
    /// timeline.
    pub fn battle(&mut self, rounds: &[Round], rungs: &[(String, String)]) -> Vec<Lane> {
        let h = &mut self.heat;
        h.battles += 1;
        h.rounds += rounds
            .iter()
            .filter(|r| r.round > 0 || !r.events.is_empty())
            .count() as u32;

        // Who is counted, and on which side. A filtered-out unit has no
        // side, so every event about it drops.
        let mut side_of: BTreeMap<u32, u8> = BTreeMap::new();
        let mut names: BTreeMap<u32, String> = BTreeMap::new();
        for round in rounds {
            for u in &round.units {
                if h.only.as_ref().is_none_or(|v| *v == u.vehicle) {
                    side_of.insert(u.id, u.side);
                    names.entry(u.id).or_insert_with(|| u.name.clone());
                }
            }
        }
        let side = |id: &UnitId| side_of.get(&id.0).copied();

        // Where everybody is *during* the round, tracked through its
        // events from where they stood before it, so a hit or a spot is
        // placed where it happened rather than where the round ended.
        let mut pos: BTreeMap<u32, Cell> = BTreeMap::new();
        let mut aboard: BTreeMap<u32, u32> = BTreeMap::new();
        let mut all_side: BTreeMap<u32, u8> = BTreeMap::new();
        let mut dead: BTreeSet<u32> = BTreeSet::new();
        if let Some(first) = rounds.first() {
            for u in &first.units {
                pos.insert(u.id, u.at);
                all_side.insert(u.id, u.side);
                if let Some(a) = u.aboard {
                    aboard.insert(u.id, a);
                }
            }
        }
        // A log that opens on round 1 (written before round 0 was) has no
        // opening positions; its first round's ends stand in for them.
        let fight: &[Round] = match rounds.first() {
            Some(r) if r.round == 0 && r.events.is_empty() => &rounds[1..],
            _ => rounds,
        };

        let mut lanes: BTreeMap<u32, Lane> = BTreeMap::new();
        let mut rung: BTreeMap<u32, char> = BTreeMap::new();
        let first_rung = rungs
            .first()
            .and_then(|r| r.1.chars().next())
            .unwrap_or('S');
        let mut seen_first = [false; 2];

        for (k, round) in fight.iter().enumerate() {
            let mut act: BTreeMap<u32, char> = BTreeMap::new();
            let mark = |act: &mut BTreeMap<u32, char>, id: u32, glyph: char| {
                // The strongest thing she did this round wins the cell.
                let order = ['.', 'r', 'd', 'm', 'f', 'F', 'h', 'X'];
                let rank = |c: char| order.iter().position(|o| *o == c).unwrap_or(0);
                let e = act.entry(id).or_insert('.');
                let combined = match (*e, glyph) {
                    ('m', 'f') | ('f', 'm') | ('F', 'm') | ('F', 'f') => 'F',
                    (a, b) if rank(b) > rank(a) => b,
                    (a, _) => a,
                };
                *e = combined;
            };
            let mut aim: BTreeMap<u32, Cell> = BTreeMap::new();
            for ev in &round.events {
                match ev {
                    Event::UnitMoved { unit, path } => {
                        if let Some(last) = path.last() {
                            let to = hex_to_offset(*last);
                            pos.insert(unit.0, to);
                            for (rider, carrier) in &aboard {
                                if *carrier == unit.0 {
                                    pos.insert(*rider, to);
                                }
                            }
                        }
                        mark(&mut act, unit.0, 'm');
                        if let Some(s) = side(unit) {
                            for h in path.iter().skip(1) {
                                self.add(MOVED, s, hex_to_offset(*h));
                            }
                        }
                    }
                    Event::Mounted { unit, into } => {
                        aboard.insert(unit.0, into.0);
                        mark(&mut act, unit.0, 'r');
                    }
                    Event::Dismounted { unit, at } => {
                        aboard.remove(&unit.0);
                        pos.insert(unit.0, hex_to_offset(*at));
                        mark(&mut act, unit.0, 'd');
                    }
                    Event::ShotFired {
                        attacker, from, at, ..
                    } => {
                        mark(&mut act, attacker.0, 'f');
                        if let Some(s) = side(attacker) {
                            let (f, t) = (hex_to_offset(*from), hex_to_offset(*at));
                            self.add(FIRED, s, f);
                            *self.lines[s as usize].entry((f, t)).or_default() += 1.0;
                            let d = from.unsigned_distance_to(*at) as usize;
                            self.heat.ranges[s as usize][d.min(BUCKETS - 1)] += 1;
                            aim.insert(attacker.0, t);
                        }
                    }
                    Event::ShotHit {
                        attacker, target, ..
                    } => {
                        mark(&mut act, target.0, 'h');
                        if let Some(s) = side(target) {
                            let at = pos.get(&target.0).or(aim.get(&attacker.0)).copied();
                            if let Some(at) = at {
                                self.add(HURT, s, at);
                            }
                        }
                    }
                    Event::UnitDestroyed { unit, at } => {
                        dead.insert(unit.0);
                        mark(&mut act, unit.0, 'X');
                        if let Some(s) = side(unit) {
                            *self.lost[s as usize].entry(hex_to_offset(*at)).or_default() += 1.0;
                        }
                    }
                    Event::UnitSpotted { unit, by_side, at } => {
                        let at = hex_to_offset(*at);
                        // The nearest crew of the spotting side, on the
                        // field: the spotter or close to her. The event
                        // does not name who looked.
                        let spotter = pos
                            .iter()
                            .filter(|(id, _)| {
                                all_side.get(id) == Some(by_side)
                                    && !aboard.contains_key(id)
                                    && !dead.contains(id)
                            })
                            .map(|(_, p)| *p)
                            .min_by_key(|p| (distance(*p, at), *p));
                        if let Some(s) = side(unit) {
                            self.add(SEEN, s, at);
                        }
                        if let Some(spotter) = spotter {
                            let b = *by_side as usize;
                            let d = distance(spotter, at) as usize;
                            self.heat.spot_ranges[b][d.min(BUCKETS - 1)] += 1;
                            if !seen_first[b] {
                                seen_first[b] = true;
                                self.heat.first[b].push(Contact {
                                    round: round.round.max(k as u32 + 1),
                                    spotted: at,
                                    spotter,
                                });
                            }
                        }
                    }
                    Event::PinnedDown { unit, at } => {
                        if let Some(s) = side(unit) {
                            self.add(PINNED, s, hex_to_offset(*at));
                        }
                    }
                    Event::MoraleChanged { unit, rung: r, .. } => {
                        let name = rungs
                            .iter()
                            .find(|(id, _)| id == r)
                            .map(|(_, name)| name.clone())
                            .unwrap_or_else(|| r.clone());
                        rung.insert(unit.0, name.chars().next().unwrap_or('?'));
                    }
                    // The ending's own word, not the score: an elimination
                    // can be won from behind on points.
                    Event::BattleEnded {
                        winner: Some(w), ..
                    } => self.heat.wins[*w as usize] += 1,
                    _ => {}
                }
            }

            for i in &round.intentions {
                let s = i.side as usize;
                if side_of.contains_key(&i.unit)
                    && let Some(kept) = keeps_order(i)
                {
                    self.heat.orders[s] += 1;
                    self.heat.kept[s] += u32::from(kept);
                }
                if side_of.contains_key(&i.unit)
                    && !dead.contains(&i.unit)
                    && let Some(end) = round.units.iter().find(|u| u.id == i.unit)
                {
                    self.heat.planned[s] += 1;
                    self.heat.arrived[s] += u32::from(distance(end.at, i.intends) <= 1);
                }
            }

            for u in &round.units {
                pos.insert(u.id, u.at);
                all_side.insert(u.id, u.side);
                match u.aboard {
                    Some(a) => {
                        aboard.insert(u.id, a);
                    }
                    None => {
                        aboard.remove(&u.id);
                    }
                }
                if u.standing && side_of.contains_key(&u.id) {
                    self.add(STOOD, u.side, u.at);
                }
            }

            // The timeline: every counted unit gets a cell every round.
            for (&id, &s) in &side_of {
                let lane = lanes.entry(id).or_insert_with(|| Lane {
                    name: names.get(&id).cloned().unwrap_or_default(),
                    side: s,
                    cells: vec![(' ', ' '); k],
                });
                let riding = round.units.iter().any(|u| u.id == id && u.aboard.is_some());
                let gone_before = lane.cells.last().is_some_and(|c| c.0 == 'X' || c.0 == ' ');
                let glyph = match act.get(&id) {
                    Some(g) => *g,
                    None if gone_before => ' ',
                    None if riding => 'r',
                    None => '.',
                };
                let r = if glyph == ' ' || glyph == 'X' {
                    ' '
                } else {
                    *rung.get(&id).unwrap_or(&first_rung)
                };
                lane.cells.push((glyph, r));
            }
        }
        lanes.into_values().collect()
    }

    /// The counts as plain lists, ready to draw or save.
    pub fn finish(mut self) -> Heat {
        let list = |m: &BTreeMap<Cell, f64>| m.iter().map(|(k, v)| (*k, *v)).collect::<Vec<_>>();
        self.heat.layers = self
            .layers
            .iter()
            .map(|pair| [list(&pair[0]), list(&pair[1])])
            .collect();
        self.heat.lost = [list(&self.lost[0]), list(&self.lost[1])];
        self.heat.lines = [
            self.lines[0].iter().map(|(k, v)| (*k, *v)).collect(),
            self.lines[1].iter().map(|(k, v)| (*k, *v)).collect(),
        ];
        self.heat
    }
}

/// A timeline as text: one row per unit, one two-letter cell per round
/// (what she did, then her morale rung's initial).
pub fn print_timeline(lanes: &[Lane], names: &[String; 2]) {
    let rounds = lanes.iter().map(|l| l.cells.len()).max().unwrap_or(0);
    let width = lanes
        .iter()
        .map(|l| l.name.chars().count())
        .max()
        .unwrap_or(4)
        .max(4);
    print!("{:width$}  ", "");
    for r in 1..=rounds {
        print!("{r:<3}");
    }
    println!();
    for side in 0..2u8 {
        println!("{}", names[side as usize]);
        for lane in lanes.iter().filter(|l| l.side == side) {
            print!("{:width$}  ", lane.name);
            for (a, m) in &lane.cells {
                print!("{a}{m} ");
            }
            println!();
        }
    }
    println!(
        "key: m moved, f fired, F both, h hit, d dismounted, r riding, X destroyed, . nothing; \
         second letter is her morale rung"
    );
}
