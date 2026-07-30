//! The battle screen: renders a `tactics_core` battle, feeds it player
//! orders, animates the resulting events, and drives AI sides.

use crate::camera::CameraFocus;
use crate::iso::{self, ArtCache, ViewCenter, ViewRotation};
use crate::map_render::{self, CurrentMap, FogOverlay};
use crate::mods::Mods;
use crate::AppState;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use tactics_core::ai::{make_battle_planner, AiPlanner};
use tactics_core::battle::{
    reachable, BattleState, EndReason, Event as BattleEvent, FireIntent, Order, SideState, UnitId,
};
use tactics_core::map::UnitPlacement;
use tactics_core::overworld::ArmyId;
use tactics_core::Hex;

/// One army committed to a field battle.
#[derive(Clone)]
pub struct BattleForce {
    pub army: ArmyId,
    pub side: u8,
    pub units: Vec<UnitPlacement>,
}

/// Why we are entering the battle state; set before switching to it.
#[derive(Resource, Clone)]
pub enum PendingBattle {
    /// A scenario map with its own unit placements (campaign/demo).
    Scenario { map_id: String },
    /// Overworld armies clashing on a terrain-picked battle map. More than
    /// two can take part: neighbours on either side may reinforce.
    Field {
        map_id: String,
        /// The army that started it, and the one that was attacked. These
        /// two decide who advances onto the contested tile afterwards.
        attacker: ArmyId,
        defender: ArmyId,
        sides: Vec<SideState>,
        attacker_side: u8,
        forces: Vec<BattleForce>,
    },
}

/// Reported back to the overworld when a field battle ends. Survivors are
/// listed per army so each one gets its own casualties back.
#[derive(Resource, Clone)]
pub struct BattleOutcome {
    pub attacker: ArmyId,
    pub defender: ArmyId,
    pub winner: Option<u8>,
    /// Both sides withdrew intact rather than one being destroyed.
    pub stalemate: bool,
    pub survivors: Vec<(ArmyId, Vec<UnitPlacement>)>,
}

/// Bookkeeping for a battle that resolves an overworld clash.
struct FieldBattle {
    attacker: ArmyId,
    defender: ArmyId,
    /// The army each unit was drawn from, indexed by unit index. Units are
    /// spawned in placement order, so this lines up with `UnitId`.
    origins: Vec<ArmyId>,
}

#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum InputMode {
    #[default]
    Normal,
    /// Next click orders blind fire at the clicked tile.
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
    field: Option<FieldBattle>,
    /// Delay before leaving the battle screen once it's decided.
    exit_timer: Option<Timer>,
}

impl Battle {
    /// The side the player commands, if any. Simultaneous rounds mean this no
    /// longer depends on whose turn it is; there is no such thing.
    fn human_side(&self) -> Option<u8> {
        (0..self.state.sides.len() as u8).find(|side| {
            self.state.sides[*side as usize].ai.is_none() && !self.planners.contains_key(side)
        })
    }

    /// The side whose fog and orders the screen shows.
    fn view_side(&self) -> u8 {
        self.human_side().unwrap_or_else(|| {
            self.state
                .sides
                .iter()
                .position(|s| s.ai.is_none())
                .unwrap_or(0) as u8
        })
    }

