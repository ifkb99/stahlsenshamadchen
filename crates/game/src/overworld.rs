//! The overworld screen: strategic army movement, objective capture,
//! income, soft fog, and handing off clashes to the battle screen.

use crate::battle::{BattleForce, BattleOutcome, PendingBattle};
use crate::camera::CameraFocus;
use crate::campaign::{self, Campaign, CampaignCommand};
use crate::iso::{self, ArtCache, ViewCenter};
use crate::map_render::{self, CurrentMap, HexOverlay};
use crate::mods::Mods;
use crate::{AppState, ScreenSet};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};
use tactics_core::Hex;
use tactics_core::ai::AiPlanner;
use tactics_core::battle::SideState;
use tactics_core::map::MapKind;
use tactics_core::overworld::{
    ArmyId, ArmyMission, ArmyUnit, OverworldEvent, OverworldOrder, OverworldState,
    make_overworld_planner,
};
use tactics_core::roster::CadetId;
use tactics_core::save::SaveGame;

#[derive(Resource)]
struct Overworld {
    state: OverworldState,
    planners: HashMap<u8, Box<dyn AiPlanner<OverworldState, OverworldOrder>>>,
    anim: VecDeque<OverworldEvent>,
    pace: Timer,
    selected: Option<ArmyId>,
    /// Tiles the selected army can reach, with the cost of getting there.
    move_range: HashMap<Hex, u32>,
    /// Enemy armies the selection could engage this turn.
    attack_targets: Vec<ArmyId>,
    /// Range highlights need respawning.
    range_dirty: bool,
    /// A triggered battle waiting on the player to pick reinforcements.
    /// Kept here rather than in its own resource so every system sees it
    /// the instant it appears; a deferred `insert_resource` would let the
    /// AI slip in one more order first.
    muster: Option<Muster>,
    /// The butcher's bill from the last battle, held until the player
    /// dismisses it. Freezes the campaign exactly as `muster` does.
    debrief: Option<AfterAction>,
    /// Whether the academy roll is open over the map.
    ///
    /// A `bool` rather than an `Option<Page>` because the page is derived
    /// from the campaign every frame: a roster that cached its own copy of
    /// who is wounded would be a roster that goes stale the moment a battle
    /// ends, which is precisely when the player opens it.
    roster: bool,
}

impl Overworld {
    /// Recompute what the selected army can do. Called whenever the
    /// selection or the board changes underneath it.
    fn refresh_range(&mut self, registry: &tactics_core::data::DataRegistry) {
        self.range_dirty = true;
        let Some(army) = self.selected.and_then(|id| self.state.army(id)) else {
            self.move_range.clear();
            self.attack_targets.clear();
            return;
        };
        if army.moved {
            self.move_range.clear();
            self.attack_targets.clear();
            return;
        }
        let id = army.id;
        self.move_range = self.state.reachable(registry, id);
        self.attack_targets = self.state.attack_targets(registry, id);
    }

    fn clear_selection(&mut self) {
        self.selected = None;
        self.move_range.clear();
        self.attack_targets.clear();
        self.range_dirty = true;
    }
}

#[derive(Component, Clone)]
struct OverworldScope;

#[derive(Component)]
struct ArmyMarker(ArmyId);

#[derive(Component)]
struct OwnerDot(Hex);

/// Tile tint showing where the selected army can go, or who it can hit.
#[derive(Component)]
struct OwRangeTile;

#[derive(Component)]
struct OwHoverTile;

/// The wedge over the army carrying headquarters — the campaign's mirror of
/// the battle's leader chevron, and rooted at the same place the campaign's
/// contact graph is: [`OverworldState::senior_army`].
#[derive(Component)]
struct ArmyChevron(ArmyId);

/// Signals green, the same wire colour the battle screen draws its ring in:
/// the two scales are one system and a player who has learned the colour on
/// the battlefield should not have to learn it again on the map.
const OW_NET_RING: Color = Color::srgba(0.3, 1.0, 0.62, 0.78);

#[derive(Component)]
struct OwBanner;

#[derive(Component)]
struct OwLog;

#[derive(Component)]
struct OwPanel;

#[derive(Component)]
struct MusterPanel;

#[derive(Component)]
struct DebriefPanel;

#[derive(Component)]
struct RosterPanel;

/// The campaign HUD's three text widgets, bundled for the same reason
/// `battle::BattleHud` is: they are one thing conceptually, and the mutual
/// `Without` filters exist only so Bevy can prove the `&mut Text` queries do
/// not alias.
#[derive(SystemParam)]
struct OverworldHud<'w, 's> {
    banner: map_render::TextSlot<'w, 's, OwBanner, OwLog, OwPanel>,
    log_text: map_render::TextSlot<'w, 's, OwLog, OwBanner, OwPanel>,
    panel: map_render::TextSlot<'w, 's, OwPanel, OwBanner, OwLog>,
}

#[derive(Resource, Default)]
struct OwLogLines(VecDeque<String>);

impl OwLogLines {
    fn push(&mut self, line: impl Into<String>) {
        self.0.push_back(line.into());
        while self.0.len() > 8 {
            self.0.pop_front();
        }
    }
}

pub struct OverworldPlugin;

impl Plugin for OverworldPlugin {
    fn build(&self, app: &mut App) {
        // Same seven phases as the battle screen, in the same order, declared
        // once in `main.rs`. What differs is only which systems go in each.
        app.init_resource::<OwLogLines>()
            .add_systems(OnEnter(AppState::Overworld), enter_overworld)
            .add_systems(
                Update,
                pump_events
                    .in_set(ScreenSet::Animate)
                    .run_if(in_state(AppState::Overworld)),
            )
            .add_systems(
                Update,
                drive_ai
                    .in_set(ScreenSet::Simulate)
                    .run_if(in_state(AppState::Overworld)),
            )
            .add_systems(
                Update,
                // These four stay chained and the chain is load-bearing: one
                // keystroke means different things to a modal and to the map
                // underneath it, so whoever is on top must have first refusal.
                (
                    muster_input,
                    handle_input,
                    // After `handle_input`, not before: dismissing the report
                    // and ending the day are both Enter, and clearing the
                    // report first would let the same keystroke fall straight
                    // through into the campaign underneath it.
                    debrief_input,
                    // Same argument again, and the key is the reason: Esc
                    // closes the roll and also drops the map selection, so
                    // the roll must have its chance first.
                    roster_input,
                )
                    .chain()
                    .in_set(ScreenSet::Input)
                    .run_if(in_state(AppState::Overworld)),
            )
            .add_systems(
                Update,
                sync_armies
                    .in_set(ScreenSet::Sync)
                    .run_if(in_state(AppState::Overworld)),
            )
            .add_systems(
                Update,
                (
                    update_owner_dots,
                    update_range_highlights,
                    update_ui,
                    update_muster_ui,
                    update_debrief_ui,
                    update_roster_ui,
                )
                    .chain()
                    .in_set(ScreenSet::Present)
                    .run_if(in_state(AppState::Overworld)),
            )
            .add_systems(
                Update,
                // Lifecycle because a campaign script can ask for a state
                // transition, and a frame that leaves the screen should have
                // been drawn first.
                apply_campaign_commands
                    .in_set(ScreenSet::Lifecycle)
                    .run_if(in_state(AppState::Overworld)),
            )
            .add_systems(OnExit(AppState::Overworld), leave_overworld);
        // The campaign map answers the same questions the battle screen does,
        // in its own terms. It did not, until the after-action report gave it
        // something worth waiting for: every campaign tour was a stopwatch,
        // which is the thing the script harness exists to stop being.
        if crate::devtools::debug_enabled() {
            app.init_resource::<crate::devtools::ScriptFacts>()
                .add_systems(
                    Update,
                    publish_script_facts
                        .after(apply_campaign_commands)
                        .run_if(in_state(AppState::Overworld)),
                );
        }
    }
}

/// Tell the script runner what the campaign map knows about itself.
///
/// `turn` is the day, which is the campaign's answer to the same question a
/// battle answers with the round. `idle` means the same thing here as there —
/// a keystroke would be acted on and a screenshot would not lie — which on
/// this screen means nothing is animating *and* nothing is holding the player
/// behind a modal. `waiting` is the other half of that pair and the reason
/// both exist: a script that photographs the after-action report has to be
/// able to wait for it to arrive, and a script driving the campaign has to be
/// able to wait for it to be gone.
fn publish_script_facts(
    overworld: Option<Res<Overworld>>,
    log: Res<OwLogLines>,
    mut facts: ResMut<crate::devtools::ScriptFacts>,
) {
    // Optional because the resource is inserted by `enter_overworld` and torn
    // down on the way into a battle, so there are frames in this state with no
    // campaign to ask. A required `Res` here panics on those frames.
    let Some(overworld) = overworld else {
        return;
    };
    // The roll counts as held even though the player opened it herself.
    // `waiting` means "a keystroke goes to a page rather than to the map",
    // which is exactly true here — and keeping it the strict complement of
    // `idle` is what stops a tour photographing the map with a panel sitting
    // on top of it.
    let held = overworld.muster.is_some() || overworld.debrief.is_some() || overworld.roster;
    // Built whole, for the reason given at the battle screen's publisher: a
    // field this screen has no answer for must read as "no answer" and not as
    // the battle's last one. This publisher is why that stopped being a
    // remark and became the shape — it set seven of the eight fields and left
    // `selected` alone, so after any battle a campaign tour's `selected` was
    // still naming the last crew the player had clicked, and an `expect
    // selected …` on the map would have passed for the wrong reason.
    //
    // A campaign map has no selected *unit* — it has a selected army, which
    // is a different question and would want a fact of its own — so the
    // honest answer here is `None`, said out loud.
    *facts = crate::devtools::ScriptFacts {
        turn: overworld.state.turn,
        idle: overworld.anim.is_empty() && !held,
        waiting: held,
        over: overworld.state.over.is_some(),
        score: Vec::new(),
        units: Vec::new(),
        log: log.0.iter().cloned().collect(),
        selected: None,
        // Said out loud rather than defaulted, like `selected` above it: the
        // campaign map has no danger overlay because it has no gunners, and
        // leaving the field alone would answer a battle's question with a
        // battle's answer two screens after it was true.
        danger: None,
    };
}

