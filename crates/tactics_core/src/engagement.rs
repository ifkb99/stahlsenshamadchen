//! The front: where a campaign's armies stop being armies and are crews on
//! tiles, for as long as they are fighting (WORLD.md W3.2–W3.3, W3.8).
//!
//! The designer's rulings: one world clock, a bubble that decides the
//! resolution, and — asked whether two fights that drift together should be
//! merged — **no real separation between engagements at all**, only different
//! things happening on different parts of the map at once. So there is one
//! tactical layer for the whole world: a single [`BattleState`] on a window
//! onto the world, holding every army in contact anywhere. An army enters it
//! when it makes contact and leaves it — folded back into a column where its
//! vehicles stand — when it has had no enemy within reach for as long as the
//! battle's own stalemate rule allows; the layer itself is disbanded when all
//! the fighting everywhere has stopped. Two fights that drift into each other
//! were always one battle, so there is nothing to merge; a fight whose sides
//! part is two groups of crews that leave when they are out of reach, so
//! there is nothing to split.
//!
//! **Nothing between rounds lives outside the battle.** The front's AI is
//! planned inside the engine, every round, by planners seeded from the
//! front's dice and the round number, so a save made mid-fight fights on the
//! same. A side a person commands, within her reach, is hers to order: the
//! clock waits for her (W4.4).

use crate::battle::{BattleState, SavedBattle, UnitId};
use crate::data::DataRegistry;
use crate::overworld::ElementId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The front's battle: live, or just off disk and waiting for
/// [`crate::overworld::OverworldState::rehydrate`] to put its caches back —
/// the same obligation a saved battle has, carried one level down.
#[derive(Debug, Clone)]
pub enum Fight {
    Live(Box<BattleState>),
    OffDisk(Box<SavedBattle>),
}

impl Serialize for Fight {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Fight::Live(b) => b.serialize(s),
            Fight::OffDisk(b) => b.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Fight {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Fight::OffDisk(Box::new(SavedBattle::deserialize(d)?)))
    }
}

/// All the fighting in the world, as one battle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Front {
    /// The clock tick the fighting began on.
    pub began: u64,
    /// Where its dice start, and its planners' seeds with them.
    pub seed: u64,
    /// The army each crew in the fighting belongs to, by unit id, in id
    /// order. A crew whose army has left is no longer here.
    pub origins: Vec<(UnitId, ElementId)>,
    /// Each army in the fighting and the last round any of its crews had an
    /// enemy within reach.
    pub contact: Vec<(ElementId, u32)>,
    pub fight: Fight,
    /// What happened on the clock's latest tick — the AI's orders and the
    /// tick's events — for a screen that is watching. News, not state.
    #[serde(skip)]
    pub recent: Vec<crate::battle::Event>,
}

impl Front {
    /// The battle, live.
    ///
    /// # Panics
    ///
    /// On a front read off disk and not yet rehydrated, which no campaign
    /// loaded through `SaveGame::from_json` can be.
    pub fn battle(&self) -> &BattleState {
        match &self.fight {
            Fight::Live(b) => b,
            Fight::OffDisk(_) => panic!("a front read off disk was never rehydrated"),
        }
    }

    /// The battle, live, to change. Panics as [`Self::battle`] does.
    pub fn battle_mut(&mut self) -> &mut BattleState {
        match &mut self.fight {
            Fight::Live(b) => b,
            Fight::OffDisk(_) => panic!("a front read off disk was never rehydrated"),
        }
    }

    /// Put the battle's caches back after a load.
    pub fn rehydrate(&mut self, registry: &DataRegistry) {
        if let Fight::OffDisk(saved) = &self.fight {
            let battle = (**saved).clone().rehydrate(registry);
            self.fight = Fight::Live(Box::new(battle));
        }
    }

    /// The army a crew in the fighting belongs to.
    pub fn origin(&self, unit: UnitId) -> Option<ElementId> {
        self.origins
            .binary_search_by_key(&unit, |(id, _)| *id)
            .ok()
            .map(|i| self.origins[i].1)
    }

    /// Every army in the fighting, in id order.
    pub fn armies(&self) -> Vec<ElementId> {
        let mut armies: Vec<ElementId> = self.contact.iter().map(|(a, _)| *a).collect();
        armies.sort_unstable();
        armies.dedup();
        armies
    }

    /// The crews of `army` in the fighting.
    pub fn crews_of(&self, army: ElementId) -> Vec<UnitId> {
        self.origins
            .iter()
            .filter(|(_, a)| *a == army)
            .map(|(u, _)| *u)
            .collect()
    }
}
