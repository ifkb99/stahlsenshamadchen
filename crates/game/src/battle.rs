//! The battle screen: renders a `tactics_core` battle, feeds it player
//! orders, animates the resulting events, and drives AI sides.

use crate::camera::CameraFocus;
use crate::iso::{self, ArtCache, ViewCenter, ViewRotation};
use crate::map_render::{self, CurrentMap, FogOverlay};
use crate::mods::Mods;
use crate::AppState;
use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};
use tactics_core::ai::{make_battle_planner, AiPlanner};
use tactics_core::battle::{
    reachable, BattleState, Event as BattleEvent, Order, SideState, UnitId,
};
use tactics_core::map::UnitPlacement;
use tactics_core::overworld::ArmyId;
use tactics_core::Hex;

/// Why we are entering the battle state; set before switching to it.
#[derive(Resource, Clone)]
pub enum PendingBattle {
    /// A scenario map with its own unit placements (campaign/demo).
    Scenario { map_id: String },
    /// Two overworld armies clashing on a terrain-picked battle map.
    Field {
        map_id: String,
        attacker: ArmyId,
        defender: ArmyId,
        sides: Vec<SideState>,
        attacker_units: Vec<UnitPlacement>,
        defender_units: Vec<UnitPlacement>,
        attacker_side: u8,
        defender_side: u8,
    },
}

/// Reported back to the overworld when a field battle ends.
#[derive(Resource, Clone)]
pub struct BattleOutcome {
    pub attacker: ArmyId,
    pub defender: ArmyId,
    pub winner: Option<u8>,
    pub attacker_survivors: Vec<UnitPlacement>,
    pub defender_survivors: Vec<UnitPlacement>,
}

#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum InputMode {
    #[default]
    Normal,
    /// Next click blind-fires at the clicked tile.
    BlindFire,
}

#[derive(Resource)]
struct Battle {
    state: BattleState,
    planners: HashMap<u8, Box<dyn AiPlanner<BattleState, Order>>>,
    /// Events waiting to be shown to the player.
    anim: VecDeque<BattleEvent>,
    pace: Timer,
    selected: Option<UnitId>,
    move_range: HashMap<Hex, u32>,
    /// Move-range highlights need respawning.
    range_dirty: bool,
    mode: InputMode,
    /// Which overworld clash this battle resolves, if any.
    field: Option<(ArmyId, ArmyId, u8, u8)>,
    /// Delay before leaving the battle screen once it's decided.
    exit_timer: Option<Timer>,
}

impl Battle {
    fn human_side(&self) -> Option<u8> {
        let side = self.state.active_side;
        self.state.sides[side as usize].ai.is_none().then_some(side)
    }
}

// --- markers --------------------------------------------------------------

#[derive(Component, Clone)]
struct BattleScope;

#[derive(Component)]
struct BattleUnit(UnitId);

#[derive(Component)]
struct HpBar(UnitId);

#[derive(Component)]
struct MoveHighlight;

#[derive(Component)]
struct HoverHighlight;

#[derive(Component)]
struct SelectHighlight;

/// Sprite walking along a path, blocking the event pump while it exists.
#[derive(Component)]
struct Mover {
    path: Vec<Hex>,
    progress: f32,
}

/// Short red flash on a damaged unit.
#[derive(Component)]
struct Flash(Timer);

/// Fading impact/tracer puff.
#[derive(Component)]
struct Puff(Timer);

#[derive(Component)]
struct TurnBanner;

#[derive(Component)]
struct LogText;

#[derive(Component)]
struct PanelPortrait;

#[derive(Component)]
struct PanelText;

/// Rolling combat log lines.
#[derive(Resource, Default)]
struct BattleLog(VecDeque<String>);

impl BattleLog {
    fn push(&mut self, line: impl Into<String>) {
        self.0.push_back(line.into());
        while self.0.len() > 8 {
            self.0.pop_front();
        }
    }
}

pub struct BattlePlugin;

impl Plugin for BattlePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BattleLog>()
            .add_systems(OnEnter(AppState::Battle), setup_battle)
            .add_systems(
                Update,
                (
                    drive_movers,
                    pump_events,
                    drive_ai,
                    handle_input,
                    sync_units,
                    update_fog,
                    update_highlights,
                    update_panel,
                    update_flashes,
                    finish_battle,
                )
                    .chain()
                    .run_if(in_state(AppState::Battle)),
            );
    }
}

