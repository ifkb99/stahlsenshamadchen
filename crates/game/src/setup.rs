//! A new campaign's world, chosen before it is made (WORLD.md W6.3).
//!
//! The designer's ruling (2026-09-27): the player sets her world the way a
//! strategy game's map setup lets her — map size, terrain, woodland and the
//! rest — and every one of those choices is content, the `world_settings`
//! of the mods (W6.1). This screen lists them, lets her step through their
//! options and roll a new seed, and shows her the world each choice makes
//! before she commits to it.
//!
//! The picture is the real thing, not an impression of it: every choice
//! builds the whole campaign, with `OverworldState::from_map_setup` — the
//! world, the campaign map its tiles add up to, the armies placed on it —
//! on a thread of its own, and the preview is drawn from that campaign's
//! tiles. When she presses Enter the campaign she was looking at is the one
//! she gets, handed over whole ([`PreparedCampaign`]); nothing is made twice
//! and nothing can differ.
//!
//! Only a generated campaign has a world to choose, so a drawn one (the
//! `frontier`) goes straight to the campaign map. `STAHL_WORLD` skips the
//! screen with a choice written the way the instruments write one
//! (`default`, or `size=large,woodland=heavy,seed=4`), which is how a tour
//! that is about something else gets past it.

use crate::mods::Mods;
use crate::{AppState, ScreenSet};
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread::JoinHandle;
use tactics_core::data::{DataRegistry, WorldSetup};
use tactics_core::map::MapKind;
use tactics_core::overworld::OverworldState;

pub struct SetupPlugin;

impl Plugin for SetupPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::WorldSetup), enter_setup)
            .add_systems(
                Update,
                poll_preview
                    .in_set(ScreenSet::Simulate)
                    .run_if(in_state(AppState::WorldSetup)),
            )
            .add_systems(
                Update,
                setup_input
                    .in_set(ScreenSet::Input)
                    .run_if(in_state(AppState::WorldSetup)),
            )
            .add_systems(
                Update,
                draw_setup
                    .in_set(ScreenSet::Present)
                    .run_if(in_state(AppState::WorldSetup)),
            )
            .add_systems(
                Update,
                publish_facts
                    .in_set(ScreenSet::Facts)
                    .run_if(in_state(AppState::WorldSetup)),
            )
            .add_systems(OnExit(AppState::WorldSetup), leave_setup);
    }
}

/// The campaign the setup screen made and the player accepted, for the
/// campaign map to take up instead of making one. An `Option` so the taker
/// can move it out of the resource rather than clone a whole world.
#[derive(Resource)]
pub(crate) struct PreparedCampaign(pub Option<OverworldState>);

/// A choice made without the screen (`STAHL_WORLD`), for the campaign map to
/// make its campaign from.
#[derive(Resource)]
pub(crate) struct ChosenWorld(pub WorldSetup);

/// Which screen a new campaign opens on, and what it takes with it.
///
/// A generated campaign opens on this one unless `STAHL_WORLD` has already
/// chosen, in which case the choice travels to the campaign map as a
/// [`ChosenWorld`]. A drawn campaign has no world to choose.
pub(crate) fn first_campaign_screen(commands: &mut Commands, registry: &DataRegistry) -> AppState {
    let map_id = crate::overworld::campaign_map_id(registry);
    let generated = registry
        .map(&map_id)
        .is_some_and(|m| m.kind == MapKind::Overworld && m.world.is_some());
    if !generated {
        return AppState::Overworld;
    }
    match std::env::var("STAHL_WORLD") {
        Ok(text) => match WorldSetup::parse(&text) {
            Ok(setup) => {
                commands.insert_resource(ChosenWorld(setup));
                AppState::Overworld
            }
            Err(e) => {
                error!("STAHL_WORLD: {e}; choosing the world on screen instead");
                AppState::WorldSetup
            }
        },
        Err(_) => AppState::WorldSetup,
    }
}

