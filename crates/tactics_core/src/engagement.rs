//! Contact bubbles: where a campaign's armies stop being armies and are crews
//! on tiles, for as long as they are fighting (WORLD.md W3.2–W3.3).
//!
//! The designer's ruling is one world clock and a bubble that decides the
//! resolution. Off contact an army is a column on the campaign clock; in
//! contact its vehicles are placed on the real ground where it stands and
//! fight a [`BattleState`] on a window onto the world, one battle tick per
//! clock tick, while every other column goes on marching. When the fight is
//! over — elimination, or the battle's own stalemate rule after rounds with
//! nobody in contact — the survivors are folded back into their armies
//! *where they are*, with what they have left.
//!
//! **Nothing between rounds lives outside the battle.** An engagement's AI
//! is planned inside the engine, every round, by planners seeded from the
//! engagement's own dice and the round number, so there is no driver to keep
//! and a save made mid-fight fights on the same. A side a human commands
//! stops the clock for her orders once she can give them (W4); until then it
//! is planned by a stand-in like everybody else.

use crate::battle::{BattleState, SavedBattle, UnitId};
use crate::data::DataRegistry;
use crate::overworld::ArmyId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A battle in an engagement: live, or just off disk and waiting for
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

/// One fight going on somewhere on the world.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Engagement {
    pub id: u32,
    /// The clock tick it began on.
    pub began: u64,
    /// The army whose march ran into the other, and the one it ran into.
    pub attacker: ArmyId,
    pub defender: ArmyId,
    /// The army each unit in the fight came from, by unit id, in id order.
    pub origins: Vec<(UnitId, ArmyId)>,
    /// Where its dice start: `world::engagement_seed` of the world's seed
    /// and this fight's key. Its planners are seeded from it too.
    pub seed: u64,
    pub fight: Fight,
    /// What happened in it on the clock's latest tick — its AI's orders and
    /// the tick's events — for a screen that is watching it (WORLD.md W5).
    /// Not saved: it is news, not state.
    #[serde(skip)]
    pub recent: Vec<crate::battle::Event>,
}

impl Engagement {
    /// The battle, live.
    ///
    /// # Panics
    ///
    /// On an engagement read off disk and not yet rehydrated, which no
    /// campaign loaded through `SaveGame::from_json` can be.
    pub fn battle(&self) -> &BattleState {
        match &self.fight {
            Fight::Live(b) => b,
            Fight::OffDisk(_) => panic!("an engagement read off disk was never rehydrated"),
        }
    }

    /// The battle, live, to change. Panics as [`Self::battle`] does.
    pub fn battle_mut(&mut self) -> &mut BattleState {
        match &mut self.fight {
            Fight::Live(b) => b,
            Fight::OffDisk(_) => panic!("an engagement read off disk was never rehydrated"),
        }
    }

    /// Put the battle's caches back after a load.
    pub fn rehydrate(&mut self, registry: &DataRegistry) {
        if let Fight::OffDisk(saved) = &self.fight {
            let battle = (**saved).clone().rehydrate(registry);
            self.fight = Fight::Live(Box::new(battle));
        }
    }

    /// The army a unit fought for.
    pub fn origin(&self, unit: UnitId) -> Option<ArmyId> {
        self.origins
            .binary_search_by_key(&unit, |(id, _)| *id)
            .ok()
            .map(|i| self.origins[i].1)
    }

    /// Every army in the fight, in id order.
    pub fn armies(&self) -> Vec<ArmyId> {
        let mut armies: Vec<ArmyId> = self.origins.iter().map(|(_, a)| *a).collect();
        armies.sort_unstable();
        armies.dedup();
        armies
    }
}