// --- setup ----------------------------------------------------------------

fn seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(4)
}

fn setup_battle(
    mut commands: Commands,
    mods: Res<Mods>,
    art: Res<ArtCache>,
    pending: Option<Res<PendingBattle>>,
    rotation: Res<ViewRotation>,
    mut center: ResMut<ViewCenter>,
    mut log: ResMut<BattleLog>,
    mut focus: ResMut<CameraFocus>,
) {
    let registry = &mods.0;
    let pending = pending
        .map(|p| p.clone())
        .unwrap_or(PendingBattle::Scenario {
            map_id: "river_crossing".into(),
        });

    let (state, field) = match &pending {
        PendingBattle::Scenario { map_id } => (
            BattleState::from_map(registry, map_id, seed()).expect("scenario map builds"),
            None,
        ),
        PendingBattle::Field {
            map_id,
            attacker,
            defender,
            sides,
            attacker_units,
            defender_units,
            attacker_side,
            defender_side,
        } => {
            let file = registry.map(map_id).expect("field battle map exists");
            let map = tactics_core::map::HexMap::from_map_file(file).expect("map parses");
            let placements =
                deploy(registry, &map, attacker_units, defender_units, *attacker_side, *defender_side);
            let state = BattleState::from_placements(registry, map, sides.clone(), &placements, seed());
            (state, Some((*attacker, *defender, *attacker_side, *defender_side)))
        }
    };

    // Dev tool: STAHL_AUTOPLAY=1 puts every side under AI control.
    let autoplay = std::env::var("STAHL_AUTOPLAY").is_ok();
    let mut planners: HashMap<u8, Box<dyn AiPlanner<BattleState, Order>>> = HashMap::new();
    for (i, side) in state.sides.iter().enumerate() {
        match &side.ai {
            Some(cfg) => {
                planners.insert(i as u8, make_battle_planner(cfg, seed().wrapping_add(i as u64)));
            }
            None if autoplay => {
                let cfg = tactics_core::ai::AiConfig {
                    planner: "utility".into(),
                    difficulty: 4,
                };
                planners.insert(i as u8, make_battle_planner(&cfg, seed().wrapping_add(i as u64)));
            }
            None => {}
        }
    }

    center.0 = state.map.center();
    map_render::spawn_map(&mut commands, &art, &state.map, rotation.0, true, BattleScope);
    commands.insert_resource(CurrentMap(state.map.clone()));

    for unit in state.alive_units() {
        spawn_unit_sprite(&mut commands, &art, unit.id, unit.side);
    }

    // Highlight sprites for hover and selection, hidden until used.
    for (marker, color) in [
        (0, Color::srgba(1.0, 1.0, 1.0, 0.35)),
        (1, Color::srgba(0.3, 0.9, 0.4, 0.5)),
    ] {
        let mut e = commands.spawn((
            Sprite {
                image: art.face.clone(),
                color,
                ..default()
            },
            Transform::default(),
            Visibility::Hidden,
            BattleScope,
        ));
        if marker == 0 {
            e.insert(HoverHighlight);
        } else {
            e.insert(SelectHighlight);
        }
    }

    spawn_battle_ui(&mut commands);
    log.0.clear();
    log.push("Battle started. LMB select/move, A attack hovered enemy, B blind fire, V wait, Enter end turn, Q/E rotate.");

    let (map_center, _) = iso::project(center.0, 0, rotation.0, center.0);
    focus.0 = map_center;

    commands.insert_resource(Battle {
        state,
        planners,
        anim: VecDeque::new(),
        pace: Timer::from_seconds(0.28, TimerMode::Repeating),
        selected: None,
        move_range: HashMap::new(),
        range_dirty: false,
        mode: InputMode::Normal,
        field,
        exit_timer: None,
    });
}

