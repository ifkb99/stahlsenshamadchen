//! A side that fights through its chain of command.
//!
//! [`SideCommand`] is what stands behind a side when its map names the
//! `command` planner: a commander brain that issues missions to formations,
//! and a utility-planner executor per formation that plans each member unit
//! in service of its standing mission. From the outside it is an ordinary
//! [`AiPlanner`] — the driver asks it for one order at a time until it
//! commits — which is the point: the hierarchy is an implementation detail
//! of the side, not a new contract for every caller.
//!
//! The brain deliberately speaks only through [`Order::SetMission`]. It could
//! reach into the executors and tell them things privately, and it must
//! never: a mission that exists only inside a planner is one the log cannot
//! report, the save cannot carry, and a replaced brain (a human, an LLM)
//! cannot see. Everything the commander decides goes through the order
//! stream like everybody else's decisions.
//!
//! The first brain is modest on purpose: it divides the map's ground worth
//! holding among its formations, biggest objective to the first-declared
//! formation, and holds each mission until it has a different one. Whether
//! that is *enough* brain is what the balance harness's delegation-tax table
//! exists to say; making it clever belongs to the willingness work, not to
//! the plumbing.
//!
//! **The brain never insists.** Every [`Order::SetMission`] below is issued at
//! [`Latitude::Delegated`](crate::battle::Latitude::Delegated), spelled out
//! rather than defaulted so that a reader can see it is a decision. `Binding`
//! is a thing a *player* says, and keeping it out of AI-vs-AI play is what
//! makes the determinism baseline still valid across the chunk that added it
//! — if `tests/snapshots/event_stream.txt` moves when latitude is touched,
//! the insistence has leaked in here.
//!
//! **When a commander stops assigning ground** is
//! [`devolved`](crate::data::PlannerRules::devolved) in the `planner` block,
//! read against each doctrine's own `delegation`. It was a constant here
//! until 2026-08-27; the reasoning behind the number lives on the field now.

use super::{AiConfig, AiPlanner, Evaluator, next_unplanned_unit};
use crate::battle::{BattleState, FireIntent, Formation, FormationId, Mission, Order};
use crate::data::{DataRegistry, DoctrineDef};
use crate::map::{Objective, ObjectiveKind};
use crate::roster::CadetId;
use std::cmp::Reverse;
use std::collections::{HashMap, VecDeque};

use super::UtilityPlanner;

/// The two formations a plan pairs, fixing and manoeuvring, as the review
/// hands them to the round-robin to leave alone.
type Pairing = (FormationId, FormationId);

/// The posture an unordered crew under fire falls back on. Mod data first —
/// the base game ships a `drill` doctrine and a mod may retune what drilled
/// self-preservation looks like — with a built-in equivalent if a mod
/// removes it, because a missing id must degrade to sensible behaviour
/// rather than to standing in the open.
fn drill_doctrine(data: &DataRegistry) -> DoctrineDef {
    data.doctrine("drill")
        .cloned()
        .unwrap_or_else(|| DoctrineDef {
            id: "drill".into(),
            name: "Battle Drill".into(),
            description: String::new(),
            aggression: 0.0,
            cover_value: 2.0,
            elevation_value: 1.2,
            concentration: 0.8,
            scouting: 0.0,
            objective_value: 0.0,
            indirect_appetite: 1.0,
            withdraw_threshold: 0.5,
            initiative: 0.5,
            delegation: 0.5,
            // A crew taking cover under fire is choosing a hex she can reach
            // this round, so there is no road to price and nobody to be
            // beaten to it by. Zero says that rather than leaving a reader to
            // work out that the terms cannot bite.
            route_caution: 0.0,
            contest_aversion: 0.0,
            screening: 0.0,
            teaches: Vec::new(),
        })
}

pub struct SideCommand {
    config: AiConfig,
    seed: u64,
    /// The cadet in command of the side: the leader of its first-declared
    /// formation. Nothing reads her yet — this is the seam the design doc
    /// promises ("the brain is constructed for the side's commanding cadet
    /// from the start"), filled in when the state is first seen so her
    /// traits and command skill can steer the brain without a rework.
    #[allow(dead_code)]
    commander: Option<CadetId>,
    /// One executor per formation this side owns, keyed by the formation's
    /// index in [`crate::battle::CommandState`]. A map, but never iterated —
    /// units are routed through it by direct lookup, so its order can leak
    /// into nothing.
    executors: HashMap<usize, UtilityPlanner>,
    /// Plans units in no formation, under the side's own doctrine — which is
    /// exactly what the whole side was before formations existed.
    fallback: UtilityPlanner,
    /// Chooses the shot of the battle drill: the self-preservation reaction
    /// of an unordered unit under fire, when the commander is a human who has
    /// said nothing. *Where* she goes is [`crate::battle::drill_destination`],
    /// the same answer the engine's mid-round reflex takes; this evaluator,
    /// under the `drill` posture from mod data, only picks what she returns
    /// fire at from there — so "she moved without orders" is always survival
    /// and never campaigning.
    drill: Evaluator,
    /// The side's doctrine, resolved once at build so the brain can consult
    /// it for formations that declare none of their own.
    side_doctrine: Option<DoctrineDef>,
    /// Built lazily on the first order, because planners are constructed
    /// before the battle exists and the formations live on it.
    built: bool,
    /// Whether this side has a brain of its own. False for the human hybrid
    /// (see [`Self::executor_only`]), where the commander is a person and
    /// this object is only her staff.
    reviews_missions: bool,
    /// The round missions were last reviewed, so the brain speaks once per
    /// round rather than once per order.
    reviewed_round: Option<u32>,
    /// When the next scheduled review falls due — the commander's pulse,
    /// priced on her `command` by the rules' `review` block. `None` is
    /// "never reviewed yet", which is always due.
    next_review: Option<u32>,
    /// What she knew at her last review, for the interrupts that wake any
    /// commander early: who led each formation, which formations were
    /// already beaten, and which contacts the picture already carried.
    /// Planner-side memory, like `pending` — rebuilt with the planner, never
    /// saved.
    known_leaders: Vec<Option<crate::battle::UnitId>>,
    known_beaten: Vec<bool>,
    known_contacts: Vec<crate::battle::UnitId>,
    pending: VecDeque<Order>,
    /// Whether the batch of orders currently draining out of [`Self::pending`]
    /// came from the battle drill rather than from anything anybody said.
    ///
    /// Read through [`crate::ai::AiPlanner::last_was_drill`] so the caller can
    /// say so out loud. A crew moving with no visible order behind her is
    /// indistinguishable from a bug, and until this existed the one case that
    /// mattered most — a personal march broken off for cover — was the one
    /// case that went unannounced.
    last_drill: bool,
    /// Whether this commander is inside somebody's playout
    /// (`crate::ai::plan::playout`): she carries the plans she was handed out
    /// and makes none of her own, or every playout would play out playouts.
    playing_out: bool,
}