    /// Whether the player may issue orders right now.
    fn accepting_orders(&self) -> bool {
        !self.state.is_over() && self.state.is_planning() && self.anim.is_empty()
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

/// Overlay showing what your own units have been ordered to do this round.
#[derive(Component)]
struct PlanHighlight;

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
struct RoundBanner;

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
                    advance_resolution,
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
    // Dev tool: STAHL_SEED=<n> replays a battle shot for shot, which is
    // what makes a misbehaving fight reproducible.
    if let Some(seed) = std::env::var("STAHL_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        return seed;
    }
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
            attacker_side,
            forces,
        } => {
            let file = registry.map(map_id).expect("field battle map exists");
            let map = tactics_core::map::HexMap::from_map_file(file).expect("map parses");
            let (placements, origins) = deploy(registry, &map, forces, *attacker_side);
            let state = BattleState::from_placements(registry, map, sides.clone(), &placements, seed());
            (
                state,
                Some(FieldBattle {
                    attacker: *attacker,
                    defender: *defender,
                    origins,
                }),
            )
        }
    };

    // Dev tool: STAHL_AUTOPLAY=1 puts every side under AI control.
    let autoplay = std::env::var("STAHL_AUTOPLAY").is_ok();
    let mut planners: HashMap<u8, Box<dyn AiPlanner<BattleState, Order>>> = HashMap::new();
    for (i, side) in state.sides.iter().enumerate() {
        match &side.ai {
            Some(cfg) => {
                planners.insert(
                    i as u8,
                    make_battle_planner(cfg, seed().wrapping_add(i as u64), registry),
                );
            }
            None if autoplay => {
                let cfg = tactics_core::ai::AiConfig {
                    planner: "utility".into(),
                    difficulty: 4,
                    doctrine: None,
                };
                planners.insert(
                    i as u8,
                    make_battle_planner(&cfg, seed().wrapping_add(i as u64), registry),
                );
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
    log.push(
        "Battle started. Both sides plan, then the round plays out at once. \
         LMB select/move, A engage hovered enemy, B blind fire, V hold, C clear orders, \
         Enter commit, Q/E rotate.",
    );

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

/// Line the attacking armies up along the west edge and the defenders along
/// the east. Returns the placements plus, for each one, the army it came
/// from, so casualties can be reported back to the right army afterwards.
fn deploy(
    registry: &tactics_core::data::DataRegistry,
    map: &tactics_core::map::HexMap,
    forces: &[BattleForce],
    attacker_side: u8,
) -> (Vec<UnitPlacement>, Vec<ArmyId>) {
    // Tiles a vehicle can actually sit on, nearest edge first. Taking spots
    // in this order lets a side deploy as deep inland as it needs to, so
    // three armies fit where one used to.
    let deployable = |west: bool| -> Vec<Hex> {
        let mut spots: Vec<(i32, i32, Hex)> = map
            .iter()
            .filter(|(_, tile)| {
                registry
                    .terrain(&tile.terrain)
                    .is_some_and(|t| t.cost_for(tactics_core::data::MovementClass::Tracked).is_some())
            })
            .map(|(hex, _)| {
                let [col, row] = tactics_core::hex_to_offset(hex);
                (if west { col } else { -col }, row, hex)
            })
            .collect();
        spots.sort_unstable_by_key(|(depth, row, _)| (*depth, *row));
        spots.into_iter().map(|(_, _, hex)| hex).collect()
    };

    let mut placements = Vec::new();
    let mut origins = Vec::new();
    for west in [true, false] {
        let mut spots = deployable(west).into_iter();
        for force in forces
            .iter()
            .filter(|f| (f.side == attacker_side) == west)
        {
            for unit in &force.units {
                let Some(hex) = spots.next() else { break };
                let mut placement = unit.clone();
                placement.at = tactics_core::hex_to_offset(hex);
                placement.side = force.side;
                placements.push(placement);
                origins.push(force.army);
            }
        }
    }
    (placements, origins)
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
            RoundBanner,
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
    // One tick per beat, not one event: everything between two `TickStarted`
    // markers happened at the same moment, so it has to be shown that way or
    // simultaneous resolution looks alternating again.
    let mut drained = Vec::new();
    while let Some(event) = battle.anim.pop_front() {
        drained.push(event);
        if matches!(battle.anim.front(), Some(BattleEvent::TickStarted { .. }) | None) {
            break;
        }
    }
    if drained.is_empty() {
        return;
    }
    let registry = &mods.0;
    // Names are copied out rather than looked up through `battle`, so the
    // loop below is free to touch the resource while logging.
    let names: Vec<String> = battle.state.units.iter().map(|u| u.name.clone()).collect();
    let name = |id: UnitId| -> String {
        names
            .get(id.index())
            .cloned()
            .unwrap_or_else(|| "???".into())
    };
    let entity_of = |id: UnitId| units.iter().find(|(_, u)| u.0 == id).map(|(e, _)| e);

    for event in &drained {
        match event {
            BattleEvent::RoundStarted { round } => {
                log.push(format!("- Round {round}: orders -"));
                battle.range_dirty = true;
            }
            // The separator itself has nothing to show.
            BattleEvent::TickStarted { .. } => {}
            // Movers spawned in the same beat animate together, which is the
            // whole point of resolving a tick at a time.
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
                opportunity,
                ..
            } => {
                let verb = if *blind {
                    "fires blind"
                } else if *opportunity {
                    "takes a shot of opportunity"
                } else {
                    "fires"
                };
                let weapon_name = registry
                    .weapon(weapon)
                    .map(|w| w.name.clone())
                    .unwrap_or_default();
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
            BattleEvent::BattleEnded { winner, reason } => {
                let text = match (winner, reason) {
                    (Some(w), _) => format!("Victory: {}", battle.state.sides[*w as usize].name),
                    (None, EndReason::Stalemate) => "Contact lost. Both sides break off.".into(),
                    (None, EndReason::Eliminated) => "Mutual destruction.".into(),
                };
                log.push(text);
                battle.exit_timer = Some(Timer::from_seconds(2.5, TimerMode::Once));
            }
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

/// Let every AI side that still owes orders make one decision. Planning is
/// simultaneous, so this is not a turn: all of them write orders at once and
/// nothing happens until the last one commits.
fn drive_ai(mods: Res<Mods>, mut battle: ResMut<Battle>, movers: Query<&Mover>) {
    if battle.state.is_over() || !battle.anim.is_empty() || !movers.is_empty() {
        return;
    }
    if !battle.state.is_planning() {
        return;
    }
    let sides: Vec<u8> = battle
        .state
        .living_sides()
        .into_iter()
        .filter(|side| battle.planners.contains_key(side) && !battle.state.has_committed(*side))
        .collect();
    for side in sides {
        let battle = &mut *battle;
        if !battle.state.is_planning() {
            break;
        }
        let Some(planner) = battle.planners.get_mut(&side) else {
            continue;
        };
        let order = planner.next_order(&mods.0, &battle.state, side);
        if battle.state.apply(&mods.0, &order).is_err() {
            // Planner confusion: never wedge the battle, just commit what it
            // has and let the round resolve.
            let _ = battle.state.apply(&mods.0, &Order::Commit { side });
        }
        battle.range_dirty = true;
    }
}

/// Play out one tick of the committed round, once the previous one has
/// finished animating. Keeping the simulation at most a tick ahead of the
/// sprites is what lets the screen show simultaneous action honestly.
fn advance_resolution(mods: Res<Mods>, mut battle: ResMut<Battle>, movers: Query<&Mover>) {
    if battle.state.is_over() || !battle.anim.is_empty() || !movers.is_empty() {
        return;
    }
    if battle.state.resolving_tick().is_none() {
        return;
    }
    let battle = &mut *battle;
    let events = battle.state.step_tick(&mods.0);
    battle.anim.extend(events);
    battle.range_dirty = true;
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
    if !movers.is_empty() || !battle.accepting_orders() {
        return;
    }
    let Some(side) = battle.human_side() else {
        return;
    };
    if battle.state.has_committed(side) {
        return;
    }

    // Enter closes this side's orders. The round only starts once every side
    // has done the same.
    if keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::KeyT) {
        battle.selected = None;
        battle.move_range.clear();
        battle.range_dirty = true;
        let battle = &mut *battle;
        match battle.state.apply(registry, &Order::Commit { side }) {
            Ok(events) => battle.anim.extend(events),
            Err(e) => log.push(format!("Can't commit: {e}")),
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
    // Hold: stay put and shoot at whatever appears.
    if keys.just_pressed(KeyCode::KeyV) {
        if let Some(unit) = battle.selected {
            set_intent(
                registry,
                &mut battle,
                &Order::SetFire {
                    unit,
                    fire: FireIntent::Hold,
                },
                &mut log,
            );
        }
        return;
    }
    // Take back a unit's orders; nothing is locked in until the commit.
    if keys.just_pressed(KeyCode::KeyC) {
        if let Some(unit) = battle.selected {
            set_intent(registry, &mut battle, &Order::ClearIntent { unit }, &mut log);
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

    // A = engage the hovered enemy with the best weapon.
    if keys.just_pressed(KeyCode::KeyA) {
        if let (Some(unit), Some(hex)) = (battle.selected, hovered)
            && let Some(target) = battle.state.spotted_enemy_at(hex, side)
        {
            let target_id = target.id;
            engage_with_best(registry, &mut battle, unit, target_id, &mut log);
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
            set_intent(
                registry,
                &mut battle,
                &Order::SetFire {
                    unit,
                    fire: FireIntent::Area { at: hex, weapon },
                },
                &mut log,
            );
        }
        return;
    }

    // Click own unit: select it. Every unit can be given orders during
    // planning, including ones that already have some.
    if let Some(unit) = battle.state.unit_at(hex).filter(|u| u.side == side) {
        let id = unit.id;
        battle.selected = Some(id);
        battle.range_dirty = true;
        battle.move_range = reachable(registry, &battle.state, id);
        return;
    }

    // Click spotted enemy: engage with the best weapon. Unspotted enemies
    // fall through to the move branch, so probing the fog by clicking is not
    // a way to find them — you drive in and get ambushed like anyone else.
    if let Some(target) = battle.state.spotted_enemy_at(hex, side) {
        let target_id = target.id;
        if let Some(unit) = battle.selected {
            engage_with_best(registry, &mut battle, unit, target_id, &mut log);
        }
        return;
    }

    // Click a reachable tile: route the unit there for this round.
    if let Some(unit) = battle.selected
        && battle.move_range.contains_key(&hex)
    {
        set_intent(registry, &mut battle, &Order::SetMove { unit, to: hex }, &mut log);
    }
}

/// Apply one planning order and refresh the overlays that show it.
fn set_intent(
    registry: &tactics_core::data::DataRegistry,
    battle: &mut Battle,
    order: &Order,
    log: &mut BattleLog,
) {
    match battle.state.apply(registry, order) {
        Ok(_) => battle.range_dirty = true,
        Err(e) => log.push(format!("Order refused: {e}")),
    }
}

fn best_weapon_for_tile(
    registry: &tactics_core::data::DataRegistry,
    state: &BattleState,
    unit: UnitId,
    at: Hex,
) -> usize {
    let from = state.unit(unit).map(|u| u.pos).unwrap_or_default();
    best_weapon_from(registry, state, unit, from, at)
}

/// The heaviest weapon that reaches `at` from `from`.
fn best_weapon_from(
    registry: &tactics_core::data::DataRegistry,
    state: &BattleState,
    unit: UnitId,
    from: Hex,
    at: Hex,
) -> usize {
    let Some(u) = state.unit(unit) else { return 0 };
    let Some(vehicle) = registry.vehicle(&u.vehicle) else {
        return 0;
    };
    let dist = from.distance_to(at);
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

/// Order a unit to engage a target with whatever gun suits the range best.
/// The shot is taken during resolution, from wherever the unit ends up.
fn engage_with_best(
    registry: &tactics_core::data::DataRegistry,
    battle: &mut Battle,
    unit: UnitId,
    target: UnitId,
    log: &mut BattleLog,
) {
    // Judge the weapon from where the unit will be standing, since it may be
    // driving into position this same round.
    let from = battle
        .state
        .unit(unit)
        .map(|u| u.planned_destination())
        .unwrap_or_default();
    let Some(tgt_pos) = battle.state.unit(target).map(|t| t.pos) else {
        return;
    };
    let weapon = best_weapon_from(registry, &battle.state, unit, from, tgt_pos);
    set_intent(
        registry,
        battle,
        &Order::SetFire {
            unit,
            fire: FireIntent::Target { target, weapon },
        },
        log,
    );
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
    let view_side = battle.view_side();
    let fog = state.fog.side(view_side);
    // A move already queued for animation belongs to the animator; snapping
    // the sprite to the destination first would spoil the walk.
    let animating: HashSet<UnitId> = battle
        .anim
        .iter()
        .filter_map(|event| match event {
            BattleEvent::UnitMoved { unit, .. } => Some(*unit),
            _ => None,
        })
        .collect();

    for (marker, mut transform, mut visibility, mut sprite) in &mut units {
        let Some(unit) = state.unit(marker.0) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        let elev = state.map.get(unit.pos).map(|t| t.elevation).unwrap_or(0);
        let (pos, z) = iso::project(unit.pos, elev, rotation.0, center.0);
        if !animating.contains(&unit.id) {
            transform.translation = Vec3::new(pos.x, pos.y + 10.0, z + 1.5);
        }
        transform.rotation =
            Quat::from_rotation_z(iso::facing_angle(unit.pos, unit.facing, rotation.0, center.0));
        let seen = unit.side == view_side || fog.spotted.contains(&unit.id);
        *visibility = if seen {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        // Dim your own units once they have their orders for the round.
        let done = state.is_planning() && unit.side == view_side && unit.planned;
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
    let fog = state.fog.side(battle.view_side());
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
    existing: Query<Entity, Or<(With<MoveHighlight>, With<PlanHighlight>)>>,
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

    // Move range and ordered plans: rebuild when either could have changed.
    if !battle.range_dirty {
        return;
    }
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

    // Your own orders, drawn so the whole round can be reviewed before it is
    // committed: amber for routes, red for what each unit will shoot at.
    if !battle.state.is_planning() {
        return;
    }
    let view_side = battle.view_side();
    let mut marks: Vec<(Hex, Color)> = Vec::new();
    for unit in battle.state.side_units(view_side) {
        for (i, hex) in unit.intent.path.iter().enumerate() {
            let last = i + 1 == unit.intent.path.len();
            let alpha = if last { 0.55 } else { 0.3 };
            marks.push((*hex, Color::srgba(1.0, 0.8, 0.3, alpha)));
        }
        let from = unit.planned_destination();
        let at = match unit.intent.fire {
            FireIntent::Hold => None,
            FireIntent::Area { at, .. } => Some(at),
            FireIntent::Target { target, .. } => battle.state.unit(target).map(|t| t.pos),
        };
        let Some(at) = at else { continue };
        for hex in from.line_to(at) {
            let alpha = if hex == at { 0.5 } else { 0.16 };
            marks.push((hex, Color::srgba(1.0, 0.3, 0.25, alpha)));
        }
    }
    for (hex, color) in marks {
        commands.spawn((
            Sprite {
                image: art.face.clone(),
                color,
                ..default()
            },
            Transform::from_translation(face_center(hex)),
            PlanHighlight,
            BattleScope,
        ));
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
    mut banner: Query<&mut Text, (With<RoundBanner>, Without<LogText>, Without<PanelText>)>,
    mut log_text: Query<&mut Text, (With<LogText>, Without<RoundBanner>, Without<PanelText>)>,
    mut panel: Query<&mut Text, (With<PanelText>, Without<RoundBanner>, Without<LogText>)>,
    mut portrait: Query<&mut ImageNode, With<PanelPortrait>>,
) {
    let state = &battle.state;
    let registry = &mods.0;
    let view_side = battle.view_side();

    if let Ok(mut text) = banner.single_mut() {
        text.0 = match state.resolving_tick() {
            Some(tick) => format!(
                "Round {} - resolving ({}/{})",
                state.round,
                tick + 1,
                tactics_core::battle::TICKS_PER_ROUND
            ),
            None if battle.human_side().is_some_and(|s| state.has_committed(s)) => {
                format!("Round {} - waiting on the other side", state.round)
            }
            None => format!("Round {} - planning", state.round),
        };
    }
    if let Ok(mut text) = log_text.single_mut() {
        text.0 = log.0.iter().cloned().collect::<Vec<_>>().join("\n");
    }

    let visible = |unit: &tactics_core::battle::Unit| {
        unit.side == view_side || state.fog.side(view_side).spotted.contains(&unit.id)
    };

    let hovered_tile = map_render::hovered_tile(&windows, &camera, &state.map, rotation.0);
    let hovered_unit = hovered_tile
        .and_then(|hex| state.unit_at(hex))
        .filter(|u| visible(u))
        .map(|u| u.id);

    let Ok(mut text) = panel.single_mut() else {
        return;
    };

    // Hovering an enemy while something is selected is a question about a
    // shot, so answer that first. Otherwise inspect whatever is under the
    // cursor, falling back to the selection and then the bare tile.
    let attack = battle.selected.zip(hovered_unit).filter(|(attacker, target)| {
        state.unit(*attacker).map(|u| u.side) != state.unit(*target).map(|u| u.side)
    });
    if let Some((attacker, target)) = attack {
        let weapon = state
            .unit(target)
            .map(|t| best_weapon_for_tile(registry, state, attacker, t.pos))
            .unwrap_or(0);
        if let Some(preview) =
            tactics_core::battle::preview_attack(registry, state, attacker, weapon, target, false)
        {
            text.0 = format_attack(&preview);
            set_portrait(&mut portrait, &art, state, target);
            return;
        }
    }

    let shown = hovered_unit
        .or(battle.selected)
        .and_then(|id| state.unit(id))
        .filter(|u| visible(u));
    if let Some(unit) = shown {
        // Describe the tile under the cursor rather than the unit's own, so
        // terrain can be read without dropping the selection.
        text.0 = format_unit(registry, state, unit, hovered_tile.unwrap_or(unit.pos));
        set_portrait(&mut portrait, &art, state, unit.id);
        return;
    }
    if let Some(hex) = hovered_tile {
        text.0 = format_tile(registry, state, hex);
        return;
    }
    text.0 = "Hover a tile for terrain\n\nLMB: select / set route\nA: engage hovered enemy\nB: blind fire a tile\nV: hold and watch\nC: clear orders\nEnter: commit the round\nQ/E: rotate view".into();
}

fn set_portrait(
    portrait: &mut Query<&mut ImageNode, With<PanelPortrait>>,
    art: &ArtCache,
    state: &BattleState,
    id: UnitId,
) {
    let Some(unit) = state.unit(id) else { return };
    let Ok(mut image) = portrait.single_mut() else {
        return;
    };
    let key = unit.crew.first().map(String::as_str).unwrap_or(&unit.vehicle);
    if let Some(handle) = art.portraits.get(key) {
        image.image = handle.clone();
    }
}

/// The shot the player is contemplating, with the arithmetic spelled out.
fn format_attack(preview: &tactics_core::battle::AttackPreview) -> String {
    let mut lines = vec![
        format!("Attack: {}", preview.target_name),
        format!("{} - {}", preview.target_vehicle, preview.target_side),
        format!(
            "HP {}/{}  at {} hexes",
            preview.target_hp, preview.target_max_hp, preview.distance
        ),
        String::new(),
        format!(
            "{} (range {}-{})",
            preview.weapon_name, preview.weapon_range[0], preview.weapon_range[1]
        ),
    ];
    if !preview.in_range {
        lines.push("OUT OF RANGE".into());
    }
    lines.push(format!("Hit {}%", preview.hit.total));
    lines.push(format!("  base {}", preview.hit.base));
    for modifier in &preview.hit.modifiers {
        lines.push(format!("  {:+} {}", modifier.delta, modifier.label));
    }
    if preview.hit.clamped {
        lines.push(format!(
            "  (capped at {}-{}%)",
            tactics_core::battle::MIN_HIT,
            tactics_core::battle::MAX_HIT
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "Damage {} (vs {:?} armor {})",
        preview.damage, preview.facing, preview.effective_armor
    ));
    lines.push(format!("Expected {:.1}", preview.expected_damage));
    if preview.lethal {
        lines.push("A hit destroys it.".into());
    }
    match &preview.counter {
        Some(counter) => lines.push(format!(
            "Return fire: {} {}% for {}",
            counter.weapon_name, counter.hit_chance, counter.damage
        )),
        None => lines.push("No return fire.".into()),
    }
    lines.push(String::new());
    if preview.in_range {
        lines.push("A or click to fire".into());
    }
    lines.join("\n")
}

fn format_unit(
    registry: &tactics_core::data::DataRegistry,
    state: &BattleState,
    unit: &tactics_core::battle::Unit,
    tile: Hex,
) -> String {
    let vehicle = registry.vehicle(&unit.vehicle);
    let mut lines = vec![
        unit.name.clone(),
        vehicle.map(|v| v.name.clone()).unwrap_or_else(|| "?".into()),
        format!("HP {}/{}", unit.hp.max(0), vehicle.map(|v| v.max_hp).unwrap_or(10)),
        format!("Side: {}", state.sides[unit.side as usize].name),
    ];
    if let Some(v) = vehicle {
        lines.push(format!(
            "Armor F{}/S{}/R{}  Move {}",
            v.armor.front, v.armor.side, v.armor.rear, v.movement.points
        ));
        for weapon in v.weapons.iter().filter_map(|w| registry.weapon(w)) {
            lines.push(format!(
                "  {} dmg {} rng {}-{}",
                weapon.name, weapon.damage, weapon.range[0], weapon.range[1]
            ));
        }
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
    lines.push(String::new());
    lines.push(format_tile(registry, state, tile));
    lines.join("\n")
}

/// What a tile does to whoever stands on it.
fn format_tile(
    registry: &tactics_core::data::DataRegistry,
    state: &BattleState,
    hex: Hex,
) -> String {
    let Some(tile) = state.map.get(hex) else {
        return String::new();
    };
    let Some(terrain) = registry.terrain(&tile.terrain) else {
        return tile.terrain.clone();
    };
    let mut lines = vec![format!("{} (elev {})", terrain.name, tile.elevation)];
    if terrain.cover > 0 {
        lines.push(format!(
            "Cover {}%: -{}% to be hit",
            terrain.cover,
            terrain.cover / 2
        ));
    } else {
        lines.push("No cover".into());
    }
    if terrain.vision_block > 0 {
        lines.push(format!("Blocks sight (+{})", terrain.vision_block));
    }
    let costs: Vec<String> = [
        (tactics_core::data::MovementClass::Tracked, "trk"),
        (tactics_core::data::MovementClass::Wheeled, "whl"),
        (tactics_core::data::MovementClass::Foot, "ft"),
    ]
    .iter()
    .map(|(class, label)| match terrain.cost_for(*class) {
        Some(cost) => format!("{label}{cost}"),
        None => format!("{label}-"),
    })
    .collect();
    lines.push(format!("Move {}", costs.join(" ")));
    lines.join("\n")
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

    if let Some(field) = &battle.field {
        // Start every participating army at zero survivors so armies that
        // were wiped out are still reported, then hand each living unit
        // back to the army it marched in with.
        let mut survivors: Vec<(ArmyId, Vec<UnitPlacement>)> = Vec::new();
        let mut slot_of = HashMap::new();
        for army in &field.origins {
            slot_of.entry(*army).or_insert_with(|| {
                survivors.push((*army, Vec::new()));
                survivors.len() - 1
            });
        }
        for unit in battle.state.alive_units() {
            let Some(army) = field.origins.get(unit.id.index()) else {
                continue;
            };
            let Some(&slot) = slot_of.get(army) else {
                continue;
            };
            survivors[slot].1.push(UnitPlacement {
                at: [0, 0],
                side: unit.side,
                vehicle: unit.vehicle.clone(),
                crew: unit.crew.clone(),
                name: Some(unit.name.clone()),
            });
        }
        commands.insert_resource(BattleOutcome {
            attacker: field.attacker,
            defender: field.defender,
            winner: battle.state.over.and_then(|r| r.winner),
            stalemate: matches!(
                battle.state.over.map(|r| r.reason),
                Some(EndReason::Stalemate)
            ),
            survivors,
        });
    }

    for entity in &scoped {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<Battle>();
    commands.remove_resource::<PendingBattle>();
    next.set(AppState::Overworld);
}