/// Place two armies' units on opposite edges of a field map.
fn deploy(
    registry: &tactics_core::data::DataRegistry,
    map: &tactics_core::map::HexMap,
    attacker_units: &[UnitPlacement],
    defender_units: &[UnitPlacement],
    attacker_side: u8,
    defender_side: u8,
) -> Vec<UnitPlacement> {
    let mut cols: Vec<i32> = map.iter().map(|(h, _)| tactics_core::hex_to_offset(h)[0]).collect();
    cols.sort_unstable();
    let (min_col, max_col) = (*cols.first().unwrap_or(&0), *cols.last().unwrap_or(&0));

    let mut placements = Vec::new();
    let mut place = |units: &[UnitPlacement], side: u8, west: bool| {
        let mut spots = Vec::new();
        for (hex, tile) in map.iter() {
            let [col, _row] = tactics_core::hex_to_offset(hex);
            let near_edge = if west { col <= min_col + 2 } else { col >= max_col - 2 };
            if !near_edge {
                continue;
            }
            let passable = registry
                .terrain(&tile.terrain)
                .is_some_and(|t| t.cost_for(tactics_core::data::MovementClass::Tracked).is_some());
            if passable {
                spots.push(hex);
            }
        }
        spots.sort_unstable_by_key(|h| (h.y, h.x));
        for (unit, hex) in units.iter().zip(spots) {
            let mut p = unit.clone();
            p.at = tactics_core::hex_to_offset(hex);
            p.side = side;
            placements.push(p);
        }
    };
    place(attacker_units, attacker_side, true);
    place(defender_units, defender_side, false);
    placements
}

fn spawn_unit_sprite(commands: &mut Commands, art: &ArtCache, id: UnitId, side: u8) {
    commands
        .spawn((
            Sprite {
                image: art
                    .units
                    .get(&(side % iso::SIDE_COLORS.len() as u8))
                    .cloned()
                    .unwrap_or_else(|| art.face.clone()),
                ..default()
            },
            Transform::default(),
            BattleUnit(id),
            BattleScope,
        ))
        .with_children(|parent| {
            parent.spawn((
                Sprite {
                    color: Color::srgba(0.0, 0.0, 0.0, 0.8),
                    custom_size: Some(Vec2::new(30.0, 5.0)),
                    ..default()
                },
                Transform::from_translation(Vec3::new(0.0, 22.0, 0.1)),
            ));
            parent.spawn((
                Sprite {
                    color: Color::srgb(0.3, 0.9, 0.3),
                    custom_size: Some(Vec2::new(28.0, 3.0)),
                    ..default()
                },
                Transform::from_translation(Vec3::new(0.0, 22.0, 0.2)),
                HpBar(id),
            ));
        });
}

fn spawn_battle_ui(commands: &mut Commands) {
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
        BattleScope,
        children![(
            Text::new(""),
            TextFont {
                font_size: 20.0.into(),
                ..default()
            },
            TextColor(Color::WHITE),
            TurnBanner,
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
        BattleScope,
        children![(
            Text::new(""),
            TextFont {
                font_size: 13.0.into(),
                ..default()
            },
            TextColor(Color::srgb(0.9, 0.9, 0.85)),
            LogText,
        )],
    ));
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(8.0),
            right: Val::Px(8.0),
            width: Val::Px(240.0),
            padding: UiRect::all(Val::Px(8.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0),
            ..default()
        },
        BackgroundColor(Color::srgba(0.05, 0.05, 0.1, 0.75)),
        BattleScope,
        children![
            (
                Node {
                    width: Val::Px(96.0),
                    height: Val::Px(96.0),
                    ..default()
                },
                ImageNode::default(),
                PanelPortrait,
            ),
            (
                Text::new("Select a unit"),
                TextFont {
                    font_size: 14.0.into(),
                    ..default()
                },
                TextColor(Color::srgb(0.92, 0.92, 0.88)),
                PanelText,
            )
        ],
    ));
}

// --- per-frame systems ----------------------------------------------------

fn drive_movers(
    mut commands: Commands,
    time: Res<Time>,
    rotation: Res<ViewRotation>,
    center: Res<ViewCenter>,
    map: Option<Res<CurrentMap>>,
    mut movers: Query<(Entity, &mut Mover, &mut Transform)>,
) {
    let Some(map) = map else { return };
    for (entity, mut mover, mut transform) in &mut movers {
        mover.progress += time.delta_secs() * 6.0;
        // A one-tile path (e.g. an instant ambush stop) finishes immediately.
        let steps = mover.path.len().saturating_sub(1) as f32;
        if mover.progress >= steps {
            let last = *mover.path.last().unwrap();
            let elev = map.0.get(last).map(|t| t.elevation).unwrap_or(0);
            let (pos, z) = iso::project(last, elev, rotation.0, center.0);
            transform.translation = Vec3::new(pos.x, pos.y + 10.0, z + 1.5);
            commands.entity(entity).remove::<Mover>();
            continue;
        }
        let i = mover.progress.floor() as usize;
        let t = mover.progress.fract();
        let (a, b) = (mover.path[i], mover.path[i + 1]);
        let ea = map.0.get(a).map(|t| t.elevation).unwrap_or(0);
        let eb = map.0.get(b).map(|t| t.elevation).unwrap_or(0);
        let (pa, za) = iso::project(a, ea, rotation.0, center.0);
        let (pb, zb) = iso::project(b, eb, rotation.0, center.0);
        // Snapped so a moving unit steps across whole pixels instead of
        // shimmering through sub-texel positions.
        let pos = pa.lerp(pb, t).round();
        transform.translation = Vec3::new(pos.x, pos.y + 10.0, za.max(zb) + 1.5);
    }
}