impl SideCommand {
    pub fn from_config(config: &AiConfig, seed: u64, data: &DataRegistry) -> Self {
        Self {
            config: config.clone(),
            seed,
            commander: None,
            executors: HashMap::new(),
            fallback: UtilityPlanner::from_config(config, seed ^ 0xC0FF_EE00, data),
            drill: Evaluator::new(drill_doctrine(data)),
            side_doctrine: None,
            built: false,
            reviews_missions: true,
            reviewed_round: None,
            next_review: None,
            known_leaders: Vec::new(),
            known_beaten: Vec::new(),
            known_contacts: Vec::new(),
            pending: VecDeque::new(),
            last_drill: false,
            playing_out: false,
        }
    }

    /// The commander a playout runs her side under: the same doctrine and
    /// the same executors, carrying out the plans on the board and making
    /// none — so a playout measures the plan it was handed, and never
    /// recurses into playouts of its own.
    pub fn for_playout(config: &AiConfig, seed: u64, data: &DataRegistry) -> Self {
        Self {
            playing_out: true,
            ..Self::from_config(config, seed, data)
        }
    }

    /// The same side, without a brain: per-formation executors and nothing
    /// else. This is the human hybrid the design doc describes — the player
    /// *is* the commander, so nothing here may issue a mission, and no
    /// [`Event::MissionAssigned`](crate::battle::Event::MissionAssigned) can
    /// come out of it.
    ///
    /// What it does instead is fill the gaps she left: a unit she did not
    /// order herself is planned by her formation's executor, under that
    /// formation's doctrine, in service of the mission she gave it. Units she
    /// *did* order are never touched, because the driver only ever asks about
    /// [`next_unplanned_unit`] and a hand-ordered unit is already planned.
    ///
    /// One rule is different from an AI side's, and deliberately so: **a unit
    /// in no formation, or in a formation under no mission, is not planned at
    /// all** — she is given a bare hold-fire, today's "planned, watching"
    /// default. An AI side would hand her to the fallback planner, which
    /// would send her off to fight on her own judgment; doing that for the
    /// human would be inventing a purpose she never gave. Delegation fills in
    /// the *how* of an order, never the *whether*. A mission still in the air
    /// counts as given ([`Formation::latest_mission`]): she has ordered it,
    /// and her subordinates plan on what they currently know, which for one
    /// transit window is still the last thing they heard.
    pub fn executor_only(config: &AiConfig, seed: u64, data: &DataRegistry) -> Self {
        Self {
            reviews_missions: false,
            ..Self::from_config(config, seed, data)
        }
    }

    /// Resolve the formations of `side` into executors, once. A formation
    /// declaring its own doctrine gets an evaluator that believes it; the
    /// rest inherit the side's.
    fn ensure_built(&mut self, registry: &DataRegistry, state: &BattleState, side: u8) {
        if self.built {
            return;
        }
        self.built = true;
        let side_doctrine = super::resolve_doctrine(&self.config, registry);
        self.side_doctrine = Some(side_doctrine.clone());
        for (index, formation) in state.command.formations().iter().enumerate() {
            if formation.side != side {
                continue;
            }
            let doctrine: DoctrineDef = formation
                .doctrine
                .as_deref()
                .and_then(|id| registry.doctrine(id).cloned())
                .unwrap_or_else(|| side_doctrine.clone());
            self.executors.insert(
                index,
                UtilityPlanner::new(
                    Evaluator::new(doctrine),
                    self.config.difficulty,
                    // Distinct stream per formation, or two platoons under
                    // the same noise level would blunder identically.
                    self.seed ^ ((index as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)),
                ),
            );
        }
        self.commander = state
            .command
            .formations()
            .iter()
            .find(|f| f.side == side)
            .and_then(|f| f.leader)
            .and_then(|id| state.unit(id))
            .and_then(|u| u.crew.first().copied());
    }

    /// Whether this member stands overwatch this round instead of moving:
    /// the executor half of bounding overwatch, with WEGO rounds as the
    /// bounds. A formation under a movement mission and in contact splits
    /// into two elements by member parity; the elements alternate by round,
    /// the bounding one advancing on the mission gradient while this one
    /// goes firm with guns up — hold-fire has been overwatch since the day
    /// it was named. Out of contact, everyone travels; a stand-fast has
    /// nothing to bound toward and a withdrawal values speed over ceremony
    /// (alternate bounds rearward is a real technique, and a refinement for
    /// later); a formation of one has nobody to cover her.
    fn overwatch_this_round(
        &self,
        state: &BattleState,
        unit: crate::battle::UnitId,
        index: usize,
    ) -> bool {
        let Some(formation) = state.command.formations().get(index) else {
            return false;
        };
        // Personal tasking excuses her from the formation's bounds too: the
        // commander put her somewhere, and standing overwatch for a bound
        // she is not part of would move her off it.
        if state.unit(unit).is_some_and(|u| u.detached()) {
            return false;
        }
        // An assault bounds too. Being ordered to press through fire is not
        // being ordered to do it badly: fire and movement is *how* an attack
        // crosses ground a defender is covering, and a cautious doctrine
        // told to assault does it by alternate bounds because that is the
        // textbook. The doctrines that genuinely trade security for tempo
        // are already excused a line below, by the aggression gate.
        if !matches!(
            formation.mission_for(unit),
            Some(Mission::Advance { .. })
                | Some(Mission::Recon { .. })
                | Some(Mission::Assault { .. })
        ) {
            return false;
        }
        // Traveling versus bounding is a doctrine's call, and the harness
        // agreed with the textbooks before this gate existed: universal
        // bounding cost massed armour half its wins (10 to 5 in 36),
        // because its identity is trading security for tempo. An
        // aggressive doctrine travels in overwatch — spread, guns ready,
        // still moving — and only the cautious ones pay the round for the
        // covered bound.
        if self.doctrine_for(index).aggression >= 0.7 {
            return false;
        }
        // Contact worth the ceremony: a spotted enemy near enough to the
        // formation to matter, not one across the map.
        let relevant = state
            .fog
            .side(formation.side)
            .spotted
            .iter()
            .filter_map(|id| state.unit(*id))
            .any(|enemy| {
                formation
                    .members
                    .iter()
                    .filter_map(|id| state.unit(*id))
                    .any(|member| member.pos.distance_to(enemy.pos) <= 15)
            });
        if !relevant {
            return false;
        }
        let living: Vec<crate::battle::UnitId> = formation
            .members
            .iter()
            .copied()
            .filter(|id| state.unit(*id).is_some())
            .collect();
        if living.len() < 2 {
            return false;
        }
        // Element by position among the living (id order), bounding element
        // by round parity — deterministic, and it swaps every round.
        let Some(position) = living.iter().position(|id| *id == unit) else {
            return false;
        };
        (position % 2) as u32 != (state.round.wrapping_add(index as u32)) % 2
    }