// Same reasoning as `battle::pump_events`: this is a setup system, and the
// optional resources it takes (a previous overworld, a battle outcome to fold
// in, a campaign script) are independent things that happen to be needed on
// the same frame, not parts of one object.
#[allow(clippy::too_many_arguments)]
fn enter_overworld(
    mut commands: Commands,
    mods: Res<Mods>,
    art: Res<ArtCache>,
    view: map_render::View,
    mut log: ResMut<OwLogLines>,
    existing: Option<ResMut<Overworld>>,
    outcome: Option<Res<BattleOutcome>>,
    campaign: Option<NonSendMut<Campaign>>,
    mut focus: ResMut<CameraFocus>,
) {
    let registry = &mods.0;

    if let Some(mut ow) = existing {
        // Returning from a battle: fold the outcome into the campaign.
        if let Some(outcome) = outcome {
            let events = ow.state.apply_battle_result(
                registry,
                outcome.attacker,
                outcome.defender,
                &outcome.survivors,
                &outcome.losses,
            );
            let headline = match (outcome.winner, outcome.stalemate) {
                (Some(w), _) => format!("Battle won by {}.", ow.state.sides[w as usize].name),
                (None, true) => "Neither side could find the other. Both withdrew.".to_string(),
                (None, false) => "The battle ended in mutual ruin.".to_string(),
            };
            log.push(headline.clone());
            // Who paid for that, before the campaign moves on. "Human" here
            // means a side with no planner attached, the same test the muster
            // prompt uses and for the same reason: under `STAHL_AUTOPLAY`
            // there is nobody to press Enter, and a modal nobody can dismiss
            // is a hang rather than a screen.
            let human =
                (0..ow.state.sides.len() as u8).find(|side| !ow.planners.contains_key(side));
            if let Some(side) = human {
                let report = after_action(&ow.state, side, headline, &events, &outcome.survivors);
                ow.debrief = Some(report);
            }
            ow.anim.extend(events);
            if let Some(campaign) = campaign {
                campaign::call_battle_end_hook(&campaign, &ow.state, outcome.winner);
            }
            commands.remove_resource::<BattleOutcome>();
        }
        ow.clear_selection();
        let center = spawn_world(&mut commands, registry, &art, view.rotation(), &ow.state);
        center_camera(&mut focus, view.rotation(), center);
        return;
    }

    // Fresh campaign: pick the first overworld map from the mods.
    let map_id = registry
        .maps
        .values()
        .find(|m| m.kind == MapKind::Overworld)
        .map(|m| m.id.clone())
        .expect("base mod provides an overworld map");
    let state = OverworldState::from_map(registry, &map_id, 1337).expect("overworld builds");
    log.push("Campaign started. LMB select army / move, Enter end turn, Q/E rotate.");

    // Dev tool: STAHL_AUTOPLAY=1 puts every side under AI control.
    let autoplay = std::env::var("STAHL_AUTOPLAY").is_ok();
    let mut planners: HashMap<u8, Box<dyn AiPlanner<OverworldState, OverworldOrder>>> =
        HashMap::new();
    for (i, side) in state.sides.iter().enumerate() {
        match &side.ai {
            Some(cfg) => {
                planners.insert(i as u8, make_overworld_planner(cfg, 1337 + i as u64));
            }
            None if autoplay => {
                let cfg = tactics_core::ai::AiConfig {
                    planner: "simple".into(),
                    difficulty: 4,
                    doctrine: None,
                };
                planners.insert(i as u8, make_overworld_planner(&cfg, 1337 + i as u64));
            }
            None => {}
        }
    }

    let center = spawn_world(&mut commands, registry, &art, view.rotation(), &state);
    center_camera(&mut focus, view.rotation(), center);

    if let Some(campaign) = campaign {
        campaign::call_start_hook(&campaign, &state);
    }
    commands.insert_resource(Overworld {
        state,
        planners,
        anim: VecDeque::new(),
        pace: Timer::from_seconds(0.3, TimerMode::Repeating),
        selected: None,
        move_range: HashMap::new(),
        attack_targets: Vec::new(),
        range_dirty: false,
        muster: None,
        debrief: None,
        roster: false,
    });
}

/// Where the single save slot lives. Beside the assets for now, which is
/// wrong for a shipped game — it belongs in the platform's data directory —
/// and right for one that is still run from its source tree.
fn save_path() -> std::path::PathBuf {
    std::path::PathBuf::from("saves/campaign.json")
}

fn center_camera(focus: &mut CameraFocus, rotation: u32, center: Hex) {
    let (pos, _) = iso::project(center, 0, rotation, center);
    focus.0 = pos;
}

/// Spawn the map tiles, army markers, owner dots, and UI for a state.
fn spawn_world(
    commands: &mut Commands,
    registry: &tactics_core::data::DataRegistry,
    art: &ArtCache,
    rotation: u32,
    state: &OverworldState,
) -> Hex {
    // Returned rather than written through a `ResMut`, so callers can take
    // `View` (which reads `ViewCenter`) without conflicting access.
    let center = state.map.center();
    commands.insert_resource(ViewCenter(center));
    map_render::spawn_map(commands, art, &state.map, rotation, false, OverworldScope);
    commands.insert_resource(CurrentMap(state.map.clone()));

    for army in state.armies.iter().filter(|a| a.alive) {
        commands
            .spawn((
                Sprite {
                    image: art
                        .armies
                        .get(&(army.side % iso::SIDE_COLORS.len() as u8))
                        .cloned()
                        .unwrap_or_else(|| art.face.clone()),
                    ..default()
                },
                Transform::default(),
                ArmyMarker(army.id),
                OverworldScope,
            ))
            // Spawned for every army and shown for the one carrying the
            // headquarters, because seniority moves: the army the net is
            // rooted at changes the moment the first-declared one is
            // destroyed, and `sync_armies` reads it fresh every frame rather
            // than waiting to be told.
            .with_children(|parent| {
                parent.spawn((
                    Sprite {
                        image: art.chevron.clone(),
                        ..default()
                    },
                    Transform::from_translation(Vec3::new(0.0, 26.0, 0.2)),
                    Visibility::Hidden,
                    ArmyChevron(army.id),
                ));
            });
    }
    commands.spawn((
        Sprite {
            image: art.face.clone(),
            color: Color::srgba(1.0, 1.0, 1.0, 0.35),
            ..default()
        },
        Transform::default(),
        Visibility::Hidden,
        OwHoverTile,
        OverworldScope,
    ));
    for (hex, tile) in state.map.iter() {
        if registry
            .terrain(&tile.terrain)
            .is_some_and(|t| t.capturable)
        {
            commands.spawn((
                Sprite {
                    color: Color::srgba(1.0, 1.0, 1.0, 0.0),
                    custom_size: Some(Vec2::splat(10.0)),
                    ..default()
                },
                Transform::default(),
                OwnerDot(hex),
                OverworldScope,
            ));
        }
    }
    spawn_ui(commands);
    center
}

fn spawn_ui(commands: &mut Commands) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            left: Val::Px(0.0),
            // Stop short of the side panel so the two never overlap.
            right: Val::Px(264.0),
            justify_content: JustifyContent::Center,
            ..default()
        },
        OverworldScope,
        children![(
            Text::new(""),
            TextFont {
                font_size: 20.0.into(),
                ..default()
            },
            TextColor(Color::WHITE),
            OwBanner,
        )],
    ));
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            bottom: Val::Px(8.0),
            left: Val::Px(8.0),
            max_width: Val::Px(620.0),
            padding: UiRect::all(Val::Px(6.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        OverworldScope,
        children![(
            Text::new(""),
            TextFont {
                font_size: 13.0.into(),
                ..default()
            },
            TextColor(Color::srgb(0.9, 0.9, 0.85)),
            OwLog,
        )],
    ));
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            right: Val::Px(8.0),
            width: Val::Px(240.0),
            padding: UiRect::all(Val::Px(8.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.05, 0.05, 0.1, 0.75)),
        OverworldScope,
        children![(
            Text::new(""),
            TextFont {
                font_size: 14.0.into(),
                ..default()
            },
            TextColor(Color::srgb(0.92, 0.92, 0.88)),
            OwPanel,
        )],
    ));
    // Muster prompt, shown only while a battle is waiting on the player.
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Percent(28.0),
            left: Val::Percent(50.0),
            margin: UiRect::left(Val::Px(-190.0)),
            width: Val::Px(380.0),
            padding: UiRect::all(Val::Px(14.0)),
            display: Display::None,
            ..default()
        },
        BackgroundColor(Color::srgba(0.06, 0.06, 0.12, 0.94)),
        Text::new(""),
        TextFont {
            font_size: 15.0.into(),
            ..default()
        },
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        MusterPanel,
        OverworldScope,
    ));
    // After-action report, shown only on the frame after a battle until the
    // player dismisses it. Wider and taller than the muster prompt because
    // it lists people by name and a roll call that scrolls off is a roll
    // call nobody reads.
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Percent(12.0),
            left: Val::Percent(50.0),
            margin: UiRect::left(Val::Px(-220.0)),
            width: Val::Px(440.0),
            padding: UiRect::all(Val::Px(16.0)),
            display: Display::None,
            ..default()
        },
        BackgroundColor(Color::srgba(0.06, 0.06, 0.12, 0.96)),
        Text::new(""),
        TextFont {
            font_size: 15.0.into(),
            ..default()
        },
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        DebriefPanel,
        OverworldScope,
    ));
    // The academy roll. Taller and further up the screen than either of the
    // prompts above it, because it is the only page here that lists the
    // whole school rather than the consequences of one afternoon.
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Percent(2.0),
            left: Val::Percent(50.0),
            margin: UiRect::left(Val::Px(-230.0)),
            width: Val::Px(460.0),
            padding: UiRect::all(Val::Px(16.0)),
            display: Display::None,
            ..default()
        },
        BackgroundColor(Color::srgba(0.06, 0.06, 0.12, 0.97)),
        Text::new(""),
        TextFont {
            font_size: 14.0.into(),
            ..default()
        },
        TextColor(Color::srgb(0.95, 0.95, 0.9)),
        RosterPanel,
        OverworldScope,
    ));
}