/// What the player has chosen so far, and the campaign it makes.
#[derive(Resource)]
struct Setup {
    map_id: String,
    map_name: String,
    /// Every setting's chosen option, defaults included, in the order the
    /// mods declare the settings.
    choices: Vec<(String, String)>,
    seed: u64,
    /// The setting the cursor is on.
    row: usize,
    /// Bumped on every change: which choice the screen is asking for.
    wanted: u64,
    /// A campaign being made, and which choice it is for. One at a time; a
    /// choice changed while one is being made is made when it finishes, so
    /// a run of key presses costs one world after the last, not one each.
    job: Option<(u64, JoinHandle<Result<Made, String>>)>,
    /// The last campaign made, and which choice it was.
    shown: Option<(u64, Result<Made, String>)>,
    preview: Handle<Image>,
    /// A line for the scripts' log fact, and for the player.
    status: String,
}

/// A campaign made from a choice, and the picture of its ground.
struct Made {
    state: OverworldState,
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    summary: String,
}

impl Setup {
    fn setup(&self) -> WorldSetup {
        WorldSetup {
            seed: Some(self.seed),
            choices: self.choices.iter().cloned().collect::<BTreeMap<_, _>>(),
        }
    }

    /// Whether the picture on screen is of the choice as it stands.
    fn ready(&self) -> bool {
        self.job.is_none() && self.shown.as_ref().is_some_and(|(n, _)| *n == self.wanted)
    }

    fn changed(&mut self) {
        self.wanted += 1;
    }
}

#[derive(Component)]
struct SetupScope;

#[derive(Component)]
struct SetupText;

#[derive(Component)]
struct SetupPreview;

/// A seed nobody chose: the clock's nanoseconds, mixed. A new world is meant
/// to be new, so this is the one place the game reaches for the time — and
/// it never reaches the simulation, which is given the seed like any other.
fn fresh_seed() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut z = nanos.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) % 1_000_000_000
}

fn enter_setup(mut commands: Commands, mods: Res<Mods>, mut images: ResMut<Assets<Image>>) {
    let registry = &mods.0;
    let map_id = crate::overworld::campaign_map_id(registry);
    let map_name = registry
        .map(&map_id)
        .map_or_else(|| map_id.clone(), |m| m.name.clone());
    let choices = registry
        .world_settings
        .iter()
        .map(|s| (s.id.clone(), s.default.clone()))
        .collect();
    let preview = images.add(blank_image());
    commands.insert_resource(Setup {
        map_id,
        map_name,
        choices,
        seed: fresh_seed(),
        row: 0,
        wanted: 1,
        job: None,
        shown: None,
        preview: preview.clone(),
        status: String::new(),
    });

    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(24.0),
            left: Val::Px(24.0),
            width: Val::Px(470.0),
            padding: UiRect::all(Val::Px(16.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.05, 0.05, 0.1, 0.85)),
        SetupScope,
        children![(
            Text::new(""),
            TextFont {
                font_size: 16.0.into(),
                ..default()
            },
            TextColor(Color::srgb(0.93, 0.93, 0.88)),
            SetupText,
        )],
    ));
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(24.0),
            left: Val::Px(520.0),
            ..default()
        },
        ImageNode::new(preview),
        SetupScope,
        SetupPreview,
    ));
}

