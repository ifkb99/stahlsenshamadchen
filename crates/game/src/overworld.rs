//! The overworld screen: strategic army movement, objective capture,
//! income, soft fog, and handing off clashes to the battle screen.

use crate::battle::{BattleForce, BattleOutcome, PendingBattle};
use crate::campaign::{self, Campaign, CampaignCommand};
use crate::camera::CameraFocus;
use crate::iso::{self, ArtCache, ViewCenter, ViewRotation};
use crate::map_render::{self, CurrentMap, HexOverlay};
use crate::mods::Mods;
use crate::AppState;
use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};
use tactics_core::ai::AiPlanner;
use tactics_core::battle::SideState;
use tactics_core::map::MapKind;
use tactics_core::overworld::{
    make_overworld_planner, ArmyId, OverworldEvent, OverworldOrder, OverworldState,
};
use tactics_core::Hex;

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

#[derive(Component)]
struct OwBanner;

#[derive(Component)]
struct OwLog;

#[derive(Component)]
struct OwPanel;

#[derive(Component)]
struct MusterPanel;

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
        app.init_resource::<OwLogLines>()
            .add_systems(OnEnter(AppState::Overworld), enter_overworld)
            .add_systems(
                Update,
                (
                    pump_events,
                    drive_ai,
                    muster_input,
                    handle_input,
                    sync_armies,
                    update_owner_dots,
                    update_range_highlights,
                    update_ui,
                    update_muster_ui,
                    apply_campaign_commands,
                )
                    .chain()
                    .run_if(in_state(AppState::Overworld)),
            )
            .add_systems(OnExit(AppState::Overworld), leave_overworld);
    }
}