fn pump_events(
    mut commands: Commands,
    time: Res<Time>,
    mods: Res<Mods>,
    mut battle: ResMut<Battle>,
    mut log: ResMut<BattleLog>,
    rotation: Res<ViewRotation>,
    center: Res<ViewCenter>,
    units: Query<(Entity, &BattleUnit)>,
    movers: Query<&Mover>,
) {
    battle.pace.tick(time.delta());
    if !movers.is_empty() || !battle.pace.just_finished() {
        return;
    }
    let Some(event) = battle.anim.pop_front() else {
        return;
    };
    let registry = &mods.0;
    let name = |id: UnitId| -> String {
        battle
            .state
            .units
            .get(id.index())
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "???".into())
    };
    let entity_of = |id: UnitId| units.iter().find(|(_, u)| u.0 == id).map(|(e, _)| e);

    match &event {
        BattleEvent::TurnStarted { side, turn } => {
            let side_name = &battle.state.sides[*side as usize].name;
            log.push(format!("- Turn {turn}: {side_name} -"));
        }
        BattleEvent::UnitMoved { unit, path } => {
            if let Some(entity) = entity_of(*unit) {
                commands.entity(entity).insert(Mover {
                    path: path.clone(),
                    progress: 0.0,
                });
            }
        }
        BattleEvent::UnitTrapped { unit, .. } => {
            log.push(format!("{} ran into an ambush!", name(*unit)));
        }
        BattleEvent::ShotFired {
            attacker,
            at,
            weapon,
            blind,
            counter,
            ..
        } => {
            let verb = if *counter {
                "returns fire"
            } else if *blind {
                "fires blind"
            } else {
                "fires"
            };
            let weapon_name = registry.weapon(weapon).map(|w| w.name.clone()).unwrap_or_default();
            log.push(format!("{} {verb} ({weapon_name})", name(*attacker)));
            spawn_puff(&mut commands, *at, rotation.0, center.0, Color::srgb(1.0, 0.9, 0.4));
        }
        BattleEvent::ShotHit {
            attacker,
            target,
            damage,
            facing,
            remaining_hp,
        } => {
            log.push(format!(
                "{} hits {} in the {:?} for {damage} ({remaining_hp} hp left)",
                name(*attacker),
                name(*target),
                facing
            ));
            if let Some(entity) = entity_of(*target) {
                commands
                    .entity(entity)
                    .insert(Flash(Timer::from_seconds(0.35, TimerMode::Once)));
            }
        }
        BattleEvent::ShotMissed { attacker, at } => {
            log.push(format!("{} misses", name(*attacker)));
            spawn_puff(&mut commands, *at, rotation.0, center.0, Color::srgba(0.8, 0.8, 0.8, 0.8));
        }
        BattleEvent::UnitDestroyed { unit, at } => {
            log.push(format!("{} is destroyed!", name(*unit)));
            spawn_puff(&mut commands, *at, rotation.0, center.0, Color::srgb(1.0, 0.4, 0.1));
            if let Some(entity) = entity_of(*unit) {
                commands.entity(entity).despawn();
            }
        }
        BattleEvent::UnitSpotted { unit, by_side, .. } => {
            if battle.state.sides[*by_side as usize].ai.is_none() {
                log.push(format!("Enemy spotted: {}", name(*unit)));
            }
        }
        BattleEvent::BattleEnded { winner } => {
            let text = match winner {
                Some(w) => format!("Victory: {}", battle.state.sides[*w as usize].name),
                None => "Mutual destruction.".into(),
            };
            log.push(text);
            battle.exit_timer = Some(Timer::from_seconds(2.5, TimerMode::Once));
        }
    }
}

