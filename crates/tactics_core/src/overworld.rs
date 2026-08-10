//! The strategic layer: armies move between objectives on a hex map,
//! capture income-producing tiles, and trigger battles when they clash.
//!
//! Information is softer than in battles: armies are visible to everyone
//! unless they sit in `concealing` terrain with no enemy adjacent.

use crate::ai::{AiConfig, AiPlanner};
use crate::data::{DataRegistry, MovementClass};
use crate::map::{HexMap, MapFile, MapKind};
use crate::roster::{CasualtyRules, GirlId, Roster, resolve_crew_fate};
use hexx::Hex;
use rand::seq::IndexedRandom;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ArmyId(pub u32);

impl ArmyId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverworldSide {
    pub name: String,
    pub funds: i32,
    /// `None` = human controlled.
    pub ai: Option<AiConfig>,
}

/// One crewed vehicle travelling with an army.
///
/// Distinct from [`crate::map::UnitPlacement`], which is *map file data*: a
/// placement names crew by definition id and carries a map coordinate that a
/// unit inside an army has no use for. This is live state — the crew are
/// [`GirlId`]s into the world's roster, so the same girls come out of a battle
/// as went in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmyUnit {
    pub vehicle: String,
    /// Who is aboard, as handles into [`OverworldState::roster`].
    pub crew: Vec<GirlId>,
    /// Display name override; otherwise the commander's name is used.
    pub name: Option<String>,
}

/// A girl who was aboard a vehicle when it was destroyed, and what destroyed
/// it — enough for the campaign to work out what became of her.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrewLoss {
    pub girl: GirlId,
    /// The vehicle she was in, for its [`crate::data::VehicleDef::safety`].
    pub vehicle: String,
    /// The damage type of the last hit the vehicle took, if the battle
    /// recorded one.
    pub killed_by: Option<crate::data::DamageType>,
}

/// What an army has been told to do, until it is told something else.
///
/// The campaign half of the vocabulary [`crate::battle::Mission`] speaks on
/// the battlefield, and deliberately the same shape: a plain snake_case serde
/// enum, so it survives a save, a replay and a round trip through a brain that
/// is not this process. What differs is what an operational order can mean —
/// there is no `Recon` here, because moving toward the enemy to find him *is*
/// an advance at four kilometres a hex, and reconnaissance is something the
/// battle it causes is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArmyMission {
    /// Move on `to` and take what is on the way. An army under this order
    /// drives at it a turn at a time and fights whatever stands in the road,
    /// because on the operational map going somewhere and attacking what is
    /// between you and it are the same act.
    Advance { to: Hex },
    /// Stand. The neutral order — what a reserve is given, and what says "no
    /// further" without cancelling the fact that orders exist at all.
    Hold,
    /// Fall back toward `to`. The movement is an advance's in reverse, but the
    /// order is not the same order: an army withdrawing carries that intent
    /// into any battle it is caught in, and its formations fight toward the
    /// way off the map rather than for the ground.
    Withdraw { to: Hex },
}

