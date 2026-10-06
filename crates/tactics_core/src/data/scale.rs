//! The scale contract: what the simulation's abstract units mean in the world.
//!
//! The engine counts in hexes, movement points, rounds and ticks. Those are
//! convenient to simulate with and meaningless to a player, and until this
//! module existed the translation lived only in a table in CLAUDE.md — every
//! range, speed and map dimension in `assets/mods` honoured it by hand and
//! nothing in the code could tell you whether they had.
//!
//! [`Scale`] is that table, as data. It comes from the `scale` block of
//! `mod.json`, so a mod can retune the tempo of the whole game — a skirmish
//! mod could halve the hex and the round together and keep every existing
//! vehicle honest — and it is what lets the UI say "1.6 km" where it used to
//! say "16".
//!
//! Two things follow from putting it here rather than in a Rust constant.
//! `ticks_per_round` is no longer a `const`, so anything that needs it takes
//! a registry; and a weapon's `reload_ticks` can no longer default to "one
//! round" at deserialization time, which is why [`crate::data::WeaponDef`]
//! stores an `Option` and resolves it against the scale on use.

use serde::{Deserialize, Serialize};

/// Physical meaning of every abstract quantity the simulation counts in.
///
/// The defaults are the shipped contract: a 100 m hex, a 60 s round of twelve
/// 5 s ticks, 10 m of height per elevation level, and a 4 km overworld hex
/// that is exactly one battle map across. Every field is defaulted
/// individually, so a `scale` block may set only what it means to change and
/// the rest stays on the contract rather than on whatever the previous mod
/// happened to declare.
///
/// The consequences worth remembering when authoring content:
///
/// - one movement point is one hex of clear terrain per round, so it is a
///   speed: at the default scale 5 MP is 30 km/h, not an abstract budget;
/// - a weapon's `range` is in hexes, so the 88's 16 is 1.6 km;
/// - `reload_ticks` is a practical aimed rate of fire, not a mechanical
///   reload — 4 ticks is a shot every 20 s.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Scale {
    /// Width of one battle hex, in metres.
    pub hex_meters: f32,
    /// Wall-clock length of one battle round, in seconds.
    pub round_seconds: f32,
    /// How many ticks one round resolves over. Movement points are spent per
    /// round but spread across these ticks, so a faster vehicle covers ground
    /// *earlier* rather than merely going further.
    pub ticks_per_round: u32,
    /// Height of one elevation level, in metres. `max_climb: 1` over a 100 m
    /// hex at 10 m per level is a 10% grade.
    pub elevation_meters: f32,
    /// Width of one overworld hex, in metres. The contract is that this is
    /// one battle map across, which is what makes a field battle a zoom-in on
    /// the tile the armies met in rather than a separate abstraction.
    pub overworld_hex_meters: f32,
    /// Height of one elevation level on the campaign map, in metres: the
    /// campaign's own vertical scale (WORLD.md W6.8). A campaign hex is four
    /// kilometres across, and drawn at the battle's ten metres a level
    /// every hill stood up as a tower, about ninety times exaggerated. A
    /// generated campaign's hexes are raised by their land's mean height
    /// in these units. Equal to `elevation_meters` is one scale for both.
    pub overworld_elevation_meters: f32,
    /// How much time one overworld turn represents, in hours.
    ///
    /// A day, which is what the campaign layer has quietly assumed all along:
    /// its banner counts "Day N" and tile income is quoted per day. The
    /// arithmetic supports it — `frontier` gives an army 4 movement, which at
    /// a 4 km hex is 16 km per turn, and 16 km/day is an ordinary sustained
    /// advance for an armoured formation even though the same tanks do 30
    /// km/h in a battle. An operational turn is mostly not spent driving.
    pub overworld_turn_hours: f32,
}

impl Default for Scale {
    fn default() -> Self {
        Self {
            hex_meters: 100.0,
            round_seconds: 60.0,
            ticks_per_round: 12,
            elevation_meters: 10.0,
            overworld_hex_meters: 4000.0,
            overworld_elevation_meters: 10.0,
            overworld_turn_hours: 24.0,
        }
    }
}