fn spawn_puff(commands: &mut Commands, at: Hex, rotation: u32, center: Hex, color: Color) {
    let (pos, z) = iso::project(at, 9, rotation, center);
    commands.spawn((
        Sprite {
            color,
            custom_size: Some(Vec2::splat(16.0)),
            ..default()
        },
        Transform::from_translation(Vec3::new(pos.x, pos.y - 9.0 * iso::ELEV_PX + 14.0, z + 2.0)),
        Puff(Timer::from_seconds(0.4, TimerMode::Once)),
        BattleScope,
    ));
}

fn drive_ai(mods: Res<Mods>, mut battle: ResMut<Battle>, movers: Query<&Mover>) {
    if battle.state.is_over() || !battle.anim.is_empty() || !movers.is_empty() {
        return;
    }
    let side = battle.state.active_side;
    let battle = &mut *battle;
    let Some(planner) = battle.planners.get_mut(&side) else {
        return;
    };
    let order = planner.next_order(&mods.0, &battle.state, side);
    match battle.state.apply(&mods.0, &order) {
        Ok(events) => battle.anim.extend(events),
        Err(_) => {
            // Planner confusion: never wedge the battle, just pass.
            if let Ok(events) = battle.state.apply(&mods.0, &Order::EndTurn) {
                battle.anim.extend(events);
            }
        }
    }
}

fn handle_input(
    mods: Res<Mods>,
    mut battle: ResMut<Battle>,
    mut log: ResMut<BattleLog>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    rotation: Res<ViewRotation>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    movers: Query<&Mover>,
) {
    let registry = &mods.0;
    if battle.state.is_over() || !movers.is_empty() || !battle.anim.is_empty() {
        return;
    }
    let Some(side) = battle.human_side() else {
        return;
    };

    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::KeyT) {
        battle.selected = None;
        battle.move_range.clear();
        battle.range_dirty = true;
        let battle = &mut *battle;
        if let Ok(events) = battle.state.apply(registry, &Order::EndTurn) {
            battle.anim.extend(events);
        }
        return;
    }
    if keys.just_pressed(KeyCode::Escape) || buttons.just_pressed(MouseButton::Right) {
        battle.selected = None;
        battle.move_range.clear();
        battle.range_dirty = true;
        battle.mode = InputMode::Normal;
        return;
    }
    if keys.just_pressed(KeyCode::KeyV) {
        if let Some(unit) = battle.selected {
            let battle = &mut *battle;
            if let Ok(events) = battle.state.apply(registry, &Order::Wait { unit }) {
                battle.anim.extend(events);
            }
            battle.selected = None;
            battle.move_range.clear();
            battle.range_dirty = true;
        }
        return;
    }
    if keys.just_pressed(KeyCode::KeyB) && battle.selected.is_some() {
        battle.mode = InputMode::BlindFire;
        log.push("Blind fire: click a target tile.");
        return;
    }

    let hovered = map_render::hovered_tile(
        &windows,
        &camera,
        &battle.state.map,
        rotation.0,
    );

    // A = attack the hovered enemy with the best weapon.
    if keys.just_pressed(KeyCode::KeyA) {
        if let (Some(unit), Some(hex)) = (battle.selected, hovered) {
            if let Some(target) = battle.state.unit_at(hex).filter(|t| t.side != side) {
                let target_id = target.id;
                attack_with_best(registry, &mut battle, unit, target_id, &mut log);
            }
        }
        return;
    }

    if !buttons.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(hex) = hovered else { return };

    if battle.mode == InputMode::BlindFire {
        battle.mode = InputMode::Normal;
        if let Some(unit) = battle.selected {
            let weapon = best_weapon_for_tile(registry, &battle.state, unit, hex);
            let battle = &mut *battle;
            match battle
                .state
                .apply(registry, &Order::BlindFire { unit, at: hex, weapon })
            {
                Ok(events) => {
                    battle.anim.extend(events);
                    battle.selected = None;
                    battle.move_range.clear();
                    battle.range_dirty = true;
                }
                Err(e) => log.push(format!("Can't blind fire there: {e}")),
            }
        }
        return;
    }

    // Click own unit: select it.
    if let Some(unit) = battle.state.unit_at(hex).filter(|u| u.side == side) {
        let id = unit.id;
        let acted = unit.acted;
        let moved = unit.moved;
        battle.selected = Some(id);
        battle.range_dirty = true;
        battle.move_range = if acted || moved {
            HashMap::new()
        } else {
            reachable(registry, &battle.state, id)
        };
        return;
    }

    // Click spotted enemy: attack with the best weapon.
    if let Some(target) = battle.state.unit_at(hex).filter(|u| u.side != side) {
        let target_id = target.id;
        if let Some(unit) = battle.selected {
            attack_with_best(registry, &mut battle, unit, target_id, &mut log);
        }
        return;
    }

    // Click a reachable tile: move.
    if let Some(unit) = battle.selected {
        if battle.move_range.contains_key(&hex) {
            let battle = &mut *battle;
            match battle.state.apply(registry, &Order::Move { unit, to: hex }) {
                Ok(events) => {
                    battle.anim.extend(events);
                    battle.move_range.clear();
                    battle.range_dirty = true;
                }
                Err(e) => log.push(format!("Can't move there: {e}")),
            }
        }
    }
}

