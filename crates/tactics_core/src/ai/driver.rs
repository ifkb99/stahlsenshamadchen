//! Driving planners against a battle: the loop everyone was writing by hand.
//!
//! Before this existed, "ask each AI side for orders until it commits" was
//! copied six times — the game's per-frame system, two examples, and three
//! test files — each with its own slightly different error handling. Chain of
//! command is about to make the thing standing behind a side considerably
//! richer (a commander planner owning per-formation subordinates), and that
//! hierarchy should be introduced in one place, not six.
//!
//! The contract is unchanged from what every copy already did: a planner is
//! asked for one [`Order`] at a time and the order is applied immediately, so
//! later decisions see earlier ones on the board. An order the battle refuses
//! force-commits the side — a confused planner must never wedge the round —
//! and the refusal is reported to the caller rather than swallowed, because a
//! planner producing illegal orders is a bug someone should get to see.
//!
//! Two granularities, one loop. [`AiDriver::step`] makes one decision per
//! uncommitted side and returns, which is what the game uses to stay
//! responsive between frames; [`AiDriver::plan_round`] drives every side it
//! controls to a commit, which is what headless callers want. Both funnel
//! through [`AiDriver::step_side`], so there is exactly one behaviour to get
//! right.

use super::AiPlanner;
use crate::battle::{BattleState, Order, OrderError};
use crate::data::DataRegistry;
use std::collections::HashMap;

/// Ceiling on decisions per side per planning phase. A full side is eight
/// units at two orders each plus the commit, so a planner that is still
/// talking after 64 has lost its way; cutting it off leaves the side
/// uncommitted, which the stalemate rules will eventually collect.
const MAX_ORDERS_PER_SIDE: usize = 64;

/// One planning decision by one side's planner, as applied to the battle.
/// Handed to observers so a caller can log or print the stream of orders
/// without owning the loop — the determinism baseline records every one of
/// these, which is why the shape is exact about rejections.
#[derive(Debug)]
pub struct Decision {
    pub side: u8,
    pub order: Order,
    /// Why the battle refused the order, if it did. A refusal force-commits
    /// the side.
    pub rejected: Option<OrderError>,
}

/// The AI planners driving a battle, one per side it controls. Sides absent
/// from the map are somebody else's — a human, usually — and are simply never
/// stepped.
#[derive(Default)]
pub struct AiDriver {
    planners: HashMap<u8, Box<dyn AiPlanner<BattleState, Order>>>,
}

impl AiDriver {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, side: u8, planner: Box<dyn AiPlanner<BattleState, Order>>) {
        self.planners.insert(side, planner);
    }

    /// Whether this driver speaks for `side`. The game asks this to work out
    /// which side is the human's.
    pub fn controls(&self, side: u8) -> bool {
        self.planners.contains_key(&side)
    }

    /// One decision by `side`'s planner, applied. `None` when there is
    /// nothing to do: the side is not ours, has committed, or the battle is
    /// not planning.
    pub fn step_side(
        &mut self,
        registry: &DataRegistry,
        state: &mut BattleState,
        side: u8,
    ) -> Option<Decision> {
        if state.is_over() || !state.is_planning() || state.has_committed(side) {
            return None;
        }
        let planner = self.planners.get_mut(&side)?;
        let order = planner.next_order(registry, state, side);
        let rejected = match state.apply(registry, &order) {
            Ok(_) => None,
            Err(error) => {
                // Never wedge the battle: commit what the side has and let
                // the round resolve. The error still reaches the caller.
                let _ = state.apply(registry, &Order::Commit { side });
                Some(error)
            }
        };
        Some(Decision {
            side,
            order,
            rejected,
        })
    }

    /// One decision for every uncommitted side this driver controls. Returns
    /// whether anything was decided, so a caller keeping derived state (the
    /// game's range overlays) knows the board's intents moved.
    pub fn step(&mut self, registry: &DataRegistry, state: &mut BattleState) -> bool {
        let mut acted = false;
        for side in state.living_sides() {
            acted |= self.step_side(registry, state, side).is_some();
        }
        acted
    }

    /// Drive every side this driver controls to a commit. Sides are planned
    /// one after another in side order; interleaving would change nothing,
    /// because planning orders only write intents and a side's planner never
    /// reads the other side's.
    pub fn plan_round(&mut self, registry: &DataRegistry, state: &mut BattleState) {
        self.plan_round_with(registry, state, |_| {});
    }

    /// [`Self::plan_round`], reporting every decision to `observe` — the
    /// playthrough example prints them, the determinism baseline records
    /// them byte for byte.
    pub fn plan_round_with(
        &mut self,
        registry: &DataRegistry,
        state: &mut BattleState,
        mut observe: impl FnMut(&Decision),
    ) {
        for side in state.living_sides() {
            for _ in 0..MAX_ORDERS_PER_SIDE {
                let Some(decision) = self.step_side(registry, state, side) else {
                    break;
                };
                let rejected = decision.rejected.is_some();
                observe(&decision);
                if rejected {
                    break;
                }
            }
        }
    }
}