fn pump_events(
    time: Res<Time>,
    mods: Res<Mods>,
    mut commands: Commands,
    mut overworld: ResMut<Overworld>,
    mut log: ResMut<OwLogLines>,
    mut next: ResMut<NextState<AppState>>,
    campaign: Option<NonSend<Campaign>>,
) {
    if overworld.muster.is_some() || overworld.debrief.is_some() || overworld.roster {
        return;
    }
    overworld.pace.tick(time.delta());
    if !overworld.pace.just_finished() {
        return;
    }
    let Some(event) = overworld.anim.pop_front() else {
        return;
    };
    let registry = &mods.0;
    match &event {
        OverworldEvent::TurnStarted { side, turn } => {
            let name = &overworld.state.sides[*side as usize].name;
            log.push(format!("- Day {turn}: {name} -"));
            // The campaign sees one on_turn per new day (first side's phase).
            if *side == 0
                && let Some(campaign) = &campaign
            {
                campaign::call_turn_hook(campaign, &overworld.state);
            }
        }
        OverworldEvent::Income { side, amount } => {
            let name = &overworld.state.sides[*side as usize].name;
            log.push(format!("{name} collects {amount} funds."));
        }
        OverworldEvent::ArmyMoved { .. } => {}
        OverworldEvent::CrewCasualty { cadet, fate } => {
            let name = overworld
                .state
                .roster
                .get(*cadet)
                .map(|g| g.name.clone())
                .unwrap_or_else(|| "A crew member".into());
            use tactics_core::roster::CrewFate;
            log.push(match fate {
                CrewFate::Unharmed => format!("{name} bailed out and made it back."),
                CrewFate::Wounded { days } => {
                    format!("{name} is hurt - out for {days} day(s).")
                }
                CrewFate::Lost { days } => {
                    format!("{name} is missing behind the lines; walking back ({days} day(s)).")
                }
                CrewFate::Killed => format!("{name} did not make it."),
            });
        }
        OverworldEvent::ObjectiveCaptured { at, side } => {
            let name = &overworld.state.sides[*side as usize].name;
            let terrain = overworld
                .state
                .map
                .get(*at)
                .and_then(|t| registry.terrain(&t.terrain))
                .map(|t| t.name.clone())
                .unwrap_or_default();
            log.push(format!("{name} captured a {terrain}."));
        }
        OverworldEvent::BattleTriggered {
            attacker,
            defender,
            at,
        } => {
            let (Some(att), Some(def)) = (
                overworld.state.army(*attacker),
                overworld.state.army(*defender),
            ) else {
                return;
            };
            log.push(format!("{} engages {}!", att.name, def.name));
            let (attacker_side, defender_side) = (att.side, def.side);
            let terrain = overworld
                .state
                .map
                .get(*at)
                .map(|t| t.terrain.clone())
                .unwrap_or_default();
            let Some(map_id) = choose_battle_map(registry, &terrain) else {
                log.push("No battle map is loaded, so the fight cannot be staged.");
                return;
            };

            // Neighbours on either side can pile in. Each side decides for
            // itself: the AI masses automatically, the player is asked.
            // "Player" means a side with no planner attached, so the
            // autoplay dev switch doesn't leave the prompt waiting for a
            // human who will never arrive.
            let human = (0..overworld.state.sides.len() as u8)
                .find(|side| !overworld.planners.contains_key(side));
            let mut joiners = Vec::new();
            let mut choices = Vec::new();
            for (side, principal, attacking) in [
                (attacker_side, *attacker, true),
                (defender_side, *defender, false),
            ] {
                if human == Some(side) {
                    choices = overworld
                        .state
                        .reinforcement_candidates(*at, side, principal, attacking);
                } else {
                    joiners.extend(tactics_core::overworld::ai_reinforcements(
                        &overworld.state,
                        *at,
                        side,
                        principal,
                        attacking,
                    ));
                }
            }

            if choices.is_empty() {
                if !joiners.is_empty() {
                    log.push(format!("{} more armies join the fight.", joiners.len()));
                }
                launch_battle(
                    &mut commands,
                    &mut overworld.state,
                    &mut next,
                    &mut log,
                    registry,
                    *attacker,
                    *defender,
                    &joiners,
                    map_id,
                );
            } else {
                // Hold the campaign here until the player has mustered.
                overworld.muster = Some(Muster {
                    attacker: *attacker,
                    defender: *defender,
                    side: human.unwrap_or(attacker_side),
                    attacking: human == Some(attacker_side),
                    choices: choices.into_iter().map(|id| (id, true)).collect(),
                    ai_joiners: joiners,
                    map_id,
                });
            }
        }
        OverworldEvent::ArmyDestroyed { army } => {
            log.push(format!("Army {} was destroyed.", army.0));
        }
        OverworldEvent::ArmyMissionAssigned { army, mission } => {
            if ours(&overworld.state, *army) {
                let name = army_name(&overworld.state, *army);
                log.push(match mission {
                    ArmyMission::Advance { to } => {
                        format!("{name} ordered to advance on {}.", place(*to))
                    }
                    ArmyMission::Hold => format!("{name} ordered to hold."),
                    ArmyMission::Withdraw { to } => {
                        format!("{name} ordered to fall back on {}.", place(*to))
                    }
                });
            }
        }
        OverworldEvent::ArmyOutOfContact { army } => {
            if ours(&overworld.state, *army) {
                log.push(format!(
                    "{} is out of radio contact.",
                    army_name(&overworld.state, *army)
                ));
            }
        }
        OverworldEvent::ArmyContactRestored { army } => {
            if ours(&overworld.state, *army) {
                log.push(format!(
                    "{} is back on the net.",
                    army_name(&overworld.state, *army)
                ));
            }
        }
        // An order taken but not yet sent. The player has to hear this or the
        // army looks like it is ignoring her; the assignment line follows on
        // the morning it actually goes out.
        OverworldEvent::ArmyOrdersWaiting { army } => {
            if ours(&overworld.state, *army) {
                log.push(format!(
                    "No contact with {}. Orders held for transmission.",
                    army_name(&overworld.state, *army)
                ));
            }
        }
        OverworldEvent::GameEnded { winner } => match winner {
            Some(w) => log.push(format!(
                "Campaign over. {} rules the frontier.",
                overworld.state.sides[*w as usize].name
            )),
            None => log.push("Campaign over. Nobody is left standing."),
        },
    }
}

/// Whether this army is one of the player's, for events that would be a leak
/// if they were not.
///
/// What an army has been *told*, and whether its side can still talk to it,
/// are facts about the enemy's chain of command — the campaign's counterpart
/// of the battle's command picture, where the rule is already that a side
/// learns what it is reported and no more. Captures and destructions stay
/// public, because a flag changing colour is something you can see. The engine
/// emits all of it regardless, so a headless consumer and the tests still see
/// both sides; this is a display rule, which is where it belongs.
///
/// With no human side at all (`STAHL_AUTOPLAY`) everything is narrated: there
/// is nobody to keep it from, and watching both chains of command is the
/// entire point of that switch.
fn ours(state: &OverworldState, army: ArmyId) -> bool {
    match state.sides.iter().position(|s| s.ai.is_none()) {
        Some(side) => state.army(army).is_none_or(|a| a.side == side as u8),
        None => true,
    }
}

/// The army's name, or a neutral stand-in for one that has since been
/// destroyed — an event about an army outlives the army, and "Army 3" in the
/// middle of a sentence reads as a bug.
fn army_name(state: &OverworldState, army: ArmyId) -> String {
    state
        .army(army)
        .map(|a| a.name.clone())
        .unwrap_or_else(|| format!("Army {}", army.0))
}

/// A hex as the map file writes it, which is the pair of numbers a player
/// reading the campaign map has any hope of matching to a place.
fn place(hex: Hex) -> String {
    let [col, row] = tactics_core::hex_to_offset(hex);
    format!("({col}, {row})")
}

/// The butcher's bill, held on screen until the player has read it.
///
/// The campaign's log already narrates every casualty, one line every three
/// tenths of a second, in among income and army movements — which is to say
/// the game already told the player that Anka is in hospital for six days and
/// she almost certainly did not see it. That is the whole of what "no stakes"
/// meant: the consequences existed and nothing ever stopped to show them.
///
/// So this stops. It is the same freeze [`Muster`] uses, for the same reason:
/// a decision or a cost that the campaign drives past is one the player never
/// makes or pays.
struct AfterAction {
    headline: String,
    /// Cadets the battle took out of the line, worst first.
    cost: Vec<String>,
    /// Everyone who came home fit, with what this battle was for her.
    home: Vec<String>,
}