    /// Rounds between this side's reviews: the commander's pulse, priced on
    /// the side commander's `command`-family skill by the rules' `review`
    /// block. Zero — every round — wherever no rules exist or the block
    /// declares no cadence, which is the brain this game always had.
    fn review_interval(&self, registry: &DataRegistry, state: &BattleState, side: u8) -> u32 {
        let Some(rules) = registry.command.as_ref() else {
            return 0;
        };
        let level = state
            .command
            .formations()
            .iter()
            .find(|f| f.side == side)
            .and_then(|f| f.leader)
            .and_then(|id| state.unit(id))
            .map(|u| {
                state.roster.crew_skill(
                    registry,
                    registry.vehicle(&u.vehicle),
                    &u.crew,
                    &u.crew_state,
                    &rules.review.skill,
                    state.terrain_at(u.pos),
                )
            })
            .unwrap_or(crate::data::AVERAGE);
        rules.review.delay(level)
    }

    /// What no commander sleeps through, however slow her pulse: command
    /// passing in one of her formations, a formation newly beaten past its
    /// threshold, or a fresh contact on the picture she has not seen before.
    /// Checked against what she knew at her last review.
    fn interrupted(&self, registry: &DataRegistry, state: &BattleState, side: u8) -> bool {
        for (index, formation) in state.command.formations().iter().enumerate() {
            if formation.side != side {
                continue;
            }
            if self.known_leaders.get(index).copied().flatten() != formation.leader {
                return true;
            }
            let doctrine = self.doctrine_for(index);
            let beaten = Self::beaten(registry, state, formation, doctrine);
            if beaten && !self.known_beaten.get(index).copied().unwrap_or(false) {
                return true;
            }
        }
        state
            .picture(side)
            .iter()
            .any(|c| c.fresh && !self.known_contacts.contains(&c.unit))
    }

    /// Note what this review saw, so the next interrupt has something honest
    /// to compare against.
    fn remember(&mut self, registry: &DataRegistry, state: &BattleState, side: u8) {
        let formations = state.command.formations();
        self.known_leaders = formations.iter().map(|f| f.leader).collect();
        self.known_beaten = formations
            .iter()
            .enumerate()
            .map(|(index, f)| {
                f.side == side && Self::beaten(registry, state, f, self.doctrine_for(index))
            })
            .collect();
        self.known_contacts = state
            .picture(side)
            .iter()
            .filter(|c| c.fresh)
            .map(|c| c.unit)
            .collect();
    }

    /// The doctrine a formation fights under: its own, else the side's.
    fn doctrine_for(&self, index: usize) -> &DoctrineDef {
        self.executors
            .get(&index)
            .map(|e| &e.evaluator.doctrine)
            .or(self.side_doctrine.as_ref())
            .expect("built before any review")
    }

