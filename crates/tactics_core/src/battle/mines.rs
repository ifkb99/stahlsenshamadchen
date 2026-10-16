//! Mines in a battle (ASSAULT.md): who knows where they are, how many are
//! left, and what happens to a crew who drives onto them.
//!
//! The scenario says where they were sown ([`crate::map::Minefield`]) and
//! the mod what they are ([`crate::data::MineDef`]). A crew of the side that
//! laid them knows the lanes and never sets one off. Anybody else entering
//! a sown hex rolls the field's density times the mine's trigger chance for
//! how she moves, scaled by what is left in that hex; a strike stops her
//! where she is, spends the mine's damage inside her, and tells her side
//! where the whole field lies — which is how a minefield is usually found.
//!
//! **Knowledge is the side's, never the fog's.** A crew plans a road round
//! a field her side knows of (`movement`'s route price) and straight
//! through one it does not, so an order is never refused, and no road
//! bends, in a way that gives away a field nobody has found. With no
//! minefields no die is thrown, which is the rule's absence down to the rng.

use std::collections::BTreeMap;

use hexx::Hex;
use rand::RngExt;
use serde::{Deserialize, Serialize};

use super::{BattleState, Event, UnitId};
use crate::data::DataRegistry;

/// What a battle remembers about its mines.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MineState {
    /// Per minefield, in the scenario's order, the sides other than its
    /// owner that have found it.
    #[serde(default)]
    found: Vec<Vec<u8>>,
    /// Mines that have gone off, by hex.
    #[serde(default)]
    spent: BTreeMap<(i32, i32), u32>,
}

impl BattleState {
    /// The first minefield, in declaration order, sown on `hex` by
    /// anybody but `side`, with its index.
    fn enemy_minefield(&self, side: u8, hex: Hex) -> Option<(usize, &crate::map::Minefield)> {
        self.scenario
            .minefields()
            .iter()
            .enumerate()
            .find(|(_, f)| f.side != side && f.hexes.contains(&hex))
    }

    /// Whether `side` knows of an enemy mine still lying on `hex`: a hex in
    /// a field she has found, not yet cleared by the mines that went off in
    /// it. What a route is priced by.
    pub fn known_mines(&self, registry: &DataRegistry, side: u8, hex: Hex) -> bool {
        let Some((index, field)) = self.enemy_minefield(side, hex) else {
            return false;
        };
        self.mines
            .found
            .get(index)
            .is_some_and(|sides| sides.contains(&side))
            && self.mines_left(registry, field, hex) > 0
    }

    /// How many of `field`'s mines are still in `hex`.
    fn mines_left(&self, registry: &DataRegistry, field: &crate::map::Minefield, hex: Hex) -> u32 {
        let per_hex = registry.mine(&field.mine).map_or(0, |m| m.per_hex);
        per_hex.saturating_sub(self.mines.spent.get(&(hex.x, hex.y)).copied().unwrap_or(0))
    }

    /// `unit` has just driven onto `hex`: roll for a mine under her if
    /// somebody else sowed one there. Returns whether one went off, in
    /// which case she has stopped.
    pub(crate) fn tread(
        &mut self,
        registry: &DataRegistry,
        unit: UnitId,
        hex: Hex,
        events: &mut Vec<Event>,
    ) -> bool {
        let Some(me) = self.unit(unit) else {
            return false;
        };
        let side = me.side;
        let Some(class) = registry.vehicle(&me.vehicle).map(|v| v.movement.class) else {
            return false;
        };
        let Some((index, field)) = self.enemy_minefield(side, hex) else {
            return false;
        };
        let Some(mine) = registry.mine(&field.mine).cloned() else {
            return false;
        };
        let left = self.mines_left(registry, field, hex);
        let per_hex = mine.per_hex.max(1);
        // Percent of a percent of a share: out of 100 × 100 × per_hex.
        let chance = u64::from(field.density.min(100))
            * u64::from(mine.trigger_for(class))
            * u64::from(left);
        if chance == 0 {
            return false;
        }
        let roll = self.rng.random_range(0..100 * 100 * u64::from(per_hex));
        if roll >= chance {
            return false;
        }
        *self.mines.spent.entry((hex.x, hex.y)).or_insert(0) += 1;
        if self.mines.found.len() <= index {
            self.mines.found.resize(index + 1, Vec::new());
        }
        if !self.mines.found[index].contains(&side) {
            self.mines.found[index].push(side);
        }
        events.push(Event::MineStruck {
            unit,
            at: hex,
            mine: mine.id.clone(),
        });
        super::combat::mine_blast(registry, self, unit, mine.damage, mine.cripples, events);
        true
    }
}
