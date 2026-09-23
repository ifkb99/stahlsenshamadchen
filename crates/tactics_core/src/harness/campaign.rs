//! A whole campaign, played out with nobody watching.
//!
//! Every other instrument in this harness fights *battles*. Permadeath, the
//! `hold_days` ending, the headquarters planner, withdrawal a hex back, the
//! muster — the rules that make a campaign one — were measured by nothing,
//! because the path from a clash on the map to a battle and back ran through
//! the game crate's screens. It runs through [`crate::field`] now, and this
//! walks it: the campaign's own planners take every side's turns, and each
//! [`OverworldEvent::BattleTriggered`] is staged, fought by the battle AI and
//! folded back exactly as the campaign screen does it.
//!
//! **What differs from a played campaign, on purpose:** every side is a
//! machine. A side the map leaves to the player (`ai: null`) gets
//! [`CampaignOptions::stand_in`] for its battles and the stock campaign
//! planner on the map, which is what `STAHL_AUTOPLAY` gives it too. Nobody
//! calls up a hurt cadet — the muster's choice is the player's, and the AI
//! has never been given one. Joiners are [`ai_reinforcements`] for both
//! sides, which is the AI's answer and not necessarily the player's.

use crate::ai::{AiConfig, AiDriver, make_battle_planner};
use crate::data::DataRegistry;
use crate::field::{Clash, battlefield_for};
use crate::overworld::{
    CampaignEnd, OverworldEvent, OverworldSetupError, OverworldState, ai_reinforcements,
    make_overworld_planner, step_planner,
};
use crate::roster::CrewFate;

/// How a headless campaign is played.
#[derive(Debug, Clone)]
pub struct CampaignOptions {
    /// The battle brain for a side the map leaves to a human.
    pub stand_in: AiConfig,
    /// Days after which the run is called off undecided. A campaign that
    /// never ends is a finding, not a hang.
    pub max_days: u32,
    /// Rounds after which a battle is abandoned where it stands. The
    /// stalemate clock ends every battle long before this in practice;
    /// reaching it is reported per battle so it cannot pass silently.
    pub max_rounds: u32,
}

impl Default for CampaignOptions {
    fn default() -> Self {
        Self {
            stand_in: AiConfig {
                planner: "utility".into(),
                difficulty: 3,
                doctrine: None,
            },
            max_days: 60,
            max_rounds: 100,
        }
    }
}

/// One clash the campaign caused.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldResult {
    pub day: u32,
    pub map_id: String,
    pub attacker_side: u8,
    /// `None` for a level score, whether stalemate or mutual ruin.
    pub winner: Option<u8>,
    pub rounds: u32,
    /// The battle hit [`CampaignOptions::max_rounds`] and was cut off.
    pub cut_off: bool,
    /// Hulls each side lost in it, indexed by side.
    pub hulls_lost: Vec<u32>,
    /// How many armies left the field by an exit.
    pub withdrew: u32,
}

/// Everything a headless campaign did.
#[derive(Debug, Clone, PartialEq)]
pub struct CampaignRun {
    /// The day the run stopped on.
    pub days: u32,
    /// Who won and by which rule, or `None` if [`CampaignOptions::max_days`]
    /// ran out first. `Some((None, _))` is everybody losing at once.
    pub end: Option<(Option<u8>, CampaignEnd)>,
    pub battles: Vec<FieldResult>,
    /// Clashes [`Clash::problem`] refused to stage. Zero on content that
    /// works; anything else is a mod the campaign cannot fight.
    pub declined: u32,
    /// Cadets killed, indexed by the side whose academy she belonged to.
    pub killed: Vec<u32>,
    /// Cadets wounded or walking back, the same way.
    pub hurt: Vec<u32>,
}

