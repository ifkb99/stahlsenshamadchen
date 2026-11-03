//! The battle log: one line per round, written by the seat
//! (`examples/command`, which includes this file by path) and by the heat
//! map's own battles, and read back by the heat map.
//!
//! It holds what the commander was *not* told as well as what she was: the
//! whole event stream, where everybody stood, and what every crew meant to
//! do when the round was committed. It is the post-mortem's record.

use serde::{Deserialize, Serialize};
use tactics_core::battle::{BattleState, Event, Mission, PersonalOrder, Unit};
use tactics_core::hex_to_offset;

/// One round: its events, every unit's side and place after it, and what
/// each crew meant to do when it was committed. A round 0 with no events is
/// the opening positions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Round {
    #[serde(default)]
    pub round: u32,
    pub events: Vec<Event>,
    pub units: Vec<Placed>,
    #[serde(default)]
    pub intentions: Vec<Intention>,
}

/// A unit's place after a round. `standing` is false for the dead, the
/// gone and the carried.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Placed {
    pub id: u32,
    pub side: u8,
    pub at: [i32; 2],
    pub standing: bool,
    /// Chassis id; absent in logs written before it was recorded.
    #[serde(default)]
    pub vehicle: String,
    #[serde(default)]
    pub name: String,
    /// The carrier she rides in, if any.
    #[serde(default)]
    pub aboard: Option<u32>,
}

/// What one crew had been told and what she meant to do about it, read
/// after planning and before resolution: the two halves of "did the order
/// land" (order against intent) and, with where she ended, of "did the plan
/// survive the round" (intent against outcome).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Intention {
    pub unit: u32,
    pub side: u8,
    pub from: [i32; 2],
    /// The ground her orders name, if they name any.
    pub ordered: Option<[i32; 2]>,
    /// Which order named it: `radio`, `radio hold`, or the mission verb.
    pub order: String,
    /// Where her planned route ends (where she stands, if she plans none).
    pub intends: [i32; 2],
    /// Her own words for it, when she set out on something new this round.
    pub doing: String,
}

/// Every unit, placed.
pub fn placed(state: &BattleState) -> Vec<Placed> {
    state
        .units
        .iter()
        .map(|u| Placed {
            id: u.id.0,
            side: u.side,
            at: hex_to_offset(u.pos),
            standing: u.fate.alive() && u.aboard.is_none(),
            vehicle: u.vehicle.clone(),
            name: u.name.clone(),
            aboard: u.aboard.map(|a| a.0),
        })
        .collect()
}

/// The ground a unit's orders name, and which order: her commander's
/// personal order first (it detaches her from the mission), else her
/// formation's mission as she has heard it. A withdrawal names an exit, not
/// ground, and a base of fire names a formation; neither is counted.
pub fn ordered_ground(state: &BattleState, u: &Unit) -> Option<([i32; 2], String)> {
    match u.orders {
        Some(PersonalOrder::Marching(m)) => return Some((hex_to_offset(m.to), "radio".into())),
        Some(PersonalOrder::Holding { .. }) => {
            return Some((hex_to_offset(u.pos), "radio hold".into()));
        }
        None => {}
    }
    let mission = state.formation_of(u.id)?.mission_for(u.id)?;
    let (at, verb) = match mission {
        Mission::Advance { to } => (*to, "advance"),
        Mission::Assault { to } => (*to, "assault"),
        Mission::Hold { at } => (at.unwrap_or(u.pos), "hold"),
        Mission::Recon { toward } => (*toward, "recon"),
        _ => return None,
    };
    Some((hex_to_offset(at), verb.into()))
}

/// What every crew on the field means to do, read after planning. `planning`
/// is the planning pass's events, for the crews' own words.
pub fn intentions(state: &BattleState, planning: &[Event]) -> Vec<Intention> {
    state
        .units
        .iter()
        .filter(|u| u.fate.alive() && u.aboard.is_none())
        .map(|u| {
            let ordered = ordered_ground(state, u);
            let doing = planning
                .iter()
                .rev()
                .find_map(|ev| match ev {
                    Event::SetOut { unit, doing, .. } if *unit == u.id => Some(doing.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            Intention {
                unit: u.id.0,
                side: u.side,
                from: hex_to_offset(u.pos),
                ordered: ordered.as_ref().map(|o| o.0),
                order: ordered.map(|o| o.1).unwrap_or_default(),
                intends: hex_to_offset(*u.intent.path.last().unwrap_or(&u.pos)),
                doing,
            }
        })
        .collect()
}

/// Hex distance between two map coordinates.
pub fn distance(a: [i32; 2], b: [i32; 2]) -> u32 {
    tactics_core::offset_to_hex(a[0], a[1])
        .unsigned_distance_to(tactics_core::offset_to_hex(b[0], b[1]))
}

/// Whether an intention keeps its order: she means to end on or by the
/// ground (within one hex of a radio order, two of a mission, since a
/// mission names ground and leaves where to stand to her), or at least to
/// close on it. A hold is kept by not leaving.
pub fn keeps_order(i: &Intention) -> Option<bool> {
    let ground = i.ordered?;
    let slack = if i.order.starts_with("radio") { 1 } else { 2 };
    let (before, after) = (distance(i.from, ground), distance(i.intends, ground));
    let holding = i.order == "hold" || i.order == "radio hold";
    Some(if holding {
        distance(i.intends, i.from) <= 1 || after <= slack
    } else {
        after <= slack || after < before
    })
}