    /// What each formation should be doing, compared against what it is
    /// doing.
    ///
    /// Whether to keep fighting outranks where: a formation beaten past its
    /// doctrine's `withdraw_threshold` is ordered out by the nearest lane,
    /// and that order is never rescinded — this is where withdrawal stops
    /// being a symptom each vehicle shows and becomes a decision a commander
    /// makes, which is what the design doc means by willingness belonging to
    /// the commander.
    ///
    /// Ground worth holding is divided round-robin, most valuable first,
    /// among the formations in declaration order — the map author ordered
    /// both lists, so the pairing is theirs to control. Doctrine colours the
    /// mission: an aggressive one is sent to *take* the ground, a cautious
    /// one to stand on it, which is the difference between marching at a
    /// bridge and defending it. `initiative` is read here too: a commander
    /// with it moves her people off ground already taken toward ground that
    /// is not, while one without it follows the letter of the original plan.
    /// A map with no ground to hold gets no missions at all, which keeps a
    /// no-objective battle exactly the fight it was before commanders
    /// existed.
    fn mission_review(&self, registry: &DataRegistry, state: &BattleState, side: u8) -> Vec<Order> {
        let mut ground: Vec<(&Objective, Option<u8>)> = state
            .objectives()
            .filter(|(o, _)| o.kind == ObjectiveKind::Hold)
            .collect();
        // Stable, so equal values keep declaration order.
        ground.sort_by_key(|(o, _)| Reverse(o.value));

        // Plans first: a formation her commander has given a part in a plan
        // is spoken for, and the round-robin below is for everybody else.
        let (mut orders, planned) = self.review_plans(registry, state, side);
        let mut next = 0usize;
        for (index, formation) in state.command.formations().iter().enumerate() {
            if formation.side != side {
                continue;
            }
            // An ordered withdrawal stands whatever happens next. Strength
            // is measured over the roster the formation went in with, so it
            // cannot "recover" when its weakest vehicle dies — but the rule
            // is stated as well as computed, because a retreat that
            // un-happens is the kind of flicker that makes an AI look
            // broken.
            // What she has *said*, not what has arrived: an order still
            // travelling is one she has already given, and a commander who
            // forgot that would re-send it every round of the transit window
            // — burying the log and resetting the clock each time, so a
            // delayed mission would never land at all.
            let ordered = formation.latest_mission();
            if matches!(ordered, Some(Mission::Withdraw { .. })) {
                continue;
            }
            if planned.contains(&index) {
                continue;
            }
            let doctrine = self.doctrine_for(index);
            if let Some(via) = self.wants_out(registry, state, formation, doctrine) {
                orders.push(Order::SetMission {
                    formation: FormationId(index as u32),
                    mission: Mission::Withdraw { via },
                    latitude: crate::battle::Latitude::Delegated,
                });
                continue;
            }

            // A commander who devolves does not micro-assign ground at all:
            // her formations keep their native judgment — the whole-map
            // objective weighing that is exactly what a defensive doctrine
            // is good at — and hear from her only when it is time to leave.
            // This is not a tuning dodge, it is what the delegation knob
            // *means*, and it was measured before it was believed: pinning
            // elastic defence to anchor hexes cost it 16 of 24 wins against
            // a flat opponent, because choosing its own ground is its game.
            if doctrine.delegation >= registry.planner.devolved {
                continue;
            }
            if ground.is_empty() {
                continue;
            }
            // Base of fire before ground. A formation with indirect guns in
            // it is not one you send to stand on a bridge: round-robining
            // the howitzer into a ground slot marched it into the assault it
            // should have been shooting for. So it is taken out of the
            // rotation entirely — the ground is divided among the rest, and
            // the guns are told whose fight to shoot into.
            //
            // Deliberately below the `ground.is_empty()` guard: a map with
            // nothing to hold still gets no missions at all, which is the
            // property that keeps a no-objective battle exactly the fight it
            // was before commanders existed. A base of fire on an empty map
            // would be the first order ever issued there.
            if let Some(supported) = Self::base_of_fire(registry, state, side, index) {
                let desired = Mission::Support {
                    formation: supported,
                };
                if ordered != Some(&desired) {
                    orders.push(Order::SetMission {
                        formation: FormationId(index as u32),
                        mission: desired,
                        // The brain never insists. Every order it issues is delegated,
                        // which is what keeps the determinism baseline valid across
                        // this change: AI-vs-AI play is bit-for-bit the old game.
                        latitude: crate::battle::Latitude::Delegated,
                    });
                }
                continue;
            }
            // Infantry next, and for the same reason as the guns: a
            // commander who cannot tell what a formation is *made of* will
            // send it to do somebody else's job. Round-robining the
            // grenadiers into a ground slot ordered a rifle platoon and its
            // taxi to march at a crossroads beside three tanks, which is how
            // a harness run loses 22 of 24 APCs and buries most of the
            // infantry in them.
            //
            // What foot troops are for is the ground itself. Their strength
            // is exactly the thing an advance throws away: concealment that
            // doubles in cover, an ambush tube that reaches three hexes, and
            // an unwillingness to be shifted once they are in the timber. So
            // a formation with anybody on foot in it is given the *covered*
            // ground and told to hold it — not because the brain knows what
            // a rifle platoon is, but because it can read that these people
            // walk, and walking troops in the open are a target while
            // walking troops in cover are a problem.
            //
            // Below the base-of-fire branch on purpose: a mortar section is
            // still a base of fire even though its crews are on their feet,
            // and shooting for the main effort is the more specific job.
            // Eyes go and look. The chassis decides who could — a vehicle
            // that sees `planner.eyes_ratio_percent` further than her longest
            // direct weapon reaches is a scout, which is a fact about the
            // hardware and not about the name on the formation — and the
            // doctrine's `screening` decides whether this commander would.
            //
            // Above the foot branch on purpose, and it is the one place that
            // argument is overruled: a section that sees three times as far
            // as she shoots is a screen first and infantry second, and
            // holding her in the nearest wood is exactly the use of her that
            // wastes what she is for. A rifle platoon sits at twice her
            // rifles and is *not* caught by this, which is the separation the
            // shipped roster happens to give and the reason the threshold is
            // data.
            //
            // Until this branch existed no doctrine could issue a `Recon` at
            // all: the chooser had `Assault`, `Advance`, `Hold`, a base of
            // fire and a withdrawal, and the executor's `Recon` arm,
            // `planner.pull_under_fire`'s gate on it and
            // `Formation::latitude_for`'s handling of it were code for a
            // mission nobody produced.
            let eyes = Self::eyes_share(registry, state, formation);
            if doctrine.screening > 0.0 && eyes > 1.0 - doctrine.screening {
                let desired = Mission::Recon {
                    toward: ground[next % ground.len()].0.anchor(),
                };
                next += 1;
                if ordered != Some(&desired) {
                    orders.push(Order::SetMission {
                        formation: FormationId(index as u32),
                        mission: desired,
                        latitude: crate::battle::Latitude::Delegated,
                    });
                }
                continue;
            }
            if Self::goes_on_foot(registry, state, formation) {
                let covered = Self::best_cover(registry, state, &ground);
                let desired = Mission::Hold {
                    at: Some(ground[covered].0.anchor()),
                };
                if ordered != Some(&desired) {
                    orders.push(Order::SetMission {
                        formation: FormationId(index as u32),
                        mission: desired,
                        // The brain never insists. Every order it issues is delegated,
                        // which is what keeps the determinism baseline valid across
                        // this change: AI-vs-AI play is bit-for-bit the old game.
                        latitude: crate::battle::Latitude::Delegated,
                    });
                }
                continue;
            }
            let mut pick = next % ground.len();
            next += 1;
            if doctrine.initiative >= 0.5
                && ground[pick].1 == Some(side)
                && let Some(open) = ground.iter().position(|(_, held)| *held != Some(side))
            {
                pick = open;
            }
            let anchor = ground[pick].0.anchor();
            // Three postures, not two, and the middle one is the change the
            // playtest asked for. A doctrine that will spend vehicles for
            // ground orders the deliberate attack — massed armour at 0.85
            // presses through what it meets, which is what it was already
            // doing and is now honestly named. The ordinary aggressive
            // doctrine, including the balanced default the player's own
            // delegation runs under, orders a movement to contact: take the
            // bridge, but fight what you meet on the way rather than driving
            // a parade route into a gun line. A cautious one still stands on
            // the ground instead of marching at it.
            let desired = if doctrine.aggression >= 0.7 {
                Mission::Assault { to: anchor }
            } else if doctrine.aggression >= 0.5 {
                Mission::Advance { to: anchor }
            } else {
                Mission::Hold { at: Some(anchor) }
            };
            // Standing orders stand. Re-issuing an identical mission every
            // round would bury the log in news that nothing changed.
            if ordered != Some(&desired) {
                orders.push(Order::SetMission {
                    formation: FormationId(index as u32),
                    mission: desired,
                    latitude: crate::battle::Latitude::Delegated,
                });
            }
        }
        orders
    }