/// Play `map_id` from day one to its ending, every side a machine.
///
/// Deterministic in `seed`: the campaign, its planners and every battle are
/// seeded from it, and nothing here walks a hash map.
pub fn play(
    registry: &DataRegistry,
    map_id: &str,
    seed: u64,
    options: &CampaignOptions,
) -> Result<CampaignRun, OverworldSetupError> {
    let mut state = OverworldState::from_map(registry, map_id, seed)?;
    let sides = state.sides.len();
    let mut planners: Vec<_> = state
        .sides
        .iter()
        .enumerate()
        .map(|(i, side)| {
            let config = side.ai.clone().unwrap_or_else(|| AiConfig {
                planner: "simple".into(),
                ..options.stand_in.clone()
            });
            make_overworld_planner(&config, seed.wrapping_add(i as u64))
        })
        .collect();

    let mut run = CampaignRun {
        days: state.turn,
        end: None,
        battles: Vec::new(),
        declined: 0,
        killed: vec![0; sides],
        hurt: vec![0; sides],
    };
    let side_of = |state: &OverworldState, cadet| {
        state
            .roster
            .get(cadet)
            .map(|c| c.owner as usize)
            .filter(|side| *side < sides)
    };

    while state.over.is_none() && state.turn <= options.max_days {
        let side = state.active_side as usize;
        let events = step_planner(planners[side].as_mut(), registry, &mut state);
        let mut queue: std::collections::VecDeque<OverworldEvent> = events.into();
        while let Some(event) = queue.pop_front() {
            match event {
                OverworldEvent::BattleTriggered {
                    attacker,
                    defender,
                    at,
                } => {
                    let terrain = state
                        .map
                        .get(at)
                        .map(|t| t.terrain.clone())
                        .unwrap_or_default();
                    let Some(map) = battlefield_for(registry, &terrain) else {
                        run.declined += 1;
                        continue;
                    };
                    let (Some(att), Some(def)) = (state.army(attacker), state.army(defender))
                    else {
                        continue;
                    };
                    let (attacker_side, defender_side) = (att.side, def.side);
                    let mut joiners = ai_reinforcements(&state, at, attacker_side, attacker, true);
                    joiners.extend(ai_reinforcements(
                        &state,
                        at,
                        defender_side,
                        defender,
                        false,
                    ));
                    let clash = Clash::muster(&state, attacker, defender, &joiners, &[], map);
                    if clash.problem(registry).is_some() {
                        run.declined += 1;
                        continue;
                    }
                    state.commit_to_battle(&joiners);
                    let battle_seed = seed
                        .wrapping_mul(0x9E37_79B9)
                        .wrapping_add(run.battles.len() as u64);
                    let (mut battle, field) = clash
                        .stage(registry, battle_seed)
                        .expect("a clash `problem` passed stages");
                    clash.inherit_missions(registry, &mut battle);

                    let mut ai = AiDriver::new();
                    for (i, side) in clash.sides.iter().enumerate() {
                        let config = side.ai.clone().unwrap_or_else(|| options.stand_in.clone());
                        ai.insert(
                            i as u8,
                            make_battle_planner(
                                &config,
                                battle_seed.wrapping_add(i as u64),
                                registry,
                            ),
                        );
                    }
                    let mut rounds = 0;
                    while !battle.is_over() && rounds < options.max_rounds {
                        ai.plan_round(registry, &mut battle);
                        battle.resolve_round(registry);
                        rounds += 1;
                    }
                    let report = field.report(registry, &battle);
                    let mut hulls_lost = vec![0; battle.sides.len()];
                    for unit in battle.lost_units() {
                        hulls_lost[unit.side as usize] += 1;
                    }
                    run.battles.push(FieldResult {
                        day: state.turn,
                        map_id: clash.map_id.clone(),
                        attacker_side,
                        winner: report.winner,
                        rounds,
                        cut_off: !battle.is_over(),
                        hulls_lost,
                        withdrew: report.withdrew.len() as u32,
                    });
                    // What the battle did to the map comes back as events of
                    // its own, and they are read by this same loop: a
                    // campaign can end on a battle's result.
                    queue.extend(state.apply_battle_result(registry, &report));
                }
                OverworldEvent::CrewCasualty { cadet, fate } => {
                    let Some(side) = side_of(&state, cadet) else {
                        continue;
                    };
                    match fate {
                        CrewFate::Killed => run.killed[side] += 1,
                        CrewFate::Wounded { .. } | CrewFate::Lost { .. } => run.hurt[side] += 1,
                        CrewFate::Unharmed => {}
                    }
                }
                OverworldEvent::GameEnded { winner, reason } => {
                    run.end = Some((winner, reason));
                }
                _ => {}
            }
        }
        run.days = state.turn;
    }
    Ok(run)
}