/// A stack of units moving as one piece on the strategic map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Army {
    pub id: ArmyId,
    pub side: u8,
    pub name: String,
    pub pos: Hex,
    pub movement: u32,
    pub moved: bool,
    /// Units that spawn into battles this army fights. Battle casualties
    /// are written back here.
    pub units: Vec<ArmyUnit>,
    pub alive: bool,
    /// Standing orders: what this army does with the turns nobody spends on
    /// it by hand.
    ///
    /// Standing in the strict sense the battle's formations already use — it
    /// is never cleared, not at the end of a turn and not when the army is cut
    /// off from its headquarters. An army told to advance on the bridge is
    /// still advancing on the bridge tomorrow, which is the entire reason the
    /// order is worth giving: it is what lets a campaign day be played by
    /// delegation rather than by moving every counter.
    ///
    /// `#[serde(default)]` so a campaign saved before missions existed opens
    /// as one whose armies have no orders, which is what it was.
    #[serde(default)]
    pub mission: Option<ArmyMission>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverworldOrder {
    /// Move toward `to`; moving onto a visible enemy army attacks it.
    MoveArmy {
        army: ArmyId,
        to: Hex,
    },
    /// Give an army its standing orders, replacing whatever it was doing.
    ///
    /// Through [`OverworldState::apply`] like every other order and for the
    /// same reason the battle's `SetMission` is: a mission that existed only
    /// inside a planner would be one the log cannot report, the save cannot
    /// carry and a replaced brain cannot see. Replacement is silent —
    /// countermanding is the ordinary business of command.
    SetMission {
        army: ArmyId,
        mission: ArmyMission,
    },
    EndTurn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverworldEvent {
    TurnStarted {
        side: u8,
        turn: u32,
    },
    Income {
        side: u8,
        amount: i32,
    },
    ArmyMoved {
        army: ArmyId,
        path: Vec<Hex>,
    },
    ObjectiveCaptured {
        at: Hex,
        side: u8,
    },
    /// What became of a girl whose vehicle was destroyed. The campaign layer
    /// shows these; the roster has already been updated.
    CrewCasualty {
        girl: GirlId,
        fate: crate::roster::CrewFate,
    },
    /// Two armies met; the game layer should run a battle and report the
    /// outcome back via [`OverworldState::apply_battle_result`].
    BattleTriggered {
        attacker: ArmyId,
        defender: ArmyId,
        at: Hex,
    },
    ArmyDestroyed {
        army: ArmyId,
    },
    /// An army was given standing orders. News in its own right: a mission is
    /// a decision somebody made, and the campaign log carries it beside the
    /// moves it will cause.
    ArmyMissionAssigned {
        army: ArmyId,
        mission: ArmyMission,
    },
    /// This army can no longer be reached by its side's headquarters. It keeps
    /// the orders it has and cannot be given new ones — an army silently
    /// ignoring the player is indistinguishable from a bug, so it is said out
    /// loud the turn it happens.
    ArmyOutOfContact {
        army: ArmyId,
    },
    ArmyContactRestored {
        army: ArmyId,
    },
    GameEnded {
        winner: Option<u8>,
    },
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum OverworldError {
    #[error("the campaign is over")]
    GameOver,
    #[error("army does not exist or was destroyed")]
    NoSuchArmy,
    #[error("it is not that side's turn")]
    NotYourTurn,
    #[error("army has already moved")]
    AlreadyMoved,
    #[error("no valid path to the destination")]
    NoPath,
    /// Named as the battle layer names it ([`crate::battle::OrderError::NotOnMap`]),
    /// because it is the same refusal at a different scale and two words for
    /// it would be two things to learn.
    #[error("tile is not on the map")]
    NotOnMap,
    #[error("army is out of radio contact and cannot be given new orders")]
    OutOfContact,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverworldState {
    pub map: Arc<HexMap>,
    pub sides: Vec<OverworldSide>,
    /// Every girl in the world, whichever academy she belongs to.
    ///
    /// One roster rather than one per side, so a [`GirlId`] means the same
    /// thing everywhere and girls can change hands without anything being
    /// renumbered — which is what an academy-scale mode will want.
    pub roster: Roster,
    /// Whether this campaign is willing to kill its characters.
    pub rules: CasualtyRules,
    pub armies: Vec<Army>,
    /// Owner side of each captured objective tile.
    #[serde(with = "crate::map::hex_keyed")]
    pub owners: HashMap<Hex, u8>,
    pub turn: u32,
    pub active_side: u8,
    /// Armies nobody at headquarters can reach, sorted by id.
    ///
    /// Parallel to the battle's `Formation::out_of_contact` and empty for the
    /// same reason: a mod that declares no `command` block never has this
    /// recomputed, so every army is in contact, no order is ever refused for
    /// being unreachable and no event is emitted. The absence of the system is
    /// the campaign as it was, with no branch in Rust to switch it off.
    ///
    /// No per-army snapshot of orders is kept, unlike the battle's [`CutOff`](crate::battle::CutOff):
    /// an army's mission cannot change behind its back, because the only thing
    /// that could change it is refused while it is cut off. Standing orders and
    /// carried orders are therefore the same thing here.
    #[serde(default)]
    pub out_of_contact: Vec<ArmyId>,
    pub over: Option<Option<u8>>,
    /// Drives casualty resolution. Seeded, and consumed in a fixed order, so
    /// a campaign replays identically.
    pub rng: ChaCha8Rng,
}

#[derive(Debug, thiserror::Error)]
pub enum OverworldSetupError {
    #[error("map `{0}` not found in registry")]
    MissingMap(String),
    #[error("map `{0}` is not an overworld map")]
    NotAnOverworldMap(String),
    #[error(transparent)]
    Map(#[from] crate::map::MapError),
}

/// Armies traverse the overworld as tracked columns with generous climb.
const ARMY_CLASS: MovementClass = MovementClass::Tracked;
const ARMY_CLIMB: i32 = 9;

impl OverworldState {
    pub fn from_map(
        registry: &DataRegistry,
        map_id: &str,
        seed: u64,
    ) -> Result<Self, OverworldSetupError> {
        let file: &MapFile = registry
            .map(map_id)
            .ok_or_else(|| OverworldSetupError::MissingMap(map_id.to_string()))?;
        if file.kind != MapKind::Overworld {
            return Err(OverworldSetupError::NotAnOverworldMap(map_id.to_string()));
        }
        let map = HexMap::from_map_file(file)?;
        let sides = file
            .sides
            .iter()
            .map(|s| OverworldSide {
                name: s.name.clone(),
                funds: s.funds,
                ai: s.ai.clone(),
            })
            .collect();
        // Enlisting as the armies are built is what turns map data into
        // people: every crew id named by the file becomes a girl belonging to
        // that army's academy, and nothing refers to a definition again.
        let mut roster = Roster::new();
        let armies = file
            .armies
            .iter()
            .enumerate()
            .map(|(i, a)| Army {
                id: ArmyId(i as u32),
                side: a.side,
                name: a.name.clone(),
                pos: crate::offset_to_hex(a.at[0], a.at[1]),
                movement: a.movement,
                moved: false,
                units: a
                    .units
                    .iter()
                    .map(|u| ArmyUnit {
                        vehicle: u.vehicle.clone(),
                        crew: u
                            .crew
                            .iter()
                            .filter_map(|def_id| {
                                roster.enlist_from_registry(registry, a.side, def_id)
                            })
                            .collect(),
                        name: u.name.clone(),
                    })
                    .collect(),
                alive: true,
                mission: None,
            })
            .collect();
        let mut state = Self {
            map: Arc::new(map),
            sides,
            roster,
            rules: CasualtyRules::default(),
            armies,
            owners: HashMap::new(),
            turn: 1,
            active_side: 0,
            out_of_contact: Vec::new(),
            over: None,
            rng: ChaCha8Rng::seed_from_u64(seed),
        };
        // Who can hear whom on the morning of day one. The events are dropped
        // because nothing has *changed* yet — an army that starts the campaign
        // outside the net was deployed there, it did not lose contact — but the
        // set itself has to be right from the first order, or a mission the
        // wire could never carry would be accepted on day one and refused on
        // day two for no reason the player could see.
        let mut unheard = Vec::new();
        state.recompute_contact(registry, state.active_side, &mut unheard);
        Ok(state)
    }

    pub fn army(&self, id: ArmyId) -> Option<&Army> {
        self.armies.get(id.index()).filter(|a| a.alive)
    }

    pub fn army_mut(&mut self, id: ArmyId) -> Option<&mut Army> {
        self.armies.get_mut(id.index()).filter(|a| a.alive)
    }

    pub fn army_at(&self, hex: Hex) -> Option<&Army> {
        self.armies.iter().find(|a| a.alive && a.pos == hex)
    }

    pub fn side_armies(&self, side: u8) -> impl Iterator<Item = &Army> {
        self.armies
            .iter()
            .filter(move |a| a.alive && a.side == side)
    }

    /// The army a side's signals net is rooted at: its first-declared living
    /// one, by [`ArmyId`].
    ///
    /// **A documented placeholder.** Contact ought to root at a *person* — the
    /// side's commanding girl, sitting in a headquarters or a command vehicle
    /// with a radius priced on her crew's `signals`, the way a battle
    /// formation's net is priced on its leader. Neither the command unit nor
    /// the academy that would issue her exists yet (TODO.md, Chain of Command:
    /// the command-unit item), so seniority stands in for command, exactly as
    /// the battle layer's succession rule does: the first army the map wrote
    /// down is the one carrying the headquarters. When the command unit
    /// arrives, this function is the only thing that has to change.
    pub fn senior_army(&self, side: u8) -> Option<ArmyId> {
        self.side_armies(side).map(|a| a.id).min()
    }

    /// Whether this army can still be given new orders.
    ///
    /// True for everything in a campaign whose mod declares no `command`
    /// block, and true for an army that no longer exists — the question a
    /// caller is asking is "will an order reach her", and one that cannot be
    /// given for some other reason is refused for that other reason.
    pub fn in_contact(&self, army: ArmyId) -> bool {
        !self.out_of_contact.contains(&army)
    }

    /// Work out which of `side`'s armies headquarters can still reach, and say
    /// so when the answer changes.
    ///
    /// The graph is the battle's, one scale up: a breadth-first walk from the
    /// [senior army](Self::senior_army), everybody inside the radius hearing
    /// her, and — with `relay` on — passing the signal along at their own
    /// radius, which is what makes a chain of companies strung across the map
    /// worth keeping joined up. Armies are visited in id order at every step,
    /// so both the resulting set and the events are the same on every machine.
    ///
    /// Recomputed for **one side at a time**, at the top of that side's turn.
    /// That is when it matters — the gate on new orders reads the active
    /// side's list, and nothing has moved since — and it is what keeps the
    /// campaign log from announcing the enemy's radio troubles to a player who
    /// has no business knowing them. Entries for other sides are carried over
    /// untouched, minus any army that has since been destroyed.
    ///
    /// Does nothing at all when no mod declares command rules: `out_of_contact`
    /// then stays empty for the whole campaign, [`Self::in_contact`] answers
    /// `true` for everybody, and nothing costs anything.
    fn recompute_contact(
        &mut self,
        registry: &DataRegistry,
        side: u8,
        events: &mut Vec<OverworldEvent>,
    ) {
        let Some(rules) = registry.command.as_ref() else {
            return;
        };
        let radius = rules.overworld_radius as i32;
        let mut heard: Vec<ArmyId> = Vec::new();
        if let Some(root) = self.senior_army(side) {
            heard.push(root);
            let mut anchors = VecDeque::from([root]);
            while let Some(anchor) = anchors.pop_front() {
                let Some(from) = self.army(anchor).map(|a| a.pos) else {
                    continue;
                };
                for army in self.side_armies(side) {
                    if heard.contains(&army.id) || army.pos.distance_to(from) > radius {
                        continue;
                    }
                    heard.push(army.id);
                    if rules.relay {
                        anchors.push_back(army.id);
                    }
                }
                if !rules.relay {
                    // Without relay the senior army is the only voice; nobody
                    // she reached extends the net.
                    break;
                }
            }
        }

        let mut next: Vec<ArmyId> = self
            .out_of_contact
            .iter()
            .copied()
            .filter(|id| self.army(*id).is_some_and(|a| a.side != side))
            .collect();
        next.extend(
            self.side_armies(side)
                .map(|a| a.id)
                .filter(|id| !heard.contains(id)),
        );
        next.sort_unstable();
        let was = std::mem::replace(&mut self.out_of_contact, next);
        for id in &self.out_of_contact {
            if !was.contains(id) {
                events.push(OverworldEvent::ArmyOutOfContact { army: *id });
            }
        }
        for id in &was {
            // Still on the map: an army that came back into contact by being
            // destroyed is not news anybody wants twice.
            if !self.out_of_contact.contains(id) && self.army(*id).is_some() {
                events.push(OverworldEvent::ArmyContactRestored { army: *id });
            }
        }
    }

    /// Soft fog: an army is hidden from `observer` only while it sits in
    /// concealing terrain with no enemy army adjacent.
    pub fn army_visible_to(&self, registry: &DataRegistry, army: &Army, observer: u8) -> bool {
        if army.side == observer {
            return true;
        }
        let concealed = self
            .map
            .get(army.pos)
            .and_then(|t| registry.terrain(&t.terrain))
            .is_some_and(|t| t.concealing);
        if !concealed {
            return true;
        }
        self.side_armies(observer)
            .any(|a| a.pos.distance_to(army.pos) <= 1)
    }

    pub fn visible_armies<'s>(&'s self, registry: &DataRegistry, observer: u8) -> Vec<&'s Army> {
        self.armies
            .iter()
            .filter(|a| a.alive && self.army_visible_to(registry, a, observer))
            .collect()
    }

    fn edge_cost(&self, registry: &DataRegistry, from: Hex, to: Hex) -> Option<u32> {
        crate::battle::movement_edge_cost(registry, &self.map, ARMY_CLASS, ARMY_CLIMB, from, to)
    }

    /// Every tile this army could end its move on, with the cheapest cost to
    /// get there. Other armies block both movement and parking, matching
    /// what [`Self::apply_move`] will actually allow. Ignores whether the
    /// army has already moved, so callers can preview a spent army's reach.
    pub fn reachable(&self, registry: &DataRegistry, id: ArmyId) -> HashMap<Hex, u32> {
        let Some(army) = self.army(id) else {
            return HashMap::new();
        };
        let mut best: HashMap<Hex, u32> = HashMap::new();
        let mut heap = BinaryHeap::new();
        best.insert(army.pos, 0);
        heap.push((Reverse(0u32), army.pos.x, army.pos.y));

        while let Some((Reverse(cost), x, y)) = heap.pop() {
            let hex = Hex::new(x, y);
            if best.get(&hex).is_some_and(|&c| c < cost) {
                continue;
            }
            for next in hex.all_neighbors() {
                if self.army_at(next).is_some() {
                    continue;
                }
                let Some(step) = self.edge_cost(registry, hex, next) else {
                    continue;
                };
                let total = cost + step;
                if total > army.movement {
                    continue;
                }
                if best.get(&next).is_none_or(|&c| total < c) {
                    best.insert(next, total);
                    heap.push((Reverse(total), next.x, next.y));
                }
            }
        }
        best
    }

    /// Visible enemy armies this army could engage this turn: those sitting
    /// next to a tile it can reach (or next to where it already stands).
    pub fn attack_targets(&self, registry: &DataRegistry, id: ArmyId) -> Vec<ArmyId> {
        let Some(army) = self.army(id) else {
            return Vec::new();
        };
        let reach = self.reachable(registry, id);
        self.armies
            .iter()
            .filter(|e| e.alive && e.side != army.side)
            .filter(|e| self.army_visible_to(registry, e, army.side))
            .filter(|e| {
                e.pos.distance_to(army.pos) == 1
                    || reach.keys().any(|hex| hex.distance_to(e.pos) == 1)
            })
            .map(|e| e.id)
            .collect()
    }

    /// Armies that may pile into a battle at `at` alongside `principal`.
    ///
    /// Attackers must still have their move in hand, since joining an
    /// assault is what they spend their turn on. Defenders answer whatever
    /// they have been up to: holding ground where you already stand is not
    /// a separate action.
    pub fn reinforcement_candidates(
        &self,
        at: Hex,
        side: u8,
        principal: ArmyId,
        attacking: bool,
    ) -> Vec<ArmyId> {
        self.armies
            .iter()
            .filter(|a| a.alive && a.side == side && a.id != principal)
            .filter(|a| a.pos.distance_to(at) <= 1)
            .filter(|a| !attacking || !a.moved)
            .map(|a| a.id)
            .collect()
    }

    /// Spend the turn of every army committed to a battle.
    pub fn commit_to_battle(&mut self, armies: &[ArmyId]) {
        for id in armies {
            if let Some(army) = self.army_mut(*id) {
                army.moved = true;
            }
        }
    }

    pub fn apply(
        &mut self,
        registry: &DataRegistry,
        order: &OverworldOrder,
    ) -> Result<Vec<OverworldEvent>, OverworldError> {
        if self.over.is_some() {
            return Err(OverworldError::GameOver);
        }
        let mut events = match order {
            OverworldOrder::MoveArmy { army, to } => self.apply_move(registry, *army, *to)?,
            OverworldOrder::SetMission { army, mission } => {
                self.apply_set_mission(*army, mission.clone())?
            }
            OverworldOrder::EndTurn => self.apply_end_turn(registry),
        };
        self.check_victory(&mut events);
        Ok(events)
    }

    /// Record an army's standing orders, refusing anything it could not
    /// actually be asked to do.
    ///
    /// Validated in the order a person would ask: does the army exist, is it
    /// yours to command today, is what you are pointing at on the map, and —
    /// last, because it is the only one that is about the *wire* rather than
    /// the order — can the order reach it at all. Whether the ground is
    /// reachable, whether the road is held, whether the withdrawal is wise:
    /// none of that is checked, for the same reason the battle's `set_mission`
    /// does not check it. A mission is an intention, not a route.
    fn apply_set_mission(
        &mut self,
        id: ArmyId,
        mission: ArmyMission,
    ) -> Result<Vec<OverworldEvent>, OverworldError> {
        let army = self.army(id).ok_or(OverworldError::NoSuchArmy)?;
        if army.side != self.active_side {
            return Err(OverworldError::NotYourTurn);
        }
        match &mission {
            ArmyMission::Advance { to } | ArmyMission::Withdraw { to } => {
                if !self.map.contains(*to) {
                    return Err(OverworldError::NotOnMap);
                }
            }
            // "Stand where you are" needs no tile to exist.
            ArmyMission::Hold => {}
        }
        if !self.in_contact(id) {
            return Err(OverworldError::OutOfContact);
        }
        // Silent replacement, exactly as the battle layer does it: a formation
        // — or an army — holding two missions at once has no meaning anybody
        // could act on. What is news is that an order was given at all, and
        // that is the event.
        self.army_mut(id).expect("checked above").mission = Some(mission.clone());
        Ok(vec![OverworldEvent::ArmyMissionAssigned {
            army: id,
            mission,
        }])
    }

    fn apply_move(
        &mut self,
        registry: &DataRegistry,
        id: ArmyId,
        to: Hex,
    ) -> Result<Vec<OverworldEvent>, OverworldError> {
        let (side, pos, movement, moved) = {
            let army = self.army(id).ok_or(OverworldError::NoSuchArmy)?;
            (army.side, army.pos, army.movement, army.moved)
        };
        if side != self.active_side {
            return Err(OverworldError::NotYourTurn);
        }
        if moved {
            return Err(OverworldError::AlreadyMoved);
        }

        let path = hexx::algorithms::a_star(pos, to, |from, next| {
            if from == next {
                return Some(0);
            }
            // Armies may not path through any other army; ending on an
            // enemy is the attack case, handled below.
            if next != to && self.army_at(next).is_some() {
                return None;
            }
            self.edge_cost(registry, from, next)
        })
        .ok_or(OverworldError::NoPath)?;

        // Trim the path to this turn's movement budget.
        let mut budget = movement;
        let mut walked = vec![pos];
        for pair in path.windows(2) {
            let Some(step) = self.edge_cost(registry, pair[0], pair[1]) else {
                break;
            };
            if step > budget {
                break;
            }
            // Stop short of any occupied tile; battle triggers if hostile.
            if self.army_at(pair[1]).is_some() {
                break;
            }
            budget -= step;
            walked.push(pair[1]);
        }
        let destination = *walked.last().expect("path starts at pos");

        let mut events = Vec::new();
        {
            let army = self.army_mut(id).expect("checked above");
            army.pos = destination;
            army.moved = true;
        }
        events.push(OverworldEvent::ArmyMoved {
            army: id,
            path: walked,
        });

        // Capture objectives by standing on them.
        if let Some(tile) = self.map.get(destination)
            && registry
                .terrain(&tile.terrain)
                .is_some_and(|t| t.capturable)
            && self.owners.get(&destination) != Some(&side)
        {
            self.owners.insert(destination, side);
            events.push(OverworldEvent::ObjectiveCaptured {
                at: destination,
                side,
            });
        }

        // If we stopped adjacent to the ordered destination because an
        // enemy holds it, that's an attack.
        if let Some(defender) = self.army_at(to)
            && defender.side != side
            && destination.distance_to(to) == 1
        {
            events.push(OverworldEvent::BattleTriggered {
                attacker: id,
                defender: defender.id,
                at: to,
            });
        }
        Ok(events)
    }

    /// Carry out the standing orders of every army whose turn nobody spent by
    /// hand. This is delegation, and it is the whole reason army missions
    /// exist: a campaign day should be playable by telling four companies what
    /// you want and pressing end-turn, not by walking each of them across the
    /// map every day.
    ///
    /// The guard is `moved`, so an army the player drove somewhere herself is
    /// never second-guessed by its own orders — hers is the newer decision.
    /// An army already standing on its objective has arrived and does nothing
    /// further; one under `Hold` was told to do nothing in the first place.
    ///
    /// A move that cannot be made this turn is skipped **without clearing the
    /// mission**: the road may be blocked by a friend, or the only path may
    /// run through an enemy that will not be there tomorrow. An army that
    /// cannot comply today tries again tomorrow, which is what a standing
    /// order means; forgetting it because of one bad day would be the system
    /// quietly deciding the player did not mean it.
    ///
    /// Everything it does goes through [`Self::apply_move`], so a mission move
    /// captures ground, triggers battles and stops short of enemies in exactly
    /// the way a hand-ordered one does. There is deliberately no second path
    /// for the AI to drive an army along.
    fn run_standing_missions(&mut self, registry: &DataRegistry) -> Vec<OverworldEvent> {
        let side = self.active_side;
        let ordered: Vec<(ArmyId, Hex)> = self
            .armies
            .iter()
            .filter(|a| a.alive && a.side == side && !a.moved)
            .filter_map(|a| match a.mission.as_ref()? {
                ArmyMission::Advance { to } | ArmyMission::Withdraw { to } => Some((a.id, *to)),
                ArmyMission::Hold => None,
            })
            .collect();
        let mut events = Vec::new();
        for (id, to) in ordered {
            // Arrived, destroyed since the list was taken, or overtaken by a
            // battle that spent its turn: nothing to do either way.
            if self.army(id).is_none_or(|a| a.moved || a.pos == to) {
                continue;
            }
            if let Ok(more) = self.apply_move(registry, id, to) {
                events.extend(more);
            }
        }
        events
    }

    fn apply_end_turn(&mut self, registry: &DataRegistry) -> Vec<OverworldEvent> {
        // Before the day turns over, everybody who was told what to do and not
        // told otherwise does it.
        let mut events = self.run_standing_missions(registry);
        let side_count = self.sides.len() as u8;
        let mut next = self.active_side;
        for _ in 0..side_count {
            next = (next + 1) % side_count;
            if self.side_armies(next).next().is_some() {
                break;
            }
        }
        if next <= self.active_side {
            self.turn += 1;
            // A new day: wounds heal and girls walking back from a wreck get
            // one day closer. Once per day rather than once per side's phase,
            // or a two-academy campaign would heal twice as fast as a four.
            self.roster.advance_day();
        }
        self.active_side = next;
        for army in self.armies.iter_mut().filter(|a| a.alive && a.side == next) {
            army.moved = false;
        }

        events.push(OverworldEvent::TurnStarted {
            side: next,
            turn: self.turn,
        });
        let amount: i32 = self
            .owners
            .iter()
            .filter(|(_, owner)| **owner == next)
            .filter_map(|(hex, _)| self.map.get(*hex))
            .filter_map(|tile| registry.terrain(&tile.terrain))
            .map(|t| t.income)
            .sum();
        if amount > 0 {
            self.sides[next as usize].funds += amount;
            events.push(OverworldEvent::Income { side: next, amount });
        }
        // Last, with everybody standing where the night left them: who can be
        // reached today decides which orders may be given today.
        self.recompute_contact(registry, next, &mut events);
        events
    }

    /// Feed a battle outcome back into the strategic layer. Every
    /// participating army gets its surviving roster back; armies that lost
    /// everything are destroyed, and a victorious attacker advances onto the
    /// contested tile.
    pub fn apply_battle_result(
        &mut self,
        registry: &DataRegistry,
        attacker: ArmyId,
        defender: ArmyId,
        survivors: &[(ArmyId, Vec<ArmyUnit>)],
        losses: &[CrewLoss],
    ) -> Vec<OverworldEvent> {
        let mut events = Vec::new();
        let defender_pos = self.army(defender).map(|a| a.pos);

        // Casualties first, so a girl's fate is settled before the surviving
        // rosters are written back. Resolved in girl-id order rather than the
        // order the battle happened to report them, because the rng is shared
        // and the campaign has to replay identically.
        let mut losses: Vec<&CrewLoss> = losses.iter().collect();
        losses.sort_by_key(|loss| loss.girl);
        for loss in losses {
            let safety = registry
                .vehicle(&loss.vehicle)
                .map(|v| v.safety)
                .unwrap_or(3);
            let fate = resolve_crew_fate(self.rules, safety, loss.killed_by, &mut self.rng);
            if let Some(girl) = self.roster.get_mut(loss.girl) {
                girl.status = fate.into();
            }
            events.push(OverworldEvent::CrewCasualty {
                girl: loss.girl,
                fate,
            });
        }

        // Everyone who came through it has one more battle behind her.
        for (_, units) in survivors {
            for unit in units {
                for girl in &unit.crew {
                    self.roster.credit_battle(*girl);
                }
            }
        }

        for (id, units) in survivors {
            let id = *id;
            if let Some(army) = self.army_mut(id) {
                army.units = units.clone();
                if army.units.is_empty() {
                    army.alive = false;
                    events.push(OverworldEvent::ArmyDestroyed { army: id });
                }
            }
        }

        if let (Some(pos), Some(att)) = (defender_pos, self.army(attacker))
            && self.army(defender).is_none()
        {
            let att_id = att.id;
            if let Some(a) = self.army_mut(att_id) {
                a.pos = pos;
            }
        }
        self.check_victory(&mut events);
        events
    }

    fn check_victory(&mut self, events: &mut Vec<OverworldEvent>) {
        if self.over.is_some() {
            return;
        }
        let mut living: Vec<u8> = self
            .armies
            .iter()
            .filter(|a| a.alive)
            .map(|a| a.side)
            .collect();
        living.sort_unstable();
        living.dedup();
        if living.len() <= 1 {
            let winner = living.first().copied();
            self.over = Some(winner);
            events.push(OverworldEvent::GameEnded { winner });
        }
    }
}

/// Which nearby armies an AI side throws into a battle. Concentration of
/// force is almost always right here -- a battle is fought to the death, so
/// arriving outnumbered is the main way to lose one -- but this is a
/// separate function so smarter (or more cowardly) doctrines can replace it.
pub fn ai_reinforcements(
    state: &OverworldState,
    at: Hex,
    side: u8,
    principal: ArmyId,
    attacking: bool,
) -> Vec<ArmyId> {
    state.reinforcement_candidates(at, side, principal, attacking)
}

/// Baseline overworld AI: push each army toward the most valuable visible
/// target (weak enemy armies and uncaptured objectives).
///
/// It issues [`OverworldOrder::MoveArmy`] and nothing else, deliberately.
/// Missions are how *someone else* — a player, or one day a campaign brain
/// that plans in weeks rather than days — tells an army what to do with the
/// turns nobody spends on it; this planner is the side's own hand and spends
/// every turn itself, so telling its armies what to do and then doing it for
/// them would be the same decision made twice.
pub struct SimpleOverworldPlanner {
    rng: ChaCha8Rng,
    /// Chance to pick a suboptimal target, derived from difficulty.
    blunder: f64,
}

impl SimpleOverworldPlanner {
    pub fn with_difficulty(difficulty: u8, seed: u64) -> Self {
        Self {
            rng: ChaCha8Rng::seed_from_u64(seed),
            blunder: match difficulty {
                1 => 0.5,
                2 => 0.3,
                3 => 0.15,
                4 => 0.05,
                _ => 0.0,
            },
        }
    }
}

pub fn make_overworld_planner(
    config: &AiConfig,
    seed: u64,
) -> Box<dyn AiPlanner<OverworldState, OverworldOrder>> {
    Box::new(SimpleOverworldPlanner::with_difficulty(
        config.difficulty.clamp(1, 5),
        seed,
    ))
}

impl AiPlanner<OverworldState, OverworldOrder> for SimpleOverworldPlanner {
    fn next_order(
        &mut self,
        registry: &DataRegistry,
        state: &OverworldState,
        side: u8,
    ) -> OverworldOrder {
        let Some(army) = state.side_armies(side).find(|a| !a.moved) else {
            return OverworldOrder::EndTurn;
        };

        let mut targets: Vec<(Hex, f32)> = Vec::new();
        // Enemy armies we can see: value inversely proportional to size.
        for enemy in state.visible_armies(registry, side) {
            if enemy.side != side {
                let strength = enemy.units.len().max(1) as f32;
                let ours = army.units.len().max(1) as f32;
                targets.push((enemy.pos, 6.0 * (ours / strength)));
            }
        }
        // Objectives we don't own.
        for (hex, tile) in state.map.iter() {
            let Some(t) = registry.terrain(&tile.terrain) else {
                continue;
            };
            if t.capturable && state.owners.get(&hex) != Some(&side) {
                targets.push((hex, 4.0 + t.income as f32 * 0.5));
            }
        }
        if targets.is_empty() {
            return OverworldOrder::EndTurn;
        }

        targets.sort_by(|a, b| {
            let da = army.pos.distance_to(a.0) as f32 - a.1;
            let db = army.pos.distance_to(b.0) as f32 - b.1;
            da.total_cmp(&db)
        });
        let pick = if targets.len() > 1 && self.rng.random_bool(self.blunder) {
            targets[1..].choose(&mut self.rng).copied()
        } else {
            None
        };
        let (dest, _) = pick.unwrap_or(targets[0]);
        OverworldOrder::MoveArmy {
            army: army.id,
            to: dest,
        }
    }
}