fn blank_image() -> Image {
    Image::new(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        vec![0, 0, 0, 0],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// Start making the campaign the screen is asking for, if nothing is being
/// made; collect one that has finished.
fn poll_preview(
    mods: Res<Mods>,
    setup: Option<ResMut<Setup>>,
    mut images: ResMut<Assets<Image>>,
    mut nodes: Query<&mut Node, With<SetupPreview>>,
) {
    let Some(mut setup) = setup else {
        return;
    };
    if setup.job.as_ref().is_some_and(|(_, h)| h.is_finished()) {
        let (n, handle) = setup.job.take().expect("checked above");
        let made = handle
            .join()
            .unwrap_or_else(|_| Err("the world could not be made".into()));
        if let Ok(made) = &made {
            let image = Image::new(
                Extent3d {
                    width: made.width,
                    height: made.height,
                    depth_or_array_layers: 1,
                },
                TextureDimension::D2,
                made.pixels.clone(),
                TextureFormat::Rgba8UnormSrgb,
                RenderAssetUsages::RENDER_WORLD,
            );
            let _ = images.insert(&setup.preview, image);
            // A row of tiles is 0.866 of a column's width apart; draw the
            // picture at the ground's true proportions, a fixed height, so
            // a large world reads as larger by its detail rather than by
            // falling off the screen.
            let height = 640.0;
            let width = height * made.width as f32 / (made.height as f32 * 0.866);
            if let Ok(mut node) = nodes.single_mut() {
                node.width = Val::Px(width);
                node.height = Val::Px(height);
            }
        }
        setup.shown = Some((n, made));
    }
    let current = setup
        .shown
        .as_ref()
        .is_some_and(|(n, _)| *n == setup.wanted);
    if setup.job.is_none() && !current {
        let registry = Arc::clone(&mods.0);
        let map_id = setup.map_id.clone();
        let choice = setup.setup();
        let n = setup.wanted;
        setup.job = Some((
            n,
            std::thread::spawn(move || make(&registry, &map_id, &choice)),
        ));
    }
    setup.status = match (&setup.job, &setup.shown) {
        (Some(_), _) => "Making the world...".into(),
        (None, Some((_, Ok(_)))) => "World ready.".into(),
        (None, Some((_, Err(e)))) => format!("This world cannot be made: {e}"),
        (None, None) => String::new(),
    };
}

/// The campaign a choice makes, and its picture: `worldgen::picture`, the
/// drawing `examples/worldgen --picture` writes too, with each side's
/// companies marked where they stand.
fn make(registry: &DataRegistry, map_id: &str, choice: &WorldSetup) -> Result<Made, String> {
    let state =
        OverworldState::from_map_setup(registry, map_id, 0, choice).map_err(|e| e.to_string())?;
    let world = state
        .world
        .clone()
        .ok_or("the campaign has no world to draw")?;
    let marks: Vec<(tactics_core::Hex, [u8; 3])> = state
        .columns()
        .iter()
        .filter_map(|c| {
            let colour = crate::iso::SIDE_COLORS[c.side as usize % crate::iso::SIDE_COLORS.len()];
            c.tile.map(|t| (t, colour))
        })
        .collect();
    let pic = tactics_core::worldgen::picture(registry, &world, &marks);
    let (mut tiles, mut wood) = (0usize, 0usize);
    for chunk in world.chunks() {
        for (_, terrain, _) in world.chunk_tiles(chunk) {
            tiles += 1;
            wood += (terrain == world.rules.terrain.wood) as usize;
        }
    }
    let sk = &world.skeleton;
    let summary = format!(
        "{} campaign hexes, {} km across\n{} towns ({} with a factory), {} rivers\n{:.0}% of the land under trees",
        world.rules.hexes(),
        ((2 * world.rules.radius + 1) as f32 * registry.scale.overworld_hex_meters / 1000.0)
            .round(),
        sk.towns.len(),
        sk.towns.iter().filter(|t| t.factory).count(),
        sk.rivers.len(),
        100.0 * wood as f64 / tiles.max(1) as f64,
    );
    Ok(Made {
        state,
        pixels: pic.rgba,
        width: pic.width,
        height: pic.height,
        summary,
    })
}

fn setup_input(
    keys: Res<ButtonInput<KeyCode>>,
    mods: Res<Mods>,
    setup: Option<ResMut<Setup>>,
    mut commands: Commands,
    mut next: ResMut<NextState<AppState>>,
) {
    let Some(mut setup) = setup else {
        return;
    };
    let settings = &mods.0.world_settings;
    let rows = settings.len();
    if rows > 0 && keys.just_pressed(KeyCode::ArrowDown) {
        setup.row = (setup.row + 1) % rows;
    }
    if rows > 0 && keys.just_pressed(KeyCode::ArrowUp) {
        setup.row = (setup.row + rows - 1) % rows;
    }
    let step = if keys.just_pressed(KeyCode::ArrowRight) {
        1
    } else if keys.just_pressed(KeyCode::ArrowLeft) {
        -1
    } else {
        0
    };
    if step != 0
        && let Some(setting) = settings.get(setup.row)
    {
        let row = setup.row;
        let options = &setting.options;
        let at = options
            .iter()
            .position(|o| o.id == setup.choices[row].1)
            .unwrap_or(0) as i32;
        let to = (at + step).rem_euclid(options.len() as i32) as usize;
        setup.choices[row].1 = options[to].id.clone();
        setup.changed();
    }
    if keys.just_pressed(KeyCode::KeyN) {
        setup.seed = fresh_seed();
        setup.changed();
    }
    // Back to what the mod describes, seed kept: the quickest way to see
    // what one setting does is from the defaults.
    if keys.just_pressed(KeyCode::Backspace) {
        for (i, setting) in settings.iter().enumerate() {
            setup.choices[i].1 = setting.default.clone();
        }
        setup.changed();
    }
    if keys.just_pressed(KeyCode::Enter) && setup.ready() {
        let Some((_, Ok(_))) = &setup.shown else {
            return;
        };
        let Some((_, Ok(made))) = setup.shown.take() else {
            return;
        };
        commands.insert_resource(PreparedCampaign(Some(made.state)));
        next.set(AppState::Overworld);
    }
}

fn draw_setup(
    mods: Res<Mods>,
    setup: Option<Res<Setup>>,
    mut text: Query<&mut Text, With<SetupText>>,
) {
    let Some(setup) = setup else {
        return;
    };
    let Ok(mut text) = text.single_mut() else {
        return;
    };
    let settings = &mods.0.world_settings;
    let mut out = format!("A new campaign\n{}\n\n", setup.map_name);
    for (i, setting) in settings.iter().enumerate() {
        let chosen = setting
            .option(&setup.choices[i].1)
            .map_or(setup.choices[i].1.as_str(), |o| o.name.as_str());
        let cursor = if i == setup.row { ">" } else { " " };
        out.push_str(&format!("{cursor} {:<18} < {chosen} >\n", setting.name));
    }
    out.push_str(&format!("\n  Seed {}\n\n", setup.seed));
    if let Some(option) = settings
        .get(setup.row)
        .and_then(|s| s.option(&setup.choices[setup.row].1))
        && !option.text.is_empty()
    {
        out.push_str(&format!("{}: {}\n\n", option.name, option.text));
    }
    match &setup.shown {
        Some((n, Ok(made))) if *n == setup.wanted => out.push_str(&made.summary),
        _ => {}
    }
    out.push_str(&format!("\n\n{}\n\n", setup.status));
    out.push_str(
        "Up/Down: a setting    Left/Right: change it\n\
         N: a new seed    Backspace: defaults\n\
         Enter: begin the campaign",
    );
    if text.0 != out {
        text.0 = out;
    }
}

fn publish_facts(
    setup: Option<Res<Setup>>,
    mods: Res<Mods>,
    mut facts: ResMut<crate::devtools::ScriptFacts>,
) {
    let Some(setup) = setup else {
        return;
    };
    // Whole, like every screen's publisher: a field this screen has no
    // answer for says so. `selected` is the setting the cursor is on, which
    // is the one piece of state a tour drives here and wants to check.
    *facts = crate::devtools::ScriptFacts {
        turn: 0,
        idle: setup.ready(),
        waiting: false,
        over: false,
        score: Vec::new(),
        units: Vec::new(),
        log: vec![setup.status.clone()],
        selected: mods.0.world_settings.get(setup.row).map(|s| s.name.clone()),
        danger: None,
    };
}

fn leave_setup(mut commands: Commands, scoped: Query<Entity, With<SetupScope>>) {
    for entity in &scoped {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<Setup>();
}