fn best_weapon_for_tile(
    registry: &tactics_core::data::DataRegistry,
    state: &BattleState,
    unit: UnitId,
    at: Hex,
) -> usize {
    let Some(u) = state.unit(unit) else { return 0 };
    let Some(vehicle) = registry.vehicle(&u.vehicle) else {
        return 0;
    };
    let dist = u.pos.distance_to(at);
    vehicle
        .weapons
        .iter()
        .enumerate()
        .filter_map(|(i, w)| registry.weapon(w).map(|w| (i, w)))
        .filter(|(_, w)| (w.range[0] as i32..=w.range[1] as i32).contains(&dist))
        .max_by_key(|(_, w)| w.damage)
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn attack_with_best(
    registry: &tactics_core::data::DataRegistry,
    battle: &mut Battle,
    unit: UnitId,
    target: UnitId,
    log: &mut BattleLog,
) {
    let Some(tgt) = battle.state.unit(target) else {
        return;
    };
    let weapon = best_weapon_for_tile(registry, &battle.state, unit, tgt.pos);
    match battle
        .state
        .apply(registry, &Order::Attack { unit, target, weapon })
    {
        Ok(events) => {
            battle.anim.extend(events);
            battle.selected = None;
            battle.move_range.clear();
            battle.range_dirty = true;
        }
        Err(e) => log.push(format!("Can't attack: {e}")),
    }
}

/// Keep unit sprites in sync with the sim (position, facing, visibility,
/// hp bars) except while a Mover animation owns them.
fn sync_units(
    battle: Res<Battle>,
    rotation: Res<ViewRotation>,
    center: Res<ViewCenter>,
    mut units: Query<
        (&BattleUnit, &mut Transform, &mut Visibility, &mut Sprite),
        (Without<Mover>, Without<HpBar>),
    >,
    mut bars: Query<(&HpBar, &mut Sprite, &mut Transform), (With<HpBar>, Without<BattleUnit>)>,
    mods: Res<Mods>,
) {
    let state = &battle.state;
    // The fog we render is the first human side's view (or side 0).
    let view_side = state
        .sides
        .iter()
        .position(|s| s.ai.is_none())
        .unwrap_or(0) as u8;
    let fog = state.fog.side(view_side);

    for (marker, mut transform, mut visibility, mut sprite) in &mut units {
        let Some(unit) = state.unit(marker.0) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let elev = state.map.get(unit.pos).map(|t| t.elevation).unwrap_or(0);
        let (pos, z) = iso::project(unit.pos, elev, rotation.0, center.0);
        transform.translation = Vec3::new(pos.x, pos.y + 10.0, z + 1.5);
        transform.rotation =
            Quat::from_rotation_z(iso::facing_angle(unit.pos, unit.facing, rotation.0, center.0));
        let seen = unit.side == view_side || fog.spotted.contains(&unit.id);
        *visibility = if seen {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        // Dim units that have finished acting on the active side.
        let done = unit.side == state.active_side && unit.acted;
        sprite.color = if done {
            Color::srgb(0.55, 0.55, 0.55)
        } else {
            Color::WHITE
        };
    }

    for (bar, mut sprite, mut transform) in &mut bars {
        if let Some(unit) = state.unit(bar.0) {
            let max = mods
                .0
                .vehicle(&unit.vehicle)
                .map(|v| v.max_hp)
                .unwrap_or(10)
                .max(1);
            let frac = (unit.hp.max(0) as f32 / max as f32).clamp(0.0, 1.0);
            sprite.custom_size = Some(Vec2::new((28.0 * frac).round(), 3.0));
            transform.translation.x = (-14.0 * (1.0 - frac)).round();
            sprite.color = if frac > 0.5 {
                Color::srgb(0.3, 0.9, 0.3)
            } else if frac > 0.25 {
                Color::srgb(0.95, 0.8, 0.2)
            } else {
                Color::srgb(0.95, 0.3, 0.2)
            };
        }
    }
}

fn update_fog(battle: Res<Battle>, mut overlays: Query<(&FogOverlay, &mut Sprite, &mut Visibility)>) {
    let state = &battle.state;
    let view_side = state
        .sides
        .iter()
        .position(|s| s.ai.is_none())
        .unwrap_or(0) as u8;
    let fog = state.fog.side(view_side);
    for (overlay, mut sprite, mut visibility) in &mut overlays {
        let alpha = if fog.visible.contains(&overlay.hex) {
            0.0
        } else if fog.explored.contains(&overlay.hex) {
            0.5
        } else {
            0.92
        };
        if alpha == 0.0 {
            *visibility = Visibility::Hidden;
        } else {
            *visibility = Visibility::Inherited;
            sprite.color = Color::srgba(0.02, 0.02, 0.06, alpha);
        }
    }
}

fn update_highlights(
    mut commands: Commands,
    mut battle: ResMut<Battle>,
    rotation: Res<ViewRotation>,
    center: Res<ViewCenter>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    art: Res<ArtCache>,
    existing: Query<Entity, With<MoveHighlight>>,
    mut hover: Query<
        (&mut Transform, &mut Visibility),
        (With<HoverHighlight>, Without<SelectHighlight>),
    >,
    mut select: Query<
        (&mut Transform, &mut Visibility),
        (With<SelectHighlight>, Without<HoverHighlight>),
    >,
) {
    let map = battle.state.map.clone();
    let face_center = |hex: Hex| -> Vec3 {
        let elev = map.get(hex).map(|t| t.elevation).unwrap_or(0);
        let (pos, z) = iso::project(hex, elev, rotation.0, center.0);
        Vec3::new(pos.x, pos.y, z + 0.6)
    };

    // Hover marker.
    if let Ok((mut transform, mut visibility)) = hover.single_mut() {
        match map_render::hovered_tile(&windows, &camera, &map, rotation.0) {
            Some(hex) => {
                transform.translation = face_center(hex);
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }
    // Selection marker.
    if let Ok((mut transform, mut visibility)) = select.single_mut() {
        match battle.selected.and_then(|id| battle.state.unit(id)) {
            Some(unit) => {
                transform.translation = face_center(unit.pos);
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }

    // Move range tiles: rebuild when the range set changes.
    if battle.range_dirty {
        battle.range_dirty = false;
        for entity in &existing {
            commands.entity(entity).despawn();
        }
        for hex in battle.move_range.keys() {
            commands.spawn((
                Sprite {
                    image: art.face.clone(),
                    color: Color::srgba(0.35, 0.55, 1.0, 0.4),
                    ..default()
                },
                Transform::from_translation(face_center(*hex)),
                MoveHighlight,
                BattleScope,
            ));
        }
    }
}

fn update_panel(
    battle: Res<Battle>,
    mods: Res<Mods>,
    art: Res<ArtCache>,
    log: Res<BattleLog>,
    rotation: Res<ViewRotation>,
    windows: Query<&Window>,
    camera: Query<(&Camera, &GlobalTransform)>,
    mut banner: Query<&mut Text, (With<TurnBanner>, Without<LogText>, Without<PanelText>)>,
    mut log_text: Query<&mut Text, (With<LogText>, Without<TurnBanner>, Without<PanelText>)>,
    mut panel: Query<&mut Text, (With<PanelText>, Without<TurnBanner>, Without<LogText>)>,
    mut portrait: Query<&mut ImageNode, With<PanelPortrait>>,
) {
    let state = &battle.state;
    let registry = &mods.0;

    if let Ok(mut text) = banner.single_mut() {
        let side = &state.sides[state.active_side as usize];
        let controller = if side.ai.is_some() { "AI" } else { "You" };
        text.0 = format!("Turn {} - {} ({controller})", state.turn, side.name);
    }
    if let Ok(mut text) = log_text.single_mut() {
        text.0 = log.0.iter().cloned().collect::<Vec<_>>().join("\n");
    }

    // Panel shows the selected unit, else the hovered unit.
    let hovered_unit = map_render::hovered_tile(&windows, &camera, &state.map, rotation.0)
        .and_then(|hex| state.unit_at(hex))
        .map(|u| u.id);
    let shown = battle.selected.or(hovered_unit).and_then(|id| state.unit(id));

    let view_side = state.sides.iter().position(|s| s.ai.is_none()).unwrap_or(0) as u8;
    if let Ok(mut text) = panel.single_mut() {
        match shown {
            Some(unit) if unit.side == view_side || state.fog.side(view_side).spotted.contains(&unit.id) => {
                let vehicle = registry.vehicle(&unit.vehicle);
                let vehicle_name = vehicle.map(|v| v.name.as_str()).unwrap_or("?");
                let max_hp = vehicle.map(|v| v.max_hp).unwrap_or(10);
                let mut lines = vec![
                    format!("{}", unit.name),
                    format!("{vehicle_name}"),
                    format!("HP {}/{max_hp}", unit.hp.max(0)),
                    format!("Side: {}", state.sides[unit.side as usize].name),
                ];
                if let Some(v) = vehicle {
                    lines.push(format!(
                        "Armor F{}/S{}/R{}  Move {}",
                        v.armor.front, v.armor.side, v.armor.rear, v.movement.points
                    ));
                }
                lines.push("Crew:".into());
                for c in &unit.crew {
                    if let Some(ch) = registry.character(c) {
                        lines.push(format!(
                            "  {} (G{} D{} A{})",
                            ch.name, ch.stats.gunnery, ch.stats.driving, ch.stats.awareness
                        ));
                    }
                }
                if let Some(tile) = state.map.get(unit.pos) {
                    if let Some(t) = registry.terrain(&tile.terrain) {
                        lines.push(format!(
                            "On {} (cover {}%, elev {})",
                            t.name, t.cover, tile.elevation
                        ));
                    }
                }
                text.0 = lines.join("\n");
                if let Ok(mut image) = portrait.single_mut() {
                    let key = unit.crew.first().map(String::as_str).unwrap_or(&unit.vehicle);
                    if let Some(handle) = art.portraits.get(key) {
                        image.image = handle.clone();
                    }
                }
            }
            _ => {
                text.0 = "Select a unit\n\nLMB: select/move\nA: attack hovered\nB: blind fire\nV: wait\nEnter: end turn\nQ/E: rotate view".into();
            }
        }
    }
}

fn update_flashes(
    mut commands: Commands,
    time: Res<Time>,
    mut flashes: Query<(Entity, &mut Flash, &mut Sprite), Without<Puff>>,
    mut puffs: Query<(Entity, &mut Puff, &mut Sprite), Without<Flash>>,
) {
    for (entity, mut flash, mut sprite) in &mut flashes {
        flash.0.tick(time.delta());
        if flash.0.is_finished() {
            sprite.color = Color::WHITE;
            commands.entity(entity).remove::<Flash>();
        } else {
            sprite.color = Color::srgb(1.0, 0.3, 0.3);
        }
    }
    for (entity, mut puff, mut sprite) in &mut puffs {
        puff.0.tick(time.delta());
        let remaining = puff.0.fraction_remaining();
        sprite.color = sprite.color.with_alpha(remaining);
        if puff.0.is_finished() {
            commands.entity(entity).despawn();
        }
    }
}

fn finish_battle(
    mut commands: Commands,
    time: Res<Time>,
    mut battle: ResMut<Battle>,
    mut next: ResMut<NextState<AppState>>,
    scoped: Query<Entity, With<BattleScope>>,
) {
    let Some(timer) = battle.exit_timer.as_mut() else {
        return;
    };
    timer.tick(time.delta());
    if !timer.is_finished() || !battle.anim.is_empty() {
        return;
    }

    if let Some((attacker, defender, attacker_side, defender_side)) = battle.field {
        let survivors = |side: u8| -> Vec<UnitPlacement> {
            battle
                .state
                .side_units(side)
                .map(|u| UnitPlacement {
                    at: [0, 0],
                    side,
                    vehicle: u.vehicle.clone(),
                    crew: u.crew.clone(),
                    name: Some(u.name.clone()),
                })
                .collect()
        };
        commands.insert_resource(BattleOutcome {
            attacker,
            defender,
            winner: battle.state.over.and_then(|r| r.winner),
            attacker_survivors: survivors(attacker_side),
            defender_survivors: survivors(defender_side),
        });
    }

    for entity in &scoped {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<Battle>();
    commands.remove_resource::<PendingBattle>();
    next.set(AppState::Overworld);
}