    /// Review every operational command's plan, and return the orders that
    /// follow and the formations those plans speak for.
    ///
    /// For each command ([`BattleState::operational_commands`]), in the order
    /// the engine lists them: the plan in flight is kept if it still stands
    /// and nothing beats its re-score by the commander's commitment; replaced
    /// if something does; dropped if it no longer stands and nothing does; and
    /// a commander with no plan adopts the best candidate she can see, if any
    /// clears its template's threshold. A plan whose commander no longer heads
    /// a command — she fell, or her group was absorbed by a senior's — is
    /// dropped, and the senior plans for everybody she now commands.
    fn review_plans(
        &self,
        registry: &DataRegistry,
        state: &BattleState,
        side: u8,
    ) -> (Vec<Order>, Vec<usize>) {
        use super::plan;
        let mut orders = Vec::new();
        let mut planned = Vec::new();
        let Some(academy) = self.side_doctrine.as_ref() else {
            return (orders, planned);
        };
        let commands = state.operational_commands(registry, side);
        for stale in state.plans(side) {
            if !commands.iter().any(|c| c.commander == stale.commander) {
                orders.push(Order::SetPlan {
                    side,
                    commander: stale.commander,
                    plan: None,
                });
            }
        }
        // The ground a plan should serve: the most valuable scoring ground
        // this side does not hold, as the round-robin would hand it out.
        let goal = state
            .objectives()
            .filter(|(o, held)| o.kind == ObjectiveKind::Hold && *held != Some(side))
            .max_by_key(|(o, _)| (o.value, Reverse(o.id.clone())))
            .map(|(o, _)| o.anchor());
        let known_now: Vec<crate::battle::UnitId> = state
            .known_enemies(registry, crate::battle::Knower::Commander(side))
            .iter()
            .map(|e| e.id)
            .collect();
        for command in &commands {
            // Two roles, two lists. Going round is for a formation that can
            // drive; holding a firing position is for anybody who can shoot
            // from one, a platoon included — infantry is the classic base of
            // fire, and without it a combined-arms force never had two groups
            // to plan with.
            let eligible: Vec<usize> = command
                .formations
                .iter()
                .copied()
                .filter(|i| self.can_manoeuvre(registry, state, side, *i))
                .collect();
            let fixers: Vec<usize> = command
                .formations
                .iter()
                .copied()
                .filter(|i| self.can_fix(registry, state, side, *i))
                .collect();
            let strength = plan::Strength::of(registry, state, command.commander);
            let mut current = state.plan_of(command.commander).cloned();
            // A plan that has run its course ends here, and her formations go
            // back to the ground they were given — the round-robin below.
            if current.as_ref().is_some_and(|p| plan::done(state, p)) {
                orders.push(Order::SetPlan {
                    side,
                    commander: command.commander,
                    plan: None,
                });
                current = None;
            }
            let standing = current.as_ref().and_then(|p| {
                let stands = fixers.contains(&p.fix.index())
                    && eligible.contains(&p.manoeuvre.index())
                    && known_now.contains(&p.target);
                if !stands {
                    return None;
                }
                let params = plan::as_fix_and_flank(registry.template(&p.template)?)?;
                plan::rescore(registry, state, p, params)
            });
            // Nominations: the cheap model's best few from every play she
            // knows, as many as her breadth allows. Inside a playout she
            // nominates nothing — she carries out what she was handed.
            let mut nominated: Vec<plan::Candidate> = Vec::new();
            if !self.playing_out
                && !eligible.is_empty()
                && fixers.iter().any(|f| eligible.iter().any(|m| m != f))
                && let Some(target) = plan::target(registry, state, side, command, goal)
            {
                let mut rng = plan::judgement_rng(self.seed, state.round, command.commander);
                for template in plan::repertoire(registry, state, academy, command.commander) {
                    let Some(params) = plan::as_fix_and_flank(template) else {
                        continue;
                    };
                    nominated.extend(plan::candidates(
                        registry,
                        state,
                        side,
                        command.commander,
                        template,
                        params,
                        &fixers,
                        &eligible,
                        target,
                        strength,
                        &mut rng,
                    ));
                }
                nominated.sort_by(|a, b| b.score.total_cmp(&a.score));
                nominated.truncate(strength.breadth);
            }
            let same = |p: &crate::battle::Plan, c: &plan::Candidate| {
                p.template == c.plan.template
                    && p.fix == c.plan.fix
                    && p.manoeuvre == c.plan.manoeuvre
                    && p.fire_position == c.plan.fire_position
                    && p.flank == c.plan.flank
            };
            // Once the word is given the attack goes in: a plan in its going
            // phase is carried through while it still stands, never swapped
            // for a better-looking one. The first measurement caught the
            // commander giving the word and adopting a new plan in the same
            // review, whose orders recalled the assault a moment after it
            // was ordered — the dithering commitment exists to prevent.
            let committed = current
                .as_ref()
                .is_some_and(|p| p.phase == crate::battle::PlanPhase::Going);
            let drop = |orders: &mut Vec<Order>| {
                orders.push(Order::SetPlan {
                    side,
                    commander: command.commander,
                    plan: None,
                })
            };
            let chosen = if self.playing_out || (committed && standing.is_some()) {
                // Carry out what she has; drop only what no longer stands.
                match (&current, standing) {
                    (Some(p), Some(_)) => Some((p.fix, p.manoeuvre)),
                    (Some(_), None) => {
                        drop(&mut orders);
                        None
                    }
                    (None, _) => None,
                }
            } else if registry.planner.playout_samples > 0
                && (!nominated.is_empty() || current.is_some())
            {
                // **The battle is the evaluator.** Play out the plan in hand
                // (or none), every nomination, and — if she has a plan —
                // dropping it, and keep the best, switching only past her
                // margin. Every playout is misread by her skill gap.
                let mut rng =
                    plan::judgement_rng(self.seed ^ 0x5EED, state.round, command.commander);
                let mut options: Vec<(Vec<Order>, Option<Pairing>)> = Vec::new();
                match (&current, standing) {
                    (Some(p), Some(_)) => options.push((Vec::new(), Some((p.fix, p.manoeuvre)))),
                    (Some(_), None) => {
                        let mut o = Vec::new();
                        drop(&mut o);
                        options.push((o, None));
                    }
                    (None, _) => options.push((Vec::new(), None)),
                }
                for c in &nominated {
                    if current.as_ref().is_some_and(|p| same(p, c)) {
                        continue;
                    }
                    options.push((plan::adopt(c), Some((c.plan.fix, c.plan.manoeuvre))));
                }
                if current.is_some() && standing.is_some() {
                    let mut o = Vec::new();
                    drop(&mut o);
                    options.push((o, None));
                }
                let values: Vec<f32> = options
                    .iter()
                    .enumerate()
                    .map(|(i, (orders, _))| {
                        let read = plan::playout(
                            registry,
                            state,
                            side,
                            orders,
                            &self.config,
                            self.seed ^ state.round as u64 ^ ((i as u64 + 1) << 32),
                        );
                        let misread = if strength.playout_noise > 0.0 {
                            rand::RngExt::random_range(
                                &mut rng,
                                -strength.playout_noise..=strength.playout_noise,
                            )
                        } else {
                            0.0
                        };
                        read + misread
                    })
                    .collect();
                let mut pick = 0;
                for (i, v) in values.iter().enumerate().skip(1) {
                    if *v > values[pick] {
                        pick = i;
                    }
                }
                if pick != 0 && values[pick] <= values[0] + strength.playout_margin {
                    pick = 0;
                }
                let (picked, formations) = options.swap_remove(pick);
                orders.extend(picked);
                formations
            } else {
                // No playouts: the cheap model decides, as it did before
                // there were any.
                let best = nominated.into_iter().next().filter(|c| {
                    registry
                        .template(&c.plan.template)
                        .and_then(plan::as_fix_and_flank)
                        .is_some_and(|p| c.score >= p.threshold)
                });
                match (&current, standing, best) {
                    (Some(p), Some(score), Some(c))
                        if !committed && !same(p, &c) && c.score > score + strength.commitment =>
                    {
                        orders.extend(plan::adopt(&c));
                        Some((c.plan.fix, c.plan.manoeuvre))
                    }
                    (Some(p), Some(_), _) => Some((p.fix, p.manoeuvre)),
                    (_, _, Some(c)) => {
                        orders.extend(plan::adopt(&c));
                        Some((c.plan.fix, c.plan.manoeuvre))
                    }
                    (Some(_), None, None) => {
                        drop(&mut orders);
                        None
                    }
                    (None, _, None) => None,
                }
            };
            if let Some((f, m)) = chosen {
                planned.extend([f.index(), m.index()]);
            }
        }
        (orders, planned)
    }