impl Scale {
    /// Length of one tick, in seconds.
    pub fn tick_seconds(&self) -> f32 {
        if self.ticks_per_round == 0 {
            return self.round_seconds;
        }
        self.round_seconds / self.ticks_per_round as f32
    }

    /// Ground distance covered by `hexes` battle hexes, in metres.
    pub fn meters(&self, hexes: i32) -> f32 {
        hexes as f32 * self.hex_meters
    }

    /// Height of `levels` elevation steps, in metres.
    pub fn elevation(&self, levels: i32) -> f32 {
        levels as f32 * self.elevation_meters
    }

    /// Wall-clock duration of `ticks` ticks, in seconds.
    pub fn seconds(&self, ticks: u32) -> f32 {
        ticks as f32 * self.tick_seconds()
    }

    /// Speed represented by a movement allowance, in km/h. One movement point
    /// buys one hex of clear terrain per round, so this is the honest reading
    /// of a vehicle's `points`.
    pub fn kph(&self, move_points: u32) -> f32 {
        if self.round_seconds <= 0.0 {
            return 0.0;
        }
        // Multiply before dividing: at f32 precision `100 / 60 * 3.6` lands
        // on 5.9999995 where `100 * 3.6 / 60` is exactly 6.
        move_points as f32 * self.hex_meters * 3.6 / self.round_seconds
    }

    /// How many battle hexes wide one overworld hex is. The contract says
    /// this should be about the width of a battle map, which is what makes
    /// "the armies met in this tile" and "here is the ground they met on"
    /// the same statement.
    pub fn battle_hexes_per_overworld_hex(&self) -> f32 {
        if self.hex_meters <= 0.0 {
            return 0.0;
        }
        self.overworld_hex_meters / self.hex_meters
    }

    /// Radius, in battle hexes, of the hexagonal battle map that represents
    /// one overworld tile.
    ///
    /// A hexagon of radius `r` is `2r + 1` hexes across, so this is the
    /// radius whose span is [`Self::battle_hexes_per_overworld_hex`]. It
    /// rounds up on a tie: a battle map that is a shade larger than its
    /// overworld tile still contains the whole tile, whereas one a shade
    /// smaller reopens the gap between "where the armies met" and "the
    /// ground they fought over" that shaping maps as tiles closes.
    pub fn battle_map_radius(&self) -> u32 {
        let across = self.battle_hexes_per_overworld_hex();
        if !across.is_finite() || across < 1.0 {
            return 0;
        }
        ((across - 1.0) / 2.0).round() as u32
    }

    /// Number of tiles in that hexagon — the centred hexagonal number.
    /// Worth knowing before adding work per tile: it grows as the square of
    /// the radius, and every hex of extra reach costs another ring.
    pub fn battle_map_tiles(&self) -> u32 {
        let r = self.battle_map_radius();
        3 * r * r + 3 * r + 1
    }

    /// Ground distance covered by `hexes` overworld hexes, in kilometres.
    pub fn overworld_km(&self, hexes: u32) -> f32 {
        hexes as f32 * self.overworld_hex_meters / 1000.0
    }

    /// Marching rate implied by an overworld movement allowance, in km/h.
    /// Compare against the battle-scale speed of the same vehicles: an
    /// operational rate is expected to be far lower than a road speed,
    /// because a turn is mostly not spent driving.
    pub fn overworld_kph(&self, move_points: u32) -> f32 {
        if self.overworld_turn_hours <= 0.0 {
            return 0.0;
        }
        self.overworld_km(move_points) / self.overworld_turn_hours
    }

    /// A battle distance for display: "800 m", "1.6 km".
    pub fn format_distance(&self, hexes: i32) -> String {
        format_meters(self.meters(hexes))
    }

    /// A strategic distance for display: "16 km".
    pub fn format_overworld_distance(&self, hexes: u32) -> String {
        format_meters(hexes as f32 * self.overworld_hex_meters)
    }

