//! The strategic layer: armies move between objectives on a hex map,
//! capture the ground the map says is worth holding, and trigger battles when
//! they clash.
//!
//! Information is softer than in battles: armies are visible to everyone
//! unless they sit in `concealing` terrain with no enemy adjacent.

use crate::ai::{AiConfig, AiPlanner};
use crate::battle::UnitId;
use crate::data::{DataRegistry, MovementClass};
use crate::map::{CampaignVictory, HexMap, MapFile, MapKind};
use crate::roster::{CadetId, CasualtyRules, Roster, resolve_crew_fate, resolve_station_fate};
use crate::worldgen::GeneratedWorld;
use hexx::Hex;
use rand::seq::IndexedRandom;
use rand::{RngExt, SeedableRng};
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::sync::Arc;

/// A node in a side's chain of command: the side itself, a company, a
/// vehicle (see [`Element`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ElementId(pub u32);

impl ElementId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverworldSide {
    pub name: String,
    /// `None` = human controlled.
    pub ai: Option<AiConfig>,
    /// The cadet who commands this side (WORLD.md W4.1): the senior cadet of
    /// the vehicle its campaign map flagged `command`, fixed when the
    /// campaign begins. For the player's side, the player.
    #[serde(default)]
    pub commander: Option<CadetId>,
}

/// One crewed vehicle travelling with an army.
///
/// Distinct from [`crate::map::UnitPlacement`], which is *map file data*: a
/// placement names crew by definition id and carries a map coordinate that a
/// unit inside an army has no use for. This is live state — the crew are
/// [`CadetId`]s into the world's roster, so the same cadets come out of a battle
/// as went in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArmyUnit {
    pub vehicle: String,
    /// Who is aboard, as handles into [`OverworldState::roster`].
    pub crew: Vec<CadetId>,
    /// Display name override; otherwise the commander's name is used.
    pub name: Option<String>,
}

/// What one battle did to one cadet — enough for the campaign to work out
/// how long it keeps her out.
///
/// Named for the common case rather than the whole of it: she is a loss to
/// the order of battle for some number of days, which is exactly what the
/// campaign does with this. Two very different things arrive as one struct
/// because the campaign's job is the same either way, and [`Self::found`] is
/// what tells them apart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrewLoss {
    pub cadet: CadetId,
    /// The vehicle she was in, for its [`crate::data::VehicleDef::safety`].
    pub vehicle: String,
    /// The damage type of the last hit the vehicle took, if the battle
    /// recorded one. Only consulted when the vehicle did not come home.
    pub killed_by: Option<crate::data::DamageType>,
    /// How she was found when the shooting stopped, for a vehicle that came
    /// home to be looked in.
    ///
    /// `None` is the case this struct was originally written for and is
    /// still the worst one: her vehicle did not come back, so nobody looked
    /// in the seat and the fate rolls have to work out what became of her
    /// from what killed it.
    ///
    /// `#[serde(default)]` so a campaign saved before wounds outlived their
    /// battle opens as one where every reported cadet was in a wreck, which
    /// is what it was.
    #[serde(default)]
    pub found: Option<crate::battle::CrewCondition>,
    /// The best `first_aid` still working aboard her vehicle, **not counting
    /// her own** — she is the one bleeding.
    ///
    /// What it buys is severity, through
    /// [`crate::data::Casualties::severe_per_aid`] and its homecoming twin.
    /// [`crate::data::AVERAGE`] when there is nobody left to help her, which
    /// is no reduction rather than a penalty: the rule's absence is the game
    /// before it, and a crew of one is already being punished by every other
    /// rule in the model.
    ///
    /// `#[serde(default)]` to average, so a campaign saved before anybody
    /// could be patched up opens as one where everybody got ordinary care.
    #[serde(default = "average_aid")]
    pub aid: i32,
}

/// Serde's default for [`CrewLoss::aid`].
fn average_aid() -> i32 {
    crate::data::AVERAGE
}

impl CrewLoss {
    /// Everyone a finished battle hurt, in the two kinds it hurts people.
    ///
    /// Pure over the battle, and in this crate rather than in the one that
    /// runs the field battle because it is the *campaign's* question — who
    /// does the academy have to account for — and because two callers now ask
    /// it. The game crate builds a [`BattleReport`] with it when a field
    /// battle ends; the balance harness asks it of battles nobody is playing,
    /// to find out what the casualty table costs a school. A second reading
    /// of "who is a casualty" written beside either one would be a second
    /// game, and the harness would eventually be measuring it.
    ///
    /// The two kinds are genuinely different questions and only [`Self::found`]
    /// tells them apart:
    ///
    /// - **Her vehicle did not come home.** Nobody looked in the seat, so
    ///   what became of her has to be worked out from what killed it —
    ///   `killed_by` and the chassis's `safety`, through
    ///   [`crate::roster::resolve_crew_fate`].
    /// - **She was hurt at her station in a vehicle that did.** The battle
    ///   tracked her condition seat by seat all fight and has already
    ///   answered whether she is hurt; all that is left is how long it keeps
    ///   her out ([`crate::roster::resolve_station_fate`]). This half used to
    ///   be thrown away at the door, so a gunner knocked out in the first
    ///   round of a battle her side won was fit again by the time the
    ///   campaign screen drew.
    ///
    /// [`crate::battle::CrewCondition::Absent`] is skipped for the reason it
    /// exists: she was in the infirmary before this battle started and is not
    /// a casualty of it.
    pub fn in_battle(
        registry: &crate::data::DataRegistry,
        state: &crate::battle::BattleState,
    ) -> Vec<Self> {
        use crate::battle::CrewCondition;
        let mut losses = Vec::new();
        // Who else aboard could do anything for her. Her own level is
        // deliberately not in it, and a seat nobody is working or that is
        // working hurt past use cannot help: `Out` and `Absent` are not
        // there, `Wounded` still is, because a cadet with a field dressing
        // can put one on somebody else.
        let aid = |unit: &crate::battle::Unit, seat: usize| -> i32 {
            let ctx = crate::data::CheckContext {
                vehicle_class: registry.vehicle(&unit.vehicle).map(|v| v.class.as_str()),
                ..Default::default()
            };
            unit.crew
                .iter()
                .enumerate()
                .filter(|(other, _)| *other != seat)
                .filter(|(other, _)| {
                    !matches!(
                        unit.crew_state.get(*other),
                        Some(CrewCondition::Out) | Some(CrewCondition::Absent)
                    )
                })
                .filter_map(|(_, id)| state.roster.skill_level(registry, *id, "first_aid", &ctx))
                .max()
                .unwrap_or(crate::data::AVERAGE)
        };
        for unit in state.lost_units() {
            for (seat, cadet) in unit.crew.iter().enumerate() {
                // She was in the infirmary when this vehicle burned, so
                // nothing that happened to it happened to her. An empty
                // `crew_state` is a crew nobody stayed behind from, which is
                // every battle written before wounds could keep anyone out.
                if unit.crew_state.get(seat) == Some(&CrewCondition::Absent) {
                    continue;
                }
                losses.push(Self {
                    cadet: *cadet,
                    vehicle: unit.vehicle.clone(),
                    killed_by: unit.last_hit_by,
                    found: None,
                    aid: aid(unit, seat),
                });
            }
        }
        // `surviving_units`, not `alive_units`: a crew that drove off the map
        // by an exit came home too, and a cadet wounded aboard her is owed the
        // same look in the seat.
        for unit in state.surviving_units() {
            for (seat, cadet) in unit.crew.iter().enumerate() {
                let found = unit.crew_state.get(seat).copied();
                if !matches!(
                    found,
                    Some(CrewCondition::Wounded) | Some(CrewCondition::Out)
                ) {
                    continue;
                }
                losses.push(Self {
                    cadet: *cadet,
                    vehicle: unit.vehicle.clone(),
                    killed_by: unit.last_hit_by,
                    found,
                    aid: aid(unit, seat),
                });
            }
        }
        losses
    }
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

/// One node of a side's chain of command (the designer's ruling,
/// 2026-09-26: there is no separate state for a detachment, it all flows from
/// the chain of command).
///
/// A side is one tree: its root is the side itself — the commander — with
/// its companies beneath, and each company's vehicles beneath that, a leaf
/// being one crewed vehicle. **What stands on the campaign map is not a kind
/// of thing but a fact about a node**: an element with a [`Place`] stands
/// there on its own, and every vehicle below it that does not stand on its
/// own goes with it. Today that is every company. A company that sends one
/// of its vehicles out gives *that node* a place; the vehicle is still the
/// company's, in the tree, and nothing else about it changes.
///
/// [`Column`] is what the map shows of a node with a place: derived from the
/// tree every time it is asked for, never stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Element {
    pub id: ElementId,
    pub side: u8,
    pub name: String,
    /// The node this one answers to; `None` for a side's root.
    pub parent: Option<ElementId>,
    /// The crewed vehicle this node is, for a leaf; `None` above that.
    pub vehicle: Option<ArmyUnit>,
    /// Whether this node carries the side's headquarters — the root of its
    /// signals net, and under [`CampaignVictory::decapitation`] the one it
    /// cannot afford to lose. Copied from [`crate::map::ArmyPlacement::headquarters`].
    pub headquarters: bool,
    /// Standing orders: what this node does with the time nobody spends on
    /// it by hand. Standing in the strict sense the battle's formations use
    /// — never cleared at the end of a day or when it is cut off.
    /// `#[serde(default)]`: a file that says nothing gave no orders.
    #[serde(default)]
    pub mission: Option<ArmyMission>,
    /// Where it stands, if it stands on its own (see the type's doc).
    pub place: Option<Place>,
    /// False once it is gone: destroyed, or a vehicle lost. Kept rather than
    /// removed, because a destroyed headquarters is how a side is known to
    /// have been decapitated and ids are never reused.
    pub alive: bool,
    /// Order among the nodes under the same column, which is the order a
    /// column's vehicles muster in: the order they joined it.
    pub seq: u32,
}

/// Where an element that stands on its own stands, and what it is doing
/// there: what used to be the whole of an army's position.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Place {
    pub pos: Hex,
    /// Movement points a turn on a drawn campaign.
    pub movement: u32,
    pub moved: bool,
    /// Where on the ground it stands on a generated world, to the tile
    /// (WORLD.md W2.1); `pos` is always the campaign hex that tile lies in.
    /// `None` on a drawn campaign map, which has no tiles.
    pub tile: Option<Hex>,
    /// Where it is marching, on a campaign that runs on the clock (W3.1).
    pub march: Option<MarchOrder>,
    /// Ticks spent on the march today, against the `march` block's hours;
    /// reset at dawn.
    pub marched_ticks: u32,
    /// Whether it is in the fighting: its vehicles are crews on tiles in the
    /// front, and it does not march until it leaves (W3.8).
    pub fighting: bool,
}

/// What the campaign map shows of an element that stands on its own: the
/// element, where it stands, and the vehicles that go with it. **Derived
/// from the tree whenever it is asked for** ([`OverworldState::army`]) and
/// never stored, so there is nothing about a column that can disagree with
/// the chain of command it is read from. To change one, change the tree:
/// [`OverworldState::place_mut`] for where it is, its element for its
/// orders.
#[derive(Debug, Clone)]
pub struct Column {
    pub id: ElementId,
    pub side: u8,
    pub name: String,
    pub pos: Hex,
    pub movement: u32,
    pub moved: bool,
    /// The vehicles that go with it, in the order they joined: every leaf
    /// below it that does not stand on its own.
    pub units: Vec<ArmyUnit>,
    pub mission: Option<ArmyMission>,
    pub headquarters: bool,
    pub tile: Option<Hex>,
    pub march: Option<MarchOrder>,
    pub marched_ticks: u32,
    pub fighting: bool,
}

/// A march in progress on the world clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarchOrder {
    /// The campaign hex it is marching on.
    pub to: Hex,
    /// Whether it goes round an enemy in its road or into him.
    pub engagement: Engagement,
    /// The tiles of the leg it is walking, next first; re-planned when it
    /// runs out short of `to`.
    pub leg: Vec<Hex>,
    /// Movement banked toward the next step, in hundredths of a tick's
    /// worth of a movement point — an integer, so the march is exact.
    pub banked: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverworldOrder {
    /// Move toward `to`; moving onto a visible enemy army attacks it.
    MoveArmy {
        army: ElementId,
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
        army: ElementId,
        mission: ArmyMission,
    },
    /// Move one vehicle, crew and all, from one of a side's armies to
    /// another standing beside it. `unit` indexes `from`'s roster.
    ///
    /// Through the order stream like everything else that changes the order
    /// of battle, because a transfer is a decision the log should carry and
    /// a replay should reproduce. The rules are the campaign's, not the
    /// screen's: same side, the giver's turn, the two armies on the same or
    /// neighbouring hexes, neither having marched today, and never the
    /// giver's last vehicle — an empty army is a destroyed one, and
    /// destroying your own company by administrative transfer is not a
    /// thing anybody means.
    TransferUnit {
        from: ElementId,
        to: ElementId,
        unit: usize,
    },
    /// Take back a node's own orders: it answers to its company again
    /// (WORLD.md W4.6b). A vehicle that was sent out marches back, and when
    /// she stands with her company she is part of its column again. Only a
    /// node that stands on its own and has a company to go back to — a
    /// company answers to its side, and there is nowhere to recall it to.
    Recall {
        element: ElementId,
    },
    EndTurn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OverworldEvent {
    /// An army made contact and entered the fighting, at `at`, against the
    /// army named if it ran into one (WORLD.md W3.2, W3.8).
    ArmyEngaged {
        army: ElementId,
        against: Option<ElementId>,
        at: Hex,
    },
    /// An army left the fighting and is a column again.
    ArmyDisengaged {
        army: ElementId,
    },
    /// Nobody is fighting anywhere any more.
    FightingOver {
        rounds: u32,
        /// Vehicles each side lost in it, by side.
        hulls_lost: Vec<u32>,
        /// The side the fight itself decided for, if it was decided: its
        /// battle ended with a winner. `None` is a fight that ended level, or
        /// one that was never decided at all — every army drifted out of
        /// contact and left it, which on one front is the usual ending.
        winner: Option<u8>,
    },
    /// The fighting is planning its next minute, and a side a person
    /// commands, within her reach, has not given her orders; the clock
    /// waits with it (WORLD.md W4.4).
    FightAwaitsOrders {
        side: u8,
    },
    TurnStarted {
        side: u8,
        turn: u32,
    },
    ArmyMoved {
        army: ElementId,
        path: Vec<Hex>,
    },
    ObjectiveCaptured {
        at: Hex,
        side: u8,
    },
    /// What became of a cadet whose vehicle was destroyed. The campaign layer
    /// shows these; the roster has already been updated.
    CrewCasualty {
        cadet: CadetId,
        fate: crate::roster::CrewFate,
    },
    /// Two armies met; the game layer should run a battle and report the
    /// outcome back via [`OverworldState::apply_battle_result`].
    BattleTriggered {
        attacker: ElementId,
        defender: ElementId,
        at: Hex,
    },
    ArmyDestroyed {
        army: ElementId,
    },
    /// An army was given standing orders. News in its own right: a mission is
    /// a decision somebody made, and the campaign log carries it beside the
    /// moves it will cause.
    ArmyMissionAssigned {
        army: ElementId,
        mission: ArmyMission,
    },
    /// This army can no longer be reached by its side's headquarters. It keeps
    /// the orders it has and cannot be given new ones — an army silently
    /// ignoring the player is indistinguishable from a bug, so it is said out
    /// loud the turn it happens.
    ArmyOutOfContact {
        army: ElementId,
    },
    ArmyContactRestored {
        army: ElementId,
    },
    /// A vehicle changed companies. Named by chassis so the log can say what
    /// moved without a second lookup.
    UnitTransferred {
        from: ElementId,
        to: ElementId,
        vehicle: String,
    },
    /// A node below a company was given orders of its own and stands on its
    /// own now (WORLD.md W4.6b): a vehicle sent out from `from`, her company.
    ArmyDetached {
        army: ElementId,
        from: ElementId,
    },
    /// A node's own orders were taken back; it is on its way home.
    ArmyRecalled {
        army: ElementId,
    },
    /// A node that stood on its own is with its company again, and part of
    /// its column.
    ArmyRejoined {
        army: ElementId,
        into: ElementId,
    },
    /// An order for this army could not be got to it and is waiting at
    /// headquarters until it can. It transmits at the first turn start that
    /// finds the army back on the net, arriving as an ordinary
    /// [`Self::ArmyMissionAssigned`].
    ///
    /// The campaign half of the battle's [`crate::battle::Event::OrdersWaiting`],
    /// and it exists for the same reason: an order silently parked is as
    /// illegible as one silently dropped.
    ArmyOrdersWaiting {
        army: ElementId,
    },
    GameEnded {
        winner: Option<u8>,
        /// Which rule ended it, so the log can say *why* and not only who.
        reason: CampaignEnd,
    },
}

/// Why a campaign ended. The rules are [`CampaignVictory`]'s plus the one
/// every campaign has always had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignEnd {
    /// Every other side has no army left on the map.
    Elimination,
    /// Every other side has lost the army carrying its headquarters.
    Decapitation,
    /// Every other side's commander is dead (WORLD.md W4.5).
    CommanderKilled,
    /// One side held every tile the map said to hold when the day turned.
    Held,
}