    /// Whether a formation can take a part in a plan: on this side, led, not
    /// beaten or leaving, and not somebody the review would give another job
    /// first — a base of fire, a screen, infantry holding cover.
    /// Whether a formation can be a plan's fixing element: everything
    /// [`Self::can_manoeuvre`] asks except that she be able to drive, because
    /// a base of fire holds a position rather than going round. A platoon
    /// qualifies; a battery and a screen do not.
    fn can_fix(
        &self,
        registry: &DataRegistry,
        state: &BattleState,
        side: u8,
        index: usize,
    ) -> bool {
        self.plan_role_open(registry, state, side, index)
    }

    fn can_manoeuvre(
        &self,
        registry: &DataRegistry,
        state: &BattleState,
        side: u8,
        index: usize,
    ) -> bool {
        self.plan_role_open(registry, state, side, index)
            && state
                .command
                .formations()
                .get(index)
                .is_some_and(|f| !Self::goes_on_foot(registry, state, f))
    }

    /// What both plan roles ask of a formation: hers, led, not leaving, not
    /// a battery, and not a screen her doctrine sends looking.
    fn plan_role_open(
        &self,
        registry: &DataRegistry,
        state: &BattleState,
        side: u8,
        index: usize,
    ) -> bool {
        let Some(formation) = state.command.formations().get(index) else {
            return false;
        };
        if formation.side != side
            || formation.leader.and_then(|id| state.unit(id)).is_none()
            || matches!(formation.latest_mission(), Some(Mission::Withdraw { .. }))
        {
            return false;
        }
        let doctrine = self.doctrine_for(index);
        if self
            .wants_out(registry, state, formation, doctrine)
            .is_some()
            || Self::lays_indirect(registry, state, formation)
        {
            return false;
        }
        let eyes = Self::eyes_share(registry, state, formation);
        !(doctrine.screening > 0.0 && eyes > 1.0 - doctrine.screening)
    }

    /// Who this formation should be shooting for, if it is the side's base
    /// of fire at all.
    ///
    /// **Fires formations are recognised, not declared.** A formation is one
    /// if anybody still alive in it lays an indirect weapon — that is what
    /// makes her a base of fire, and reading it off the hardware means a mod
    /// that adds a mortar section gets this behaviour with no new field and
    /// no engine change. It also means a battery that has lost its last
    /// howitzer stops being one and rejoins the ground rotation at the next
    /// review, which is the honest answer.
    ///
    /// She supports the side's **largest other formation** — most members
    /// still on the field, ties to the first declared — because the base of
    /// fire covers the main effort, and the biggest formation is the closest
    /// thing this brain has to one until control measures give it a real
    /// one. `None` when there is nobody else to shoot for: a lone formation
    /// takes ground like anybody else rather than standing off from nothing.
    fn base_of_fire(
        registry: &DataRegistry,
        state: &BattleState,
        side: u8,
        index: usize,
    ) -> Option<String> {
        let formation = state.command.formations().get(index)?;
        if !Self::lays_indirect(registry, state, formation) {
            return None;
        }
        let living = |f: &Formation| {
            f.members
                .iter()
                .filter(|id| state.unit(**id).is_some())
                .count()
        };
        state
            .command
            .formations()
            .iter()
            .enumerate()
            .filter(|(other, f)| f.side == side && *other != index)
            // `Reverse` on the index so an equal-sized tie goes to the
            // first-declared formation rather than to whichever `max_by_key`
            // happened to visit last.
            .max_by_key(|(other, f)| (living(f), Reverse(*other)))
            .map(|(_, f)| f.id.clone())
    }