    /// A weapon's `[min, max]` band for display: "100 m - 1.6 km".
    pub fn format_range(&self, range: [u32; 2]) -> String {
        format!(
            "{} - {}",
            self.format_distance(range[0] as i32),
            self.format_distance(range[1] as i32)
        )
    }

    /// An elevation level for display: "30 m".
    pub fn format_elevation(&self, levels: i32) -> String {
        format_meters(self.elevation(levels))
    }

    /// A movement allowance for display: "30 km/h".
    pub fn format_speed(&self, move_points: u32) -> String {
        format!("{} km/h", trim(self.kph(move_points), 0))
    }

    /// A tick count for display: "20 s", "1.5 min".
    pub fn format_duration(&self, ticks: u32) -> String {
        let seconds = self.seconds(ticks);
        if seconds < 90.0 {
            format!("{} s", trim(seconds, 0))
        } else {
            format!("{} min", trim(seconds / 60.0, 1))
        }
    }
}

/// Metres below a kilometre, kilometres above it. Distances at this scale are
/// hex multiples, so one decimal is always enough to be exact.
fn format_meters(meters: f32) -> String {
    if meters.abs() < 1000.0 {
        format!("{} m", trim(meters, 0))
    } else {
        format!("{} km", trim(meters / 1000.0, 1))
    }
}

/// Round to `places` decimals and drop a trailing `.0`, so a scale that
/// happens to divide evenly reads as "2 km" rather than "2.0 km".
fn trim(value: f32, places: usize) -> String {
    let text = format!("{value:.places$}");
    match text.split_once('.') {
        Some((whole, frac)) if frac.chars().all(|c| c == '0') => whole.to_string(),
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_scale_is_the_shipped_contract() {
        let scale = Scale::default();
        assert_eq!(scale.tick_seconds(), 5.0);
        assert_eq!(scale.meters(16), 1600.0);
        assert_eq!(scale.elevation(3), 30.0);
        assert_eq!(scale.battle_hexes_per_overworld_hex(), 40.0);
    }

    #[test]
    fn one_movement_point_is_six_kilometres_an_hour() {
        // The whole reason `points` is a speed and not a budget: a hex of
        // clear terrain per 60 s round is 100 m/min.
        let scale = Scale::default();
        assert_eq!(scale.kph(1), 6.0);
        assert_eq!(scale.kph(5), 30.0);
        assert_eq!(scale.format_speed(7), "42 km/h");
    }

    #[test]
    fn distances_read_in_metres_then_kilometres() {
        let scale = Scale::default();
        assert_eq!(scale.format_distance(8), "800 m");
        assert_eq!(scale.format_distance(16), "1.6 km");
        assert_eq!(scale.format_distance(20), "2 km");
        assert_eq!(scale.format_range([3, 40]), "300 m - 4 km");
        assert_eq!(scale.format_duration(4), "20 s");
        assert_eq!(scale.format_duration(24), "2 min");
    }

    #[test]
    fn a_mod_may_override_one_field_without_disturbing_the_rest() {
        // Partial blocks fall back to the contract, not to whatever a
        // previously loaded mod declared, so a scale block is readable on its
        // own terms.
        let scale: Scale = serde_json::from_str(r#"{ "hex_meters": 50 }"#).unwrap();
        assert_eq!(scale.hex_meters, 50.0);
        assert_eq!(scale.round_seconds, Scale::default().round_seconds);
        assert_eq!(scale.ticks_per_round, Scale::default().ticks_per_round);
    }

    #[test]
    fn halving_the_hex_and_the_round_together_preserves_speeds() {
        // The property that makes the scale block worth having: a mod can
        // change tempo without invalidating every vehicle's `points`.
        let fast = Scale {
            hex_meters: 50.0,
            round_seconds: 30.0,
            ..Scale::default()
        };
        assert_eq!(fast.kph(5), Scale::default().kph(5));
    }
}