/// What a field battle hands back to the campaign.
///
/// One struct rather than a list of arguments because it crosses a crate
/// boundary in one direction and a test boundary in the other, and every
/// field added to it — `withdrew` was the first — would otherwise be a
/// signature change at every call site. The battle fills it in from what it
/// knows (who is alive, who left, who was hurt) and the campaign decides
/// what that cost, because whether this game kills its characters is a
/// campaign rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BattleReport {
    pub attacker: ElementId,
    pub defender: ElementId,
    /// Who the battle went to, if anybody; the campaign only uses it for the
    /// headline, since what actually happened is in the rosters below.
    pub winner: Option<u8>,
    /// Both sides broke contact rather than one being destroyed.
    pub stalemate: bool,
    /// Every army that took part, with the vehicles it still has. An army
    /// that lost everything is listed with none, so its destruction is
    /// reported rather than inferred from its absence.
    pub survivors: Vec<(ElementId, Vec<ArmyUnit>)>,
    /// Cadets who were aboard a vehicle that was destroyed, and cadets hurt
    /// at their station in one that came home.
    pub losses: Vec<CrewLoss>,
    /// Armies whose every surviving vehicle left the field by an exit. They
    /// were not beaten and they are not where the battle was: the campaign
    /// puts them a hex back along the way they were going.
    #[serde(default)]
    pub withdrew: Vec<ElementId>,
}

impl BattleReport {
    /// A report with only the rosters in it: no headline, nobody withdrew.
    /// What a test that is about casualties or about who holds the tile
    /// wants to say, and what every caller said before the report had more
    /// fields than that.
    pub fn of(
        attacker: ElementId,
        defender: ElementId,
        survivors: Vec<(ElementId, Vec<ArmyUnit>)>,
        losses: Vec<CrewLoss>,
    ) -> Self {
        Self {
            attacker,
            defender,
            winner: None,
            stalemate: false,
            survivors,
            losses,
            withdrew: Vec::new(),
        }
    }
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
    /// A transfer between armies that are not beside each other, not the
    /// same side, or would empty the giver. One variant for the three
    /// because the fix is the same — pick another army or another day.
    #[error("those armies cannot exchange vehicles today")]
    NoTransfer,
    /// An order to a vehicle that cannot be sent out on her own: not on the
    /// clock, her company fighting, her company's last, or the vehicle her
    /// side's commander rides in — headquarters does not go to look.
    #[error("that vehicle cannot be sent out on her own")]
    NoDetach,
    /// A recall of a node with no company to go back to.
    #[error("there is nobody to recall her to")]
    NoRecall,
    // There is deliberately no `OutOfContact` refusal any more. An order to an
    // army beyond the net used to be rejected; it now waits at headquarters
    // and transmits when the wire comes back, so being unreachable is a delay
    // rather than an error and there is nothing left for this variant to say.
}

/// An army on its way into the fighting: itself, where each of its vehicles
/// stands on the ground, who crews each, and the ids the battle will know
/// them by.
type Entering = (
    Column,
    Vec<crate::map::UnitPlacement>,
    Vec<Vec<CadetId>>,
    Vec<UnitId>,
);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverworldState {
    pub map: Arc<HexMap>,
    pub sides: Vec<OverworldSide>,
    /// Every cadet in the world, whichever academy she belongs to.
    ///
    /// One roster rather than one per side, so a [`CadetId`] means the same
    /// thing everywhere and cadets can change hands without anything being
    /// renumbered — which is what an academy-scale mode will want.
    pub roster: Roster,
    /// Whether this campaign is willing to kill its characters.
    pub rules: CasualtyRules,
    /// Every side's chain of command, every node of it, in id order (see
    /// [`Element`]). The campaign's order of battle is this and nothing
    /// else; what stands on the map is read off it.
    pub elements: Vec<Element>,
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
    pub out_of_contact: Vec<ElementId>,
    /// Missions given to armies nobody could reach, waiting at headquarters
    /// in army-id order — one slot each, because a newer order replaces an
    /// older one that never went out rather than queueing behind it.
    ///
    /// The campaign's counterpart of the battle's radioed-order queue, minus
    /// the re-pathing: an army mission already names ground rather than a
    /// route, so there is nothing to recompute when it finally transmits.
    /// Empty for the whole campaign where no mod declares command rules —
    /// everybody is in contact then, and an order given is an order received.
    #[serde(default)]
    pub waiting_missions: Vec<(ElementId, ArmyMission)>,
    pub over: Option<Option<u8>>,
    /// What ends this campaign beyond running out of armies, copied from the
    /// map. `#[serde(default)]` is the empty rule, so a campaign saved before
    /// the block existed opens as one fought to elimination, which it was.
    #[serde(default)]
    pub victory: CampaignVictory,
    /// Who has held every tile of [`CampaignVictory::hold`] through how many
    /// dawns in a row: `(side, nights)`. `None` until somebody holds the set
    /// at a dawn, and back to `None` the first dawn nobody does. Saved,
    /// because a campaign loaded on the third night of three must not owe a
    /// fourth. `#[serde(default)]` is nobody, which is what a campaign saved
    /// before the streak existed had.
    #[serde(default)]
    pub hold_streak: Option<(u8, u32)>,
    /// Drives casualty resolution. Seeded, and consumed in a fixed order, so
    /// a campaign replays identically.
    pub rng: ChaCha8Rng,
    /// The seed the world was made from, kept so every engagement can derive
    /// its own dice from it (`world::engagement_seed`) rather than from how
    /// many battles came before it or from the clock on the wall.
    /// `#[serde(default)]`: a campaign saved before it existed fights its
    /// battles from seed 0, which is still one fixed answer.
    #[serde(default)]
    pub seed: u64,
    /// The world this campaign is fought over, when it was generated rather
    /// than drawn: its campaign map is this world's summary and its battles
    /// are fought on this world's ground. Saved as how to make it again.
    /// `None` for a drawn campaign.
    #[serde(default)]
    pub world: Option<Arc<GeneratedWorld>>,
    /// The generated world's ground held whole, with its grids: what a march
    /// is routed over. Built from `world` the first time a march needs it
    /// (about 15 ms for the base mod's 160,000 tiles) and never saved — it is
    /// a function of `world` and the terrain definitions.
    #[serde(skip)]
    ground: std::sync::OnceLock<Arc<crate::world::World>>,
    /// What it costs a column of each kind to get from one campaign hex's
    /// standing tile to a neighbour's, over the tiles of the two: the
    /// campaign's coarse graph, filled in as marches ask (WORLD.md W2.2).
    /// Shared by clones and never saved — every entry is a pure function of
    /// the world, the terrain definitions and the column.
    #[serde(skip)]
    passages: Arc<std::sync::Mutex<HashMap<Passage, Option<u32>>>>,
    /// Ticks since the campaign's first dawn, on a campaign that runs on the
    /// world clock (WORLD.md W3.1). Every march, halt and dawn is read off
    /// it; `turn` is the day it falls in. Zero on a drawn campaign, which
    /// runs on turns.
    #[serde(default)]
    pub clock: u64,
    /// The fighting: one battle holding every army in contact anywhere
    /// (WORLD.md W3.8), or `None` when nobody is fighting.
    #[serde(default)]
    pub front: Option<crate::engagement::Front>,
    /// The next unit id the world gives out: a vehicle lifted into a fight
    /// is named by the world, not counted from zero, so two fights never
    /// share a name.
    #[serde(default)]
    pub next_unit_id: u32,
    /// Whether a person commands the fights of the sides nobody's AI
    /// commands (WORLD.md W4.2–W4.4). When she does, the clock waits at the
    /// planning phase of any fight her companies are in and she can reach
    /// over the net, for her orders; her orders past her companies reach
    /// down; and a fight she cannot reach is fought by her companies' own
    /// leaders. Off — the harness, and the game until it can show a fight —
    /// every side of every fight is planned by the engine.
    #[serde(default)]
    pub human_command: bool,
}

/// The kind of column a march is priced for: which movement classes are in
/// it (as a bit per [`MovementClass::index`]) and the climb its most cautious
/// vehicle will take. Two armies of the same kind pay the same for every
/// step, so the coarse graph is cached by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ColumnKind {
    classes: u8,
    climb: i32,
}

/// One edge of the coarse graph: a column kind crossing from one campaign
/// hex to a neighbour.
type Passage = (ColumnKind, (i32, i32), (i32, i32));