    /// Whether anybody still with this formation fights on their feet.
    ///
    /// Recognised, not declared — the same rule `lays_indirect` follows, and
    /// for the same reasons. A formation is an infantry element because it
    /// has infantry in it, read straight off the chassis' movement class, so
    /// a mod that adds a paratroop platoon or a pioneer section gets the
    /// commander's infantry judgment on the day it is written, with no new
    /// field to fill in and nothing in Rust naming a vehicle.
    ///
    /// Passengers count. A platoon in the back of her taxi is the reason the
    /// taxi is anywhere, and a carrier whose element stops being infantry the
    /// moment the doors shut would be ordered to drive at a crossroads with
    /// the tanks — which is precisely the behaviour this exists to end.
    /// What share of a formation's living vehicles are *eyes*: they see
    /// further than they shoot by [`crate::data::PlannerRules::eyes_ratio_percent`].
    ///
    /// Read off the hardware, never off a name, the way
    /// [`Self::lays_indirect`] and [`Self::goes_on_foot`] are. Indirect
    /// weapons are not counted: a howitzer reaching forty hexes says nothing
    /// about whether the crew firing it can see, and counting it would make
    /// every artillery battery the least scout-like thing on the field for
    /// the wrong reason.
    fn eyes_share(registry: &DataRegistry, state: &BattleState, formation: &Formation) -> f32 {
        let ratio = registry.planner.eyes_ratio_percent;
        if ratio <= 0 {
            return 0.0;
        }
        let mut seen = 0;
        let mut eyes = 0;
        for unit in formation.members.iter().filter_map(|id| state.unit(*id)) {
            let Some(vehicle) = registry.vehicle(&unit.vehicle) else {
                continue;
            };
            seen += 1;
            let reach = vehicle
                .weapons
                .iter()
                .filter_map(|w| registry.weapon(w))
                .filter(|w| !w.indirect)
                .map(|w| w.range[1])
                .max()
                .unwrap_or(0);
            // A vehicle with no direct weapon at all is nothing but eyes.
            if reach == 0 || vehicle.vision_range * 100 > reach * ratio as u32 {
                eyes += 1;
            }
        }
        if seen == 0 {
            return 0.0;
        }
        eyes as f32 / seen as f32
    }

    fn goes_on_foot(registry: &DataRegistry, state: &BattleState, formation: &Formation) -> bool {
        formation
            .members
            .iter()
            .filter_map(|id| state.unit(*id))
            .any(|unit| {
                registry
                    .vehicle(&unit.vehicle)
                    .is_some_and(|v| v.movement.class == crate::data::MovementClass::Foot)
            })
    }

    /// Which of the ground on offer is the best country to be infantry in:
    /// the objective whose hexes carry the most cover, ties to the earlier
    /// entry — which, since `ground` is sorted by value, means the more
    /// valuable of two equally wooded places.
    ///
    /// Cover rather than value because the choice is about employment, not
    /// about worth: the crossroads may be the prize, but a platoon holding a
    /// treeline is holding something, and a platoon standing on open tarmac
    /// is a casualty list. The armour is still sent for the prize — this
    /// picks from the same list without removing anything from it, so two
    /// formations converging on one objective is allowed and is usually the
    /// right answer.
    fn best_cover(
        registry: &DataRegistry,
        state: &BattleState,
        ground: &[(&Objective, Option<u8>)],
    ) -> usize {
        let cover = |objective: &Objective| -> i32 {
            let hexes: Vec<i32> = objective
                .hexes
                .iter()
                .filter_map(|h| state.map.get(*h))
                .filter_map(|t| registry.terrain(t.terrain))
                .map(|def| def.cover)
                .collect();
            if hexes.is_empty() {
                return 0;
            }
            hexes.iter().sum::<i32>() / hexes.len() as i32
        };
        ground
            .iter()
            .enumerate()
            // `Reverse` on the index so an equal-cover tie goes to the
            // earlier — and therefore more valuable — objective rather than
            // to whichever `max_by_key` happened to visit last.
            .max_by_key(|(index, (objective, _))| (cover(objective), Reverse(*index)))
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    /// Whether anybody still on the field in this formation carries a weapon
    /// that shoots over things.
    fn lays_indirect(registry: &DataRegistry, state: &BattleState, formation: &Formation) -> bool {
        formation
            .members
            .iter()
            .filter_map(|id| state.unit(*id))
            .any(|unit| {
                registry.vehicle(&unit.vehicle).is_some_and(|v| {
                    v.weapons
                        .iter()
                        .any(|w| registry.weapon(w).is_some_and(|w| w.indirect))
                })
            })
    }

    /// Whether this formation is beaten badly enough that its commander
    /// orders it out, and by which lane. `None` while it fights on — which
    /// includes having nowhere to go: a side with no exit of its own holds,
    /// because driving for a lane that does not exist is not a retreat.
    /// Whether a formation has lost more than its doctrine will bear.
    /// Strength is measured over the roster it went in with — the dead and
    /// the exited still count against the whole — so it is monotonic and a
    /// retreat cannot un-happen when the weakest vehicle stops dragging the
    /// average down. `withdraw_threshold` is the fraction of strength *lost*
    /// before a doctrine looks for the way out, so the stubborn 0.85 fights
    /// to a remnant and the elastic 0.45 leaves with something to rebuild.
    fn beaten(
        registry: &DataRegistry,
        state: &BattleState,
        formation: &Formation,
        doctrine: &DoctrineDef,
    ) -> bool {
        // Substance — cadets and module hits still aboard, over the full
        // complement — replaces the hit-point fraction. A dead vehicle
        // still counts her full complement in the denominator, so losses
        // pull the formation toward beaten exactly as they always did.
        let (mut have, mut max) = (0i64, 0i64);
        for id in &formation.members {
            let Some(unit) = state.units.get(id.index()) else {
                continue;
            };
            let (h, t) = state.substance(registry, unit);
            if unit.alive() {
                have += i64::from(h);
            }
            max += i64::from(t);
        }
        let strength = have as f32 / max.max(1) as f32;
        strength < (1.0 - doctrine.withdraw_threshold).clamp(0.0, 1.0)
    }

    fn wants_out(
        &self,
        registry: &DataRegistry,
        state: &BattleState,
        formation: &Formation,
        doctrine: &DoctrineDef,
    ) -> Option<String> {
        if !Self::beaten(registry, state, formation, doctrine) {
            return None;
        }
        let from = formation
            .leader
            .and_then(|id| state.unit(id))
            .or_else(|| formation.members.iter().find_map(|id| state.unit(*id)))
            .map(|u| u.pos)?;
        // The lane itself is not this brain's rule to have: the player and the
        // campaign choose a withdrawal route the same way, so they choose it in
        // the same function.
        crate::battle::nearest_exit(state, formation.side, from)
    }
}

impl AiPlanner<BattleState, Order> for SideCommand {
    fn last_was_drill(&self) -> bool {
        self.last_drill
    }