/// Build the after-action report for `side` out of what the campaign just
/// applied.
///
/// A free function taking plain data rather than a method on anything, so it
/// can be tested without a window: what this screen says is exactly the sort
/// of thing that rots silently, because nobody reviewing a diff can see it
/// and running the game to look costs a minute a time.
///
/// The casualties come from the events rather than from the roster because
/// the roster has already been written and cannot tell "hurt today" from
/// "hurt on Tuesday and still recovering" — the difference the player is
/// here for. Survivors come from the returned rosters for the same reason:
/// they are who was *in this battle*, not who the academy has.
fn after_action(
    state: &OverworldState,
    side: u8,
    headline: String,
    events: &[OverworldEvent],
    survivors: &[(ArmyId, Vec<ArmyUnit>)],
) -> AfterAction {
    use tactics_core::roster::CrewFate;

    let mine = |cadet: &CadetId| state.roster.get(*cadet).is_some_and(|g| g.owner == side);
    let name = |cadet: &CadetId| {
        state
            .roster
            .get(*cadet)
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "A crew member".into())
    };

    // Worst first, so the line that matters is the one at the top of the
    // page rather than wherever the cadet-id order happened to put it.
    let rank = |fate: &CrewFate| match fate {
        CrewFate::Killed => 0,
        CrewFate::Wounded { days } => 1000 - *days.min(&999) as i32,
        CrewFate::Lost { days } => 2000 - *days.min(&999) as i32,
        CrewFate::Unharmed => 3000,
    };
    let mut hurt: Vec<(i32, CadetId, CrewFate)> = events
        .iter()
        .filter_map(|e| match e {
            OverworldEvent::CrewCasualty { cadet, fate } if mine(cadet) => {
                Some((rank(fate), *cadet, *fate))
            }
            _ => None,
        })
        .filter(|(_, _, fate)| !matches!(fate, CrewFate::Unharmed))
        .collect();
    hurt.sort_by_key(|(rank, cadet, _)| (*rank, *cadet));

    let cost = hurt
        .iter()
        .map(|(_, cadet, fate)| match fate {
            CrewFate::Killed => format!("  {} - killed in action", name(cadet)),
            CrewFate::Wounded { days } => {
                format!("  {} - wounded, back in {days} day(s)", name(cadet))
            }
            CrewFate::Lost { days } => {
                format!("  {} - walking back, {days} day(s)", name(cadet))
            }
            CrewFate::Unharmed => format!("  {}", name(cadet)),
        })
        .collect();

    // Anyone hurt is already named above; listing her twice would make the
    // page read as though the academy had two of her.
    let taken: Vec<CadetId> = hurt.iter().map(|(_, cadet, _)| *cadet).collect();
    let mut home: Vec<CadetId> = survivors
        .iter()
        .flat_map(|(_, units)| units.iter())
        .flat_map(|unit| unit.crew.iter())
        .filter(|cadet| mine(cadet) && !taken.contains(cadet))
        .copied()
        .collect();
    home.sort();
    home.dedup();
    let home = home
        .iter()
        .filter_map(|cadet| state.roster.get(*cadet))
        // A cadet who was on the roll but stayed in the infirmary did not
        // fight this battle and has nothing to say about it.
        .filter(|cadet| cadet.status.is_ready())
        .map(|cadet| match cadet.battles {
            0 | 1 => format!("  {} - her first", cadet.name),
            n => format!("  {} - {n} battles", cadet.name),
        })
        .collect();

    AfterAction {
        headline,
        cost,
        home,
    }
}

/// A triggered battle waiting for the player to pick reinforcements. While
/// one exists the campaign is frozen: no events pump, no AI moves.
struct Muster {
    attacker: ArmyId,
    defender: ArmyId,
    /// The side the player is mustering for.
    side: u8,
    /// Whether that side is the attacker, which is what the wording and the
    /// eligibility rules hinge on.
    attacking: bool,
    /// Candidate armies and whether each is currently marked to join.
    choices: Vec<(ArmyId, bool)>,
    /// Reinforcements the other side already committed.
    ai_joiners: Vec<ArmyId>,
    map_id: String,
}

/// Commit everyone to the fight and hand it to the battle screen.
///
/// The armies are committed only once the battle is known to be stageable.
/// Content the mods no longer ship — a chassis an army still lists, a cadet
/// the roster has lost — used to reach `spawn_unit` and take the game down
/// with it; now the clash is declined with a line in the log and the campaign
/// carries on, which is the difference between a bad mod and a lost run.
#[allow(clippy::too_many_arguments)]
fn launch_battle(
    commands: &mut Commands,
    state: &mut OverworldState,
    next: &mut NextState<AppState>,
    log: &mut OwLogLines,
    registry: &tactics_core::data::DataRegistry,
    attacker: ArmyId,
    defender: ArmyId,
    joiners: &[ArmyId],
    map_id: String,
) {
    let sides: Vec<SideState> = state
        .sides
        .iter()
        .map(|s| SideState {
            name: s.name.clone(),
            ai: s.ai.clone(),
        })
        .collect();
    let attacker_side = state.army(attacker).map(|a| a.side).unwrap_or(0);

    let mut forces = Vec::new();
    for id in [attacker, defender].iter().chain(joiners) {
        if let Some(army) = state.army(*id) {
            // An army could be listed twice if it were both principal and
            // joiner; keep the first entry so unit origins stay unique.
            if forces.iter().any(|f: &BattleForce| f.army == *id) {
                continue;
            }
            forces.push(BattleForce {
                army: *id,
                side: army.side,
                units: army.units.clone(),
                mission: army.mission.clone(),
            });
        }
    }

    // Snapshot of the campaign's cadets. The battle reads it; casualties come
    // back as events and are applied to the campaign's own copy.
    let roster = std::sync::Arc::new(state.roster.clone());
    if let Some(problem) = crate::battle::field_battle_problem(
        registry,
        &map_id,
        &sides,
        attacker_side,
        &forces,
        &roster,
    ) {
        log.push(format!("This battle cannot be staged: {problem}"));
        return;
    }

    state.commit_to_battle(joiners);
    commands.insert_resource(PendingBattle::Field {
        map_id,
        roster,
        attacker,
        defender,
        sides,
        attacker_side,
        forces,
    });
    next.set(AppState::Battle);
}

/// Number keys toggle who joins, Enter commits.
fn muster_input(
    mut commands: Commands,
    mods: Res<Mods>,
    mut overworld: ResMut<Overworld>,
    mut next: ResMut<NextState<AppState>>,
    mut log: ResMut<OwLogLines>,
    keys: Res<ButtonInput<KeyCode>>,
) {
    let Some(muster) = overworld.muster.as_mut() else {
        return;
    };

    const DIGITS: [KeyCode; 9] = [
        KeyCode::Digit1,
        KeyCode::Digit2,
        KeyCode::Digit3,
        KeyCode::Digit4,
        KeyCode::Digit5,
        KeyCode::Digit6,
        KeyCode::Digit7,
        KeyCode::Digit8,
        KeyCode::Digit9,
    ];
    for (i, key) in DIGITS.iter().enumerate().take(muster.choices.len()) {
        if keys.just_pressed(*key) {
            muster.choices[i].1 = !muster.choices[i].1;
        }
    }
    if keys.just_pressed(KeyCode::KeyN) {
        for choice in &mut muster.choices {
            choice.1 = false;
        }
    }
    if keys.just_pressed(KeyCode::KeyM) {
        for choice in &mut muster.choices {
            choice.1 = true;
        }
    }
    if !keys.just_pressed(KeyCode::Enter) {
        return;
    }

    let muster = overworld.muster.take().expect("checked above");
    let mut joiners = muster.ai_joiners;
    joiners.extend(
        muster
            .choices
            .iter()
            .filter(|(_, on)| *on)
            .map(|(id, _)| *id),
    );
    if !joiners.is_empty() {
        log.push(format!("{} more armies join the fight.", joiners.len()));
    }
    launch_battle(
        &mut commands,
        &mut overworld.state,
        &mut next,
        &mut log,
        &mods.0,
        muster.attacker,
        muster.defender,
        &joiners,
        muster.map_id,
    );
}

fn update_muster_ui(
    overworld: Res<Overworld>,
    mut panel: Query<(&mut Node, &mut Text), With<MusterPanel>>,
) {
    let Ok((mut node, mut text)) = panel.single_mut() else {
        return;
    };
    let Some(muster) = overworld.muster.as_ref() else {
        node.display = Display::None;
        return;
    };
    node.display = Display::Flex;

    let state = &overworld.state;
    let name = |id: ArmyId| {
        state
            .army(id)
            .map(|a| format!("{} ({} units)", a.name, a.units.len()))
            .unwrap_or_else(|| "?".into())
    };
    let role = if muster.attacking {
        "assault"
    } else {
        "defence"
    };
    let side_name = state
        .sides
        .get(muster.side as usize)
        .map(|s| s.name.as_str())
        .unwrap_or("");
    let mut lines = vec![
        format!("{} vs {}", name(muster.attacker), name(muster.defender)),
        String::new(),
        format!("Nearby {side_name} armies can join the {role}."),
        format!(
            "Joining spends their turn.{}",
            if muster.attacking {
                ""
            } else {
                " Defenders answer for free."
            }
        ),
        String::new(),
    ];
    for (i, (id, joining)) in muster.choices.iter().enumerate() {
        let mark = if *joining { "x" } else { " " };
        lines.push(format!("  {}. [{mark}] {}", i + 1, name(*id)));
    }
    if !muster.ai_joiners.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "{} enemy {} joining.",
            muster.ai_joiners.len(),
            if muster.ai_joiners.len() == 1 {
                "army is"
            } else {
                "armies are"
            }
        ));
    }
    // Who is not coming, and why. The other end of the after-action report:
    // the debrief says what a battle cost and this is where that cost is
    // actually paid, which is the only place it can be *felt* — a name
    // missing from a crew list is an abstraction until the moment you are
    // about to fight without her.
    let unavailable: Vec<&tactics_core::roster::Cadet> = state
        .roster
        .of_side(muster.side)
        .filter(|cadet| !cadet.status.is_ready())
        .collect();
    if !unavailable.is_empty() {
        lines.push(String::new());
        lines.push(format!(
            "{} not fit to deploy:",
            if unavailable.len() == 1 {
                "1 cadet".to_string()
            } else {
                format!("{} cadets", unavailable.len())
            }
        ));
        // Named, and capped: a page that lists twenty names is one nobody
        // reads, and the count above is the number that matters.
        for cadet in unavailable.iter().take(4) {
            lines.push(format!(
                "  {}{}",
                cadet.name,
                match cadet.status.days_out() {
                    Some(0) | None => String::new(),
                    Some(days) => format!(" ({days} day(s))"),
                }
            ));
        }
        if unavailable.len() > 4 {
            lines.push(format!("  ...and {} more", unavailable.len() - 4));
        }
    }
    lines.push(String::new());
    lines.push("1-9 toggle  M all  N none  Enter fight".into());
    text.0 = lines.join("\n");
}

