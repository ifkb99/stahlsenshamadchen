//! The overworld screen: strategic army movement, objective capture,
//! income, soft fog, and handing off clashes to the battle screen.

use crate::battle::{BattleOutcome, PendingBattle};
use crate::campaign::{self, Campaign, CampaignCommand};
use crate::iso::{self, ArtCache, ViewCenter, ViewRotation};
use crate::map_render::{self, CurrentMap};
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
}

#[derive(Component, Clone)]
struct OverworldScope;

#[derive(Component)]
struct ArmyMarker(ArmyId);

#[derive(Component)]
struct OwnerDot(Hex);

#[derive(Component)]
struct OwBanner;

#[derive(Component)]
struct OwLog;

#[derive(Component)]
struct OwPanel;

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
                    handle_input,
                    sync_armies,
                    update_owner_dots,
                    update_ui,
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
    mut camera: Query<&mut Transform, With<Camera2d>>,
) {
    let registry = &mods.0;

    if let Some(mut ow) = existing {
        // Returning from a battle: fold the outcome into the campaign.
        if let Some(outcome) = outcome {
            let events = ow.state.apply_battle_result(
                outcome.attacker,
                outcome.defender,
                outcome.attacker_survivors.clone(),
                outcome.defender_survivors.clone(),
            );
            ow.anim.extend(events);
            match outcome.winner {
                Some(w) => log.push(format!("Battle won by {}.", ow.state.sides[w as usize].name)),
                None => log.push("The battle ended in mutual ruin."),
            }
            if let Some(campaign) = campaign {
                campaign::call_battle_end_hook(&campaign, &ow.state, outcome.winner);
            }
            commands.remove_resource::<BattleOutcome>();
        }
        ow.selected = None;
        spawn_world(&mut commands, registry, &art, rotation.0, &mut center, &ow.state);
        center_camera(&mut camera, rotation.0, center.0);
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
                };
                planners.insert(i as u8, make_overworld_planner(&cfg, 1337 + i as u64));
            }
            None => {}
        }
    }

    spawn_world(&mut commands, registry, &art, rotation.0, &mut center, &state);
    center_camera(&mut camera, rotation.0, center.0);

    if let Some(campaign) = campaign {
        campaign::call_start_hook(&campaign, &state);
    }
    commands.insert_resource(Overworld {
        state,
        planners,
        anim: VecDeque::new(),
        pace: Timer::from_seconds(0.3, TimerMode::Repeating),
        selected: None,
    });
}

fn center_camera(
    camera: &mut Query<&mut Transform, With<Camera2d>>,
    rotation: u32,
    center: Hex,
) {
    if let Ok(mut cam) = camera.single_mut() {
        let (pos, _) = iso::project(center, 0, rotation, center);
        cam.translation.x = pos.x;
        cam.translation.y = pos.y;
    }
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
            right: Val::Px(0.0),
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
            let terrain = overworld
                .state
                .map
                .get(*at)
                .map(|t| t.terrain.clone())
                .unwrap_or_default();
            let map_id = choose_battle_map(registry, &terrain);
            let sides: Vec<SideState> = overworld
                .state
                .sides
                .iter()
                .map(|s| SideState {
                    name: s.name.clone(),
                    ai: s.ai.clone(),
                })
                .collect();
            commands.insert_resource(PendingBattle::Field {
                map_id,
                attacker: *attacker,
                defender: *defender,
                sides,
                attacker_units: att.units.clone(),
                defender_units: def.units.clone(),
                attacker_side: att.side,
                defender_side: def.side,
            });
            next.set(AppState::Battle);
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
    if overworld.state.over.is_some() || !overworld.anim.is_empty() {
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
    if overworld.state.over.is_some() || !overworld.anim.is_empty() {
        return;
    }
    let side = overworld.state.active_side;
    if overworld.state.sides[side as usize].ai.is_some() {
        return;
    }

    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::KeyT) {
        overworld.selected = None;
        let ow = &mut *overworld;
        if let Ok(events) = ow.state.apply(&mods.0, &OverworldOrder::EndTurn) {
            ow.anim.extend(events);
        }
        return;
    }
    if keys.just_pressed(KeyCode::Escape) || buttons.just_pressed(MouseButton::Right) {
        overworld.selected = None;
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
                ow.selected = None;
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
    if let Ok(mut text) = panel.single_mut() {
        match overworld.selected.and_then(|id| state.army(id)) {
            Some(army) => {
                let mut lines = vec![
                    army.name.clone(),
                    format!("Side: {}", state.sides[army.side as usize].name),
                    format!("Movement: {}", army.movement),
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
                text.0 = lines.join("\n");
            }
            None => {
                text.0 = "Select an army\n\nLMB: select/move\nEnter: end day\nQ/E: rotate view\n\nMove onto an enemy army\nto start a battle.".into();
            }
        }
    }
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