    fn next_order(&mut self, registry: &DataRegistry, state: &BattleState, side: u8) -> Order {
        self.ensure_built(registry, state, side);

        // The commander speaks first, once per round: missions before unit
        // orders, so the executors already know what the ground is worth by
        // the time they plan the first vehicle. A brainless side skips this
        // entirely — somebody else has already spoken, or nobody has.
        if self.reviews_missions && self.reviewed_round != Some(state.round) {
            // Once per round the commander considers whether to think at
            // all: on her pulse she reviews; between pulses only an
            // interrupt — command passing, a formation breaking, a fresh
            // contact — wakes her, and the plan otherwise stands, which is
            // what plans are for. Marking the round either way keeps this
            // from re-running per order.
            self.reviewed_round = Some(state.round);
            // A plan's trigger is watched every round, pulse or no pulse: the
            // word to go is the one order that cannot wait for her next
            // review, because the moment it answers — the fix in place, the
            // flankers found — does not.
            let words: Vec<Order> = state
                .plans(side)
                .flat_map(|plan| super::plan::trigger(registry, state, plan))
                .collect();
            self.pending.extend(words);
            let due = self.next_review.is_none_or(|at| state.round >= at);
            if due || self.interrupted(registry, state, side) {
                let interval = self.review_interval(registry, state, side);
                self.next_review = Some(state.round + 1 + interval);
                self.last_drill = false;
                self.pending
                    .extend(self.mission_review(registry, state, side));
                self.remember(registry, state, side);
            }
        }

        while let Some(order) = self.pending.pop_front() {
            // A unit destroyed since its orders were queued has nothing to
            // say; a mission cannot go stale the same way.
            let unit = match &order {
                Order::SetMove { unit, .. } | Order::SetFire { unit, .. } => Some(*unit),
                _ => None,
            };
            if unit.is_none_or(|u| state.unit(u).is_some()) {
                return order;
            }
        }

        let Some(unit) = next_unplanned_unit(state, side) else {
            return Order::Commit { side };
        };
        let formation = state
            .command
            .formations()
            .iter()
            .position(|f| f.contains(unit));
        // Without a brain, an unmissioned unit is one nobody has decided
        // about, and deciding for her is not this object's job — with one
        // exception every army since 1918 has drilled: nobody under fire
        // waits for permission to survive. Threatened, she executes the
        // battle drill (return fire, seek cover, want nothing else on the
        // map); safe, she watches her arc exactly as before, so the
        // parking lot stays parked. Any explicit order — including the
        // deliberate "hold and watch" — outranks the drill, because it
        // marks her planned before this is ever consulted.
        let personal = state.unit(unit).is_some_and(|u| u.detached());
        if !self.reviews_missions
            && (personal
                || !formation
                    .and_then(|index| state.command.formations().get(index))
                    .is_some_and(|f| f.latest_mission().is_some()))
        {
            // Survival first, *unless she was told otherwise*: the drill
            // outranks an ordinary personal order, because nobody drives a
            // parade route through effective fire to keep an appointment —
            // but a commander who said "press on" has already answered that
            // objection, and overriding her anyway is what made an order
            // feel like a suggestion. See [`Latitude`]: this is the
            // per-unit twin of the `Advance`/`Assault` distinction.
            //
            // Asked through `Unit::yields_to_drill`, the one gate, which the
            // engine's mid-round reflex asks too; a crew holding ground she
            // was sent to under a binding order holds it, because the
            // insistence arrived with her.
            //
            // Where she goes is the engine's own drill, asked here five
            // seconds sooner: the reachable ground where the guns bearing on
            // her can do least. This used to be the whole evaluator under a
            // `drill` doctrine — a second model of the same reaction, which
            // could send her to one hex at the planning table and have the
            // reflex move her to another a tick after she arrived. What the
            // evaluator still decides is her shot from wherever she ends up.
            //
            // Every gun she knows of, not only the ones she has caught up
            // with on her reaction clock: the planning table is a pause, and
            // nothing else planned at it — the executors, the evaluator's
            // threat term — reads the clock either. The clock is for
            // reacting while the round runs, which is the reflex's half.
            let yields = state.unit(unit).is_none_or(|u| u.yields_to_drill());
            let noticed = if yields {
                crate::battle::threats(registry, state, unit)
            } else {
                Vec::new()
            };
            if !noticed.is_empty() {
                self.last_drill = true;
                let pos = state.unit(unit).map(|u| u.pos).unwrap_or_default();
                let dest = crate::battle::drill_destination(registry, state, unit, &noticed);
                let fire = match self
                    .drill
                    .score_tile(registry, state, unit, dest.unwrap_or(pos))
                    .attack
                {
                    Some((target, weapon)) => FireIntent::Target { target, weapon },
                    None => FireIntent::Hold,
                };
                if let Some(to) = dest {
                    self.pending.push_back(Order::SetFire { unit, fire });
                    return Order::SetMove { unit, to };
                }
                return Order::SetFire { unit, fire };
            }
            // A standing personal destination marches on: one round's worth
            // of ground toward it, the same leg the engine walked on the
            // round it was given.
            if let Some((tasking, pos)) =
                state.unit(unit).and_then(|u| Some((u.march()?.to, u.pos)))
                && tasking != pos
            {
                let step = crate::battle::reachable(registry, state, unit)
                    .into_keys()
                    .min_by_key(|h| (tasking.distance_to(*h), h.x, h.y));
                if let Some(step) = step
                    && step != pos
                {
                    return Order::SetMove { unit, to: step };
                }
            }
            return Order::SetFire {
                unit,
                fire: FireIntent::Hold,
            };
        }
        // Bounding overwatch: her element stands firm this round while the
        // other advances. Before the executor, because standing firm IS her
        // plan — guns up on her arc, covering the bound.
        if let Some(index) = formation
            && self.overwatch_this_round(state, unit, index)
        {
            return Order::SetFire {
                unit,
                fire: FireIntent::Hold,
            };
        }
        let executor = formation
            .and_then(|index| self.executors.get_mut(&index))
            .unwrap_or(&mut self.fallback);
        self.last_drill = false;
        self.pending = executor.plan_unit(registry, state, unit).into();
        match self.pending.pop_front() {
            Some(order) => order,
            // Nothing to say about this unit: hold fire so it counts as
            // planned and the round can proceed.
            None => Order::SetFire {
                unit,
                fire: FireIntent::Hold,
            },
        }
    }
}