/// Enter (or Space) dismisses the after-action report and lets the campaign
/// run on.
///
/// One key, and no choice on the page: this is a thing to have read, not a
/// decision. Anything the player wants to *do* about her casualties belongs
/// on a roster screen she can open whenever she likes, not on a modal she is
/// being held behind.
fn debrief_input(mut overworld: ResMut<Overworld>, keys: Res<ButtonInput<KeyCode>>) {
    if overworld.debrief.is_none() {
        return;
    }
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::Space) {
        overworld.debrief = None;
    }
}

fn update_debrief_ui(
    overworld: Res<Overworld>,
    mut panel: Query<(&mut Node, &mut Text), With<DebriefPanel>>,
) {
    let Ok((mut node, mut text)) = panel.single_mut() else {
        return;
    };
    let Some(debrief) = overworld.debrief.as_ref() else {
        node.display = Display::None;
        return;
    };
    node.display = Display::Flex;

    let mut lines = vec![format!("Day {} - after action", overworld.state.turn)];
    lines.push(String::new());
    lines.push(debrief.headline.clone());
    lines.push(String::new());
    // The cost first and by name, because that is the whole reason the
    // campaign is being stopped. A battle nobody was hurt in says so rather
    // than showing an empty heading, so the page always reads as an answer.
    if debrief.cost.is_empty() {
        lines.push("Nobody was hurt.".into());
    } else {
        lines.push("Out of the line:".into());
        lines.extend(debrief.cost.iter().cloned());
    }
    if !debrief.home.is_empty() {
        lines.push(String::new());
        lines.push("Came home:".into());
        lines.extend(debrief.home.iter().cloned());
    }
    lines.push(String::new());
    lines.push("Enter to continue.".into());
    text.0 = lines.join("\n");
}

/// The academy roll: who the school has, where each of them is posted, and
/// who is fit to go out tomorrow.
///
/// A separate page from the after-action report on purpose, and the
/// difference is worth stating because it is the whole reason there are two.
/// The report is a thing to have *read* — it stops the campaign, names what
/// one afternoon cost, and has one key on it. The roll is a thing to *look
/// at*: the player opens it when she wants it, it answers "can I fight
/// tomorrow" rather than "what happened today", and it is derived fresh from
/// the campaign every frame so it cannot disagree with the map underneath it.
struct RosterPage {
    title: String,
    summary: String,
    /// One block per army, in order of battle: the army's name, then a line
    /// for every seat in it.
    sections: Vec<(String, Vec<String>)>,
    /// Cadets on the roll with no vehicle to sit in. Not an edge case: a
    /// crew whose tank was destroyed comes home to an academy that still has
    /// her and no longer has anywhere to put her, and a roster that quietly
    /// dropped her would be the screen telling the player she is dead.
    unposted: Vec<String>,
}

/// How a cadet's condition reads on the roll.
///
/// Deliberately not the same words as the after-action report's. That page
/// reports an event — *wounded today* — and this one reports a state — *not
/// available for six days*. Saying "wounded" in both places would make the
/// roll look like a report the player had already dismissed.
fn availability(status: tactics_core::roster::CadetStatus) -> String {
    use tactics_core::roster::CadetStatus;
    match status {
        CadetStatus::Ready => "fit".into(),
        CadetStatus::Wounded { days } => format!("infirmary, {days} day(s)"),
        CadetStatus::Lost { days } => format!("walking back, {days} day(s)"),
        CadetStatus::Dead => "killed in action".into(),
    }
}

/// How many battles she has behind her, or nothing at all.
///
/// Nothing at all on purpose: on day one of a campaign every cadet has zero,
/// and a column of "0 battle(s)" is twenty-four lines of the page saying the
/// same thing about everybody. A number here means somebody has a history.
fn veteran(battles: u32) -> String {
    match battles {
        0 => String::new(),
        1 => " - 1 battle".into(),
        n => format!(" - {n} battles"),
    }
}

/// Build the roll for `side`.
///
/// A free function over plain data, for the same reason `after_action` is one:
/// a page nobody can see in a diff and nobody can screenshot from this shell
/// is a page that rots, and the only defence is to be able to assert on it.
fn roster_page(
    registry: &tactics_core::data::DataRegistry,
    state: &OverworldState,
    side: u8,
) -> RosterPage {
    let mut posted: Vec<CadetId> = Vec::new();
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();

    for army in state.armies.iter().filter(|a| a.alive && a.side == side) {
        let mut lines: Vec<String> = Vec::new();
        for unit in &army.units {
            let vehicle = registry.vehicle(&unit.vehicle);
            let name = vehicle
                .map(|v| v.name.clone())
                .unwrap_or_else(|| unit.vehicle.clone());
            // Seats are positional — crew *i* fills the chassis's *i*th
            // slot — so the seat a cadet is in is read off the pairing
            // rather than stored anywhere. This page is the only place in
            // the game that shows it, and "who is driving" is exactly the
            // sort of thing a player wants to check before a battle.
            let slots = vehicle.map(|v| v.crew_slots.clone()).unwrap_or_default();
            if unit.crew.is_empty() {
                continue;
            }
            // The vehicle is a heading rather than a clause on every line.
            // That is a layout decision taken from a screenshot: with the
            // chassis repeated per cadet, every single line on this page
            // wrapped, and a wrapped list of twenty-four people is not a
            // list anybody reads.
            lines.push(format!("  {name}"));
            for (index, id) in unit.crew.iter().enumerate() {
                posted.push(*id);
                let Some(cadet) = state.roster.get(*id) else {
                    continue;
                };
                let seat = slots
                    .get(index)
                    .map(|role| {
                        registry
                            .role(role)
                            .map(|r| r.name.clone())
                            .unwrap_or_else(|| role.clone())
                    })
                    .unwrap_or_else(|| "supernumerary".into());
                lines.push(format!(
                    "    {} - {seat} - {}{}",
                    cadet.name,
                    availability(cadet.status),
                    veteran(cadet.battles)
                ));
            }
        }
        if !lines.is_empty() {
            sections.push((army.name.clone(), lines));
        }
    }

    let unposted = state
        .roster
        .of_side(side)
        .filter(|cadet| !posted.contains(&cadet.id))
        .map(|cadet| {
            format!(
                "  {} - {}{}",
                cadet.name,
                availability(cadet.status),
                veteran(cadet.battles)
            )
        })
        .collect();

    let all: Vec<&tactics_core::roster::Cadet> = state.roster.of_side(side).collect();
    let fit = all.iter().filter(|c| c.status.is_ready()).count();
    RosterPage {
        title: format!(
            "{} - the roll, day {}",
            state.sides[side as usize].name, state.turn
        ),
        summary: format!("{} cadets, {fit} fit to deploy.", all.len()),
        sections,
        unposted,
    }
}

impl RosterPage {
    /// The page as the panel draws it. One function so the test and the
    /// screen read the same text, which is the only way an assertion about
    /// wording defends anything.
    fn lines(&self) -> Vec<String> {
        let mut lines = vec![self.title.clone(), String::new(), self.summary.clone()];
        for (army, cadets) in &self.sections {
            lines.push(String::new());
            lines.push(army.clone());
            lines.extend(cadets.iter().cloned());
        }
        if !self.unposted.is_empty() {
            lines.push(String::new());
            lines.push("Without a vehicle:".into());
            lines.extend(self.unposted.iter().cloned());
        }
        lines.push(String::new());
        lines.push("R or Esc to close.".into());
        lines
    }
}

/// `R` opens the roll and `R` or `Esc` closes it.
///
/// Ordered after `handle_input` for the same reason `debrief_input` is: `Esc`
/// also clears the map selection, and closing the page first would let one
/// keystroke do both. `handle_input` refuses to act at all while the page is
/// open, so the two never both fire.
fn roster_input(mut overworld: ResMut<Overworld>, keys: Res<ButtonInput<KeyCode>>) {
    if overworld.muster.is_some() || overworld.debrief.is_some() {
        return;
    }
    if keys.just_pressed(KeyCode::KeyR) {
        overworld.roster = !overworld.roster;
        return;
    }
    if overworld.roster && keys.just_pressed(KeyCode::Escape) {
        overworld.roster = false;
    }
}

fn update_roster_ui(
    mods: Res<Mods>,
    overworld: Res<Overworld>,
    mut panel: Query<(&mut Node, &mut Text), With<RosterPanel>>,
) {
    let Ok((mut node, mut text)) = panel.single_mut() else {
        return;
    };
    if !overworld.roster {
        node.display = Display::None;
        return;
    }
    node.display = Display::Flex;
    // The player's own academy, which is the side nobody is playing for her.
    // The same derivation the rest of this screen uses, rather than the
    // active side: whose turn it is changes twice a day and whose school
    // this is does not.
    let side = overworld
        .state
        .sides
        .iter()
        .position(|s| s.ai.is_none())
        .unwrap_or(0) as u8;
    text.0 = roster_page(&mods.0, &overworld.state, side)
        .lines()
        .join("\n");
}

/// Which battlefield a clash on this ground is fought on, best answer first.
///
/// Three answers, in the order of how much the content actually said:
///
/// 1. `TerrainDef::battlefield`, the link written down. Several terrains may
///    name one battlefield — a city and a factory are both a fight through
///    the same town — and `validate-mods` refuses a name that is not a battle
///    map, so this arm cannot fail quietly.
/// 2. The `battle_<terrain>` convention, kept because it is what every map
///    shipped before the field existed and a mod that declares nothing has to
///    behave exactly as it did.
/// 3. Any battle map at all, which is a shrug: the fight happens somewhere
///    rather than not at all. Nothing on the shipped campaign map reaches it,
///    and `every_terrain_the_campaign_fields_names_its_own_battlefield` is
///    what keeps that true.
///
/// `None` when the loaded mods ship no battle map at all. That used to be an
/// `expect`, on the reasoning that the base mod always provides one — which is
/// true of the base mod and says nothing about the mod somebody loads on top
/// of it. A campaign that cannot stage a fight should say so and carry on.
fn choose_battle_map(registry: &tactics_core::data::DataRegistry, terrain: &str) -> Option<String> {
    let is_battle = |id: &String| {
        registry
            .maps
            .get(id)
            .is_some_and(|m| m.kind == MapKind::Battle)
    };
    if let Some(named) = registry
        .terrain(terrain)
        .and_then(|t| t.battlefield.clone())
        .filter(&is_battle)
    {
        return Some(named);
    }
    let preferred = format!("battle_{terrain}");
    if is_battle(&preferred) {
        return Some(preferred);
    }
    registry
        .maps
        .values()
        .find(|m| m.kind == MapKind::Battle)
        .map(|m| m.id.clone())
}