fn enter_overworld(
    mut commands: Commands,
    mods: Res<Mods>,
    art: Res<ArtCache>,
    rotation: Res<ViewRotation>,
    mut center: ResMut<ViewCenter>,
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
                outcome.attacker,
                outcome.defender,
                &outcome.survivors,
            );
            ow.anim.extend(events);
            match (outcome.winner, outcome.stalemate) {
                (Some(w), _) => {
                    log.push(format!("Battle won by {}.", ow.state.sides[w as usize].name))
                }
                (None, true) => log.push("Neither side could find the other. Both withdrew."),
                (None, false) => log.push("The battle ended in mutual ruin."),
            }
            if let Some(campaign) = campaign {
                campaign::call_battle_end_hook(&campaign, &ow.state, outcome.winner);
            }
            commands.remove_resource::<BattleOutcome>();
        }
        ow.clear_selection();
        spawn_world(&mut commands, registry, &art, rotation.0, &mut center, &ow.state);
        center_camera(&mut focus, rotation.0, center.0);
        return;
    }

    // Fresh campaign: pick the first overworld map from the mods.
    let map_id = registry
        .maps
        .values()
        .find(|m| m.kind == MapKind::Overworld)
        .map(|m| m.id.clone())
        .expect("base mod provides an overworld map");
    let state = OverworldState::from_map(registry, &map_id).expect("overworld builds");
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

    spawn_world(&mut commands, registry, &art, rotation.0, &mut center, &state);
    center_camera(&mut focus, rotation.0, center.0);

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
    });
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
    center: &mut ViewCenter,
    state: &OverworldState,
) {
    center.0 = state.map.center();
    map_render::spawn_map(commands, art, &state.map, rotation, false, OverworldScope);
    commands.insert_resource(CurrentMap(state.map.clone()));

    for army in state.armies.iter().filter(|a| a.alive) {
        commands.spawn((
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
        ));
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
        if registry.terrain(&tile.terrain).is_some_and(|t| t.capturable) {
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
    if overworld.muster.is_some() {
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
            if *side == 0 {
                if let Some(campaign) = &campaign {
                    campaign::call_turn_hook(campaign, &overworld.state);
                }
            }
        }
        OverworldEvent::Income { side, amount } => {
            let name = &overworld.state.sides[*side as usize].name;
            log.push(format!("{name} collects {amount} funds."));
        }
        OverworldEvent::ArmyMoved { .. } => {}
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
        OverworldEvent::BattleTriggered { attacker, defender, at } => {
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
            let map_id = choose_battle_map(registry, &terrain);

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
        OverworldEvent::GameEnded { winner } => match winner {
            Some(w) => log.push(format!(
                "Campaign over. {} rules the frontier.",
                overworld.state.sides[*w as usize].name
            )),
            None => log.push("Campaign over. Nobody is left standing."),
        },
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
fn launch_battle(
    commands: &mut Commands,
    state: &mut OverworldState,
    next: &mut NextState<AppState>,
    attacker: ArmyId,
    defender: ArmyId,
    joiners: &[ArmyId],
    map_id: String,
) {
    state.commit_to_battle(joiners);
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
            });
        }
    }

    commands.insert_resource(PendingBattle::Field {
        map_id,
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
    joiners.extend(muster.choices.iter().filter(|(_, on)| *on).map(|(id, _)| *id));
    if !joiners.is_empty() {
        log.push(format!("{} more armies join the fight.", joiners.len()));
    }
    launch_battle(
        &mut commands,
        &mut overworld.state,
        &mut next,
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
    let role = if muster.attacking { "assault" } else { "defence" };
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
    lines.push(String::new());
    lines.push("1-9 toggle  M all  N none  Enter fight".into());
    text.0 = lines.join("\n");
}

/// Prefer a battle map named `battle_<terrain>`, fall back to any battle map.
fn choose_battle_map(registry: &tactics_core::data::DataRegistry, terrain: &str) -> String {
    let preferred = format!("battle_{terrain}");
    if registry.maps.contains_key(&preferred) {
        return preferred;
    }
    registry
        .maps
        .values()
        .find(|m| m.kind == MapKind::Battle)
        .map(|m| m.id.clone())
        .expect("base mod provides a battle map")
}

fn drive_ai(mods: Res<Mods>, mut overworld: ResMut<Overworld>) {
    if overworld.state.over.is_some() || !overworld.anim.is_empty() || overworld.muster.is_some() {
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
            if let OverworldOrder::MoveArmy { army, .. } = order {
                if let Some(a) = ow.state.army_mut(army) {
                    a.moved = true;
                    return;
                }
            }
            if let Ok(events) = ow.state.apply(&mods.0, &OverworldOrder::EndTurn) {
                ow.anim.extend(events);
            }
        }
    }
}

fn handle_input(
    mods: Res<Mods>,
    mut overworld: ResMut<Overworld>,
    mut log: ResMut<OwLogLines>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    rotation: Res<ViewRotation>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
) {
    if overworld.state.over.is_some() || !overworld.anim.is_empty() || overworld.muster.is_some() {
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
    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(hex) = map_render::hovered_tile(&windows, &camera, &overworld.state.map, rotation.0)
    else {
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
    rotation: Res<ViewRotation>,
    center: Res<ViewCenter>,
    mut markers: Query<(&ArmyMarker, &mut Transform, &mut Visibility, &mut Sprite)>,
) {
    let state = &overworld.state;
    let view_side = state
        .sides
        .iter()
        .position(|s| s.ai.is_none())
        .unwrap_or(0) as u8;
    for (marker, mut transform, mut visibility, mut sprite) in &mut markers {
        let Some(army) = state.army(marker.0) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let elev = state.map.get(army.pos).map(|t| t.elevation).unwrap_or(0);
        let (pos, z) = iso::project(army.pos, elev, rotation.0, center.0);
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
}

/// Tint the tiles the selected army can reach, mark the enemies it can
/// engage, and follow the cursor with a hover marker.
fn update_range_highlights(
    mut commands: Commands,
    mut overworld: ResMut<Overworld>,
    art: Res<ArtCache>,
    rotation: Res<ViewRotation>,
    center: Res<ViewCenter>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    existing: Query<Entity, With<OwRangeTile>>,
    mut hover: Query<(&mut Transform, &mut Visibility), With<OwHoverTile>>,
) {
    let map = overworld.state.map.clone();
    let face_at = |hex: Hex| HexOverlay::face(hex).translation(&map, rotation.0, center.0);

    if let Ok((mut transform, mut visibility)) = hover.single_mut() {
        match map_render::hovered_tile(&windows, &camera, &map, rotation.0) {
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
            Transform::from_translation(overlay.translation(&map, rotation.0, center.0)),
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
}

fn update_owner_dots(
    overworld: Res<Overworld>,
    rotation: Res<ViewRotation>,
    center: Res<ViewCenter>,
    mut dots: Query<(&OwnerDot, &mut Transform, &mut Sprite)>,
) {
    let state = &overworld.state;
    for (dot, mut transform, mut sprite) in &mut dots {
        let elev = state.map.get(dot.0).map(|t| t.elevation).unwrap_or(0);
        let (pos, z) = iso::project(dot.0, elev, rotation.0, center.0);
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
    rotation: Res<ViewRotation>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    mut banner: Query<&mut Text, (With<OwBanner>, Without<OwLog>, Without<OwPanel>)>,
    mut log_text: Query<&mut Text, (With<OwLog>, Without<OwBanner>, Without<OwPanel>)>,
    mut panel: Query<&mut Text, (With<OwPanel>, Without<OwBanner>, Without<OwLog>)>,
) {
    let state = &overworld.state;
    if let Ok(mut text) = banner.single_mut() {
        let side = &state.sides[state.active_side as usize];
        let controller = if side.ai.is_some() { "AI" } else { "You" };
        text.0 = format!(
            "Day {} - {} ({controller}) - Funds {}",
            state.turn, side.name, side.funds
        );
    }
    if let Ok(mut text) = log_text.single_mut() {
        text.0 = log.0.iter().cloned().collect::<Vec<_>>().join("\n");
    }
    let Ok(mut text) = panel.single_mut() else {
        return;
    };
    let view_side = state
        .sides
        .iter()
        .position(|s| s.ai.is_none())
        .unwrap_or(0) as u8;
    let hovered = map_render::hovered_tile(&windows, &camera, &state.map, rotation.0);

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
                "Movement: {}{}",
                army.movement,
                if army.moved { " (spent)" } else { "" }
            ),
            "Units:".into(),
        ];
        for u in &army.units {
            let vehicle = mods
                .0
                .vehicle(&u.vehicle)
                .map(|v| v.name.clone())
                .unwrap_or_else(|| u.vehicle.clone());
            let commander = u
                .crew
                .first()
                .and_then(|c| mods.0.character(c))
                .map(|c| c.name.clone())
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
    text.0 = "Hover a tile for terrain\n\nLMB: select/move\nEnter: end day\nQ/E: rotate view\n\nMove onto an enemy army\nto start a battle.".into();
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