/// What a march needs to know about a column: its pace.
#[derive(Debug, Clone, Copy)]
struct Pace {
    kind: ColumnKind,
    /// Movement points a round of its slowest vehicle.
    points: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum OverworldSetupError {
    #[error("map `{0}` not found in registry")]
    MissingMap(String),
    #[error("map `{0}` is not an overworld map")]
    NotAnOverworldMap(String),
    #[error(transparent)]
    Map(#[from] crate::map::MapError),
    #[error(transparent)]
    World(#[from] crate::worldgen::WorldGenError),
    #[error(transparent)]
    Setting(#[from] crate::data::SettingError),
}

/// Armies traverse the overworld as tracked columns with generous climb.
const ARMY_CLASS: MovementClass = MovementClass::Tracked;
const ARMY_CLIMB: i32 = 9;

/// What a move does about a hostile army standing in the road.
///
/// Not a mod-facing knob and not part of any order's wire format: it is the one
/// internal difference between the two things a move can mean, and it exists so
/// that mission execution and the player's click can share
/// [`OverworldState::move_army`] instead of the AI growing a second, subtly
/// different mover of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Engagement {
    /// Go round. Hostile armies are impassable except at the ordered
    /// destination, and halting short of one is just a day's march that ended.
    /// This is what a hand order means — the player's click on a patch of
    /// ground is not a declaration of war on whatever happens to be between her
    /// and it — and what a withdrawal means, which is trying to be somewhere
    /// else rather than trying to fight.
    Avoid,
    /// Movement to contact. Hostile armies are ordinary ground as far as the
    /// route is concerned, the walk halts on the tile in front of the first one
    /// it actually meets, and halting there is an attack. Only a delegated
    /// [`ArmyMission::Advance`] moves this way, because "move on `to` and take
    /// what is on the way" is exactly what that order says.
    EnRoute,
}

impl OverworldState {
    /// The campaign `map_id` describes, as its author wrote it: on a
    /// generated map, the world with every setting at its default.
    pub fn from_map(
        registry: &DataRegistry,
        map_id: &str,
        seed: u64,
    ) -> Result<Self, OverworldSetupError> {
        Self::from_map_setup(registry, map_id, seed, &crate::data::WorldSetup::default())
    }

    /// The campaign `map_id` describes, on a world made to the player's
    /// choices (WORLD.md W6.1): her settings applied to the world's rules,
    /// and her seed, if she named one, in place of the map's. A drawn
    /// campaign has no world to choose and ignores `setup`.
    pub fn from_map_setup(
        registry: &DataRegistry,
        map_id: &str,
        seed: u64,
        setup: &crate::data::WorldSetup,
    ) -> Result<Self, OverworldSetupError> {
        let file: &MapFile = registry
            .map(map_id)
            .ok_or_else(|| OverworldSetupError::MissingMap(map_id.to_string()))?;
        if file.kind != MapKind::Overworld {
            return Err(OverworldSetupError::NotAnOverworldMap(map_id.to_string()));
        }
        // A drawn campaign map is its tiles and its armies' coordinates; a
        // generated one is a world, the campaign map its chunks add up to,
        // and each army's place resolved against the features it names.
        let (map, world, positions): (HexMap, Option<Arc<GeneratedWorld>>, Vec<Hex>) =
            match &file.world {
                None => (
                    HexMap::from_map_file(file)?,
                    None,
                    file.armies
                        .iter()
                        .map(|a| crate::offset_to_hex(a.at[0], a.at[1]))
                        .collect(),
                ),
                Some(spec) => {
                    let rules = spec
                        .rules
                        .clone()
                        .or_else(|| registry.worldgen.clone())
                        .ok_or(crate::worldgen::WorldGenError::NoRules)?
                        .with_settings(&registry.world_settings, &setup.choices)?;
                    let world = GeneratedWorld::with_rules(
                        rules,
                        registry.scale.battle_map_radius(),
                        setup.seed.or(spec.seed).unwrap_or(seed),
                    )?;
                    let places: Vec<crate::map::Place> = file
                        .armies
                        .iter()
                        .map(|a| {
                            a.place.unwrap_or(crate::map::Place {
                                feature: crate::map::PlaceFeature::Town,
                                toward: crate::map::Toward::West,
                                rank: 0,
                            })
                        })
                        .collect();
                    let positions = world.place_armies(&places);
                    (world.campaign_map(), Some(Arc::new(world)), positions)
                }
            };
        let mut sides: Vec<OverworldSide> = file
            .sides
            .iter()
            .map(|s| OverworldSide {
                name: s.name.clone(),
                ai: s.ai.clone(),
                commander: None,
            })
            .collect();
        // Enlisting as the armies are built is what turns map data into
        // people: every crew id named by the file becomes a cadet belonging to
        // that army's academy, and nothing refers to a definition again.
        //
        // Once per academy, though, and this is not a nicety. A campaign map
        // is written by hand and the base game's `frontier` names its ten
        // characters across eighteen vehicles, so stamping one cadet per
        // mention gave Kuhlmann three Rosa Steiners — which the after-action
        // report then dutifully listed three times. One person cannot crew
        // two vehicles, and a roster the player cannot tell apart is a roster
        // she cannot care about, which is the whole premise of having one.
        //
        // The second mention is dropped rather than being an error: the
        // vehicle gets an anonymous crew, exactly as a placement naming
        // nobody does, and `validate_into` says so out loud where a content
        // author will hear it.
        let mut roster = Roster::new();
        let mut taken: std::collections::HashSet<(u8, &str)> = std::collections::HashSet::new();
        // The chain of command, numbered so that a company is still known by
        // the number its army always had: the companies first, in the order
        // the map declares them, then each side's root, then the vehicles,
        // company by company.
        let mut elements: Vec<Element> = Vec::new();
        let companies = file.armies.len();
        for (i, a) in file.armies.iter().enumerate() {
            elements.push(Element {
                id: ElementId(i as u32),
                side: a.side,
                name: a.name.clone(),
                parent: Some(ElementId((companies + a.side as usize) as u32)),
                vehicle: None,
                headquarters: a.headquarters,
                mission: None,
                place: Some(Place {
                    pos: positions[i],
                    movement: a.movement,
                    moved: false,
                    tile: world.as_ref().map(|w| w.stand_tile(registry, positions[i])),
                    march: None,
                    marched_ticks: 0,
                    fighting: false,
                }),
                alive: true,
                seq: i as u32,
            });
        }
        for (side, s) in file.sides.iter().enumerate() {
            let id = elements.len() as u32;
            elements.push(Element {
                id: ElementId(id),
                side: side as u8,
                name: s.name.clone(),
                parent: None,
                vehicle: None,
                headquarters: false,
                mission: None,
                place: None,
                alive: true,
                seq: id,
            });
        }
        // Who commands each side (WORLD.md W4.1): the senior cadet — by rank,
        // then who enlisted first — of the first vehicle its map flags
        // `command`. Read once; after this she is a cadet like any other, and
        // her command vehicle is whichever vehicle she is riding in.
        for (i, a) in file.armies.iter().enumerate() {
            for u in &a.units {
                let unit = ArmyUnit {
                    vehicle: u.vehicle.clone(),
                    crew: u
                        .crew
                        .iter()
                        .filter(|def_id| taken.insert((a.side, def_id.as_str())))
                        .filter_map(|def_id| roster.enlist_from_registry(registry, a.side, def_id))
                        .collect(),
                    name: u.name.clone(),
                };
                let side = a.side as usize;
                if u.command && sides.get(side).is_some_and(|s| s.commander.is_none()) {
                    sides[side].commander = unit.crew.iter().copied().max_by_key(|c| {
                        (
                            roster
                                .get(*c)
                                .and_then(|x| registry.rank_index(x.rank.as_deref())),
                            Reverse(c.0),
                        )
                    });
                }
                let id = elements.len() as u32;
                elements.push(Element {
                    id: ElementId(id),
                    side: a.side,
                    name: unit.name.clone().unwrap_or_else(|| {
                        registry
                            .vehicle(&unit.vehicle)
                            .map_or_else(|| unit.vehicle.clone(), |v| v.name.clone())
                    }),
                    parent: Some(ElementId(i as u32)),
                    vehicle: Some(unit),
                    headquarters: false,
                    mission: None,
                    place: None,
                    alive: true,
                    seq: id,
                });
            }
        }
        let mut state = Self {
            map: Arc::new(map),
            sides,
            roster,
            // The campaign takes the stakes its content declares and then
            // owns them: `rules` is saved, so a settings screen can turn
            // permadeath off for one campaign without touching a mod, and a
            // mod that wants a gentle game says so once in its `casualties`
            // block rather than asking anybody to edit Rust.
            rules: CasualtyRules {
                permadeath: registry.casualties.permadeath,
            },
            elements,
            owners: HashMap::new(),
            turn: 1,
            active_side: 0,
            out_of_contact: Vec::new(),
            waiting_missions: Vec::new(),
            over: None,
            victory: file.victory.clone(),
            hold_streak: None,
            rng: ChaCha8Rng::seed_from_u64(seed),
            seed,
            world,
            ground: std::sync::OnceLock::new(),
            passages: Arc::default(),
            clock: 0,
            front: None,
            next_unit_id: 0,
            human_command: false,
        };
        // Who can hear whom on the morning of day one. The events are dropped
        // because nothing has *changed* yet — an army that starts the campaign
        // outside the net was deployed there, it did not lose contact — but the
        // set itself has to be right from the first order, or a mission the
        // wire could never carry would be accepted on day one and refused on
        // day two for no reason the player could see.
        // No waiting missions are transmitted here, unlike at every later turn
        // start: a campaign that has not begun cannot have been given an order
        // yet, so the queue is necessarily empty.
        let mut unheard = Vec::new();
        state.recompute_contact(registry, state.active_side, &mut unheard);
        Ok(state)
    }

    /// The node `id`, alive or not.
    pub fn element(&self, id: ElementId) -> Option<&Element> {
        self.elements.get(id.index())
    }

    /// The node `id`, alive or not, to change.
    pub fn element_mut(&mut self, id: ElementId) -> Option<&mut Element> {
        self.elements.get_mut(id.index())
    }

    /// The node a vehicle goes with on the map: itself if it stands on its
    /// own, else the nearest node above it that does.
    fn column_of(&self, id: ElementId) -> Option<ElementId> {
        let mut at = self.element(id)?;
        loop {
            if at.place.is_some() {
                return at.alive.then_some(at.id);
            }
            at = self.element(at.parent?)?;
        }
    }

    /// The living vehicles that go with the column `id`, in the order they
    /// joined it.
    pub fn members(&self, id: ElementId) -> Vec<ElementId> {
        let mut leaves: Vec<&Element> = self
            .elements
            .iter()
            .filter(|e| e.alive && e.vehicle.is_some() && self.column_of(e.id) == Some(id))
            .collect();
        leaves.sort_by_key(|e| (e.seq, e.id));
        leaves.into_iter().map(|e| e.id).collect()
    }

    /// What the map shows of `element`, if it stands on its own and lives.
    fn view(&self, element: &Element) -> Option<Column> {
        let place = element.place.as_ref().filter(|_| element.alive)?;
        // A vehicle standing on her own is still her company's, and says so.
        let name = match (
            &element.vehicle,
            element.parent.and_then(|p| self.element(p)),
        ) {
            (Some(_), Some(company)) => format!("{} of {}", element.name, company.name),
            _ => element.name.clone(),
        };
        Some(Column {
            id: element.id,
            side: element.side,
            name,
            pos: place.pos,
            movement: place.movement,
            moved: place.moved,
            units: self
                .members(element.id)
                .into_iter()
                .filter_map(|m| self.element(m)?.vehicle.clone())
                .collect(),
            mission: element.mission.clone(),
            headquarters: element.headquarters,
            tile: place.tile,
            march: place.march.clone(),
            marched_ticks: place.marched_ticks,
            fighting: place.fighting,
        })
    }

    /// The column `id`: a living node that stands on its own, as the map
    /// shows it. `None` for anything else.
    pub fn army(&self, id: ElementId) -> Option<Column> {
        self.view(self.element(id)?)
    }

    /// Every column on the map, in id order.
    pub fn columns(&self) -> Vec<Column> {
        self.elements.iter().filter_map(|e| self.view(e)).collect()
    }

    /// Where the column `id` stands, to change.
    pub fn place_mut(&mut self, id: ElementId) -> Option<&mut Place> {
        self.elements
            .get_mut(id.index())
            .filter(|e| e.alive)?
            .place
            .as_mut()
    }

    /// The first column, by id, standing on `hex`.
    pub fn army_at(&self, hex: Hex) -> Option<Column> {
        self.elements
            .iter()
            .find(|e| e.alive && e.place.as_ref().is_some_and(|p| p.pos == hex))
            .and_then(|e| self.view(e))
    }

    /// `side`'s columns, in id order.
    pub fn side_armies(&self, side: u8) -> impl Iterator<Item = Column> + '_ {
        self.elements
            .iter()
            .filter(move |e| e.side == side)
            .filter_map(|e| self.view(e))
    }

    /// The root of `side`'s chain of command: the side itself.
    pub fn side_root(&self, side: u8) -> Option<ElementId> {
        self.elements
            .iter()
            .find(|e| e.side == side && e.parent.is_none())
            .map(|e| e.id)
    }

    /// Put a new company on the map for `side`, under its root, with these
    /// vehicles: a node with a place, and a leaf under it for each vehicle.
    /// What the campaign's content does at the start, and what a test that
    /// wants a company of its own does.
    pub fn add_company(
        &mut self,
        side: u8,
        name: &str,
        place: Place,
        units: Vec<ArmyUnit>,
        headquarters: bool,
    ) -> ElementId {
        let id = ElementId(self.elements.len() as u32);
        let parent = self.side_root(side);
        let seq = self.next_seq();
        self.elements.push(Element {
            id,
            side,
            name: name.to_string(),
            parent,
            vehicle: None,
            headquarters,
            mission: None,
            place: Some(place),
            alive: true,
            seq,
        });
        for unit in units {
            self.add_vehicle(id, unit);
        }
        id
    }

    /// Replace every vehicle of the column `id` with `units`: the ones it
    /// had are gone and these join it, in this order. For setting up a
    /// campaign's order of battle by hand; a fight hands survivors back
    /// through its own matching, which keeps each vehicle's place.
    pub fn set_vehicles(&mut self, id: ElementId, units: Vec<ArmyUnit>) {
        for m in self.members(id) {
            if let Some(e) = self.element_mut(m) {
                e.alive = false;
            }
        }
        for unit in units {
            self.add_vehicle(id, unit);
        }
    }

    /// A vehicle joins the node `under`, last.
    pub fn add_vehicle(&mut self, under: ElementId, unit: ArmyUnit) -> ElementId {
        let id = ElementId(self.elements.len() as u32);
        let side = self.element(under).map_or(0, |e| e.side);
        let seq = self.next_seq();
        self.elements.push(Element {
            id,
            side,
            name: unit.name.clone().unwrap_or_else(|| unit.vehicle.clone()),
            parent: Some(under),
            vehicle: Some(unit),
            headquarters: false,
            mission: None,
            place: None,
            alive: true,
            seq,
        });
        id
    }

    /// The next place in joining order.
    fn next_seq(&self) -> u32 {
        self.elements.iter().map(|e| e.seq + 1).max().unwrap_or(0)
    }

    /// Hand the column `id` back the vehicles it has after a fight: each
    /// survivor is matched to the vehicle it was — the next one along, in
    /// joining order, with her chassis and her crew — and written over it,
    /// and a vehicle nobody came back in is lost. A survivor matched to
    /// nothing (which a report from the campaign's own fights never holds)
    /// joins it last. Returns whether the column has anything left.
    fn set_survivors(&mut self, id: ElementId, units: Vec<ArmyUnit>) -> bool {
        let members = self.members(id);
        let mut next = 0;
        let mut kept = Vec::new();
        let mut strays = Vec::new();
        for unit in units {
            let found = members[next..].iter().position(|m| {
                self.element(*m)
                    .and_then(|e| e.vehicle.as_ref())
                    .is_some_and(|v| {
                        v.vehicle == unit.vehicle && unit.crew.iter().all(|c| v.crew.contains(c))
                    })
            });
            match found {
                Some(offset) => {
                    let m = members[next + offset];
                    next += offset + 1;
                    kept.push(m);
                    if let Some(e) = self.element_mut(m) {
                        e.vehicle = Some(unit);
                    }
                }
                None => strays.push(unit),
            }
        }
        for m in members {
            if !kept.contains(&m)
                && let Some(e) = self.element_mut(m)
            {
                e.alive = false;
            }
        }
        let leaf = self.element(id).is_some_and(|e| e.vehicle.is_some());
        for unit in strays {
            if !leaf {
                kept.push(self.add_vehicle(id, unit));
            }
        }
        if kept.is_empty()
            && let Some(e) = self.element_mut(id)
        {
            e.alive = false;
        }
        !kept.is_empty()
    }

    /// The army a side's signals net is rooted at: the one the map flagged as
    /// [headquarters](Column::headquarters) while it lives, and otherwise its
    /// first-declared living army, by [`ElementId`].
    ///
    /// The fallback is succession, the same rule a battle formation uses when
    /// its leader dies: somebody has to give the orders, and seniority is who.
    /// It is also the whole of the rule for a map that flags nobody, which is
    /// every map written before the flag existed. Whether losing the flagged
    /// army *also* loses the campaign is a separate question the map answers
    /// with [`CampaignVictory::decapitation`]; the net does not decide it.
    ///
    /// **Still a placeholder in one respect.** Contact ought to root at a
    /// *person* — the side's commanding cadet, in a command vehicle with a
    /// radius priced on her crew's `signals`, the way a battle formation's
    /// net is priced on its leader. The command vehicle does not exist yet
    /// (TODO.md, Chain of Command: the command-unit item); when it does, it
    /// will live inside this army, and this function is the only thing that
    /// has to change.
    pub fn senior_army(&self, side: u8) -> Option<ElementId> {
        self.headquarters(side)
            .or_else(|| self.side_armies(side).map(|a| a.id).min())
    }

    /// The living army flagged as `side`'s headquarters, if the map flagged
    /// one and it is still on the map. `None` for a side that flagged nobody
    /// *and* for one whose headquarters has been destroyed; [`Self::decapitated`]
    /// tells those apart.
    pub fn headquarters(&self, side: u8) -> Option<ElementId> {
        self.side_armies(side)
            .filter(|a| a.headquarters)
            .map(|a| a.id)
            .min()
    }

    /// Whether `side` flagged a headquarters and has lost it. Reads the flag
    /// on dead armies too, which is the point: a side that never flagged one
    /// cannot be decapitated, and a side whose flagged army is gone has been.
    pub fn decapitated(&self, side: u8) -> bool {
        self.elements
            .iter()
            .any(|e| e.side == side && e.headquarters)
            && self.headquarters(side).is_none()
    }

    /// Whether `side` is out of the campaign under the rules the map wrote:
    /// no army left, or — when the map said losing headquarters is losing —
    /// no headquarters left.
    pub fn defeated(&self, side: u8) -> bool {
        self.side_armies(side).next().is_none()
            || (self.victory.decapitation && self.decapitated(side))
            || (self.victory.commander && self.commander_killed(side))
    }

    /// Whether `side`'s commander is dead — not wounded, not missing: a
    /// wound is not a death (WORLD.md W4.5).
    pub fn commander_killed(&self, side: u8) -> bool {
        self.sides
            .get(side as usize)
            .and_then(|s| s.commander)
            .and_then(|c| self.roster.get(c))
            .is_some_and(|c| c.status == crate::roster::CadetStatus::Dead)
    }

    /// Who commands `side` today (WORLD.md W4.5): its commander, if she is
    /// fit; while she is in the infirmary or walking back, the most senior
    /// cadet of the side who is fit and riding with an army — by rank, then
    /// who enlisted first — until she is back. `None` for a side nobody
    /// commands, or with nobody left to.
    pub fn acting_commander(&self, registry: &DataRegistry, side: u8) -> Option<CadetId> {
        let commander = self.sides.get(side as usize)?.commander?;
        if self
            .roster
            .get(commander)
            .is_some_and(|c| c.status.is_ready())
        {
            return Some(commander);
        }
        self.side_armies(side)
            .flat_map(|a| a.units.into_iter().flat_map(|u| u.crew))
            .filter(|c| self.roster.get(*c).is_some_and(|x| x.status.is_ready()))
            .max_by_key(|c| {
                (
                    self.roster
                        .get(*c)
                        .and_then(|x| registry.rank_index(x.rank.as_deref())),
                    Reverse(c.0),
                )
            })
    }

    /// How much of the ground the map says to hold `side` holds: `(held,
    /// total)` over every tile of the terrains in [`CampaignVictory::hold`],
    /// or `None` when the map names no such ground. The number the campaign
    /// screen prints and the dawn check reads, so they cannot disagree.
    pub fn hold_progress(&self, registry: &DataRegistry, side: u8) -> Option<(usize, usize)> {
        if self.victory.hold.is_empty() {
            return None;
        }
        let (mut held, mut total) = (0, 0);
        for (hex, tile) in self.map.iter() {
            if !self.victory.hold.iter().any(|t| t == tile.terrain)
                || !registry.terrain(tile.terrain).is_some_and(|t| t.capturable)
            {
                continue;
            }
            total += 1;
            if self.owners.get(&hex) == Some(&side) {
                held += 1;
            }
        }
        Some((held, total))
    }

    /// The hold rule in words, for the screen: "hold every Factory", or
    /// `None` when the map declares none. Named after the terrains rather
    /// than counted, because the count is what [`Self::hold_progress`] is for.
    pub fn hold_objective(&self, registry: &DataRegistry) -> Option<String> {
        if self.victory.hold.is_empty() {
            return None;
        }
        let names: Vec<String> = self
            .victory
            .hold
            .iter()
            .map(|id| {
                registry
                    .terrain(id)
                    .map(|t| t.name.clone())
                    .unwrap_or_else(|| id.clone())
            })
            .collect();
        let nights = self.victory.hold_days;
        Some(if nights > 1 {
            format!("hold every {} for {nights} nights", names.join(" and "))
        } else {
            format!("hold every {}", names.join(" and "))
        })
    }

    /// How many dawns running `side` has held the set, for the screen.
    pub fn nights_held(&self, side: u8) -> u32 {
        match self.hold_streak {
            Some((who, nights)) if who == side => nights,
            _ => 0,
        }
    }

    /// Whether this army can still be given new orders.
    ///
    /// True for everything in a campaign whose mod declares no `command`
    /// block, and true for an army that no longer exists — the question a
    /// caller is asking is "will an order reach her", and one that cannot be
    /// given for some other reason is refused for that other reason.
    pub fn in_contact(&self, army: ElementId) -> bool {
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
        let mut heard: Vec<ElementId> = Vec::new();
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

        let mut next: Vec<ElementId> = self
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
    pub fn army_visible_to(&self, registry: &DataRegistry, army: &Column, observer: u8) -> bool {
        if army.side == observer {
            return true;
        }
        let concealed = self
            .map
            .get(army.pos)
            .and_then(|t| registry.terrain(t.terrain))
            .is_some_and(|t| t.concealing);
        if !concealed {
            return true;
        }
        self.side_armies(observer)
            .any(|a| a.pos.distance_to(army.pos) <= 1)
    }

    pub fn visible_armies(&self, registry: &DataRegistry, observer: u8) -> Vec<Column> {
        self.columns()
            .into_iter()
            .filter(|a| self.army_visible_to(registry, a, observer))
            .collect()
    }

    /// The generated world's ground held whole, built the first time a march
    /// needs it. `None` on a drawn campaign map.
    fn ground(&self, registry: &DataRegistry) -> Option<&Arc<crate::world::World>> {
        let world = self.world.as_ref()?;
        Some(self.ground.get_or_init(|| {
            let mut tiles = HexMap::default();
            for chunk in world.chunks() {
                for (hex, terrain, level) in world.chunk_tiles(chunk) {
                    tiles.insert(hex, terrain, level);
                }
            }
            Arc::new(crate::world::World::build(registry, tiles))
        }))
    }

    /// What an army marches as. The vehicles that carry it set the pace: a
    /// column with anything on wheels or tracks is priced by those, since its
    /// infantry ride; a column on foot walks. Of those, the slowest sets the
    /// speed and the most cautious the climb (WORLD.md W2.3).
    fn column(registry: &DataRegistry, army: &Column) -> Pace {
        let vehicles: Vec<&crate::data::VehicleDef> = army
            .units
            .iter()
            .filter_map(|u| registry.vehicle(&u.vehicle))
            .collect();
        let riding: Vec<&crate::data::VehicleDef> = vehicles
            .iter()
            .copied()
            .filter(|v| v.movement.class != MovementClass::Foot)
            .collect();
        let pacing = if riding.is_empty() {
            &vehicles
        } else {
            &riding
        };
        let classes = pacing
            .iter()
            .fold(0u8, |bits, v| bits | 1 << v.movement.class.index());
        Pace {
            kind: ColumnKind {
                classes: if classes == 0 {
                    1 << ARMY_CLASS.index()
                } else {
                    classes
                },
                climb: pacing
                    .iter()
                    .map(|v| v.movement.max_climb)
                    .min()
                    .unwrap_or(ARMY_CLIMB),
            },
            points: pacing
                .iter()
                .map(|v| v.movement.points)
                .min()
                .unwrap_or(1)
                .max(1),
        }
    }

    /// What one step costs this kind of column: the dearest of its classes,
    /// or `None` if any of them cannot make it — a column goes where all of
    /// it can go.
    fn column_step(
        ground: &crate::world::World,
        kind: ColumnKind,
        from: Hex,
        to: Hex,
    ) -> Option<u32> {
        MovementClass::ALL
            .iter()
            .filter(|c| kind.classes & (1 << c.index()) != 0)
            .map(|c| ground.moves().cost(*c, kind.climb, from, to))
            .try_fold(0u32, |dearest, cost| Some(dearest.max(cost?)))
    }

    /// The movement points this army's column spends in a day's march.
    pub fn day_budget(&self, registry: &DataRegistry, id: ElementId) -> u32 {
        let Some(army) = self.army(id) else {
            return 0;
        };
        let rounds_per_hour = (3600.0 / registry.scale.round_seconds).round() as u32;
        registry
            .march
            .day_budget(Self::column(registry, &army).points, rounds_per_hour)
    }

    /// The cheapest tile route for this kind of column from `from` to `to`,
    /// by A*, over tiles `open` allows; `None` if there is none. Ties go to the
    /// lower cost so far and then the coordinate, last.
    fn tile_route(
        ground: &crate::world::World,
        kind: ColumnKind,
        from: Hex,
        to: Hex,
        open: impl Fn(Hex) -> bool,
    ) -> Option<(Vec<Hex>, u32)> {
        let mut best: HashMap<Hex, u32> = HashMap::from([(from, 0)]);
        let mut came: HashMap<Hex, Hex> = HashMap::new();
        let mut heap = BinaryHeap::new();
        heap.push(Reverse((
            from.unsigned_distance_to(to),
            0u32,
            from.x,
            from.y,
        )));
        while let Some(Reverse((_, cost, x, y))) = heap.pop() {
            let at = Hex::new(x, y);
            if at == to {
                let mut path = vec![to];
                let mut here = to;
                while let Some(prev) = came.get(&here) {
                    path.push(*prev);
                    here = *prev;
                }
                path.reverse();
                return Some((path, cost));
            }
            if best.get(&at).is_some_and(|b| *b < cost) {
                continue;
            }
            for next in at.all_neighbors() {
                if next != to && !open(next) {
                    continue;
                }
                let Some(step) = Self::column_step(ground, kind, at, next) else {
                    continue;
                };
                let total = cost + step;
                if best.get(&next).is_none_or(|b| total < *b) {
                    best.insert(next, total);
                    came.insert(next, at);
                    heap.push(Reverse((
                        total + next.unsigned_distance_to(to),
                        total,
                        next.x,
                        next.y,
                    )));
                }
            }
        }
        None
    }

    /// What it costs `kind` to cross from campaign hex `a`'s standing tile
    /// to neighbouring `b`'s, over the tiles of those two hexes alone: one
    /// edge of the coarse graph `reachable` searches. Cached.
    fn passage(
        &self,
        registry: &DataRegistry,
        world: &GeneratedWorld,
        ground: &crate::world::World,
        kind: ColumnKind,
        a: Hex,
        b: Hex,
    ) -> Option<u32> {
        let key = (kind, (a.x, a.y), (b.x, b.y));
        if let Some(cost) = self.passages.lock().expect("unpoisoned").get(&key) {
            return *cost;
        }
        let radius = world.chunk_radius;
        let cost = Self::tile_route(
            ground,
            kind,
            world.stand_tile(registry, a),
            world.stand_tile(registry, b),
            |h| {
                let c = crate::world::chunk_of(h, radius);
                c == a || c == b
            },
        )
        .map(|(_, cost)| cost);
        self.passages.lock().expect("unpoisoned").insert(key, cost);
        cost
    }

    /// The campaign hexes a march passes through from `from` to `to`, by A*
    /// over the coarse graph, through hexes `open` allows. The heuristic is
    /// the tile distance from a hex's standing tile to the goal's: a coarse
    /// route is a chain of passages between standing tiles and a tile costs
    /// at least a point, so it never over-estimates — and it is tight on open
    /// ground, so the search prices only the passages near the line to the
    /// goal. A Dijkstra priced a thousand of them (1,150 ms); a bound of
    /// whole hexes between, too loose, several hundred (994 ms). Ties go to
    /// the coordinate, last.
    #[allow(clippy::too_many_arguments)]
    fn coarse_route(
        &self,
        registry: &DataRegistry,
        world: &GeneratedWorld,
        ground: &crate::world::World,
        kind: ColumnKind,
        from: Hex,
        to: Hex,
        open: &dyn Fn(Hex) -> bool,
    ) -> Option<Vec<Hex>> {
        let goal = world.stand_tile(registry, to);
        let floor = |h: Hex| world.stand_tile(registry, h).unsigned_distance_to(goal);
        let mut best: HashMap<Hex, u32> = HashMap::from([(from, 0)]);
        let mut came: HashMap<Hex, Hex> = HashMap::new();
        let mut heap = BinaryHeap::new();
        heap.push((Reverse((floor(from), 0u32)), from.x, from.y));
        while let Some((Reverse((_, cost)), x, y)) = heap.pop() {
            let hex = Hex::new(x, y);
            if hex == to {
                let mut path = vec![to];
                let mut here = to;
                while let Some(prev) = came.get(&here) {
                    path.push(*prev);
                    here = *prev;
                }
                path.reverse();
                return Some(path);
            }
            if best.get(&hex).is_some_and(|&c| c < cost) {
                continue;
            }
            for next in hex.all_neighbors() {
                if !self.map.contains(next) || !open(next) {
                    continue;
                }
                let Some(step) = self.passage(registry, world, ground, kind, hex, next) else {
                    continue;
                };
                let total = cost + step;
                if best.get(&next).is_none_or(|&c| total < c) {
                    best.insert(next, total);
                    came.insert(next, hex);
                    heap.push((Reverse((total + floor(next), total)), next.x, next.y));
                }
            }
        }
        None
    }

    /// `reachable` on a generated world: the coarse graph of campaign hexes,
    /// each edge the tile-level passage between neighbours, searched within
    /// the day's march. An estimate of the march — the march itself cuts
    /// corners the coarse graph cannot — and never an over-estimate of what
    /// the route through the standing tiles would cost.
    fn reachable_on_ground(&self, registry: &DataRegistry, army: &Column) -> HashMap<Hex, u32> {
        let (Some(world), Some(ground)) = (self.world.as_ref(), self.ground(registry)) else {
            return HashMap::new();
        };
        let kind = Self::column(registry, army).kind;
        let budget = self.day_budget(registry, army.id);
        let mut best: HashMap<Hex, u32> = HashMap::from([(army.pos, 0)]);
        let mut heap = BinaryHeap::new();
        heap.push((Reverse(0u32), army.pos.x, army.pos.y));
        while let Some((Reverse(cost), x, y)) = heap.pop() {
            let hex = Hex::new(x, y);
            if best.get(&hex).is_some_and(|&c| c < cost) {
                continue;
            }
            for next in hex.all_neighbors() {
                if !self.map.contains(next)
                    || self
                        .army_at(next)
                        .is_some_and(|other| other.side != army.side)
                {
                    continue;
                }
                let Some(step) = self.passage(registry, world, ground, kind, hex, next) else {
                    continue;
                };
                let total = cost + step;
                if total > budget {
                    continue;
                }
                if best.get(&next).is_none_or(|&c| total < c) {
                    best.insert(next, total);
                    heap.push((Reverse(total), next.x, next.y));
                }
            }
        }
        best.retain(|hex, _| *hex == army.pos || self.army_at(*hex).is_none());
        best
    }

    /// The next leg of a march on a generated world: the tiles from where the
    /// army stands toward the standing tile of `to`, as far as the first
    /// coarse hex beyond a day's march (WORLD.md W2.1–W2.3). The clock walks
    /// it (`step_march`) and asks for another when it runs out.
    ///
    /// The rules are the campaign's, read at the tile: a hostile army's hex
    /// is a wall to a march that means to avoid contact; a march that does
    /// not avoid it is routed through, and halts on its border when it gets
    /// there — that is where the fight is.
    fn plan_leg(
        &self,
        registry: &DataRegistry,
        army: &Column,
        to: Hex,
        engagement: Engagement,
    ) -> Result<Vec<Hex>, OverworldError> {
        let (Some(world), Some(ground)) = (self.world.as_ref(), self.ground(registry)) else {
            return Err(OverworldError::NoPath);
        };
        let radius = world.chunk_radius;
        let column = Self::column(registry, army);
        let start = army
            .tile
            .unwrap_or_else(|| world.stand_tile(registry, army.pos));
        let hostile_hex = |c: Hex| self.army_at(c).is_some_and(|other| other.side != army.side);
        let open_hex = |c: Hex| {
            c == to || c == army.pos || engagement == Engagement::EnRoute || !hostile_hex(c)
        };
        let budget = self.day_budget(registry, army.id);

        // Hierarchical, in two steps (WORLD.md W2.2). The coarse route: the
        // campaign hexes to pass through, over the graph `reachable` reads.
        // Then the fine one, over the tiles, aimed at the first hex on the
        // coarse route beyond a day's march and confined to a corridor of
        // the coarse route and its neighbours — the tiles a day can use,
        // rather than every tile between here and a goal days away (208 ms
        // for a march across the world, planned whole; the corridor is what
        // a day needs). Every passage the coarse graph prices lies inside
        // the corridor, so the fine route is never dearer than the coarse
        // one: what `reachable` offers, the march delivers.
        let coarse = self
            .coarse_route(
                registry,
                world,
                ground,
                column.kind,
                army.pos,
                to,
                &open_hex,
            )
            .ok_or(OverworldError::NoPath)?;
        let mut spent = 0;
        let mut waypoint = coarse.len() - 1;
        for (i, pair) in coarse.windows(2).enumerate() {
            spent += self
                .passage(registry, world, ground, column.kind, pair[0], pair[1])
                .unwrap_or(u32::MAX / 4);
            if spent > budget {
                waypoint = (i + 2).min(coarse.len() - 1);
                break;
            }
        }
        let goal_hex = coarse[waypoint];
        let goal = world.stand_tile(registry, goal_hex);
        let mut corridor: std::collections::HashSet<Hex> = std::collections::HashSet::new();
        for c in &coarse[..=waypoint] {
            corridor.insert(*c);
            corridor.extend(c.all_neighbors());
        }
        let (route, _) = Self::tile_route(ground, column.kind, start, goal, |h| {
            let c = crate::world::chunk_of(h, radius);
            corridor.contains(&c) && open_hex(c)
        })
        .ok_or(OverworldError::NoPath)?;
        Ok(route.into_iter().skip(1).collect())
    }

    /// Whether this campaign runs on the world clock: a generated one does
    /// (WORLD.md W3.1); a drawn one keeps its turns.
    pub fn clocked(&self) -> bool {
        self.world.is_some()
    }

    /// Ticks until the next dawn on the clock.
    pub fn ticks_to_dawn(&self, registry: &DataRegistry) -> u64 {
        let per_day = Self::ticks_per_day(registry);
        per_day - self.clock % per_day
    }

    /// The time of day on the clock, as `(hours, minutes)` since dawn.
    pub fn time_of_day(&self, registry: &DataRegistry) -> (u32, u32) {
        let per_day = Self::ticks_per_day(registry);
        let seconds = ((self.clock % per_day) as f32 * registry.scale.tick_seconds()) as u32;
        (seconds / 3600, seconds / 60 % 60)
    }

    /// Ticks in a day, off the scale.
    fn ticks_per_day(registry: &DataRegistry) -> u64 {
        (86_400.0 / registry.scale.tick_seconds()).round() as u64
    }

    /// Run the clock toward the next dawn: every march walks, tick by tick,
    /// all at once. Stops at the end of the first tick on which a column
    /// ran into an enemy — the fight is somebody's to fight before the day
    /// goes on — or at dawn, which it then keeps.
    fn run_clock(&mut self, registry: &DataRegistry) -> Vec<OverworldEvent> {
        self.advance_clock(registry, u64::MAX)
    }

    /// Run the clock on by at most `ticks`: every march walks, all at once.
    /// Stops early at the end of a tick on which a column met an enemy, and
    /// at dawn. What real-time play with speed controls drives: the speed is
    /// only how many ticks a wall-clock second asks for, so how fast the
    /// clock is run changes nothing a replay could see (WORLD.md W3.1).
    pub fn advance_clock(&mut self, registry: &DataRegistry, ticks: u64) -> Vec<OverworldEvent> {
        let per_day = Self::ticks_per_day(registry);
        let mut events = Vec::new();
        for _ in 0..ticks {
            // A war that is over stays over. The fighting can end it in the
            // middle of a tick (a headquarters destroyed, a commander
            // killed), and nothing below asked: the same call went on
            // marching columns, taking towns and running the next dawn, so
            // a caller who asked for "the rest of the day" — the game's
            // Enter — was shown the world some hours after the campaign
            // ended rather than the moment it did.
            if self.over.is_some() {
                return events;
            }
            // A fight she can reach waits for her orders, and so does the
            // clock (WORLD.md W4.4): the pause is news reaching her, and the
            // world does not go on without her while she gives them.
            if let Some(side) = self.awaiting_orders() {
                events.push(OverworldEvent::FightAwaitsOrders { side });
                return events;
            }
            self.clock += 1;
            events.extend(self.run_front(registry));
            if self.over.is_some() {
                return events;
            }
            events.extend(self.bring_home(registry));
            let marching: Vec<ElementId> = self
                .columns()
                .into_iter()
                .filter(|a| a.march.is_some() && !a.fighting)
                .map(|a| a.id)
                .collect();
            for id in marching {
                events.extend(self.step_march(registry, id));
            }
            if self.clock.is_multiple_of(per_day) {
                events.extend(self.dawn(registry));
                return events;
            }
            if events
                .iter()
                .any(|e| matches!(e, OverworldEvent::BattleTriggered { .. }))
            {
                return events;
            }
        }
        events
    }

    /// One tick of one march: halt if this is the halt in the hour, rest if
    /// the day's hours are marched, else bank the tick's movement and walk
    /// what it pays for.
    fn step_march(&mut self, registry: &DataRegistry, id: ElementId) -> Vec<OverworldEvent> {
        let mut events = Vec::new();
        let Some(army) = self.army(id) else {
            return events;
        };
        let Some(mut order) = army.march.clone() else {
            return events;
        };
        let (Some(world), Some(ground)) = (self.world.clone(), self.ground(registry).cloned())
        else {
            return events;
        };
        let march = &registry.march;
        let per_minute = (60.0 / registry.scale.tick_seconds()).round() as u32;
        let per_hour = 60 * per_minute;
        let day = march.hours_per_day * per_hour;
        let in_friends_hex = self
            .elements
            .iter()
            .any(|e| e.alive && e.id != id && e.place.as_ref().is_some_and(|p| p.pos == army.pos));
        // Rest for the night once the day's hours are marched — unless that
        // would leave the column parked inside a friend's hex.
        if army.marched_ticks >= day && !in_friends_hex {
            return events;
        }
        let marched = army.marched_ticks + 1;
        if let Some(a) = self.place_mut(id) {
            a.marched_ticks = marched;
        }
        // The last minutes of every marching hour are the halt.
        if (marched - 1) % per_hour
            >= per_hour.saturating_sub(march.halt_minutes_per_hour * per_minute)
        {
            return events;
        }
        let column = Self::column(registry, &army);
        let per_round = registry.scale.ticks_per_round as u64;
        order.banked += column.points as u64 * march.column_percent as u64;
        let radius = world.chunk_radius;
        let mut tile = army
            .tile
            .unwrap_or_else(|| world.stand_tile(registry, army.pos));
        let mut pos = army.pos;
        let mut finished = false;
        loop {
            if order.leg.is_empty() {
                if pos == order.to {
                    finished = true;
                    break;
                }
                let here = Column {
                    tile: Some(tile),
                    pos,
                    ..army.clone()
                };
                match self.plan_leg(registry, &here, order.to, order.engagement) {
                    Ok(leg) if !leg.is_empty() => order.leg = leg,
                    _ => {
                        finished = true;
                        break;
                    }
                }
            }
            let next = order.leg[0];
            let Some(cost) = Self::column_step(&ground, column.kind, tile, next) else {
                order.leg.clear();
                continue;
            };
            let need = cost as u64 * 100 * per_round;
            if order.banked < need {
                break;
            }
            let hex = crate::world::chunk_of(next, radius);
            if hex != pos {
                // Contact: a hostile army in the hex ahead, fighting already
                // or not — the column halts on the border of his ground and
                // enters the fighting there (WORLD.md W3.2, W3.8). A friend's
                // hex with a fight in it: she joins the fighting on her own
                // side.
                let ahead = self.army_at(hex);
                let against = ahead.as_ref().filter(|o| o.side != army.side).map(|o| o.id);
                let joining = ahead
                    .as_ref()
                    .is_some_and(|o| o.side == army.side && o.fighting);
                if against.is_some() || joining {
                    if let Some(a) = self.place_mut(id) {
                        a.tile = Some(tile);
                        a.pos = pos;
                        a.march = None;
                    }
                    events.extend(self.enter_fighting(registry, id, against, hex));
                    return events;
                }
                // Nobody finishes a march in a friend's hex: arriving there,
                // the column stops on its border instead.
                if hex == order.to && self.army_at(hex).is_some_and(|o| o.id != id) {
                    finished = true;
                    break;
                }
            }
            order.banked -= need;
            order.leg.remove(0);
            tile = next;
            if hex != pos {
                let from = pos;
                pos = hex;
                events.push(OverworldEvent::ArmyMoved {
                    army: id,
                    path: vec![from, hex],
                });
                events.extend(self.take_ground(registry, army.side, hex));
            }
        }
        if let Some(a) = self.place_mut(id) {
            a.tile = Some(tile);
            a.pos = pos;
            a.march = if finished { None } else { Some(order) };
        }
        events
    }

    /// Put a campaign that came off disk back together: the front's battle
    /// gets its caches back. `SaveGame::from_json` calls it, the way it
    /// rehydrates a battle of its own.
    pub fn rehydrate(&mut self, registry: &DataRegistry) {
        if let Some(front) = &mut self.front {
            front.rehydrate(registry);
        }
    }

    /// Lift an army's vehicles onto the ground around where it stands: each
    /// on the nearest tile, spiralling out from the army's, that its chassis
    /// can stand on and nobody has taken. One vehicle a tile.
    fn lift(
        &self,
        registry: &DataRegistry,
        army: &Column,
        taken: &mut std::collections::HashSet<Hex>,
    ) -> Vec<(crate::map::UnitPlacement, Vec<CadetId>)> {
        let Some(ground) = self.ground(registry) else {
            return Vec::new();
        };
        let centre = army.tile.unwrap_or(Hex::ZERO);
        let formation = format!("army-{}", army.id.0);
        let mut placed = Vec::new();
        for (i, unit) in army.units.iter().enumerate() {
            let Some(vehicle) = registry.vehicle(&unit.vehicle) else {
                continue;
            };
            let spot = centre.spiral_range(0..=12).find(|h| {
                !taken.contains(h)
                    && ground.get(*h).is_some_and(|t| {
                        registry
                            .terrain(t.terrain)
                            .is_some_and(|d| d.cost_for(vehicle.movement.class).is_some())
                    })
            });
            let Some(spot) = spot else {
                continue;
            };
            taken.insert(spot);
            placed.push((
                crate::map::UnitPlacement {
                    aboard_at: None,
                    at: crate::hex_to_offset(spot),
                    side: army.side,
                    vehicle: unit.vehicle.clone(),
                    crew: Vec::new(),
                    name: unit.name.clone(),
                    facing: None,
                    formation: Some(formation.clone()),
                    leads: i == 0,
                },
                unit.crew.clone(),
            ));
        }
        placed
    }

    /// The company an army is in the fighting.
    fn company_of(&self, army: &Column) -> crate::map::FormationDef {
        crate::map::FormationDef {
            id: format!("army-{}", army.id.0),
            name: army.name.clone(),
            side: army.side,
            doctrine: self
                .sides
                .get(army.side as usize)
                .and_then(|s| s.ai.as_ref())
                .and_then(|ai| ai.doctrine.clone()),
        }
    }

    /// The ground an army holds, as an objective of the fighting: the tiles
    /// round where it stands, worth holding. Without it neither side's
    /// commander had anything to fight *for* in open country, the attacker
    /// was sent at the one tile the defender occupied — which nobody can
    /// choose to stand on — and the two sat nine hexes apart until the
    /// stalemate clock ran out.
    fn ground_of(army: &Column) -> crate::map::Objective {
        let held = army.tile.unwrap_or(Hex::ZERO);
        crate::map::Objective {
            id: format!("ground-{}", army.id.0),
            name: format!("{}'s ground", army.name),
            hexes: held.range(2).collect(),
            value: 3,
            kind: crate::map::ObjectiveKind::Hold,
            side: None,
        }
    }

    /// The towns fought over are the ones near the fighting (WORLD.md W3.4):
    /// each within `towns.contested_within` tiles of some crew in it is an
    /// objective on its own tiles, worth `towns.worth` or `factory_worth`.
    /// Asked whenever an army enters and at every planning pulse, because
    /// the front is the whole world's fighting: making every town an
    /// objective would pull a crew in one fight toward a factory a day's
    /// march away, and crews move, so the set is kept as they do. A town
    /// dropped from it has already gone to whoever held it, since holding is
    /// capture at every pulse.
    fn contest_towns(&mut self) {
        let (Some(world), Some(front)) = (self.world.as_ref(), self.front.as_mut()) else {
            return;
        };
        let towns = &world.rules.towns;
        let battle = front.battle_mut();
        let here: Vec<Hex> = battle
            .units
            .iter()
            .filter(|u| u.alive())
            .map(|u| u.pos)
            .collect();
        for (i, town) in world.skeleton.towns.iter().enumerate() {
            let id = format!("town-{i}");
            let near = here
                .iter()
                .any(|h| h.unsigned_distance_to(town.centre) <= towns.contested_within);
            let present = battle.objectives().any(|(o, _)| o.id == id);
            if near && !present {
                battle.add_objective(crate::map::Objective {
                    id,
                    name: if town.factory {
                        "the factory".into()
                    } else {
                        "the town".into()
                    },
                    hexes: town.centre.range(town.radius).collect(),
                    value: if town.factory {
                        towns.factory_worth
                    } else {
                        towns.worth
                    },
                    kind: crate::map::ObjectiveKind::Hold,
                    side: None,
                });
            } else if !near && present {
                battle.remove_objective(&id);
            }
        }
    }

    /// `army` makes contact at `at` — with `against`, the army it ran into,
    /// or with a fight a friend is in — and enters the fighting (WORLD.md
    /// W3.2, W3.8): its vehicles, and his if he is not fighting already, are
    /// lifted onto the ground where each stands and put into the one battle
    /// the world's fighting is, which is raised if nobody was fighting. She
    /// came to find him: her company assaults his ground; his holds it. A
    /// column joining a friend's fight moves up onto the nearest ground being
    /// fought over.
    fn enter_fighting(
        &mut self,
        registry: &DataRegistry,
        army: ElementId,
        against: Option<ElementId>,
        at: Hex,
    ) -> Vec<OverworldEvent> {
        let Some(world) = self.world.clone() else {
            return Vec::new();
        };
        let mut entering: Vec<Column> = Vec::new();
        for id in [Some(army), against].into_iter().flatten() {
            if let Some(a) = self.army(id).filter(|a| !a.fighting) {
                entering.push(a);
            }
        }
        if entering.is_empty() {
            return Vec::new();
        }
        let mut taken: std::collections::HashSet<Hex> = self
            .front
            .as_ref()
            .map(|f| {
                f.battle()
                    .units
                    .iter()
                    .filter(|u| u.alive())
                    .map(|u| u.pos)
                    .collect()
            })
            .unwrap_or_default();
        let mut musters: Vec<Entering> = Vec::new();
        for a in &entering {
            let lifted = self.lift(registry, a, &mut taken);
            if lifted.is_empty() {
                continue;
            }
            let (placements, crews): (Vec<_>, Vec<_>) = lifted.into_iter().unzip();
            let first = self.next_unit_id;
            let ids: Vec<UnitId> = (0..placements.len() as u32)
                .map(|i| UnitId(first + i))
                .collect();
            self.next_unit_id += placements.len() as u32;
            musters.push((a.clone(), placements, crews, ids));
        }
        if musters.is_empty() {
            return Vec::new();
        }
        // Raise the fighting if nobody was fighting: one battle, on a window
        // onto the world. The towns near it join it below.
        if self.front.is_none() {
            let (first, placements, crews, ids) = musters.remove(0);
            let scenario = crate::map::Scenario::with_formations(vec![self.company_of(&first)]);
            let sides = self
                .sides
                .iter()
                .map(|s| crate::battle::SideState {
                    name: s.name.clone(),
                    ai: s.ai.clone(),
                })
                .collect();
            let seed = crate::world::engagement_seed(
                self.seed,
                crate::world::EngagementKey {
                    when: self.clock,
                    at,
                    attacker: army.0,
                    defender: against.map_or(army.0, |d| d.0),
                },
            );
            let Ok(battle) = crate::battle::BattleState::from_muster_on(
                registry,
                crate::world::World::window_onto(world.clone()),
                scenario,
                sides,
                crate::battle::Muster {
                    placements: &placements,
                    crews: &crews,
                    ids: &ids,
                },
                Arc::new(self.roster.mustered(&[])),
                seed,
            ) else {
                return Vec::new();
            };
            let round = battle.round;
            self.front = Some(crate::engagement::Front {
                began: self.clock,
                seed,
                origins: ids.iter().map(|u| (*u, first.id)).collect(),
                contact: vec![(first.id, round)],
                fight: crate::engagement::Fight::Live(Box::new(battle)),
                recent: Vec::new(),
            });
        }
        let front = self.front.as_mut().expect("raised above");
        let round = front.battle().round;
        for (a, placements, crews, ids) in musters {
            let def = crate::map::FormationDef {
                id: format!("army-{}", a.id.0),
                name: a.name.clone(),
                side: a.side,
                doctrine: None,
            };
            let def = crate::map::FormationDef {
                doctrine: self
                    .sides
                    .get(a.side as usize)
                    .and_then(|s| s.ai.as_ref())
                    .and_then(|ai| ai.doctrine.clone()),
                ..def
            };
            let muster = crate::battle::Muster {
                placements: &placements,
                crews: &crews,
                ids: &ids,
            };
            if front
                .battle_mut()
                .reinforce(registry, &def, muster)
                .is_err()
            {
                continue;
            }
            front.origins.extend(ids.iter().map(|u| (*u, a.id)));
            front.origins.sort_unstable_by_key(|(u, _)| *u);
            front.contact.push((a.id, round));
        }
        // The defender's ground is fought over for as long as he is here.
        if let Some(def) = against.and_then(|d| self.army(d)) {
            let front = self.front.as_mut().expect("raised above");
            let objective = Self::ground_of(&def);
            if !front
                .battle()
                .scenario
                .objectives()
                .iter()
                .any(|o| o.id == objective.id)
            {
                front.battle_mut().add_objective(objective);
            }
        }
        self.contest_towns();
        // Orders: she assaults his ground; he holds it; a friend joining
        // moves up onto the nearest ground being fought over.
        let attacker = self.army(army);
        let front = self.front.as_mut().expect("raised above");
        let battle = front.battle_mut();
        let from = attacker.as_ref().and_then(|a| a.tile).unwrap_or(at);
        let nearest_free = |battle: &crate::battle::BattleState, id: Option<&str>| {
            battle
                .scenario
                .objectives()
                .iter()
                .filter(|o| id.is_none_or(|id| o.id == id) && o.id.starts_with("ground-"))
                .flat_map(|o| o.hexes.iter().copied())
                .filter(|h| battle.occupants(*h).next().is_none())
                .min_by_key(|h| (h.unsigned_distance_to(from), h.x, h.y))
        };
        let target = match against {
            Some(d) => nearest_free(battle, Some(&format!("ground-{}", d.0))),
            None => nearest_free(battle, None),
        };
        let formation_of = |battle: &crate::battle::BattleState, id: ElementId| {
            battle
                .formations()
                .iter()
                .position(|f| f.id == format!("army-{}", id.0))
                .map(|i| crate::battle::FormationId(i as u32))
        };
        if let (Some(to), Some(formation)) = (target, formation_of(battle, army)) {
            let mission = if against.is_some() {
                crate::battle::Mission::Assault { to }
            } else {
                crate::battle::Mission::Advance { to }
            };
            let _ = battle.apply(
                registry,
                &crate::battle::Order::SetMission {
                    formation,
                    mission,
                    latitude: crate::battle::Latitude::Delegated,
                },
            );
        }
        let mut events = Vec::new();
        for a in &entering {
            if let Some(x) = self.place_mut(a.id) {
                x.fighting = true;
                x.march = None;
            }
            events.push(OverworldEvent::ArmyEngaged {
                army: a.id,
                against: if a.id == army { against } else { Some(army) },
                at,
            });
        }
        self.mark_commanders(registry);
        events
    }

    /// Whether a person commands `side` in fights: it is nobody's AI's and the
    /// campaign says a person commands.
    fn commanded_in_person(&self, side: u8) -> bool {
        self.human_command
            && self
                .sides
                .get(side as usize)
                .is_some_and(|s| s.ai.is_none())
    }

    /// Whether a person can command `side` in the fighting right now: she
    /// commands it in person and one of its companies there is on her net.
    fn within_her_reach(&self, side: u8) -> bool {
        self.commanded_in_person(side)
            && self.front.as_ref().is_some_and(|f| {
                f.armies().iter().any(|a| {
                    self.army(*a).is_some_and(|army| army.side == side)
                        && !self.out_of_contact.contains(a)
                })
            })
    }

    /// Mark in the front's battle who commands in person, and the vehicle
    /// she rides in if it is there — her acting commander's.
    fn mark_commanders(&mut self, registry: &DataRegistry) {
        let sides: Vec<(u8, Option<CadetId>)> = (0..self.sides.len() as u8)
            .filter(|s| self.commanded_in_person(*s))
            .map(|s| (s, self.acting_commander(registry, s)))
            .collect();
        let Some(front) = self.front.as_mut() else {
            return;
        };
        let battle = front.battle_mut();
        battle.commanders = sides
            .into_iter()
            .map(|(side, cadet)| {
                let rides = cadet.and_then(|c| {
                    battle
                        .units
                        .iter()
                        .find(|u| u.alive() && u.side == side && u.crew.contains(&c))
                        .map(|u| u.id)
                });
                (side, rides)
            })
            .collect();
    }

    /// The fighting, if there is any.
    pub fn front(&self) -> Option<&crate::engagement::Front> {
        self.front.as_ref()
    }

    /// Give an order in the fighting for a side a person commands — a
    /// mission to one of her companies, an order to a crew (which reaches
    /// down), or her commit (WORLD.md W4.2–W4.4).
    pub fn order_in_fight(
        &mut self,
        registry: &DataRegistry,
        order: &crate::battle::Order,
    ) -> Result<Vec<crate::battle::Event>, crate::battle::OrderError> {
        let front = self
            .front
            .as_mut()
            .ok_or(crate::battle::OrderError::NoSuchSide)?;
        front.battle_mut().apply(registry, order)
    }

    /// The side whose orders the fighting is waiting for, if it is: in its
    /// planning phase, with a side a person commands, can reach, and has not
    /// committed.
    pub fn awaiting_orders(&self) -> Option<u8> {
        let front = self.front.as_ref()?;
        let battle = front.battle();
        if battle.is_over() || !battle.is_planning() {
            return None;
        }
        battle
            .living_sides()
            .into_iter()
            .find(|s| self.within_her_reach(*s) && !battle.has_committed(*s))
    }

    /// Whether any crew of `army` in the fighting has an enemy within reach:
    /// as far as she can see or shoot.
    fn army_in_contact(
        registry: &DataRegistry,
        front: &crate::engagement::Front,
        army: ElementId,
    ) -> bool {
        let battle = front.battle();
        front.crews_of(army).into_iter().any(|id| {
            let Some(unit) = battle.unit(id) else {
                return false;
            };
            let sight = crate::battle::stats::vision_range(registry, &battle.roster, unit, None);
            let gun = registry
                .vehicle(&unit.vehicle)
                .map(|v| {
                    v.weapons
                        .iter()
                        .filter_map(|w| registry.weapon(w))
                        .map(|w| w.range[1])
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            let reach = sight.max(gun) as i32;
            battle
                .units
                .iter()
                .any(|o| o.alive() && o.side != unit.side && o.pos.distance_to(unit.pos) <= reach)
        })
    }

    /// One clock tick of the fighting. At the top of each round: every army
    /// that has had nobody within reach for the battle's stalemate rounds,
    /// or has nobody left, leaves it; the towns go to whoever holds them; and
    /// every side a person is not giving the orders for is planned — by
    /// planners seeded from the front's dice and the round, so nothing
    /// between rounds lives outside the battle. Then the battle resolves one
    /// tick. When nobody anywhere is fighting any more, the front is gone.
    fn run_front(&mut self, registry: &DataRegistry) -> Vec<OverworldEvent> {
        let mut events = Vec::new();
        let Some(front) = self.front.as_ref() else {
            return events;
        };
        if front.battle().is_planning() && !front.battle().is_over() {
            let round = front.battle().round;
            let patience = registry.balance.stalemate_rounds;
            let mut leaving = Vec::new();
            let mut contact = front.contact.clone();
            for (army, last) in &mut contact {
                let anybody = front
                    .crews_of(*army)
                    .iter()
                    .any(|u| front.battle().unit(*u).is_some());
                if !anybody {
                    leaving.push(*army);
                } else if Self::army_in_contact(registry, front, *army) {
                    *last = round;
                } else if round.saturating_sub(*last) >= patience {
                    leaving.push(*army);
                }
            }
            if let Some(f) = self.front.as_mut() {
                f.contact = contact;
            }
            for army in leaving {
                events.extend(self.leave_fighting(registry, army));
            }
            // The towns go to whoever holds them.
            let held: Vec<(u8, Hex)> = match (self.front.as_ref(), self.world.as_ref()) {
                (Some(f), Some(w)) => f
                    .battle()
                    .objectives()
                    .filter_map(|(o, holder)| {
                        let i: usize = o.id.strip_prefix("town-")?.parse().ok()?;
                        let town = w.skeleton.towns.get(i)?;
                        Some((holder?, crate::world::chunk_of(town.centre, w.chunk_radius)))
                    })
                    .collect(),
                _ => Vec::new(),
            };
            for (side, hex) in held {
                events.extend(self.take_ground(registry, side, hex));
            }
            self.contest_towns();
            if self.front.as_ref().is_some_and(|f| f.contact.is_empty()) {
                events.extend(self.disband_front(registry));
                return events;
            }
            let in_person: Vec<u8> = (0..self.sides.len() as u8)
                .filter(|s| self.within_her_reach(*s))
                .collect();
            let stand_in = AiConfig {
                planner: "utility".into(),
                difficulty: 3,
                doctrine: None,
            };
            let Some(front) = self.front.as_mut() else {
                return events;
            };
            let seed = front.seed;
            let mut recent = Vec::new();
            let battle = front.battle_mut();
            let round = battle.round as u64;
            let mut ai = crate::ai::AiDriver::new();
            for (side, s) in battle.sides.iter().enumerate() {
                if in_person.contains(&(side as u8)) {
                    continue;
                }
                let config = s.ai.clone().unwrap_or_else(|| stand_in.clone());
                ai.insert(
                    side as u8,
                    crate::ai::make_battle_planner(
                        &config,
                        seed ^ round.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ side as u64,
                        registry,
                    ),
                );
            }
            ai.plan_round_with(registry, battle, |d| {
                recent.extend(d.events.iter().cloned())
            });
            front.recent = recent;
        } else if let Some(front) = self.front.as_mut() {
            front.recent.clear();
        }
        let Some(front) = self.front.as_mut() else {
            return events;
        };
        let ticked = front.battle_mut().step_tick(registry);
        front.recent.extend(ticked);
        if front.battle().is_over() {
            events.extend(self.disband_front(registry));
        }
        events
    }

    /// An army leaves the fighting (WORLD.md W3.3, W3.8): what the fighting
    /// did to its people goes onto the campaign's roll, and what is left of it
    /// is a column again, standing where its first surviving vehicle stands.
    /// Its ground is no longer fought over. If it ends in a hex with an enemy
    /// column, it falls back a hex — the campaign keeps one army to a hex.
    fn leave_fighting(&mut self, registry: &DataRegistry, army: ElementId) -> Vec<OverworldEvent> {
        let Some(front) = self.front.as_ref() else {
            return Vec::new();
        };
        let crews = front.crews_of(army);
        let battle = front.battle();
        let origins: Vec<(UnitId, ElementId)> = crews.iter().map(|u| (*u, army)).collect();
        let field = crate::field::FieldBattle {
            attacker: army,
            defender: army,
            origins: origins.clone(),
        };
        let mut report = field.report(registry, battle);
        // Only this army's people: the rest are still fighting.
        let aboard: std::collections::HashSet<CadetId> = crews
            .iter()
            .filter_map(|u| battle.lookup(*u))
            .flat_map(|u| u.crew.iter().copied())
            .collect();
        report.losses.retain(|loss| aboard.contains(&loss.cadet));
        report.winner = None;
        report.withdrew.clear();
        let stand = crews
            .iter()
            .filter_map(|u| battle.unit(*u))
            .map(|u| u.pos)
            .next();
        let radius = self.world.as_ref().map_or(20, |w| w.chunk_radius);
        let mut events = self.apply_losses_and_survivors(registry, &report);
        if let Some(front) = self.front.as_mut() {
            front.battle_mut().lift_out(&crews);
            front
                .battle_mut()
                .remove_objective(&format!("ground-{}", army.0));
            front.origins.retain(|(_, a)| *a != army);
            front.contact.retain(|(a, _)| *a != army);
        }
        let side = self.army(army).map(|a| a.side);
        if let Some(a) = self.place_mut(army) {
            a.fighting = false;
            if let Some(tile) = stand {
                a.tile = Some(tile);
                a.pos = crate::world::chunk_of(tile, radius);
            }
        }
        if let (Some(side), Some(a)) = (side, self.army(army)) {
            events.extend(self.take_ground(registry, side, a.pos));
            if self
                .columns()
                .iter()
                .any(|o| !o.fighting && o.id != army && o.pos == a.pos && o.side != side)
                && let Some(to) = self.fallback_hex(registry, army, a.pos)
            {
                self.place_army(registry, army, to, &mut events);
            }
        }
        events.push(OverworldEvent::ArmyDisengaged { army });
        self.check_victory(&mut events);
        events
    }

    /// Nobody is fighting anywhere any more: every army still in the front
    /// leaves it, and the front is gone.
    fn disband_front(&mut self, registry: &DataRegistry) -> Vec<OverworldEvent> {
        let Some(front) = self.front.as_ref() else {
            return Vec::new();
        };
        let rounds = front.battle().round;
        let winner = front.battle().over.and_then(|result| result.winner);
        let mut hulls_lost = vec![0; front.battle().sides.len()];
        for unit in front.battle().lost_units() {
            hulls_lost[unit.side as usize] += 1;
        }
        let mut events = Vec::new();
        for army in front.armies() {
            events.extend(self.leave_fighting(registry, army));
        }
        self.front = None;
        events.push(OverworldEvent::FightingOver {
            rounds,
            hulls_lost,
            winner,
        });
        events
    }

    /// Capture `hex` for `side` if it is capturable and not already hers.
    fn take_ground(&mut self, registry: &DataRegistry, side: u8, hex: Hex) -> Vec<OverworldEvent> {
        if let Some(tile) = self.map.get(hex)
            && registry.terrain(tile.terrain).is_some_and(|t| t.capturable)
            && self.owners.get(&hex) != Some(&side)
        {
            self.owners.insert(hex, side);
            return vec![OverworldEvent::ObjectiveCaptured { at: hex, side }];
        }
        Vec::new()
    }

    /// Dawn on the clock: a new day for everybody at once — wounds heal, the
    /// day's marching hours are restored, the net is walked for every side,
    /// held orders go out, whoever held the ground through the night has
    /// held it, and the first side with an army begins the day's orders.
    fn dawn(&mut self, registry: &DataRegistry) -> Vec<OverworldEvent> {
        let mut events = Vec::new();
        self.turn += 1;
        self.roster.advance_day();
        for place in self
            .elements
            .iter_mut()
            .filter(|e| e.alive)
            .filter_map(|e| e.place.as_mut())
        {
            place.moved = false;
            place.marched_ticks = 0;
        }
        let side_count = self.sides.len() as u8;
        for side in 0..side_count {
            self.recompute_contact(registry, side, &mut events);
            events.extend(self.transmit_waiting_missions(side));
        }
        // Standing orders set out at dawn, every side's at once: an army
        // told to advance does not wait for anybody's phase to march.
        for side in 0..side_count {
            events.extend(self.run_standing_missions_for(registry, side));
        }
        let first = (0..side_count)
            .find(|s| self.side_armies(*s).next().is_some())
            .unwrap_or(0);
        self.active_side = first;
        events.push(OverworldEvent::TurnStarted {
            side: first,
            turn: self.turn,
        });
        self.check_held(registry, &mut events);
        events
    }

    fn edge_cost(&self, registry: &DataRegistry, from: Hex, to: Hex) -> Option<u32> {
        crate::battle::movement_edge_cost(registry, &self.map, ARMY_CLASS, ARMY_CLIMB, from, to)
    }

    /// Every tile this army could end its move on, with the cheapest cost to
    /// get there. Ignores whether the army has already moved, so callers can
    /// preview a spent army's reach.
    ///
    /// **A friendly army is driven past, not parked on.** Two divisions do not
    /// share a tile, but a road with a friend on it is still a road: the old
    /// rule refused the whole route, which is why the campaign map would not
    /// let a column follow the column in front of it. This is the same split
    /// the battle layer makes between `passable` and `destination_blocked`,
    /// and it is made the same way — expand through, then filter what can be
    /// stopped on. A visible enemy still blocks outright, because reaching one
    /// is a battle rather than a move and [`Self::attack_targets`] is what
    /// answers for that.
    pub fn reachable(&self, registry: &DataRegistry, id: ElementId) -> HashMap<Hex, u32> {
        let Some(army) = self.army(id) else {
            return HashMap::new();
        };
        if self.world.is_some() {
            return self.reachable_on_ground(registry, &army);
        }
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
                // An enemy is the end of the road; a friend is traffic.
                if self
                    .army_at(next)
                    .is_some_and(|other| other.side != army.side)
                {
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
        // Driven past, not parked on: the tile a friend is standing on is not
        // somewhere this army may finish, so it leaves the set even though the
        // search was allowed to cross it.
        best.retain(|hex, _| *hex == army.pos || self.army_at(*hex).is_none());
        best
    }

    /// Visible enemy armies this army could engage this turn: those sitting
    /// next to a tile it can reach (or next to where it already stands).
    pub fn attack_targets(&self, registry: &DataRegistry, id: ElementId) -> Vec<ElementId> {
        let Some(army) = self.army(id) else {
            return Vec::new();
        };
        let reach = self.reachable(registry, id);
        self.columns()
            .into_iter()
            .filter(|e| e.side != army.side)
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
        principal: ElementId,
        attacking: bool,
    ) -> Vec<ElementId> {
        self.side_armies(side)
            .filter(|a| a.id != principal)
            .filter(|a| a.pos.distance_to(at) <= 1)
            .filter(|a| !attacking || !a.moved)
            .map(|a| a.id)
            .collect()
    }

    /// Spend the turn of every army committed to a battle.
    pub fn commit_to_battle(&mut self, armies: &[ElementId]) {
        for id in armies {
            if let Some(army) = self.place_mut(*id) {
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
            OverworldOrder::TransferUnit { from, to, unit } => {
                self.apply_transfer(*from, *to, *unit)?
            }
            OverworldOrder::Recall { element } => self.apply_recall(*element)?,
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
        id: ElementId,
        mission: ArmyMission,
    ) -> Result<Vec<OverworldEvent>, OverworldError> {
        let army = self.army(id).ok_or(OverworldError::NoSuchArmy)?;
        if army.side != self.active_side && !self.clocked() {
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
        // An army nobody can reach is not refused its orders, it is *not yet*
        // given them: the order waits at headquarters and goes out on the
        // first morning the wire is up. Refusing was the earlier model and it
        // made the player's only recourse "remember to ask again", which is
        // bookkeeping rather than command.
        if !self.in_contact(id) {
            self.hold_mission(id, mission);
            return Ok(vec![OverworldEvent::ArmyOrdersWaiting { army: id }]);
        }
        // Silent replacement, exactly as the battle layer does it: a formation
        // — or an army — holding two missions at once has no meaning anybody
        // could act on. What is news is that an order was given at all, and
        // that is the event.
        self.element_mut(id).expect("checked above").mission = Some(mission.clone());
        Ok(vec![OverworldEvent::ArmyMissionAssigned {
            army: id,
            mission,
        }])
    }

    /// Park a mission for an army out of range, replacing anything already
    /// parked for her. Kept in army-id order so what transmits first cannot
    /// depend on the order the player happened to click in.
    /// Move vehicle `unit` from `from` to `to`. See
    /// [`OverworldOrder::TransferUnit`] for the rules.
    fn apply_transfer(
        &mut self,
        from: ElementId,
        to: ElementId,
        unit: usize,
    ) -> Result<Vec<OverworldEvent>, OverworldError> {
        if from == to {
            return Err(OverworldError::NoTransfer);
        }
        let giver = self.army(from).ok_or(OverworldError::NoSuchArmy)?;
        let taker = self.army(to).ok_or(OverworldError::NoSuchArmy)?;
        if giver.side != self.active_side && !self.clocked() {
            return Err(OverworldError::NotYourTurn);
        }
        if giver.side != taker.side || giver.pos.distance_to(taker.pos) > 1 {
            return Err(OverworldError::NoTransfer);
        }
        // A day's march is the day: a column that has driven cannot also
        // have spent the morning cross-loading, and one that has taken a
        // vehicle on has spent its morning too. Same guard as the move.
        if giver.moved || taker.moved {
            return Err(OverworldError::AlreadyMoved);
        }
        if giver.units.len() <= 1 || unit >= giver.units.len() {
            return Err(OverworldError::NoTransfer);
        }
        // The vehicle goes under the other company, last in its order: the
        // tree is the order of battle, so moving her is changing whom she
        // answers to.
        let leaf = self.members(from)[unit];
        let seq = self.next_seq();
        let moved = self.element_mut(leaf).expect("a member");
        moved.parent = Some(to);
        moved.seq = seq;
        let name = moved
            .vehicle
            .as_ref()
            .map(|v| v.vehicle.clone())
            .unwrap_or_default();
        Ok(vec![OverworldEvent::UnitTransferred {
            from,
            to,
            vehicle: name,
        }])
    }

    fn hold_mission(&mut self, id: ElementId, mission: ArmyMission) {
        match self.waiting_missions.iter_mut().find(|(a, _)| *a == id) {
            Some(slot) => slot.1 = mission,
            None => {
                self.waiting_missions.push((id, mission));
                self.waiting_missions.sort_by_key(|(a, _)| *a);
            }
        }
    }

    /// Send out every parked mission whose army is back on the net.
    ///
    /// Called at a turn start, immediately after contact is recomputed, so an
    /// army that closed up overnight has its orders before it is asked to
    /// carry any out. Only the side whose contact was just read is considered:
    /// everybody else's entry in `out_of_contact` is yesterday's answer, and
    /// transmitting on it would be headquarters talking down a wire nobody has
    /// checked.
    ///
    /// What lands is an ordinary mission assignment — same field written, same
    /// event — because a delayed order is not a different kind of order.
    fn transmit_waiting_missions(&mut self, side: u8) -> Vec<OverworldEvent> {
        let mut events = Vec::new();
        let waiting = std::mem::take(&mut self.waiting_missions);
        let mut still_waiting = Vec::new();
        for (id, mission) in waiting {
            // Destroyed while the order sat in the tray: dropped in silence,
            // there being nobody to give it to.
            let Some(army) = self.army(id) else { continue };
            if army.side != side || !self.in_contact(id) {
                still_waiting.push((id, mission));
                continue;
            }
            self.element_mut(id).expect("checked above").mission = Some(mission.clone());
            events.push(OverworldEvent::ArmyMissionAssigned { army: id, mission });
        }
        self.waiting_missions = still_waiting;
        events
    }

    /// Move an army toward `to` the way a hand order does: hostile armies are
    /// obstacles to be gone round, and the only fight this can start is the one
    /// the player pointed at.
    ///
    /// This is the public shape of movement and its semantics are the player's
    /// — see [`Engagement::Avoid`]. Delegated missions reach the same code
    /// through [`Self::move_army`] with their own engagement rule; there is
    /// deliberately no second movement path.
    fn apply_move(
        &mut self,
        registry: &DataRegistry,
        id: ElementId,
        to: Hex,
    ) -> Result<Vec<OverworldEvent>, OverworldError> {
        // An order to a vehicle that goes with her company sends her out on
        // her own: she takes a place of her own where her company stands,
        // and the march is hers (WORLD.md W4.6b). If the march cannot be
        // made she was never sent.
        let detached = self.army(id).is_none();
        let from = if detached {
            Some(self.detach(id)?)
        } else {
            None
        };
        match self.move_army(registry, id, to, Engagement::Avoid) {
            Ok(mut events) => {
                if let Some(from) = from {
                    events.insert(0, OverworldEvent::ArmyDetached { army: id, from });
                }
                Ok(events)
            }
            Err(e) => {
                if detached && let Some(el) = self.element_mut(id) {
                    el.place = None;
                    el.mission = None;
                }
                Err(e)
            }
        }
    }

    /// Give a vehicle that goes with her company a place of her own, where
    /// the company stands, and a standing order to hold — so that when she
    /// gets where she is sent she stays there until she is recalled (the
    /// designer's ruling), exactly as a crew's march in battle becomes a
    /// hold. Returns her company. See [`OverworldError::NoDetach`] for what
    /// is refused.
    fn detach(&mut self, id: ElementId) -> Result<ElementId, OverworldError> {
        let element = self.element(id).ok_or(OverworldError::NoSuchArmy)?;
        if !element.alive || element.place.is_some() {
            return Err(OverworldError::NoSuchArmy);
        }
        let vehicle = element.vehicle.as_ref().ok_or(OverworldError::NoDetach)?;
        let home = self.column_of(id).ok_or(OverworldError::NoSuchArmy)?;
        let column = self.army(home).ok_or(OverworldError::NoSuchArmy)?;
        let commander = self
            .sides
            .get(element.side as usize)
            .and_then(|s| s.commander);
        if !self.clocked()
            || column.fighting
            || column.units.len() <= 1
            || commander.is_some_and(|c| vehicle.crew.contains(&c))
        {
            return Err(OverworldError::NoDetach);
        }
        let place = Place {
            march: None,
            moved: false,
            fighting: false,
            ..self
                .element(home)
                .and_then(|e| e.place.clone())
                .expect("a column")
        };
        let element = self.element_mut(id).expect("checked above");
        element.place = Some(place);
        element.mission = Some(ArmyMission::Hold);
        Ok(home)
    }

    /// Take a node's own orders back. See [`OverworldOrder::Recall`].
    fn apply_recall(&mut self, id: ElementId) -> Result<Vec<OverworldEvent>, OverworldError> {
        self.army(id).ok_or(OverworldError::NoSuchArmy)?;
        if self.home_of(id).is_none() {
            return Err(OverworldError::NoRecall);
        }
        let element = self.element_mut(id).expect("checked above");
        element.mission = None;
        if let Some(place) = element.place.as_mut() {
            place.march = None;
        }
        Ok(vec![OverworldEvent::ArmyRecalled { army: id }])
    }

    /// The column a node that stands on its own belongs with: its parent's.
    /// `None` for a company, whose parent is its side.
    fn home_of(&self, id: ElementId) -> Option<ElementId> {
        self.column_of(self.element(id)?.parent?)
    }

    /// Every node that stands on its own with no orders of its own goes
    /// home (WORLD.md W4.6b): it marches for its company, and once it stands
    /// with it — on its hex or the next, since a column does not finish a
    /// march inside a friend's — its place is dropped and it is part of the
    /// column again. Nothing is stored to say she is on her way back: a
    /// node stands apart while it has orders of its own, and one without
    /// them is going home because that is what having none means.
    fn bring_home(&mut self, registry: &DataRegistry) -> Vec<OverworldEvent> {
        let returning: Vec<(ElementId, ElementId)> = self
            .elements
            .iter()
            .filter(|e| e.alive && e.mission.is_none())
            .filter(|e| e.place.as_ref().is_some_and(|p| !p.fighting))
            // A company answers to its side, which stands nowhere: only a
            // node below a company has a column to go home to. Asked first
            // because this runs every tick and nearly every node is one.
            .filter(|e| {
                e.parent
                    .and_then(|p| self.element(p))
                    .is_some_and(|p| p.parent.is_some())
            })
            .filter_map(|e| Some((e.id, self.home_of(e.id)?)))
            .collect();
        let mut events = Vec::new();
        for (id, home) in returning {
            let (Some(her), Some(company)) = (self.army(id), self.army(home)) else {
                continue;
            };
            if company.fighting {
                continue;
            }
            if her.pos.distance_to(company.pos) <= 1 {
                if let Some(e) = self.element_mut(id) {
                    e.place = None;
                }
                events.push(OverworldEvent::ArmyRejoined {
                    army: id,
                    into: home,
                });
            } else if her.march.as_ref().is_none_or(|m| m.to != company.pos)
                && let Ok(more) = self.move_army(registry, id, company.pos, Engagement::Avoid)
            {
                events.extend(more);
            }
        }
        events
    }

    fn move_army(
        &mut self,
        registry: &DataRegistry,
        id: ElementId,
        to: Hex,
        engagement: Engagement,
    ) -> Result<Vec<OverworldEvent>, OverworldError> {
        let (side, pos, movement, moved) = {
            let army = self.army(id).ok_or(OverworldError::NoSuchArmy)?;
            (army.side, army.pos, army.movement, army.moved)
        };
        // On the clock an order is given on any tick (WORLD.md W4.4): there
        // is no turn to be out of, and a newer order replaces the march in
        // progress rather than being refused for it.
        if side != self.active_side && !self.clocked() {
            return Err(OverworldError::NotYourTurn);
        }
        if moved && !self.clocked() {
            return Err(OverworldError::AlreadyMoved);
        }

        // On a generated world the march is over the real ground, tile by
        // tile; it produces the same three things the march below does, and
        // everything after them is shared.
        //
        // And it is carried out by the clock (WORLD.md W3.1): the order sets
        // the march, and the day walks it, tick by tick, simultaneously with
        // everybody else's — see `run_clock`.
        if self.world.is_some() {
            let army = self.army(id).expect("checked above").clone();
            // Ordered onto the hex it stands on: that is a hold, and it ends
            // whatever march it was on. The campaign planner says "stay" this
            // way; refusing it spent the army's day and left it marching.
            if army.pos == to {
                let a = self.place_mut(id).expect("checked above");
                a.march = None;
                a.moved = true;
                return Ok(Vec::new());
            }
            let leg = self.plan_leg(registry, &army, to, engagement)?;
            let a = self.place_mut(id).expect("checked above");
            a.march = Some(MarchOrder {
                to,
                engagement,
                leg,
                banked: 0,
            });
            a.moved = true;
            return Ok(Vec::new());
        }

        let path = hexx::algorithms::a_star(pos, to, |from, next| {
            if from == next {
                return Some(0);
            }
            // Ending on an enemy is the attack case, handled below, so `to`
            // itself is always open. Of the rest, **only a hostile army can
            // block a route, and only a march that means to avoid contact.**
            //
            // A friend used to block, and that was the campaign map's version
            // of one crew to a hex: a column could not follow the column in
            // front of it, and the only reason was that the pathfinder could
            // not tell "cannot stop here" from "cannot cross here". It stops
            // being able to stop there in the trim below, which is where the
            // rule belongs.
            //
            // A hostile army is a wall to a march that means to avoid it and
            // ordinary ground to one that means to hit whatever it finds.
            // Routing an advance *around* the enemy in its road was the old
            // behaviour and it is precisely the bug — an operational advance
            // that side-steps contact is not an advance.
            if next != to
                && let Some(other) = self.army_at(next)
                && other.side != side
                && engagement == Engagement::Avoid
            {
                return None;
            }
            self.edge_cost(registry, from, next)
        })
        .ok_or(OverworldError::NoPath)?;

        // Trim the path to this turn's movement budget.
        //
        // Why the walk stopped is recorded here rather than worked out again
        // from where the army ended up: an army can halt one tile short of an
        // enemy because the day ran out, and that is a march that stopped, not
        // an attack that started. Only a walk halted *by* a hostile army it
        // could otherwise have stepped onto has made contact.
        let mut budget = movement;
        let mut walked = vec![pos];
        let mut blocked_by: Option<(ElementId, Hex)> = None;
        for pair in path.windows(2) {
            let Some(step) = self.edge_cost(registry, pair[0], pair[1]) else {
                break;
            };
            if step > budget {
                break;
            }
            // A hostile tile is the end of the march and a battle. A friendly
            // one is traffic: drive past it and keep going, which is what
            // `reachable` now offers routes through.
            if let Some(other) = self.army_at(pair[1])
                && other.side != side
            {
                blocked_by = Some((other.id, pair[1]));
                break;
            }
            budget -= step;
            walked.push(pair[1]);
        }
        // Two armies do not share a tile, so if the budget ran out on top of a
        // friend, fall back to the last tile that is actually free. Without
        // this the pass-through above would let a column stop inside the
        // column it was following.
        while walked.len() > 1
            && self
                .army_at(*walked.last().expect("non-empty"))
                .is_some_and(|other| other.id != id)
        {
            walked.pop();
        }
        let destination = *walked.last().expect("path starts at pos");
        Ok(self.arrive(registry, id, side, to, walked, destination, blocked_by))
    }

    /// The end of a march, whichever kind: stand where it stopped, take what
    /// is capturable there, and start the fight that stopped it or that it
    /// was sent to have.
    #[allow(clippy::too_many_arguments)]
    fn arrive(
        &mut self,
        registry: &DataRegistry,
        id: ElementId,
        side: u8,
        to: Hex,
        walked: Vec<Hex>,
        destination: Hex,
        blocked_by: Option<(ElementId, Hex)>,
    ) -> Vec<OverworldEvent> {
        let mut events = Vec::new();
        {
            let army = self.place_mut(id).expect("checked above");
            army.pos = destination;
            army.moved = true;
        }
        events.push(OverworldEvent::ArmyMoved {
            army: id,
            path: walked,
        });

        // Capture objectives by standing on them.
        if let Some(tile) = self.map.get(destination)
            && registry.terrain(tile.terrain).is_some_and(|t| t.capturable)
            && self.owners.get(&destination) != Some(&side)
        {
            self.owners.insert(destination, side);
            events.push(OverworldEvent::ObjectiveCaptured {
                at: destination,
                side,
            });
        }

        // An army brought up short by an enemy standing in its road has made
        // contact, and under an advance that is the fight it was sent to have.
        // At most one battle comes out of one move: the walk stopped at the
        // first hostile and nothing goes past it, so this and the ordered
        // destination below are alternatives rather than two chances to fire.
        //
        // Under `Avoid` this branch can only be the enemy sitting on `to`
        // itself, which is the same battle the next branch would have
        // announced, on the same tile — the hand order's behaviour is
        // unchanged whichever of the two says it.
        if let Some((defender, at)) = blocked_by {
            events.push(OverworldEvent::BattleTriggered {
                attacker: id,
                defender,
                at,
            });
        } else if let Some(defender) = self.army_at(to)
            // If we stopped adjacent to the ordered destination because an
            // enemy holds it, that's an attack.
            && defender.side != side
            && destination.distance_to(to) == 1
        {
            events.push(OverworldEvent::BattleTriggered {
                attacker: id,
                defender: defender.id,
                at: to,
            });
        }
        events
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
    /// Everything it does goes through [`Self::move_army`], the same mover the
    /// player's own click drives, so a mission captures ground and reports its
    /// battles by exactly the same code. There is deliberately no second path
    /// for the AI to drive an army along.
    ///
    /// What the mission chooses is the one thing the two moves differ on, the
    /// [`Engagement`]. An `Advance` is movement to contact — it paths as though
    /// hostile armies were open ground and attacks the first one that stops it,
    /// because an operational order to take ground is an order to take what is
    /// standing on the road to it, and the alternative is what this used to do:
    /// walk up beside the enemy and wait there for ever. A `Withdraw` avoids,
    /// as a hand order does. An army falling back is trying to be somewhere
    /// else, and one that started a battle on the way out would be obeying the
    /// opposite of what it was told.
    fn run_standing_missions(&mut self, registry: &DataRegistry) -> Vec<OverworldEvent> {
        self.run_standing_missions_for(registry, self.active_side)
    }

    /// [`Self::run_standing_missions`] for one side, whichever side's phase it
    /// is: at dawn on the clock every side's standing orders set out at once.
    fn run_standing_missions_for(
        &mut self,
        registry: &DataRegistry,
        side: u8,
    ) -> Vec<OverworldEvent> {
        let ordered: Vec<(ElementId, Hex, Engagement)> = self
            .side_armies(side)
            .filter(|a| !a.moved)
            .filter_map(|a| match a.mission.as_ref()? {
                ArmyMission::Advance { to } => Some((a.id, *to, Engagement::EnRoute)),
                ArmyMission::Withdraw { to } => Some((a.id, *to, Engagement::Avoid)),
                ArmyMission::Hold => None,
            })
            .collect();
        let mut events = Vec::new();
        for (id, to, engagement) in ordered {
            // Arrived, destroyed since the list was taken, or overtaken by a
            // battle that spent its turn: nothing to do either way.
            if self.army(id).is_none_or(|a| a.moved || a.pos == to) {
                continue;
            }
            if let Ok(more) = self.move_army(registry, id, to, engagement) {
                events.extend(more);
            }
        }
        events
    }

    fn apply_end_turn(&mut self, registry: &DataRegistry) -> Vec<OverworldEvent> {
        // Before the day turns over, everybody who was told what to do and not
        // told otherwise does it.
        let mut events = self.run_standing_missions(registry);
        // On the clock the sides give their orders in turn at dawn, and when
        // the last has, the day runs for everybody at once — until dawn, or
        // until a column meets an enemy, when the fight is fought and the
        // same end-of-turn runs the rest of the day.
        if self.clocked() {
            let side_count = self.sides.len() as u8;
            let later =
                (self.active_side + 1..side_count).find(|s| self.side_armies(*s).next().is_some());
            if let Some(next) = later {
                self.active_side = next;
                events.push(OverworldEvent::TurnStarted {
                    side: next,
                    turn: self.turn,
                });
                return events;
            }
            events.extend(self.run_clock(registry));
            return events;
        }
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
            // A new day: wounds heal and cadets walking back from a wreck get
            // one day closer. Once per day rather than once per side's phase,
            // or a two-academy campaign would heal twice as fast as a four.
            self.roster.advance_day();
        }
        let dawn = next <= self.active_side;
        self.active_side = next;
        for place in self
            .elements
            .iter_mut()
            .filter(|e| e.alive && e.side == next)
            .filter_map(|e| e.place.as_mut())
        {
            place.moved = false;
        }

        events.push(OverworldEvent::TurnStarted {
            side: next,
            turn: self.turn,
        });
        // Last, with everybody standing where the night left them: who can be
        // reached today decides which orders may be given today.
        self.recompute_contact(registry, next, &mut events);
        // ...and whatever headquarters has been holding for the ones it can
        // reach again goes out with the morning's traffic.
        events.extend(self.transmit_waiting_missions(next));
        // Last of all, whoever held the ground through the night has held
        // it. Dawn rather than the moment of capture, so that taking the last
        // factory on your turn is not the end of the campaign but the start
        // of the enemy's last chance to take it back; and after the morning's
        // events rather than before them, so the ending is the last thing the
        // day says.
        if dawn {
            self.check_held(registry, &mut events);
        }
        events
    }

    /// Feed a battle outcome back into the strategic layer. Every
    /// participating army gets its surviving roster back; armies that lost
    /// everything are destroyed, armies that withdrew fall back a hex, and
    /// the victor advances onto the contested tile if it has been vacated.
    pub fn apply_battle_result(
        &mut self,
        registry: &DataRegistry,
        report: &BattleReport,
    ) -> Vec<OverworldEvent> {
        let attacker = report.attacker;
        let defender = report.defender;
        let attacker_pos = self.army(attacker).map(|a| a.pos);
        let defender_pos = self.army(defender).map(|a| a.pos);
        let mut events = self.apply_losses_and_survivors(registry, report);

        // An army that left the field by an exit is not where the battle
        // was. It falls back a hex — along its own orders if it is under a
        // withdrawal, and otherwise straight away from whoever it was
        // fighting — so that a withdrawal *goes* somewhere instead of leaving
        // the army standing on the ground it just gave up, in contact with
        // the enemy it just broke contact with. In id order, because two
        // armies falling back onto one hex is settled by who moves first.
        let mut withdrew: Vec<ElementId> = report.withdrew.clone();
        withdrew.sort_unstable();
        withdrew.dedup();
        for id in withdrew {
            let Some(army) = self.army(id) else {
                continue;
            };
            let enemy = if army.side == self.army(attacker).map_or(u8::MAX, |a| a.side) {
                defender_pos
            } else {
                attacker_pos
            };
            let Some(enemy) = enemy else {
                continue;
            };
            if let Some(to) = self.fallback_hex(registry, id, enemy) {
                self.place_army(registry, id, to, &mut events);
            }
        }

        // The ground was contested and the attacker is the one still on it.
        // Advancing onto a *vacated* tile rather than onto a destroyed
        // defender's is what makes the two ways of losing ground the same
        // ground lost: a defender who withdrew has yielded it exactly as one
        // who burned has. An attacker who herself withdrew has yielded her
        // claim, and one who has been wiped out has nobody to advance.
        if let (Some(pos), Some(att)) = (defender_pos, self.army(attacker))
            && !report.withdrew.contains(&attacker)
            && self.army_at(pos).is_none()
        {
            let att_id = att.id;
            self.place_army(registry, att_id, pos, &mut events);
        }
        self.check_victory(&mut events);
        events
    }

    /// What any fight did to the people in it, whichever kind of fight it
    /// was: every casualty resolved (in cadet-id order, through the campaign
    /// rng), every survivor credited, every army handed back what it has
    /// left and destroyed if that is nothing. Shared by a battle fought as an
    /// event and an engagement fought on the ground (WORLD.md W3.3); what
    /// each does about *where* the armies are afterwards is its own.
    fn apply_losses_and_survivors(
        &mut self,
        registry: &DataRegistry,
        report: &BattleReport,
    ) -> Vec<OverworldEvent> {
        let mut events = Vec::new();
        // Casualties first, so a cadet's fate is settled before the surviving
        // rosters are written back. Resolved in cadet-id order rather than the
        // order the battle happened to report them, because the rng is shared
        // and the campaign has to replay identically.
        let mut losses: Vec<&CrewLoss> = report.losses.iter().collect();
        losses.sort_by_key(|loss| loss.cadet);
        for loss in losses {
            let safety = registry
                .vehicle(&loss.vehicle)
                .map(|v| v.safety)
                .unwrap_or(3);
            // Two tables, one decision: a cadet pulled out of a wreck is
            // priced by what wrecked it, and a cadet carried home in her own
            // tank by how the crew found her. Both roll through the campaign
            // rng in cadet-id order, so a replay agrees with the day it
            // replays.
            let fate = match loss.found {
                None => resolve_crew_fate(
                    self.rules,
                    &registry.casualties,
                    safety,
                    loss.killed_by,
                    loss.aid,
                    &mut self.rng,
                ),
                Some(found) => resolve_station_fate(
                    self.rules,
                    &registry.casualties,
                    found,
                    loss.aid,
                    &mut self.rng,
                ),
            };
            if let Some(cadet) = self.roster.get_mut(loss.cadet) {
                // Never *shortens* a recovery already under way. A cadet the
                // muster called up went out hurt, so a battle can hand back a
                // gentler answer than the one she carried into it, and
                // writing it straight over her would have her signed fit on
                // the strength of having been shot at.
                cadet.status = cadet.status.worse_of(fate.into());
            }
            events.push(OverworldEvent::CrewCasualty {
                cadet: loss.cadet,
                fate,
            });
        }

        // Everyone who came through it has one more battle behind her.
        for (_, units) in &report.survivors {
            for unit in units {
                for cadet in &unit.crew {
                    self.roster.credit_battle(*cadet);
                }
            }
        }

        for (id, units) in &report.survivors {
            let id = *id;
            // A vehicle that marched out with no named crew is given an
            // anonymous one at the battle — enlisted into the battle's *copy*
            // of the roster, so her handle means nothing here. Writing those
            // handles back would leave an army holding ids the campaign
            // cannot resolve, which is not a crash but is a cadet-shaped hole
            // in every roster read afterwards. The academy's rolls are the
            // academy's: a crew member the campaign never enlisted does not
            // join it by having fought once.
            let mut units = units.clone();
            for unit in &mut units {
                unit.crew.retain(|cadet| self.roster.get(*cadet).is_some());
            }
            if self.army(id).is_some() && !self.set_survivors(id, units) {
                events.push(OverworldEvent::ArmyDestroyed { army: id });
            }
        }

        events
    }

    /// Where an army that withdrew from a battle at `enemy` ends up: one hex
    /// back, along its orders if it has any, and otherwise away.
    ///
    /// Under a [`ArmyMission::Withdraw`] the first step of the road toward
    /// where it was told to go, so a withdrawal ordered and a withdrawal
    /// fought agree about which way is back. Otherwise the free neighbouring
    /// hex furthest from the enemy, cheapest to enter among those, with the
    /// coordinate last as the only key a reflection does not preserve
    /// (CLAUDE.md, the tiebreak invariant). `None` when there is nowhere to
    /// go — hemmed in by armies or the map edge — and the army stands where
    /// it was.
    fn fallback_hex(&self, registry: &DataRegistry, id: ElementId, enemy: Hex) -> Option<Hex> {
        let army = self.army(id)?;
        let pos = army.pos;
        let free = |hex: Hex| self.army_at(hex).is_none();
        if let Some(ArmyMission::Withdraw { to }) = army.mission
            && to != pos
            && let Some(path) = hexx::algorithms::a_star(pos, to, |from, next| {
                if from == next {
                    return Some(0);
                }
                if self
                    .army_at(next)
                    .is_some_and(|other| other.side != army.side)
                {
                    return None;
                }
                self.edge_cost(registry, from, next)
            })
            && let Some(step) = path.get(1).copied()
            && free(step)
        {
            return Some(step);
        }
        let mut candidates: Vec<(Hex, u32)> = pos
            .all_neighbors()
            .into_iter()
            .filter(|hex| free(*hex))
            .filter_map(|hex| self.edge_cost(registry, pos, hex).map(|cost| (hex, cost)))
            .collect();
        candidates
            .sort_by_key(|(hex, cost)| (Reverse(hex.distance_to(enemy)), *cost, hex.x, hex.y));
        candidates.first().map(|(hex, _)| *hex)
    }

    /// Put an army on a hex without a march: the way a victor takes the
    /// contested tile and a withdrawn army falls back. Captures what it
    /// stands on, exactly as [`Self::move_army`] does — an army standing on a
    /// factory holds it however it came to be standing there — and reports
    /// the step as a one-hex [`OverworldEvent::ArmyMoved`] so the screen
    /// can show it.
    fn place_army(
        &mut self,
        registry: &DataRegistry,
        id: ElementId,
        to: Hex,
        events: &mut Vec<OverworldEvent>,
    ) {
        let Some(side) = self.army(id).map(|a| a.side) else {
            return;
        };
        // Placed rather than marched — a withdrawal, a victor advancing —
        // so on a generated world it stands where a column stands on that hex.
        let tile = self.world.as_ref().map(|w| w.stand_tile(registry, to));
        let Some(army) = self.place_mut(id) else {
            return;
        };
        let from = army.pos;
        if from == to {
            return;
        }
        army.pos = to;
        if tile.is_some() {
            army.tile = tile;
        }
        events.push(OverworldEvent::ArmyMoved {
            army: id,
            path: vec![from, to],
        });
        if let Some(tile) = self.map.get(to)
            && registry.terrain(tile.terrain).is_some_and(|t| t.capturable)
            && self.owners.get(&to) != Some(&side)
        {
            self.owners.insert(to, side);
            events.push(OverworldEvent::ObjectiveCaptured { at: to, side });
        }
    }

    /// Whether anybody has won by the rules that read the board — running
    /// the enemy out of armies, or out of headquarters — and if so, say so.
    ///
    /// One test, [`Self::defeated`], and the campaign is over when at most
    /// one side passes it: that side wins, or nobody does if none is left.
    /// Elimination is reported ahead of decapitation when both would be
    /// true, because a side with no armies has no headquarters either and the
    /// larger fact is the one worth saying.
    fn check_victory(&mut self, events: &mut Vec<OverworldEvent>) {
        if self.over.is_some() {
            return;
        }
        let standing: Vec<u8> = (0..self.sides.len() as u8)
            .filter(|side| !self.defeated(*side))
            .collect();
        if standing.len() > 1 {
            return;
        }
        let winner = standing.first().copied();
        let eliminated = (0..self.sides.len() as u8)
            .filter(|side| Some(*side) != winner)
            .all(|side| self.side_armies(side).next().is_none());
        let commander = (0..self.sides.len() as u8)
            .filter(|side| Some(*side) != winner)
            .any(|side| self.victory.commander && self.commander_killed(side));
        let reason = if eliminated {
            CampaignEnd::Elimination
        } else if commander {
            CampaignEnd::CommanderKilled
        } else {
            CampaignEnd::Decapitation
        };
        self.finish(winner, reason, events);
    }

    /// The dawn check: whether one side held every tile the map said to
    /// hold through the night, and for as many nights running as it asked.
    fn check_held(&mut self, registry: &DataRegistry, events: &mut Vec<OverworldEvent>) {
        if self.over.is_some() {
            return;
        }
        let holder = (0..self.sides.len() as u8).find(|side| {
            self.hold_progress(registry, *side)
                .is_some_and(|(held, total)| total > 0 && held == total)
        });
        self.hold_streak = holder.map(|side| match self.hold_streak {
            Some((who, nights)) if who == side => (side, nights + 1),
            _ => (side, 1),
        });
        if let Some((side, nights)) = self.hold_streak
            && nights >= self.victory.hold_days.max(1)
        {
            self.finish(Some(side), CampaignEnd::Held, events);
        }
    }

    fn finish(
        &mut self,
        winner: Option<u8>,
        reason: CampaignEnd,
        events: &mut Vec<OverworldEvent>,
    ) {
        self.over = Some(winner);
        events.push(OverworldEvent::GameEnded { winner, reason });
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
    principal: ElementId,
    attacking: bool,
) -> Vec<ElementId> {
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
///
/// The one thing it knows beyond "what is worth going to" is what the map
/// said the campaign turns on. Under [`CampaignVictory::decapitation`] the
/// enemy's headquarters is the target that ends the war and its own is the
/// army that must not be caught, and it plays both: the enemy's
/// headquarters is worth [`Self::HEADQUARTERS_WORTH`] times an ordinary
/// army, and its own backs away from a *stronger* force that could reach
/// it and never picks a fight with one. It is still an army — five vehicles
/// with a staff aboard, not a staff car — so against an equal or weaker
/// force it fights like any other; a headquarters that ran from parity
/// would be chased off every objective on the map by one company. On a map
/// that declares no decapitation rule the flag is just where the radio is,
/// and the planner reads nothing.
pub struct SimpleOverworldPlanner {
    rng: ChaCha8Rng,
    /// Chance to pick a suboptimal target, derived from difficulty.
    blunder: f64,
}

impl SimpleOverworldPlanner {
    /// How much more the enemy's headquarters is worth than another army of
    /// the same size, when losing it loses the campaign. Three, because the
    /// score an ordinary army earns is scaled by relative strength and a
    /// headquarters is usually the best-found army on the map; anything
    /// smaller left the planner preferring the weak flank column it could
    /// beat to the command it could end the war on.
    pub const HEADQUARTERS_WORTH: f32 = 3.0;

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

    /// Whether a visible enemy could march onto `hex` next turn: within its
    /// movement plus one, since a march that stops beside a hex attacks it.
    /// Crow flight rather than the road, which is the same generosity the
    /// rest of this planner allows itself.
    fn within_reach(enemies: &[&Column], hex: Hex) -> bool {
        enemies
            .iter()
            .any(|e| e.pos.distance_to(hex) <= e.movement as i32 + 1)
    }

    /// Whether `enemy` outnumbers `army` in vehicles: the one measure of
    /// strength this planner has, and the one it already scores targets by.
    fn stronger(enemy: &Column, army: &Column) -> bool {
        enemy.units.len() > army.units.len()
    }

    /// Where the headquarters goes: the reachable hex that keeps it furthest
    /// from the nearest enemy that could reach it, or where it stands if
    /// nothing can. Distance first, then the cheaper road, then the
    /// coordinate — last, because it is the one key a reflection does not
    /// preserve.
    fn shelter(
        registry: &DataRegistry,
        state: &OverworldState,
        army: &Column,
        enemies: &[&Column],
    ) -> Hex {
        let mut options: Vec<(Hex, u32)> = state
            .reachable(registry, army.id)
            .into_iter()
            .filter(|(hex, _)| *hex == army.pos || state.army_at(*hex).is_none())
            .collect();
        let nearest = |hex: Hex| {
            enemies
                .iter()
                .map(|e| e.pos.distance_to(hex))
                .min()
                .unwrap_or(i32::MAX)
        };
        options.sort_by_key(|(hex, cost)| (Reverse(nearest(*hex)), *cost, hex.x, hex.y));
        options.first().map_or(army.pos, |(hex, _)| *hex)
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

/// On the clock, a side's planner gives the day's orders: asked until it
/// has nothing more to say, each order applied as it comes, and nobody's
/// turn ended — the clock runs on its own (WORLD.md W3.1, real time). An
/// order the engine refuses spends that army's day, as in `step_planner`.
pub fn plan_day(
    planner: &mut dyn AiPlanner<OverworldState, OverworldOrder>,
    registry: &DataRegistry,
    state: &mut OverworldState,
    side: u8,
) -> Vec<OverworldEvent> {
    let mut events = Vec::new();
    for _ in 0..64 {
        let order = planner.next_order(registry, state, side);
        if order == OverworldOrder::EndTurn {
            break;
        }
        match state.apply(registry, &order) {
            Ok(more) => events.extend(more),
            Err(_) => {
                if let OverworldOrder::MoveArmy { army, .. } = order
                    && let Some(a) = state.place_mut(army)
                {
                    a.moved = true;
                } else {
                    break;
                }
            }
        }
    }
    events
}

/// Ask the active side's planner for one order and apply it.
///
/// The one loop both callers walk: the campaign screen's `drive_ai` and the
/// harness's headless campaign. It carries the one piece of judgment the
/// loop has — an order the engine refuses must not wedge the day — and that
/// piece was written in the game crate, where no headless run could reach
/// it. A refused march spends the army's turn; anything else refused ends
/// the side's turn.
pub fn step_planner(
    planner: &mut dyn AiPlanner<OverworldState, OverworldOrder>,
    registry: &DataRegistry,
    state: &mut OverworldState,
) -> Vec<OverworldEvent> {
    let side = state.active_side;
    let order = planner.next_order(registry, state, side);
    match state.apply(registry, &order) {
        Ok(events) => events,
        Err(_) => {
            if let OverworldOrder::MoveArmy { army, .. } = order
                && let Some(a) = state.place_mut(army)
            {
                a.moved = true;
                return Vec::new();
            }
            state
                .apply(registry, &OverworldOrder::EndTurn)
                .unwrap_or_default()
        }
    }
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
        let seen: Vec<Column> = state
            .visible_armies(registry, side)
            .into_iter()
            .filter(|e| e.side != side)
            .collect();
        let enemies: Vec<&Column> = seen.iter().collect();
        let guarded = state.victory.decapitation && army.headquarters;

        // The headquarters, when losing it is losing, and a stronger force
        // that could reach it: it backs away. Whatever it does spends the
        // turn — a move to its own hex is how it says "hold" — because a
        // planner that returned no order for an unmoved army would be asked
        // about the same army for ever.
        if guarded {
            let stronger: Vec<&Column> = enemies
                .iter()
                .copied()
                .filter(|e| Self::stronger(e, &army))
                .collect();
            if Self::within_reach(&stronger, army.pos) {
                let to = Self::shelter(registry, state, &army, &stronger);
                return OverworldOrder::MoveArmy { army: army.id, to };
            }
        }

        let mut targets: Vec<(Hex, f32)> = Vec::new();
        // Enemy armies we can see: value inversely proportional to size, and
        // the one carrying the enemy's headquarters worth the campaign. A
        // guarded headquarters never picks a fight with a stronger army.
        for enemy in &enemies {
            if guarded && Self::stronger(enemy, &army) {
                continue;
            }
            let strength = enemy.units.len().max(1) as f32;
            let ours = army.units.len().max(1) as f32;
            let worth = if state.victory.decapitation && enemy.headquarters {
                Self::HEADQUARTERS_WORTH
            } else {
                1.0
            };
            targets.push((enemy.pos, 6.0 * worth * (ours / strength)));
        }
        // Objectives we don't own.
        for (hex, tile) in state.map.iter() {
            let Some(t) = registry.terrain(tile.terrain) else {
                continue;
            };
            if t.capturable && state.owners.get(&hex) != Some(&side) {
                targets.push((hex, 4.0 + t.value as f32 * 0.5));
            }
        }
        if targets.is_empty() {
            return OverworldOrder::EndTurn;
        }

        // The coordinate last, and only because `map.iter()` walks a hash
        // map: two targets tied on the score would otherwise be ordered by
        // wherever the table happened to put them, and the planner would be
        // a different planner on every machine.
        targets.sort_by(|a, b| {
            let da = army.pos.distance_to(a.0) as f32 - a.1;
            let db = army.pos.distance_to(b.0) as f32 - b.1;
            da.total_cmp(&db)
                .then(a.0.x.cmp(&b.0.x))
                .then(a.0.y.cmp(&b.0.y))
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