fn drive_ai(mods: Res<Mods>, mut overworld: ResMut<Overworld>) {
    if overworld.state.over.is_some()
        || !overworld.anim.is_empty()
        || overworld.muster.is_some()
        || overworld.debrief.is_some()
        || overworld.roster
    {
        return;
    }
    let side = overworld.state.active_side;
    let ow = &mut *overworld;
    let Some(planner) = ow.planners.get_mut(&side) else {
        return;
    };
    let order = planner.next_order(&mods.0, &ow.state, side);
    match ow.state.apply(&mods.0, &order) {
        Ok(events) => ow.anim.extend(events),
        Err(_) => {
            // Don't wedge on a stubborn army: mark it moved and continue.
            if let OverworldOrder::MoveArmy { army, .. } = order
                && let Some(a) = ow.state.army_mut(army)
            {
                a.moved = true;
                return;
            }
            if let Ok(events) = ow.state.apply(&mods.0, &OverworldOrder::EndTurn) {
                ow.anim.extend(events);
            }
        }
    }
}

// Save and load need to respawn the world, which means commands, art and the
// camera on top of everything input already wanted. Same reasoning as the
// other two systems carrying this allow: the parameters are the dependency
// list and do not decompose into a smaller noun.
#[allow(clippy::too_many_arguments)]
fn handle_input(
    mut commands: Commands,
    mods: Res<Mods>,
    art: Res<ArtCache>,
    mut overworld: ResMut<Overworld>,
    mut log: ResMut<OwLogLines>,
    mut focus: ResMut<CameraFocus>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    view: map_render::View,
    scoped: Query<Entity, With<OverworldScope>>,
) {
    if overworld.state.over.is_some()
        || !overworld.anim.is_empty()
        || overworld.muster.is_some()
        || overworld.debrief.is_some()
        || overworld.roster
    {
        return;
    }
    let side = overworld.state.active_side;
    if overworld.state.sides[side as usize].ai.is_some() {
        return;
    }

    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::KeyT) {
        overworld.clear_selection();
        let ow = &mut *overworld;
        if let Ok(events) = ow.state.apply(&mods.0, &OverworldOrder::EndTurn) {
            ow.anim.extend(events);
        }
        return;
    }
    if keys.just_pressed(KeyCode::Escape) || buttons.just_pressed(MouseButton::Right) {
        overworld.clear_selection();
        return;
    }
    // F5/F9 save and load the campaign. A single slot, because the point of
    // this first pass is that the state survives at all — slots, naming and a
    // menu belong with the rest of the menu work.
    if keys.just_pressed(KeyCode::F5) {
        let save = SaveGame::new(&mods.0, Some(overworld.state.clone()), None);
        match tactics_core::save::write(save_path(), &save) {
            Ok(()) => log.push(format!("Saved to {}.", save_path().display())),
            Err(e) => log.push(format!("Could not save: {e}")),
        }
        return;
    }
    if keys.just_pressed(KeyCode::F9) {
        match tactics_core::save::read(&mods.0, save_path()) {
            Ok((save, warnings)) => match save.overworld {
                Some(state) => {
                    // Clear the old world first. `spawn_world` also spawns the
                    // HUD, so respawning without despawning leaves two banners
                    // and two log panels — and `update_ui` reads them with
                    // `single_mut()`, which errors on a duplicate and silently
                    // stops updating. The symptom is a campaign that loads
                    // correctly and then appears frozen on the old day, which
                    // is a very slow thing to diagnose from the outside.
                    for entity in &scoped {
                        commands.entity(entity).despawn();
                    }
                    let center = spawn_world(&mut commands, &mods.0, &art, view.rotation(), &state);
                    center_camera(&mut focus, view.rotation(), center);
                    overworld.state = state;
                    overworld.clear_selection();
                    overworld.anim.clear();
                    log.push("Loaded.");
                    // Say so rather than swallowing it: a content patch
                    // between saving and loading can move numbers under the
                    // player's campaign.
                    for warning in warnings {
                        log.push(warning);
                    }
                }
                None => log.push("That save has no campaign in it."),
            },
            Err(e) => log.push(format!("Could not load: {e}")),
        }
        return;
    }
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(hex) = view.hovered(&overworld.state.map) else {
        return;
    };

    // Click own army: select. Click elsewhere with a selection: move order.
    if let Some(army) = overworld.state.army_at(hex).filter(|a| a.side == side) {
        overworld.selected = Some(army.id);
        overworld.refresh_range(&mods.0);
        return;
    }
    if let Some(army) = overworld.selected {
        let ow = &mut *overworld;
        match ow
            .state
            .apply(&mods.0, &OverworldOrder::MoveArmy { army, to: hex })
        {
            Ok(events) => {
                ow.anim.extend(events);
                ow.clear_selection();
            }
            Err(e) => log.push(format!("Can't move there: {e}")),
        }
    }
}

fn sync_armies(
    mods: Res<Mods>,
    overworld: Res<Overworld>,
    view: map_render::View,
    mut markers: Query<(&ArmyMarker, &mut Transform, &mut Visibility, &mut Sprite)>,
    mut chevrons: Query<(&ArmyChevron, &mut Visibility, &mut Sprite), Without<ArmyMarker>>,
) {
    let state = &overworld.state;
    let view_side = state.sides.iter().position(|s| s.ai.is_none()).unwrap_or(0) as u8;
    for (marker, mut transform, mut visibility, mut sprite) in &mut markers {
        let Some(army) = state.army(marker.0) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let elev = state.map.get(army.pos).map(|t| t.elevation).unwrap_or(0);
        let (pos, z) = iso::project(army.pos, elev, view.rotation(), view.center());
        transform.translation = Vec3::new(pos.x, pos.y + 16.0, z + 1.5);
        let seen = state.army_visible_to(&mods.0, army, view_side);
        *visibility = if seen {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        let done = army.side == state.active_side && army.moved;
        sprite.color = if overworld.selected == Some(army.id) {
            Color::srgb(1.3, 1.3, 1.3)
        } else if done {
            Color::srgb(0.55, 0.55, 0.55)
        } else {
            Color::WHITE
        };
    }

    // Whoever is carrying headquarters this morning. Every side gets one,
    // and the enemy's shows only where her army marker is already drawn —
    // the chevron inherits its parent's visibility, so it cannot become a
    // way of finding an army the map is hiding.
    let seniors: Vec<ArmyId> = (0..state.sides.len() as u8)
        .filter_map(|side| state.senior_army(side))
        .collect();
    for (chevron, mut visibility, mut sprite) in &mut chevrons {
        let senior = seniors.contains(&chevron.0);
        *visibility = if senior {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if let Some(army) = state.army(chevron.0) {
            sprite.color = map_render::side_color(army.side);
        }
    }
}

/// Tint the tiles the selected army can reach, mark the enemies it can
/// engage, and follow the cursor with a hover marker.
fn update_range_highlights(
    mut commands: Commands,
    mut overworld: ResMut<Overworld>,
    mods: Res<Mods>,
    art: Res<ArtCache>,
    view: map_render::View,
    existing: Query<Entity, With<OwRangeTile>>,
    mut hover: Query<(&mut Transform, &mut Visibility), With<OwHoverTile>>,
) {
    let map = overworld.state.map.clone();
    let face_at = |hex: Hex| view.face_at(&map, hex);

    if let Ok((mut transform, mut visibility)) = hover.single_mut() {
        match view.hovered(&map) {
            Some(hex) => {
                transform.translation = face_at(hex);
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }

    if !overworld.range_dirty {
        return;
    }
    overworld.range_dirty = false;
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    let mut tint = |hex: Hex, color: Color| {
        let overlay = HexOverlay::face(hex);
        commands.spawn((
            Sprite {
                image: art.face.clone(),
                color,
                ..default()
            },
            Transform::from_translation(overlay.translation(&map, view.rotation(), view.center())),
            overlay,
            OwRangeTile,
            OverworldScope,
        ));
    };
    for hex in overworld.move_range.keys() {
        tint(*hex, Color::srgba(0.35, 0.55, 1.0, 0.35));
    }
    for id in &overworld.attack_targets {
        if let Some(enemy) = overworld.state.army(*id) {
            tint(enemy.pos, Color::srgba(1.0, 0.3, 0.25, 0.45));
        }
    }

    // The wire, over the move range rather than under it: a day's drive and a
    // radio horizon are both a handful of hexes out, so on this map the two
    // land on each other constantly, and the ring is the thinner and more
    // precise of the two. It says how far this army may be *sent* before it
    // stops answering, which is a different question from how far it can
    // drive today and is often the binding one.
    //
    // Drawn round the selection rather than round headquarters because
    // `relay` is on — every army in contact retransmits at its own radius, so
    // this really is her horizon — and because selection is own-side only,
    // which keeps the enemy's net off the screen without a rule having to say
    // so.
    let ring: Vec<Hex> = net_radius(&mods.0)
        .zip(overworld.selected.and_then(|id| overworld.state.army(id)))
        .map(|(radius, army)| {
            army.pos
                .ring(radius)
                .filter(|hex| map.get(*hex).is_some())
                .collect()
        })
        .unwrap_or_default();
    for hex in ring {
        let overlay = HexOverlay::face_over(hex);
        commands.spawn((
            Sprite {
                image: art.face.clone(),
                color: OW_NET_RING,
                ..default()
            },
            Transform::from_translation(overlay.translation(&map, view.rotation(), view.center())),
            overlay,
            OwRangeTile,
            OverworldScope,
        ));
    }
}

/// How far a campaign radio carries, in overworld hexes, or `None` for a mod
/// that prices no chain of command.
///
/// The campaign's own number rather than the battle's, because an overworld
/// hex is forty battle hexes and one figure serving both would leave
/// headquarters either deaf on the map or omniscient on the field. One reader
/// for the ring and for the panel line, so the two cannot disagree about
/// whether there is a net at all.
fn net_radius(registry: &tactics_core::data::DataRegistry) -> Option<u32> {
    registry.command.as_ref().map(|r| r.overworld_radius)
}

fn update_owner_dots(
    overworld: Res<Overworld>,
    view: map_render::View,
    mut dots: Query<(&OwnerDot, &mut Transform, &mut Sprite)>,
) {
    let state = &overworld.state;
    for (dot, mut transform, mut sprite) in &mut dots {
        let elev = state.map.get(dot.0).map(|t| t.elevation).unwrap_or(0);
        let (pos, z) = iso::project(dot.0, elev, view.rotation(), view.center());
        transform.translation = Vec3::new(pos.x + 18.0, pos.y + 8.0, z + 1.2);
        sprite.color = match state.owners.get(&dot.0) {
            Some(side) => map_render::side_color(*side),
            None => Color::srgba(1.0, 1.0, 1.0, 0.25),
        };
    }
}

fn update_ui(
    overworld: Res<Overworld>,
    mods: Res<Mods>,
    log: Res<OwLogLines>,
    view: map_render::View,
    mut hud: OverworldHud,
    mut warned: Local<bool>,
) {
    map_render::warn_if_duplicated(hud.banner.iter().count(), "overworld banner", &mut warned);

    let state = &overworld.state;
    if let Ok(mut text) = hud.banner.single_mut() {
        let side = &state.sides[state.active_side as usize];
        let controller = if side.ai.is_some() { "AI" } else { "You" };
        text.0 = format!(
            "Day {} - {} ({controller}) - Funds {}",
            state.turn, side.name, side.funds
        );
    }
    if let Ok(mut text) = hud.log_text.single_mut() {
        text.0 = log.0.iter().cloned().collect::<Vec<_>>().join("\n");
    }
    let Ok(mut text) = hud.panel.single_mut() else {
        return;
    };
    let view_side = state.sides.iter().position(|s| s.ai.is_none()).unwrap_or(0) as u8;
    let hovered = view.hovered(&state.map);

    // Whatever is under the cursor wins, so the player can read the board
    // without losing their selection.
    let hovered_army = hovered
        .and_then(|hex| state.army_at(hex))
        .filter(|a| state.army_visible_to(&mods.0, a, view_side))
        .map(|a| a.id);
    if let Some(army) = hovered_army
        .or(overworld.selected)
        .and_then(|id| state.army(id))
    {
        let mut lines = vec![
            army.name.clone(),
            format!("Side: {}", state.sides[army.side as usize].name),
            format!(
                "Movement: {} ({} hexes){}",
                mods.0.scale.format_overworld_distance(army.movement),
                army.movement,
                if army.moved { " (spent)" } else { "" }
            ),
        ];
        // Standing orders and the wire, above the roster: what an army has
        // been told and whether it can be told anything else are the two
        // facts a player picks a move on.
        lines.push(match &army.mission {
            Some(ArmyMission::Advance { to }) => format!("Orders: advance on {}", place(*to)),
            Some(ArmyMission::Hold) => "Orders: hold".into(),
            Some(ArmyMission::Withdraw { to }) => format!("Orders: fall back on {}", place(*to)),
            None => "Orders: none".into(),
        });
        // The number behind the ring on the map, for the same reason the
        // battle panel names its own: "out of radio contact" says the wire is
        // dead, and only the radius says how far she had to drive to kill it.
        if let Some(radius) = net_radius(&mods.0) {
            lines.push(format!(
                "Radio: {radius} hexes ({})",
                mods.0.scale.format_overworld_distance(radius)
            ));
        }
        if !state.in_contact(army.id) {
            lines.push("Out of radio contact".into());
        }
        // An order given but not yet sent is its own line rather than a
        // rewrite of the one above: "what she is doing" and "what she is about
        // to be told" are different facts, and a panel that showed only the
        // second would have the player watching an army ignore orders it has
        // never heard.
        if let Some((_, mission)) = state.waiting_missions.iter().find(|(id, _)| *id == army.id) {
            lines.push(match mission {
                ArmyMission::Advance { to } => {
                    format!("Waiting to transmit: advance on {}", place(*to))
                }
                ArmyMission::Hold => "Waiting to transmit: hold".into(),
                ArmyMission::Withdraw { to } => {
                    format!("Waiting to transmit: fall back on {}", place(*to))
                }
            });
        }
        lines.push("Units:".into());
        for u in &army.units {
            let vehicle = mods
                .0
                .vehicle(&u.vehicle)
                .map(|v| v.name.clone())
                .unwrap_or_else(|| u.vehicle.clone());
            let commander = u
                .crew
                .first()
                .and_then(|id| state.roster.get(*id))
                .map(|cadet| cadet.name.clone())
                .unwrap_or_default();
            lines.push(format!("  {vehicle} - {commander}"));
        }
        lines.push(String::new());
        // The tile under the cursor, so terrain can be inspected without
        // dropping the selection.
        lines.push(describe_tile(&mods.0, state, hovered.unwrap_or(army.pos)));
        text.0 = lines.join("\n");
        return;
    }
    if let Some(hex) = hovered {
        text.0 = describe_tile(&mods.0, state, hex);
        return;
    }
    text.0 = "Hover a tile for terrain\n\nLMB: select/move\nEnter: end day\nR: the academy roll\nQ/E: rotate view\n\nMove onto an enemy army\nto start a battle.".into();
}

/// What a strategic tile is worth and what it costs to cross.
fn describe_tile(
    registry: &tactics_core::data::DataRegistry,
    state: &OverworldState,
    hex: Hex,
) -> String {
    let Some(tile) = state.map.get(hex) else {
        return String::new();
    };
    let Some(terrain) = registry.terrain(&tile.terrain) else {
        return tile.terrain.clone();
    };
    let mut lines = vec![format!("{} (elev {})", terrain.name, tile.elevation)];
    match terrain.cost_for(tactics_core::data::MovementClass::Tracked) {
        Some(cost) => lines.push(format!("Move cost {cost}")),
        None => lines.push("Impassable".into()),
    }
    if terrain.cover > 0 {
        lines.push(format!("Cover {}% in battle", terrain.cover));
    }
    if terrain.concealing {
        lines.push("Hides armies from afar".into());
    }
    if terrain.capturable {
        let owner = match state.owners.get(&hex) {
            Some(side) => state
                .sides
                .get(*side as usize)
                .map(|s| s.name.clone())
                .unwrap_or_default(),
            None => "unclaimed".into(),
        };
        lines.push(format!("Objective: {owner}"));
        if terrain.income > 0 {
            lines.push(format!("Income {}/day", terrain.income));
        }
    }
    lines.join("\n")
}

fn apply_campaign_commands(
    mut commands: Commands,
    campaign: Option<NonSendMut<Campaign>>,
    mut overworld: ResMut<Overworld>,
    mut log: ResMut<OwLogLines>,
    mut next: ResMut<NextState<AppState>>,
) {
    let Some(campaign) = campaign else { return };
    for command in campaign.drain_commands() {
        match command {
            CampaignCommand::Message(text) => log.push(format!("[Campaign] {text}")),
            CampaignCommand::SetFunds { side, amount } => {
                if let Some(s) = overworld.state.sides.get_mut(side as usize) {
                    s.funds = amount;
                }
            }
            CampaignCommand::GiveFunds { side, amount } => {
                if let Some(s) = overworld.state.sides.get_mut(side as usize) {
                    s.funds += amount;
                }
            }
            CampaignCommand::StartBattle { map_id } => {
                commands.insert_resource(PendingBattle::Scenario { map_id });
                next.set(AppState::Battle);
            }
        }
    }
}

fn leave_overworld(mut commands: Commands, scoped: Query<Entity, With<OverworldScope>>) {
    for entity in &scoped {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<CurrentMap>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tactics_core::roster::CrewFate;

    fn registry() -> tactics_core::data::DataRegistry {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
        tactics_core::data::DataRegistry::load_dir(&root)
            .expect("mods load")
            .0
    }

    /// The roll accounts for every cadet in the academy exactly once, says
    /// where each of them sits, and says who cannot go out.
    ///
    /// Three separate promises, and each of them is a way this screen would
    /// silently lie. A cadet listed twice makes the school look bigger than
    /// it is; a cadet listed nowhere reads as dead; and a seat left unnamed
    /// takes away the one thing this page can tell a player that no other
    /// screen does — who is driving.
    #[test]
    fn the_roll_accounts_for_every_cadet_in_the_academy() {
        let reg = registry();
        let mut state = OverworldState::from_map(&reg, "frontier", 5).expect("overworld");
        let page = roster_page(&reg, &state, 0);
        let body = page.lines().join("\n");

        let all: Vec<String> = state.roster.of_side(0).map(|c| c.name.clone()).collect();
        assert!(!all.is_empty(), "frontier should field an academy");
        for name in &all {
            assert_eq!(
                body.matches(name.as_str()).count(),
                1,
                "{name} appears other than exactly once on the roll:\n{body}"
            );
        }
        assert!(
            page.summary.contains(&format!("{} cadets", all.len())),
            "the summary should count the school: {}",
            page.summary
        );
        // Seats, by the name the mod gives the role rather than a slot index.
        assert!(
            body.contains("Commander") && body.contains("Driver"),
            "the roll does not say who sits where:\n{body}"
        );
        // Nobody else's school is on the player's page.
        for name in state.roster.of_side(1).map(|c| c.name.clone()) {
            assert!(
                !body.contains(&name),
                "{name} fights for the other academy and is on our roll:\n{body}"
            );
        }

        // A cadet who is hurt reads as unavailable, in the roll's own words
        // rather than the after-action report's — this page answers "can I
        // fight tomorrow", not "what happened today".
        let hurt = state.roster.of_side(0).map(|c| c.id).next().unwrap();
        let name = state.roster.get(hurt).unwrap().name.clone();
        state.roster.get_mut(hurt).unwrap().status =
            tactics_core::roster::CadetStatus::Wounded { days: 4 };
        let body = roster_page(&reg, &state, 0).lines().join("\n");
        let line = body
            .lines()
            .find(|l| l.contains(&name))
            .expect("she is still on the roll");
        assert!(
            line.contains("infirmary") && line.contains('4'),
            "a cadet in the infirmary reads as fit: {line}"
        );
        assert!(
            roster_page(&reg, &state, 0)
                .summary
                .contains(&format!("{} fit", state.roster.of_side(0).count() - 1)),
            "the summary should count her out"
        );
    }

    /// A cadet whose vehicle did not come home is still somebody the academy
    /// has.
    ///
    /// The case the `unposted` section exists for, and the reason it is not
    /// an edge case worth skipping: she survives her tank far more often than
    /// not, and a roll that only walked the order of battle would drop her
    /// from the school on the day she most needs to be on it.
    #[test]
    fn a_cadet_without_a_vehicle_is_still_on_the_roll() {
        let reg = registry();
        let mut state = OverworldState::from_map(&reg, "frontier", 5).expect("overworld");
        let army = state.side_armies(0).next().unwrap().id;
        let orphaned: Vec<String> = state.army(army).unwrap().units[0]
            .crew
            .iter()
            .filter_map(|id| state.roster.get(*id))
            .map(|c| c.name.clone())
            .collect();
        assert!(!orphaned.is_empty(), "the first vehicle should be crewed");
        state.army_mut(army).unwrap().units.remove(0);

        let page = roster_page(&reg, &state, 0);
        let body = page.lines().join("\n");
        for name in &orphaned {
            assert!(
                page.unposted.iter().any(|l| l.contains(name.as_str())),
                "{name} lost her vehicle and fell off the roll:\n{body}"
            );
            assert_eq!(
                body.matches(name.as_str()).count(),
                1,
                "{name} is on the roll twice:\n{body}"
            );
        }
    }

    /// The after-action report names the player's own cadets, worst first, and
    /// says nothing about anybody else's.
    ///
    /// The ordering is the part worth pinning. The campaign resolves fates in
    /// cadet-id order because the rng demands it, which means without a sort
    /// here the one line the player most needs — somebody died — turns up
    /// wherever the roster happened to put her, under four lines about
    /// bruises.
    #[test]
    fn the_debrief_names_our_own_casualties_worst_first() {
        let reg = registry();
        let state = OverworldState::from_map(&reg, "frontier", 5).expect("overworld");
        let ours: Vec<CadetId> = state.roster.of_side(0).map(|g| g.id).take(3).collect();
        let theirs = state
            .roster
            .of_side(1)
            .map(|g| g.id)
            .next()
            .expect("side 1 has cadets");
        assert!(ours.len() >= 3, "frontier should field enough cadets");

        let events = vec![
            OverworldEvent::CrewCasualty {
                cadet: ours[0],
                fate: CrewFate::Wounded { days: 2 },
            },
            OverworldEvent::CrewCasualty {
                cadet: ours[1],
                fate: CrewFate::Killed,
            },
            OverworldEvent::CrewCasualty {
                cadet: ours[2],
                fate: CrewFate::Unharmed,
            },
            OverworldEvent::CrewCasualty {
                cadet: theirs,
                fate: CrewFate::Killed,
            },
        ];
        let report = after_action(&state, 0, "Battle won by us.".into(), &events, &[]);

        assert_eq!(
            report.cost.len(),
            2,
            "the unharmed cadet and the enemy's dead do not belong on our bill: {:#?}",
            report.cost
        );
        let dead = state.roster.get(ours[1]).unwrap().name.clone();
        assert!(
            report.cost[0].contains(&dead),
            "the worst news goes first, got {:#?}",
            report.cost
        );
        let enemy = state.roster.get(theirs).unwrap().name.clone();
        assert!(
            !report.cost.iter().any(|line| line.contains(&enemy)),
            "the enemy's losses are not our casualty list"
        );
    }

    /// A cadet who came home fit is listed as having come home, and a cadet who
    /// was hurt is not listed twice.
    #[test]
    fn a_girl_is_on_one_side_of_the_ledger_or_the_other() {
        let reg = registry();
        let state = OverworldState::from_map(&reg, "frontier", 5).expect("overworld");
        let army = state.side_armies(0).next().expect("side 0 has an army");
        let units = army.units.clone();
        let hurt = units[0].crew[0];

        let events = vec![OverworldEvent::CrewCasualty {
            cadet: hurt,
            fate: CrewFate::Wounded { days: 4 },
        }];
        let survivors = vec![(army.id, units.clone())];
        let report = after_action(&state, 0, "Battle won by us.".into(), &events, &survivors);

        let name = state.roster.get(hurt).unwrap().name.clone();
        assert!(
            report.cost.iter().any(|line| line.contains(&name)),
            "she is on the bill"
        );
        assert!(
            !report.home.iter().any(|line| line.contains(&name)),
            "and therefore not also in the roll call of who came home fit"
        );
        assert!(
            !report.home.is_empty(),
            "everybody else in her army came home and the page should say so"
        );
    }

    /// The campaign can stage the fights its own armies would cause.
    ///
    /// `launch_battle` now asks before it commits anybody, so a campaign whose
    /// content has gone missing declines one clash instead of taking the run
    /// down inside `spawn_unit`. The check is only worth having if the shipped
    /// content passes it, and this is where that is said: every pair of
    /// hostile armies on the campaign map, on the map their ground would pick,
    /// with the campaign's own roster.
    #[test]
    fn every_clash_the_campaign_map_can_produce_can_be_staged() {
        let reg = registry();
        let state = OverworldState::from_map(&reg, "frontier", 5).expect("overworld");
        let sides: Vec<SideState> = state
            .sides
            .iter()
            .map(|s| SideState {
                name: s.name.clone(),
                ai: s.ai.clone(),
            })
            .collect();
        let roster = std::sync::Arc::new(state.roster.clone());
        let terrain = state
            .map
            .get(state.armies[0].pos)
            .map(|t| t.terrain.clone())
            .unwrap_or_default();
        let map_id = choose_battle_map(&reg, &terrain).expect("the base mod ships a battle map");

        let mut clashes = 0;
        for attacker in &state.armies {
            for defender in state.armies.iter().filter(|d| d.side != attacker.side) {
                let forces: Vec<BattleForce> = [attacker, defender]
                    .iter()
                    .map(|a| BattleForce {
                        army: a.id,
                        side: a.side,
                        units: a.units.clone(),
                        mission: a.mission.clone(),
                    })
                    .collect();
                assert_eq!(
                    crate::battle::field_battle_problem(
                        &reg,
                        &map_id,
                        &sides,
                        attacker.side,
                        &forces,
                        &roster,
                    ),
                    None,
                    "{} attacking {} cannot be staged",
                    attacker.name,
                    defender.name
                );
                clashes += 1;
            }
        }
        assert!(clashes > 0, "this test is meaningless without two sides");
    }

    /// A campaign with no battle map declines the fight rather than crashing.
    ///
    /// `choose_battle_map` used to `expect` its way past this on the reasoning
    /// that the base mod always ships one — true of the base mod, and nothing
    /// at all about the mod somebody loads on top of it.
    #[test]
    fn a_campaign_with_no_battlefield_to_fight_on_says_so_instead_of_panicking() {
        let reg = registry();
        assert!(
            choose_battle_map(&reg, "grass").is_some(),
            "the base mod ships battle maps, so this fixture is right way up"
        );
        let bare = tactics_core::data::DataRegistry::default();
        assert_eq!(
            choose_battle_map(&bare, "grass"),
            None,
            "with no maps loaded there is nowhere to fight"
        );
    }

    /// Every terrain the campaign map is made of names the battlefield a
    /// clash on it is fought over, and does so *itself*.
    ///
    /// The third arm of `choose_battle_map` — any battle map at all — is a
    /// shrug, and until the `battlefield` field existed the shipped campaign
    /// reached it constantly: `deep_forest`, `city`, `factory`, `highway` and
    /// `mountains` all name no `battle_<terrain>` map, so a fight in the
    /// mountains was resolved on whichever battlefield the map table happened
    /// to iterate first. That is not a wrong answer anybody could see, which
    /// is exactly why it wants a test rather than a glance.
    ///
    /// Written against the terrain the campaign map actually *uses* rather
    /// than against the roster, because a terrain nobody has put on a map is
    /// allowed to have no battlefield yet.
    #[test]
    fn every_terrain_the_campaign_fields_names_its_own_battlefield() {
        let reg = registry();
        let file = reg
            .map("frontier")
            .expect("the base mod ships a campaign map");
        let map = tactics_core::map::HexMap::from_map_file(file).expect("campaign map parses");
        let mut terrains: Vec<String> = map.iter().map(|(_, t)| t.terrain.clone()).collect();
        terrains.sort();
        terrains.dedup();
        assert!(terrains.len() > 1, "a one-terrain campaign proves nothing");
        for terrain in &terrains {
            let def = reg
                .terrain(terrain)
                .unwrap_or_else(|| panic!("the campaign map stands on `{terrain}`"));
            let named = def.battlefield.clone().unwrap_or_else(|| {
                panic!(
                    "terrain `{terrain}` is on the campaign map and names no battlefield, so a                      clash there falls through to whichever battle map iterates first"
                )
            });
            assert!(
                reg.maps
                    .get(&named)
                    .is_some_and(|m| m.kind == MapKind::Battle),
                "terrain `{terrain}` is fought on `{named}`, which is not a battle map"
            );
            assert_eq!(
                choose_battle_map(&reg, terrain).as_deref(),
                Some(named.as_str()),
                "a clash on `{terrain}` must be fought where the terrain says"
            );
        }
    }
}
