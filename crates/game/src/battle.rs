//! The battle screen: renders a `tactics_core` battle, feeds it player
//! orders, animates the resulting events, and drives AI sides.

use crate::AppState;
use crate::camera::CameraFocus;
use crate::iso::{self, ArtCache, ViewCenter};
use crate::map_render::{self, CurrentMap, FogOverlay, HexOverlay};
use crate::mods::Mods;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tactics_core::Hex;
use tactics_core::ai::{AiConfig, AiDriver, SideCommand, make_battle_planner};
use tactics_core::battle::{
    BattleState, Contact, EndReason, Event as BattleEvent, FireIntent, Formation, FormationId,
    Mission, Order, SideState, Unit, UnitId, reachable,
};
use tactics_core::map::{ObjectiveKind, UnitPlacement};
use tactics_core::overworld::ArmyId;
use tactics_core::overworld::{ArmyMission, ArmyUnit, CrewLoss};
use tactics_core::roster::{GirlId, Roster};

/// One army committed to a field battle.
#[derive(Clone)]
pub struct BattleForce {
    pub army: ArmyId,
    pub side: u8,
    pub units: Vec<ArmyUnit>,
    /// The standing orders this army was carrying when it was committed, so
    /// what was decided on the map can colour the fight it caused. See
    /// `inherit_army_missions`.
    pub mission: Option<ArmyMission>,
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
        /// The campaign's girls, so the crews that fight are the same people
        /// who walk away from it.
        roster: Arc<Roster>,
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
    pub survivors: Vec<(ArmyId, Vec<ArmyUnit>)>,
    /// Girls who were aboard a vehicle that was destroyed. What became of
    /// them is the campaign's decision, not the battle's.
    pub losses: Vec<CrewLoss>,
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
    ai: AiDriver,
    /// Events waiting to be shown to the player.
    anim: VecDeque<BattleEvent>,
    pace: Timer,
    selected: Option<UnitId>,
    /// The formation the player is commanding, as an index into
    /// `state.formations()`. Mutually exclusive with `selected`: one of them
    /// is a conversation with a crew and the other a conversation with a
    /// platoon, and the keyboard would not know which one a keystroke meant.
    formation: Option<usize>,
    /// The player's own staff: executors that fill in whatever she left
    /// unplanned when she commits. Built on first use and kept for the rest
    /// of the battle, because a planner that forgot which round it last
    /// reviewed would review every one of them again.
    delegate: Option<AiDriver>,
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
        (0..self.state.sides.len() as u8)
            .find(|side| self.state.sides[*side as usize].ai.is_none() && !self.ai.controls(*side))
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

    /// The formations the player commands, in the order their map declared
    /// them — which is the order `F` walks and the order the panel names.
    fn own_formations(&self) -> Vec<usize> {
        let side = self.view_side();
        self.state
            .formations()
            .iter()
            .enumerate()
            .filter(|(_, f)| f.side == side)
            .map(|(index, _)| index)
            .collect()
    }

    /// The formation currently being commanded.
    fn formation(&self) -> Option<&Formation> {
        self.formation.and_then(|i| self.state.formations().get(i))
    }

    /// Walk to the next of the player's formations, then off the end back to
    /// none. Cycling past the last one is the same gesture as `Esc`: there is
    /// no mode to get stuck in.
    fn cycle_formation(&mut self) {
        let own = self.own_formations();
        self.formation = match self.formation {
            None => own.first().copied(),
            Some(current) => own
                .iter()
                .position(|i| *i == current)
                .and_then(|at| own.get(at + 1))
                .copied(),
        };
    }

    /// Whether committing now means handing unplanned units to their
    /// formations' executors.
    ///
    /// Only units under a mission count. A unit the player left alone in a
    /// formation nobody has ordered is not a gap in her plan, she *is* the
    /// plan — see [`SideCommand::executor_only`], which enforces the same
    /// rule one level down so the two cannot disagree.
    fn delegating(&self, side: u8) -> bool {
        // Any unplanned unit at all: the staff decides what "unplanned"
        // means for each of them — mission executors for the missioned, the
        // battle drill for the threatened, and a hold-and-watch for the
        // rest, which is exactly what a bare commit used to imply. Gating
        // this on missions existing was the second playtest's drill bug: a
        // player who had issued no missions committed straight past the
        // staff, and nobody under fire ever drilled.
        self.state.unplanned_units(side).next().is_some()
    }
}

/// Whether this mod prices a chain of command at all. With no `command` block
/// there is no picture to read and no contact to lose, so every display and
/// order rule below falls back to the fog — which is the game exactly as it
/// was, with no switch anywhere to say so.
fn command_rules(registry: &tactics_core::data::DataRegistry) -> bool {
    registry.command.is_some()
}

/// How the screen shows one unit to the side it is drawn for.
///
/// Under command rules the display follows the *command picture* rather than
/// the side's fog: what her units can see is what they shoot at, and what has
/// been reported to her is what she is allowed to know. The two come apart
/// exactly where the design wants them to — a scout out of contact sees
/// things her commander is never told.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    /// Not on the picture at all: nothing is drawn, and orders that would
    /// reveal it are refused by falling through to the ambush path rather
    /// than by a message.
    Hidden,
    /// Where it really is, drawn solidly. Own units and fresh contacts.
    Real,
    /// A stale contact: last reported position, drawn as a ghost. The unit
    /// itself may be anywhere by now.
    Ghost(Hex),
}

/// Where to draw `unit` for `view_side`, and how solid it is.
///
/// One function because two systems need the same answer and disagreeing
/// would be a fog leak: `sync_units` places the sprite, and `pump_events`
/// asks before letting a move animation take it over. A ghost must never
/// walk the enemy's real route across the screen.
fn shown_to(state: &BattleState, command_rules: bool, view_side: u8, unit: &Unit) -> Shown {
    if unit.side == view_side {
        return Shown::Real;
    }
    if !command_rules {
        return match state.fog.side(view_side).spotted.contains(&unit.id) {
            true => Shown::Real,
            false => Shown::Hidden,
        };
    }
    match state.picture(view_side).iter().find(|c| c.unit == unit.id) {
        Some(contact) if contact.fresh => Shown::Real,
        Some(contact) => Shown::Ghost(contact.at),
        None => Shown::Hidden,
    }
}

// --- markers --------------------------------------------------------------

#[derive(Component, Clone)]
struct BattleScope;

#[derive(Component)]
struct BattleUnit(UnitId);

#[derive(Component)]
struct HpBar(UnitId);

/// The little widgets riding on a unit sprite — the health bar and the black
/// slot behind it. Named as a group because a ghost hides all of them at
/// once: a contact reported an hour ago says where somebody was, never how
/// badly she is hurt now.
#[derive(Component)]
struct UnitBadge(UnitId);

/// The wedge riding over whoever leads a formation.
///
/// Deliberately *not* a [`UnitBadge`]: the badges say how hurt somebody is,
/// which a stale report cannot know, so a ghost hides them. Who is in command
/// is not that kind of fact — it was true when the report was filed and the
/// report is what the ghost is — so the chevron stays, dimmed with the rest
/// of the ghost.
#[derive(Component)]
struct LeaderChevron(UnitId);

/// The battle HUD's four text/image widgets.
///
/// They are one thing conceptually — the panel on the right and the banner
/// above it — and the mutual `Without` filters only exist so Bevy can prove
/// the `&mut Text` queries do not alias. Grouping them keeps that plumbing out
/// of the system signature.
#[derive(SystemParam)]
struct BattleHud<'w, 's> {
    banner: map_render::TextSlot<'w, 's, RoundBanner, LogText, PanelText>,
    log_text: map_render::TextSlot<'w, 's, LogText, RoundBanner, PanelText>,
    panel: map_render::TextSlot<'w, 's, PanelText, RoundBanner, LogText>,
    portrait: Query<'w, 's, &'static mut ImageNode, With<PanelPortrait>>,
}

/// Every per-round highlight, for the despawn-and-rebuild pass.
type HighlightFilter = Or<(
    With<MoveHighlight>,
    With<PlanHighlight>,
    With<FormationHighlight>,
    With<NetRing>,
)>;

/// Unit sprites, excluding the ones a `Mover` animation currently owns.
type UnitSprites<'w, 's> = Query<
    'w,
    's,
    (
        &'static BattleUnit,
        &'static mut Transform,
        &'static mut Visibility,
        &'static mut Sprite,
    ),
    (Without<Mover>, Without<HpBar>),
>;

/// The little health bar riding above each unit.
type HpBars<'w, 's> = Query<
    'w,
    's,
    (&'static HpBar, &'static mut Sprite, &'static mut Transform),
    (With<HpBar>, Without<BattleUnit>),
>;

/// The wedge riding above the health bar on whoever is leading.
type LeaderChevrons<'w, 's> = Query<
    'w,
    's,
    (
        &'static LeaderChevron,
        &'static mut Visibility,
        &'static mut Sprite,
        &'static mut Transform,
    ),
    (Without<BattleUnit>, Without<UnitBadge>, Without<HpBar>),
>;

/// Everything riding on a unit sprite rather than being one: the health bar,
/// the slot behind it, and the leader's chevron.
///
/// Bundled for the same two reasons `BattleHud` is. They are one thing — the
/// furniture `sync_units` hangs off a vehicle — and the mutual `Without`
/// filters exist only so Bevy can prove three `&mut Visibility` / `&mut
/// Sprite` queries over sibling entities do not alias, which is plumbing that
/// has no business in a system signature.
#[derive(SystemParam)]
struct UnitWidgets<'w, 's> {
    bars: HpBars<'w, 's>,
    badges: Query<'w, 's, (&'static UnitBadge, &'static mut Visibility), Without<BattleUnit>>,
    chevrons: LeaderChevrons<'w, 's>,
}

#[derive(Component)]
struct MoveHighlight;

/// Overlay showing what your own units have been ordered to do this round.
#[derive(Component)]
struct PlanHighlight;

/// Overlay under every member of the formation being commanded, so a mission
/// is visibly given to *these four vehicles* rather than to a name in a list.
#[derive(Component)]
struct FormationHighlight;

/// The formation marker's colour: violet, because it belongs to neither
/// side's palette nor to the amber of ground worth taking. It says "these are
/// the girls you are talking to", which is not a fact about the map.
const FORMATION_MARKER: Color = Color::srgba(0.65, 0.5, 1.0, 0.5);

/// The same marker under a girl who cannot hear a word of it. Kept at the
/// violet's weight and swung to red rather than made a new symbol: it is the
/// *same* fact — she is in the formation you are commanding — with the one
/// thing that matters about her tonight said in the colour of a warning.
const FORMATION_CUT_OFF: Color = Color::srgba(1.0, 0.3, 0.35, 0.55);

/// One hex of the leader's radio horizon.
///
/// A ring rather than a disc, and that is a legibility decision rather than a
/// cheap one: the base mod's eight-hex radio fills two hundred tiles, which
/// would bury the map it is drawn over. What a player actually reads off it
/// is the *edge* — how much further this platoon can be sent before it stops
/// answering — so the edge is the only part drawn.
#[derive(Component)]
struct NetRing;

/// Signals green: the last colour in the battle palette not already spoken
/// for. Blue is move range, amber is ground worth taking, sky blue is a way
/// off the map, red is a gun line and violet is the formation itself — so the
/// wire gets green — and at nearly twice the alpha of the filled overlays,
/// because a line one tile wide has to hold its own against terrain that is
/// already olive. The first pass at 0.62 read as a slightly brighter patch of
/// grass in the screenshot loop rather than as a drawn line.
const NET_RING: Color = Color::srgba(0.3, 1.0, 0.62, 0.78);

#[derive(Component)]
struct HoverHighlight;

#[derive(Component)]
struct SelectHighlight;

/// The two markers that follow the mouse and the selection.
///
/// They are one thing — where the player's attention is — and their mutual
/// `Without` filters exist only so Bevy can prove the two `&mut Transform`
/// queries do not alias, which is the same reason `BattleHud` and
/// `UnitWidgets` exist.
#[derive(SystemParam)]
struct Cursors<'w, 's> {
    hover: map_render::MarkerQuery<'w, 's, HoverHighlight, SelectHighlight>,
    select: map_render::MarkerQuery<'w, 's, SelectHighlight, HoverHighlight>,
}

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

/// A hex belonging to `map.objectives()[index]`, tinted by who holds it.
#[derive(Component)]
struct ObjectiveMarker {
    index: usize,
}

/// Ground nobody has taken yet: a pale amber that reads as "worth something"
/// without belonging to either side's colour.
const OBJECTIVE_NEUTRAL: Color = Color::srgba(1.0, 0.85, 0.35, 0.30);

/// An exit lane. Deliberately not the amber of ground to be taken — an exit
/// is somewhere to go, not something to hold, and colouring the two alike
/// would invite the player to garrison their own way out.
const OBJECTIVE_EXIT: Color = Color::srgba(0.45, 0.8, 1.0, 0.30);

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
                    update_objective_markers,
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
    view: map_render::View,
    mut log: ResMut<BattleLog>,
    mut focus: ResMut<CameraFocus>,
) {
    let registry = &mods.0;
    let pending = pending
        .map(|p| p.clone())
        .unwrap_or(PendingBattle::Scenario {
            map_id: "river_crossing".into(),
        });

    let (mut state, field) = match &pending {
        PendingBattle::Scenario { map_id } => (
            BattleState::from_map(registry, map_id, seed()).expect("scenario map builds"),
            None,
        ),
        PendingBattle::Field {
            map_id,
            roster,
            attacker,
            defender,
            sides,
            attacker_side,
            forces,
        } => {
            let file = registry.map(map_id).expect("field battle map exists");
            let map = tactics_core::map::HexMap::from_map_file(file).expect("map parses");
            let (placements, crews, origins) = deploy(registry, &map, forces, *attacker_side);
            // The campaign's own roster, so these are the same girls who will
            // carry whatever happens here back out again.
            let state = BattleState::from_placements(
                registry,
                map,
                sides.clone(),
                &placements,
                &crews,
                roster.clone(),
                seed(),
            );
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
    let mut ai = AiDriver::new();
    for (i, side) in state.sides.iter().enumerate() {
        match &side.ai {
            Some(cfg) => {
                ai.insert(
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
                ai.insert(
                    i as u8,
                    make_battle_planner(&cfg, seed().wrapping_add(i as u64), registry),
                );
            }
            None => {}
        }
    }

    // The view pivots on the map's centroid. Written through `Commands`
    // rather than a `ResMut` so this system can also take `View`, which reads
    // the same resource — two conflicting accesses would panic at runtime.
    let center = state.map.center();
    commands.insert_resource(ViewCenter(center));
    map_render::spawn_map(
        &mut commands,
        &art,
        &state.map,
        view.rotation(),
        true,
        BattleScope,
    );
    commands.insert_resource(CurrentMap(state.map.clone()));

    // Objectives are drawn once and only ever recoloured, because which hexes
    // they cover cannot change during a battle. They are `HexOverlay`s, so
    // `reposition_map` carries them through view rotation with everything
    // else.
    for (index, objective) in state.map.objectives().iter().enumerate() {
        for hex in &objective.hexes {
            let overlay = HexOverlay::face(*hex);
            commands.spawn((
                Sprite {
                    image: art.face.clone(),
                    color: OBJECTIVE_NEUTRAL,
                    ..default()
                },
                Transform::from_translation(overlay.translation(
                    &state.map,
                    view.rotation(),
                    center,
                )),
                overlay,
                ObjectiveMarker { index },
                BattleScope,
            ));
        }
    }

    for unit in state.alive_units() {
        spawn_unit_sprite(&mut commands, &art, unit.id, unit.side, &unit.vehicle);
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
         Enter commit, Q/E rotate. F picks a formation; G advance, X assault, H hold, R recon \
         on the hovered hex, W withdraw; Shift queues a mission behind the current one.",
    );

    // What was decided on the campaign map is what the formations here try to
    // do. Issued after the log is cleared so the player can read the orders
    // that arrived with her.
    if let PendingBattle::Field {
        forces,
        attacker,
        defender,
        ..
    } = &pending
    {
        for line in inherit_army_missions(registry, &mut state, forces, *attacker, *defender) {
            log.push(line);
        }
    }

    let (map_center, _) = iso::project(center, 0, view.rotation(), center);
    focus.0 = map_center;

    commands.insert_resource(Battle {
        state,
        ai,
        anim: VecDeque::new(),
        pace: Timer::from_seconds(0.28, TimerMode::Repeating),
        selected: None,
        formation: None,
        delegate: None,
        move_range: HashMap::new(),
        range_dirty: false,
        mode: InputMode::Normal,
        field,
        exit_timer: None,
    });
}

/// Hand the battle whatever its armies were already trying to do, and say so
/// in the log.
///
/// Only [`ArmyMission::Withdraw`] maps onto a battle mission today, and only
/// for the two *principal* armies — the one that attacked and the one that was
/// attacked — because those are the two whose intent caused this fight;
/// a neighbour who piled in came to help with somebody else's decision.
/// A withdrawing army's formations are ordered out by the nearest lane their
/// side may use, which is the same rule their own commander would have applied
/// once they were beaten: the campaign's order is that they should not wait to
/// be beaten first.
///
/// `Advance` and `Hold` deliberately do **not** map. The battle brain already
/// advances on the ground the map declares worth holding, so translating them
/// would either say what it is already saying or overrule it with a hex chosen
/// four kilometres away — and an operational advance is not a tactical one. If
/// they ever do map, it should be through the objectives, not around them.
///
/// The orders go through `BattleState::apply` like everyone else's, so they
/// travel at the signals net's speed and appear in the log and in any replay.
/// A map that offers the side no exit produces nothing at all.
fn inherit_army_missions(
    registry: &tactics_core::data::DataRegistry,
    state: &mut BattleState,
    forces: &[BattleForce],
    attacker: ArmyId,
    defender: ArmyId,
) -> Vec<String> {
    let mut lines = Vec::new();
    for principal in [attacker, defender] {
        let Some(force) = forces
            .iter()
            .find(|f| f.army == principal)
            .filter(|f| matches!(f.mission, Some(ArmyMission::Withdraw { .. })))
        else {
            continue;
        };
        let side = force.side;
        let ordered: Vec<usize> = state
            .formations()
            .iter()
            .enumerate()
            .filter(|(_, f)| f.side == side)
            .map(|(index, _)| index)
            .collect();
        for index in ordered {
            let Some(via) = formation_exit(state, index) else {
                continue;
            };
            let name = state.formations()[index].id.clone();
            let order = Order::SetMission {
                formation: FormationId(index as u32),
                mission: Mission::Withdraw { via },
            };
            if state.apply(registry, &order).is_ok() {
                lines.push(format!("{name} is under orders to break contact."));
            }
        }
    }
    lines
}

/// Line the attacking armies up along the west edge and the defenders along
/// the east. Returns the placements plus, for each one, the army it came
/// from, so casualties can be reported back to the right army afterwards.
#[allow(clippy::type_complexity)]
fn deploy(
    registry: &tactics_core::data::DataRegistry,
    map: &tactics_core::map::HexMap,
    forces: &[BattleForce],
    attacker_side: u8,
) -> (Vec<UnitPlacement>, Vec<Vec<GirlId>>, Vec<ArmyId>) {
    // Tiles a vehicle can actually sit on, nearest edge first. Taking spots
    // in this order lets a side deploy as deep inland as it needs to, so
    // three armies fit where one used to.
    let deployable = |west: bool| -> Vec<Hex> {
        let mut spots: Vec<(i32, i32, Hex)> = map
            .iter()
            .filter(|(_, tile)| {
                registry.terrain(&tile.terrain).is_some_and(|t| {
                    t.cost_for(tactics_core::data::MovementClass::Tracked)
                        .is_some()
                })
            })
            // A side deploys at the shallowest tiles of its own edge, which
            // is precisely where that side's retreat lane is. Standing on an
            // exit means taking it, so without this the leading vehicles
            // would drive off the map on the first tick and the battle would
            // be over before anyone saw an enemy. Nobody forms up on the road
            // home.
            .filter(|(hex, _)| {
                !map.objectives()
                    .iter()
                    .any(|o| o.kind == ObjectiveKind::Exit && o.contains(*hex))
            })
            .map(|(hex, _)| {
                let [col, row] = tactics_core::hex_to_offset(hex);
                (if west { col } else { -col }, row, hex)
            })
            .collect();
        spots.sort_unstable_by_key(|(depth, row, _)| (*depth, *row));
        spots.into_iter().map(|(_, _, hex)| hex).collect()
    };

    // An army *is* a formation, which is what the design doc means by
    // "`ArmyPlacement` on the overworld maps naturally". The declarations
    // themselves belong to the terrain map — that is where a battlefield's
    // order of battle is written, and it is the case `CommandState::from_placements`
    // already documents itself against — so an army fills the next-declared
    // formation of its own side, first army into the first-declared one, and
    // its first vehicle leads it. A map that declares none for a side leaves
    // that side the flat pool it has always been, so a field battle on a
    // formationless map is exactly the battle it was.
    let slots = |side: u8| -> Vec<String> {
        map.formations()
            .iter()
            .filter(|f| f.side == side)
            .map(|f| f.id.clone())
            .collect()
    };
    let mut taken: HashMap<u8, usize> = HashMap::new();
    let mut led: Vec<String> = Vec::new();

    let mut placements = Vec::new();
    let mut crews = Vec::new();
    let mut origins = Vec::new();
    for west in [true, false] {
        let mut spots = deployable(west).into_iter();
        for force in forces.iter().filter(|f| (f.side == attacker_side) == west) {
            // More armies than the map named formations is an ordinary muster
            // — three companies piling into a two-platoon map — and they wrap
            // round rather than being left out of the chain of command.
            let available = slots(force.side);
            let formation = (!available.is_empty()).then(|| {
                let next = taken.entry(force.side).or_default();
                let id = available[*next % available.len()].clone();
                *next += 1;
                id
            });
            for unit in &force.units {
                let Some(hex) = spots.next() else { break };
                // Seniority is arrival order, exactly as it is for a map's own
                // placements: the first vehicle into a formation leads it, and
                // succession works down the list from there.
                let leads = formation.as_ref().is_some_and(|id| !led.contains(id));
                if leads {
                    led.push(formation.clone().expect("leads implies a formation"));
                }
                // The crew travels alongside as girl handles rather than in
                // the placement: a `UnitPlacement` names crew by definition
                // id, which is the thing this whole refactor is getting away
                // from.
                placements.push(UnitPlacement {
                    at: tactics_core::hex_to_offset(hex),
                    side: force.side,
                    vehicle: unit.vehicle.clone(),
                    crew: Vec::new(),
                    name: unit.name.clone(),
                    facing: None,
                    formation: formation.clone(),
                    leads,
                });
                crews.push(unit.crew.clone());
                origins.push(force.army);
            }
        }
    }
    (placements, crews, origins)
}

fn spawn_unit_sprite(commands: &mut Commands, art: &ArtCache, id: UnitId, side: u8, vehicle: &str) {
    commands
        .spawn((
            Sprite {
                image: art
                    .vehicle_sprite(vehicle, side % iso::SIDE_COLORS.len() as u8, 0)
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
                UnitBadge(id),
            ));
            parent.spawn((
                Sprite {
                    color: Color::srgb(0.3, 0.9, 0.3),
                    custom_size: Some(Vec2::new(28.0, 3.0)),
                    ..default()
                },
                Transform::from_translation(Vec3::new(0.0, 22.0, 0.2)),
                HpBar(id),
                UnitBadge(id),
            ));
            // Spawned for everybody and shown for the few, because command
            // passes: the girl who inherits a formation mid-battle needs the
            // wedge to appear over her without anything spawning a sprite in
            // the middle of a round. `sync_units` reads `Formation.leader`
            // every frame, so there is no event to miss.
            parent.spawn((
                Sprite {
                    image: art.chevron.clone(),
                    ..default()
                },
                Transform::from_translation(Vec3::new(0.0, 31.0, 0.2)),
                Visibility::Hidden,
                LeaderChevron(id),
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
    view: map_render::View,
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
            let (pos, z) = iso::project(last, elev, view.rotation(), view.center());
            transform.translation = Vec3::new(pos.x, pos.y + 10.0, z + 1.5);
            commands.entity(entity).remove::<Mover>();
            continue;
        }
        let i = mover.progress.floor() as usize;
        let t = mover.progress.fract();
        let (a, b) = (mover.path[i], mover.path[i + 1]);
        let ea = map.0.get(a).map(|t| t.elevation).unwrap_or(0);
        let eb = map.0.get(b).map(|t| t.elevation).unwrap_or(0);
        let (pa, za) = iso::project(a, ea, view.rotation(), view.center());
        let (pb, zb) = iso::project(b, eb, view.rotation(), view.center());
        // Snapped so a moving unit steps across whole pixels instead of
        // shimmering through sub-texel positions.
        let pos = pa.lerp(pb, t).round();
        transform.translation = Vec3::new(pos.x, pos.y + 10.0, za.max(zb) + 1.5);
    }
}

// A Bevy system's parameters are its dependency list, and these eight do not
// form any smaller noun: commands, the clock, content, the battle, the log,
// the view, and two disjoint queries. Bundling them further would invent a
// type that exists only to satisfy a lint. The groupings that *were* real —
// the view, the HUD widgets — already have names.
#[allow(clippy::too_many_arguments)]
fn pump_events(
    mut commands: Commands,
    time: Res<Time>,
    mods: Res<Mods>,
    mut battle: ResMut<Battle>,
    mut log: ResMut<BattleLog>,
    view: map_render::View,
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
        if matches!(
            battle.anim.front(),
            Some(BattleEvent::TickStarted { .. }) | None
        ) {
            break;
        }
    }
    if drained.is_empty() {
        return;
    }
    // Command traffic is a side's own business: the enemy commander's
    // orders, her formations' contact troubles and her radio queue must not
    // read out in the player's log — that is her net, and listening to it is
    // the electronic-warfare future, not a freebie. Fighting events (shots,
    // spots, wrecks) stay side-blind exactly as before.
    let view_side = battle.view_side();
    let own = |formation: &str| {
        battle
            .state
            .formations()
            .iter()
            .find(|f| f.id == formation)
            .is_none_or(|f| f.side == view_side)
    };
    let own_unit = |unit: &UnitId| {
        battle
            .state
            .units
            .get(unit.index())
            .is_none_or(|u| u.side == view_side)
    };
    drained.retain(|event| match event {
        BattleEvent::MissionAssigned { formation, .. }
        | BattleEvent::MissionReceived { formation, .. }
        | BattleEvent::MissionCompleted { formation, .. }
        | BattleEvent::CommandPassed { formation, .. } => own(formation),
        BattleEvent::OutOfContact { unit }
        | BattleEvent::ContactRestored { unit }
        | BattleEvent::OrdersWaiting { unit }
        | BattleEvent::OrdersDelivered { unit }
        | BattleEvent::TookCover { unit, .. }
        | BattleEvent::ContactReported { by: unit, .. } => own_unit(unit),
        // How much ammunition the enemy has left is her quartermaster's
        // secret, not something the sound of her gun gives away — and what
        // is broken or bleeding inside her hull even more so. A brew-up or
        // a bail-out, by contrast, is visible across the battlefield.
        BattleEvent::WeaponDry { unit, .. }
        | BattleEvent::CrewHit { unit, .. }
        | BattleEvent::ModuleHit { unit, .. } => own_unit(unit),
        _ => true,
    });
    if drained.is_empty() {
        return;
    }
    let registry = &mods.0;
    let rules = command_rules(registry);
    let view_side = battle.view_side();
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
                // Collapse quiet rounds. The log keeps eight lines, and a
                // banner every round eats all of them — which defeats the
                // whole point of reporting morale and refusals, since the
                // player is meant to see a crew wavering *before* it costs
                // them. A round in which nothing happened does not need
                // announcing twice.
                let quiet = log
                    .0
                    .back()
                    .is_some_and(|last| last.starts_with("- Round "));
                if quiet {
                    log.0.pop_back();
                }
                log.push(format!("- Round {round}: orders -"));
                battle.range_dirty = true;
            }
            // The separator itself has nothing to show.
            BattleEvent::TickStarted { .. } => {}
            // Movers spawned in the same beat animate together, which is the
            // whole point of resolving a tick at a time. A unit the screen is
            // not showing where it really is — a ghost — must not be handed
            // to the animator: the walk would drag the marker along the
            // enemy's true route, which is the picture leaking through the
            // one system that does not consult it.
            BattleEvent::UnitMoved { unit, path } => {
                let real = battle
                    .state
                    .unit(*unit)
                    .is_some_and(|u| shown_to(&battle.state, rules, view_side, u) == Shown::Real);
                if real && let Some(entity) = entity_of(*unit) {
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
                spawn_puff(
                    &mut commands,
                    *at,
                    view.rotation(),
                    view.center(),
                    Color::srgb(1.0, 0.9, 0.4),
                );
            }
            BattleEvent::ShotHit {
                attacker,
                target,
                facing,
                ..
            } => {
                log.push(format!(
                    "{} penetrates {} through the {:?}.",
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
                spawn_puff(
                    &mut commands,
                    *at,
                    view.rotation(),
                    view.center(),
                    Color::srgba(0.8, 0.8, 0.8, 0.8),
                );
            }
            BattleEvent::UnitDestroyed { unit, at } => {
                log.push(format!("{} is destroyed!", name(*unit)));
                spawn_puff(
                    &mut commands,
                    *at,
                    view.rotation(),
                    view.center(),
                    Color::srgb(1.0, 0.4, 0.1),
                );
                if let Some(entity) = entity_of(*unit) {
                    commands.entity(entity).despawn();
                }
            }
            BattleEvent::UnitSpotted { unit, by_side, .. } => {
                if battle.state.sides[*by_side as usize].ai.is_none() {
                    log.push(format!("Enemy spotted: {}", name(*unit)));
                }
            }
            // Said in the log, because a girl doing something other than what
            // she was told has to be attributable or it reads as a bug.
            BattleEvent::MoraleChanged { unit, rung, obeys } => {
                let who = battle
                    .state
                    .unit(*unit)
                    .map(|u| u.name.clone())
                    .unwrap_or_else(|| "A crew".into());
                log.push(if *obeys {
                    format!("{who} is {rung}.")
                } else {
                    format!("{who} is {rung} and will not advance.")
                });
            }
            BattleEvent::OrderRefused { unit, rung } => {
                let who = battle
                    .state
                    .unit(*unit)
                    .map(|u| u.name.clone())
                    .unwrap_or_else(|| "A crew".into());
                log.push(format!("{who} refuses to advance - {rung}."));
            }
            // The mid-round drill. Same sentence shape as the planning-table
            // drill's line, so the player learns one idiom for "she decided
            // this herself" wherever in the round it happens.
            BattleEvent::TookCover { unit, .. } => {
                log.push(format!(
                    "{} is under fire and breaks for cover.",
                    name(*unit)
                ));
            }
            BattleEvent::CrewHit { unit, girl, out } => {
                let who = battle
                    .state
                    .roster
                    .get(*girl)
                    .map(|g| g.name.clone())
                    .unwrap_or_else(|| "somebody".into());
                log.push(if *out {
                    format!(
                        "{who} is hit aboard {} and slumps at her station.",
                        name(*unit)
                    )
                } else {
                    format!("{who} is wounded aboard {}.", name(*unit))
                });
            }
            BattleEvent::ModuleHit {
                unit,
                module,
                destroyed,
            } => {
                let what = mods
                    .0
                    .module(module)
                    .map(|m| m.name.clone())
                    .unwrap_or_else(|| module.clone());
                log.push(format!(
                    "{}'s {what} is {}.",
                    name(*unit),
                    if *destroyed { "destroyed" } else { "damaged" }
                ));
            }
            BattleEvent::BrewedUp { unit } => {
                log.push(format!("{} brews up!", name(*unit)));
            }
            BattleEvent::Abandoned { unit } => {
                log.push(format!("The crew abandons {}.", name(*unit)));
            }
            // The armor holding is news the player must hear, or the shot
            // reads as the game eating a hit.
            BattleEvent::ShotBounced { target, facing, .. } => {
                log.push(format!(
                    "The round bounces off {}'s {facing:?} armor.",
                    name(*target)
                ));
            }
            BattleEvent::WeaponDry { unit, weapon } => {
                log.push(format!(
                    "{} has fired her last {weapon} round.",
                    name(*unit)
                ));
            }
            // Withdrawing is not dying, and the screen has to say so plainly:
            // the sprite vanishes either way, and a player who reads a
            // successful withdrawal as a loss has been told a lie by the UI.
            BattleEvent::UnitExited { unit, at, .. } => {
                log.push(format!("{} withdraws off the map.", name(*unit)));
                spawn_puff(
                    &mut commands,
                    *at,
                    view.rotation(),
                    view.center(),
                    Color::srgb(0.6, 0.85, 1.0),
                );
                if let Some(entity) = entity_of(*unit) {
                    commands.entity(entity).despawn();
                }
            }
            // Ground changing hands is worth saying out loud: it is the only
            // thing that moves the score, and a battle decided on points that
            // never mentioned the points would read as arbitrary.
            BattleEvent::ObjectiveTaken {
                objective, side, ..
            } => {
                let what = battle
                    .state
                    .map
                    .objectives()
                    .iter()
                    .find(|o| &o.id == objective)
                    .map(|o| o.name.clone())
                    .unwrap_or_else(|| objective.clone());
                log.push(match side {
                    Some(s) => format!("{what} taken by {}.", battle.state.sides[*s as usize].name),
                    None => format!("{what} is contested."),
                });
            }
            // A formation being given a mission goes in the log for the reason
            // the whole system is built around legibility: four vehicles
            // turning north together should be explained by a line the player
            // already read, not inferred afterwards. Both commanders speak
            // here — the enemy's brain and the player's own keystroke — which
            // is the point of routing missions through the order stream.
            BattleEvent::MissionAssigned { formation, mission } => {
                let who = formation_name(&battle.state, formation);
                log.push(format!(
                    "{who} ordered to {}.",
                    mission_sentence(&battle.state, Some(mission))
                ));
            }
            // ...and the same order arriving, some ticks later, when the mod
            // prices a signals net. The gap between the two lines is the thing
            // the player is meant to feel: her platoon carried on doing the
            // last thing it heard, and the log says why.
            BattleEvent::MissionReceived { formation, mission } => {
                let who = formation_name(&battle.state, formation);
                log.push(format!(
                    "{who} receives the order to {}.",
                    mission_sentence(&battle.state, Some(mission))
                ));
            }
            // A unit that has stopped answering the radio is announced for the
            // same reason a wavering crew is: she is about to do something
            // other than what she was told, and silent deviation is
            // indistinguishable from a bug.
            // The plan advancing is an order the player gave when she queued
            // the leg; saying both halves keeps the turn legible.
            BattleEvent::MissionCompleted { formation, mission } => {
                let who = formation_name(&battle.state, formation);
                log.push(format!(
                    "{who} has done it: {} complete. Moving to the next order.",
                    mission_sentence(&battle.state, Some(mission))
                ));
            }
            BattleEvent::OutOfContact { unit } => {
                log.push(format!("{} is out of contact.", name(*unit)));
            }
            BattleEvent::ContactRestored { unit } => {
                log.push(format!("{} is back in contact.", name(*unit)));
            }
            // The player's own order, acknowledged rather than refused. This
            // line replaces the flat refusal the input path used to print, and
            // it has to be at least as loud: she clicked, something visibly
            // did not happen on the board, and the only thing standing between
            // that and "the game ate my click" is this sentence.
            BattleEvent::OrdersWaiting { unit } => {
                log.push(format!(
                    "No contact with {} - orders will be radioed when she can hear them.",
                    name(*unit)
                ));
            }
            BattleEvent::OrdersDelivered { unit } => {
                // With the destination in the sentence when the order is a
                // march: the player learns both that it got through and what
                // will now happen across the coming rounds.
                match battle.state.units.get(unit.index()).and_then(|u| u.tasking) {
                    Some(to) => log.push(format!(
                        "{} has her orders and is on her way to {}.",
                        name(*unit),
                        hex_label(to)
                    )),
                    None => log.push(format!("{} has her orders.", name(*unit))),
                }
            }
            // The upward wire: a report reaching the commander is the log's
            // business even when the spot itself was already shown, because
            // who *told* her — and that somebody could — is the information.
            BattleEvent::ContactReported { unit, by, at } => {
                log.push(format!(
                    "{} reports {} at {}.",
                    name(*by),
                    name(*unit),
                    hex_label(*at)
                ));
            }
            // A formation changing hands is the loudest thing that can happen
            // to it short of dying, and the player is about to watch its whole
            // pressure jump: the line explaining why has to arrive first.
            BattleEvent::CommandPassed {
                formation,
                from,
                to,
            } => {
                let who = formation_name(&battle.state, formation);
                log.push(format!(
                    "{} is gone. {} takes command of {who}.",
                    name(*from),
                    name(*to)
                ));
            }
            BattleEvent::BattleEnded { winner, reason } => {
                let text = match (winner, reason) {
                    (Some(w), EndReason::Objectives) => format!(
                        "Victory on objectives: {} ({} points)",
                        battle.state.sides[*w as usize].name,
                        battle.state.score(*w)
                    ),
                    (Some(w), EndReason::Stalemate) => format!(
                        "Contact lost. {} holds the ground, {} to {}.",
                        battle.state.sides[*w as usize].name,
                        battle.state.score(*w),
                        battle
                            .state
                            .score
                            .iter()
                            .enumerate()
                            .filter(|(s, _)| *s != *w as usize)
                            .map(|(_, v)| *v)
                            .max()
                            .unwrap_or(0),
                    ),
                    // The reason is the news here, not the result: a battle
                    // that ended because somebody's headquarters died has to
                    // say so, or the player watching a healthy force walk off
                    // the field will read it as a bug.
                    (Some(w), EndReason::Decapitated) => format!(
                        "Command broken. {} takes the field.",
                        battle.state.sides[*w as usize].name
                    ),
                    (Some(w), _) => format!("Victory: {}", battle.state.sides[*w as usize].name),
                    (None, EndReason::Stalemate) => {
                        "Contact lost. Both sides break off, with nothing to show for it.".into()
                    }
                    (None, EndReason::Eliminated) => "Mutual destruction.".into(),
                    (None, EndReason::Objectives) => "The ground changed hands.".into(),
                    (None, EndReason::Decapitated) => {
                        "Command broken, with nobody left to profit by it.".into()
                    }
                };
                log.push(text);
                battle.exit_timer = Some(Timer::from_seconds(2.5, TimerMode::Once));
            }
        }
    }
}

/// What to call a formation in the log: the name its map gave it, falling back
/// to the bare id so a formation from a mod this build does not know about is
/// still named rather than silently anonymous.
fn formation_name(state: &BattleState, id: &str) -> String {
    state
        .map
        .formations()
        .iter()
        .find(|f| f.id == id)
        .map(|f| f.display_name().to_string())
        .unwrap_or_else(|| id.to_string())
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
    // One decision per side per frame, not a whole round: an MCTS side can
    // take seconds per order, and the UI has to stay responsive under it.
    let battle = &mut *battle;
    let decisions = battle.ai.step(&mods.0, &mut battle.state);
    if !decisions.is_empty() {
        battle.range_dirty = true;
        // Planning orders used to be silent, but a mission being assigned is
        // news the log carries; route whatever the orders announced through
        // the same animation queue every other event takes.
        for decision in decisions {
            battle.anim.extend(decision.events);
        }
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
    view: map_render::View,
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
        battle.formation = None;
        battle.move_range.clear();
        battle.range_dirty = true;
        commit_round(registry, &mut battle, side, &mut log);
        return;
    }
    if keys.just_pressed(KeyCode::Escape) || buttons.just_pressed(MouseButton::Right) {
        battle.selected = None;
        battle.formation = None;
        battle.move_range.clear();
        battle.range_dirty = true;
        battle.mode = InputMode::Normal;
        return;
    }
    // F walks the player's formations. Talking to a platoon and talking to a
    // crew are different conversations, so taking up one drops the other.
    if keys.just_pressed(KeyCode::KeyF) {
        battle.selected = None;
        battle.move_range.clear();
        battle.mode = InputMode::Normal;
        battle.cycle_formation();
        battle.range_dirty = true;
        return;
    }
    // Hold: stay put and shoot at whatever appears.
    if keys.just_pressed(KeyCode::KeyV) {
        if let Some(unit) = battle.selected {
            set_intent(
                registry,
                &mut battle,
                &Order::Radio {
                    unit,
                    to: None,
                    fire: Some(FireIntent::Hold),
                },
                &mut log,
            );
        }
        return;
    }
    // Take back a unit's orders; nothing is locked in until the commit.
    if keys.just_pressed(KeyCode::KeyC) {
        if let Some(unit) = battle.selected {
            set_intent(
                registry,
                &mut battle,
                &Order::ClearIntent { unit },
                &mut log,
            );
        }
        return;
    }
    if keys.just_pressed(KeyCode::KeyB) && battle.selected.is_some() {
        battle.mode = InputMode::BlindFire;
        log.push("Blind fire: click a target tile.");
        return;
    }

    let hovered = view.hovered(&battle.state.map);

    // Mission orders. These go through `Order::SetMission` — the same entry
    // point the commander brain speaks through — so the player's decisions
    // and the AI's produce the same events, land in the same save and read
    // the same way in a replay. That is the whole reason missions are orders
    // rather than a UI concept.
    if let Some(index) = battle.formation
        && let Some(asked) = mission_from_keys(&keys, &battle.state, index, hovered)
    {
        match asked {
            Ok(mission) => {
                // Shift queues instead of replacing: "…and then this." The
                // engine refuses a leg behind a stand-fast or a retreat, and
                // that refusal reaches the log like any other.
                let queue = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
                let formation = FormationId(index as u32);
                let order = if queue {
                    Order::QueueMission { formation, mission }
                } else {
                    Order::SetMission { formation, mission }
                };
                let battle = &mut *battle;
                match battle.state.apply(registry, &order) {
                    // `MissionAssigned` comes back out here; pushing it into
                    // the same queue the resolution uses is what makes the
                    // player's own order arrive in the log beside the enemy's.
                    Ok(events) => {
                        battle.anim.extend(events);
                        battle.range_dirty = true;
                    }
                    Err(e) => log.push(format!("Order refused: {e}")),
                }
            }
            Err(why) => log.push(why),
        }
        return;
    }

    // A = engage the hovered enemy with the best weapon.
    if keys.just_pressed(KeyCode::KeyA) {
        if let (Some(unit), Some(hex)) = (battle.selected, hovered) {
            match aim_at(&battle.state, command_rules(registry), hex, side) {
                Aim::Enemy(target) => {
                    engage_with_best(registry, &mut battle, unit, target, &mut log)
                }
                Aim::Ghost(why) => log.push(why),
                Aim::Nothing => {}
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
            set_intent(
                registry,
                &mut battle,
                &Order::Radio {
                    unit,
                    to: None,
                    fire: Some(FireIntent::Area { at: hex, weapon }),
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
        battle.formation = None;
        battle.range_dirty = true;
        battle.move_range = reachable(registry, &battle.state, id);
        return;
    }

    // Click an enemy the player is entitled to shoot at: engage with the best
    // weapon. Enemies she has not been told about fall through to the move
    // branch, so probing the fog — or the picture — by clicking is not a way
    // to find them; you drive in and get ambushed like anyone else.
    match aim_at(&battle.state, command_rules(registry), hex, side) {
        Aim::Enemy(target) => {
            if let Some(unit) = battle.selected {
                engage_with_best(registry, &mut battle, unit, target, &mut log);
            }
            return;
        }
        // A ghost is drawn, so unlike an unreported enemy it has to be
        // answered: the player can see the marker and needs to be told why
        // her gunner will not lay on it.
        Aim::Ghost(why) => {
            log.push(why);
            return;
        }
        Aim::Nothing => {}
    }

    // Click a reachable tile: route the unit there for this round.
    if let Some(unit) = battle.selected
        && battle.move_range.contains_key(&hex)
    {
        set_intent(
            registry,
            &mut battle,
            &Order::Radio {
                unit,
                to: Some(hex),
                fire: None,
            },
            &mut log,
        );
    }
}

/// Apply one planning order and refresh the overlays that show it.
///
/// The player's direct orders travel as [`Order::Radio`] rather than
/// `SetMove`/`SetFire`, because they are the *commander* speaking and a
/// commander needs a wire. The engine decides what that costs: a girl on the
/// net gets her orders instantly and identically to before, one who is not
/// has them held at the radio and delivered when she can hear again. Nothing
/// is gated here any more — the refusal this function used to print became an
/// acknowledgement, and it now hangs off the `OrdersWaiting` event so the
/// player's own order and the enemy's news arrive in the log by the same road.
///
/// Events come back out and go into the animation queue for exactly that
/// reason; the mission path already does the same.
fn set_intent(
    registry: &tactics_core::data::DataRegistry,
    battle: &mut Battle,
    order: &Order,
    log: &mut BattleLog,
) {
    match battle.state.apply(registry, order) {
        Ok(events) => {
            battle.anim.extend(events);
            battle.range_dirty = true;
        }
        Err(e) => log.push(format!("Order refused: {e}")),
    }
}

/// Close the player's planning — through her staff, if she left them
/// anything to do.
///
/// A unit she did not order herself, in a formation she *has* given a
/// mission, is planned by that formation's executor: the same object an AI
/// side runs on, under the same doctrine, in service of the same mission.
/// That is what delegation means here, and it is why the mission vocabulary
/// is shared — her platoon carries out her intent by the same machinery the
/// enemy's does. With nothing delegated this is the bare commit it always was.
fn commit_round(
    registry: &tactics_core::data::DataRegistry,
    battle: &mut Battle,
    side: u8,
    log: &mut BattleLog,
) {
    if !battle.delegating(side) {
        match battle.state.apply(registry, &Order::Commit { side }) {
            Ok(events) => battle.anim.extend(events),
            Err(e) => log.push(format!("Can't commit: {e}")),
        }
        return;
    }
    let Battle {
        state,
        delegate,
        anim,
        ..
    } = battle;
    let driver = delegate.get_or_insert_with(|| {
        // Difficulty 5, i.e. no scoring noise: difficulty is how well an
        // *opponent* executes, and the player's own subordinates have no
        // reason to blunder on her behalf. Doctrine is left unstated so each
        // formation fights under whatever its map declared, falling back to
        // the balanced default.
        let config = AiConfig {
            planner: "command".into(),
            difficulty: 5,
            doctrine: None,
        };
        let mut driver = AiDriver::new();
        driver.insert(
            side,
            Box::new(SideCommand::executor_only(&config, seed(), registry)),
        );
        driver
    });
    // The executors' own `Commit` closes the side, so this both fills the
    // gaps and hands the round in.
    let mut events = Vec::new();
    let mut refused = Vec::new();
    let mut drilled = Vec::new();
    driver.plan_round_with(registry, state, |decision| {
        events.extend(decision.events.iter().cloned());
        if let Some(error) = &decision.rejected {
            refused.push(error.to_string());
        }
        if let Order::SetMove { unit, .. } = decision.order {
            drilled.push(unit);
        }
    });
    anim.extend(events);
    for error in refused {
        log.push(format!("Your staff fumbled an order: {error}"));
    }
    // A move for a unit outside any mission is the battle drill: she is
    // under fire and nobody told her anything, so she is taking cover on
    // her own. Said out loud, because a vehicle moving without a visible
    // order behind it is indistinguishable from a bug — the same bargain
    // every deviation in this game makes. (Filtered here rather than in the
    // closure: the driver holds the state mutably while it runs, and an
    // executor-only side never changes a formation's missions mid-drive, so
    // reading them afterwards answers the same.)
    drilled.retain(|unit| {
        state
            .command
            .formation_of(*unit)
            .is_none_or(|f| f.latest_mission().is_none())
    });
    for unit in drilled {
        let name = state
            .units
            .get(unit.index())
            .map(|u| u.name.clone())
            .unwrap_or_default();
        log.push(format!("{name} is under fire and takes cover on her own."));
    }
}

/// The mission the player just asked for, if she pressed one of the mission
/// keys. `Err` is a key that was pressed but could not be turned into an
/// order — nothing under the cursor, no lane out — and carries the line to
/// say so, because a keystroke that does nothing at all reads as broken.
///
/// `W` doubles as the camera's pan-up key, which is the same bargain `A`
/// already makes: a tap issues the order, a hold moves the camera.
fn mission_from_keys(
    keys: &ButtonInput<KeyCode>,
    state: &BattleState,
    formation: usize,
    hovered: Option<Hex>,
) -> Option<Result<Mission, String>> {
    let needs_ground = |what: &str| Err(format!("Hover the ground first, then press {what}."));
    if keys.just_pressed(KeyCode::KeyG) {
        return Some(match hovered {
            Some(to) => Ok(Mission::Advance { to }),
            None => needs_ground("G to advance"),
        });
    }
    // `X` rather than the mnemonic `T`: T already commits the round beside
    // Enter, and a key that both closes planning and issues an attack is a
    // key nobody can press with confidence. X is free, sits beside the other
    // order keys (C, V, B), and reads as the attack it is.
    if keys.just_pressed(KeyCode::KeyX) {
        return Some(match hovered {
            Some(to) => Ok(Mission::Assault { to }),
            None => needs_ground("X to assault"),
        });
    }
    if keys.just_pressed(KeyCode::KeyH) {
        // The one mission that needs no ground: `Hold { at: None }` is
        // "stand where you are", which is a real order and the reserve's.
        return Some(Ok(Mission::Hold { at: hovered }));
    }
    if keys.just_pressed(KeyCode::KeyR) {
        return Some(match hovered {
            Some(toward) => Ok(Mission::Recon { toward }),
            None => needs_ground("R to reconnoitre"),
        });
    }
    if keys.just_pressed(KeyCode::KeyW) {
        return Some(match formation_exit(state, formation) {
            Some(via) => Ok(Mission::Withdraw { via }),
            None => Err("There is no way off this map for your side.".into()),
        });
    }
    None
}

/// The retreat lane a formation would take: the nearest exit objective its
/// side is entitled to use, measured from its leader.
///
/// The rule itself lives in `tactics_core::battle::nearest_exit`, shared with
/// the commander brain and with the campaign's withdrawal orders — the player
/// picking a formation and pressing `W` should get the lane her opposite
/// number would have chosen, not a different one. All this adds is the leader
/// the lane is measured from.
fn formation_exit(state: &BattleState, formation: usize) -> Option<String> {
    let formation = state.formations().get(formation)?;
    let from = formation
        .leader
        .and_then(|id| state.unit(id))
        .or_else(|| formation.members.iter().find_map(|id| state.unit(*id)))
        .map(|u| u.pos)?;
    tactics_core::battle::nearest_exit(state, formation.side, from)
}

/// What the player may order a shot at on one hex.
enum Aim {
    /// Somebody she has been told about: a fresh contact, or — with no
    /// command rules — an ordinary spotted enemy.
    Enemy(UnitId),
    /// A ghost marker: the last reported position of a unit nobody can see
    /// now. Carries the line explaining the refusal.
    Ghost(String),
    /// Nothing she knows about. Handled by doing what an empty tile does,
    /// never by a message: a refusal naming an enemy she has not been told
    /// about would leak exactly what the command picture exists to withhold.
    Nothing,
}

fn aim_at(state: &BattleState, command_rules: bool, hex: Hex, side: u8) -> Aim {
    if !command_rules {
        return match state.spotted_enemy_at(hex, side) {
            Some(enemy) => Aim::Enemy(enemy.id),
            None => Aim::Nothing,
        };
    }
    // The real unit first: a fresh contact is drawn where it is, so that is
    // where the player is pointing when she means to shoot at it.
    if let Some(enemy) = state.unit_at(hex).filter(|u| u.side != side)
        && state
            .picture(side)
            .iter()
            .any(|c| c.unit == enemy.id && c.fresh)
    {
        return Aim::Enemy(enemy.id);
    }
    match state
        .picture(side)
        .iter()
        .find(|c| !c.fresh && c.at == hex)
        .and_then(|c| Some((c, state.units.get(c.unit.index())?)))
    {
        Some((contact, unit)) => Aim::Ghost(format!(
            "{} was last reported here, {}. Nobody has eyes on her now - B blind-fires the hex.",
            unit.name,
            report_age(state, contact)
        )),
        None => Aim::Nothing,
    }
}

/// How old a report is, in the words the log uses elsewhere: rounds, because
/// that is the clock the player is reading off the banner.
fn report_age(state: &BattleState, contact: &Contact) -> String {
    match state.round.saturating_sub(contact.round) {
        0 => "this round".into(),
        1 => "a round ago".into(),
        n => format!("{n} rounds ago"),
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
        &Order::Radio {
            unit,
            to: None,
            fire: Some(FireIntent::Target { target, weapon }),
        },
        log,
    );
}

/// Keep unit sprites in sync with the sim (position, facing, visibility,
/// hp bars) except while a Mover animation owns them.
///
/// Enemies are drawn from the command picture rather than the side's fog
/// wherever a mod prices a chain of command: solid where a report is fresh, a
/// dimmed ghost at the last reported hex where it is not, nothing at all
/// where nobody has said anything. Own units and the terrain fog overlay are
/// untouched — eyes see ground, and the picture is about contacts.
fn sync_units(
    battle: Res<Battle>,
    art: Res<ArtCache>,
    view: map_render::View,
    mut units: UnitSprites,
    mut widgets: UnitWidgets,
    mods: Res<Mods>,
) {
    let state = &battle.state;
    let view_side = battle.view_side();
    let rules = command_rules(&mods.0);
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
        // Where the screen puts her is not always where she is: a ghost
        // stands on the hex somebody last reported, however far the unit has
        // driven since.
        let shown = shown_to(state, rules, view_side, unit);
        let at = match shown {
            Shown::Ghost(reported) => reported,
            _ => unit.pos,
        };
        let elev = state.map.get(at).map(|t| t.elevation).unwrap_or(0);
        let (pos, z) = iso::project(at, elev, view.rotation(), view.center());
        if !animating.contains(&unit.id) {
            transform.translation = Vec3::new(pos.x, pos.y + 10.0, z + 1.5);
        }
        // Facing is either a sheet frame or a bodily rotation, depending on
        // whether this vehicle ships art. A drawn isometric vehicle must not
        // be spun — it would tip over — so it swaps to the frame for its
        // direction and mirrors for the three western ones. The generated
        // blob has no frames and is symmetric enough to just rotate.
        let angle = iso::facing_angle(at, unit.facing, view.rotation(), view.center());
        let side = unit.side % iso::SIDE_COLORS.len() as u8;
        if art.has_vehicle_frames(&unit.vehicle, side) {
            let (frame, flip) = iso::facing_frame(angle);
            if let Some(image) = art.vehicle_sprite(&unit.vehicle, side, frame) {
                sprite.image = image;
            }
            sprite.flip_x = flip;
            transform.rotation = Quat::IDENTITY;
        } else {
            transform.rotation = Quat::from_rotation_z(angle);
        }
        *visibility = match shown {
            Shown::Hidden => Visibility::Hidden,
            _ => Visibility::Inherited,
        };
        // Dim your own units once they have their orders for the round, and
        // a ghost further still: a marker on a report is not a vehicle you
        // are looking at, and it must not read like one.
        let done = state.is_planning() && unit.side == view_side && unit.planned;
        sprite.color = match (shown, done) {
            (Shown::Ghost(_), _) => Color::srgba(1.0, 1.0, 1.0, 0.45),
            (_, true) => Color::srgb(0.55, 0.55, 0.55),
            _ => Color::WHITE,
        };
    }

    // The badges say how hurt somebody is, which a stale report does not
    // know. They inherit their parent's visibility, so hiding them is only
    // ever about the ghost case.
    for (badge, mut visibility) in &mut widgets.badges {
        let ghost = state
            .unit(badge.0)
            .is_some_and(|u| matches!(shown_to(state, rules, view_side, u), Shown::Ghost(_)));
        *visibility = if ghost {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }

    // Who is in charge, read fresh off the formations rather than plumbed
    // through `CommandPassed`: succession happens in the middle of a tick and
    // the screen would have to hear about it anyway, so the cheap thing and
    // the correct thing are the same one.
    let leaders: HashSet<UnitId> = state.formations().iter().filter_map(|f| f.leader).collect();
    for (chevron, mut visibility, mut sprite, mut transform) in &mut widgets.chevrons {
        let Some(unit) = state.unit(chevron.0).filter(|u| leaders.contains(&u.id)) else {
            *visibility = Visibility::Hidden;
            continue;
        };
        // Inherited, never Visible: the parent's own rule about whether this
        // unit is on the picture at all is the one that decides, so a chevron
        // can never draw an enemy the fog is hiding.
        *visibility = Visibility::Inherited;
        let shown = shown_to(state, rules, view_side, unit);
        let dim = matches!(shown, Shown::Ghost(_));
        sprite.color = map_render::side_color(unit.side).with_alpha(if dim { 0.45 } else { 1.0 });
        // A rank marking is not painted on the turret. A vehicle with no art
        // is turned bodily to face, and a child sprite would be swung round
        // with it, so the wedge undoes its parent's rotation — both the spin
        // and the orbit it would otherwise be carried through.
        let at = match shown {
            Shown::Ghost(reported) => reported,
            _ => unit.pos,
        };
        let art_side = unit.side % iso::SIDE_COLORS.len() as u8;
        let upright = if art.has_vehicle_frames(&unit.vehicle, art_side) {
            Quat::IDENTITY
        } else {
            Quat::from_rotation_z(-iso::facing_angle(
                at,
                unit.facing,
                view.rotation(),
                view.center(),
            ))
        };
        transform.rotation = upright;
        transform.translation = upright * Vec3::new(0.0, 31.0, 0.2);
    }

    for (bar, mut sprite, mut transform) in &mut widgets.bars {
        if let Some(unit) = state.unit(bar.0) {
            // Condition — girls and modules over the full complement — is
            // what the bar shows now that hit points are gone. Same bar,
            // honest quantity.
            let frac = state.condition(&mods.0, unit).clamp(0.0, 1.0);
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

fn update_fog(
    battle: Res<Battle>,
    mut overlays: Query<(&HexOverlay, &mut Sprite, &mut Visibility), With<FogOverlay>>,
) {
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
    mods: Res<Mods>,
    view: map_render::View,
    art: Res<ArtCache>,
    existing: Query<Entity, HighlightFilter>,
    mut cursors: Cursors,
) {
    let map = battle.state.map.clone();
    let face_at = |hex: Hex| view.face_at(&map, hex);

    // Hover marker.
    if let Ok((mut transform, mut visibility)) = cursors.hover.single_mut() {
        match view.hovered(&map) {
            Some(hex) => {
                transform.translation = face_at(hex);
                *visibility = Visibility::Inherited;
            }
            None => *visibility = Visibility::Hidden,
        }
    }
    // Selection marker.
    if let Ok((mut transform, mut visibility)) = cursors.select.single_mut() {
        match battle.selected.and_then(|id| battle.state.unit(id)) {
            Some(unit) => {
                transform.translation = face_at(unit.pos);
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
        let overlay = HexOverlay::face(*hex);
        commands.spawn((
            Sprite {
                image: art.face.clone(),
                color: Color::srgba(0.35, 0.55, 1.0, 0.4),
                ..default()
            },
            Transform::from_translation(overlay.translation(&map, view.rotation(), view.center())),
            overlay,
            MoveHighlight,
            BattleScope,
        ));
    }

    // The formation being commanded, drawn as its net: the ground its members
    // are standing on, coloured by whether they can still hear the order, and
    // the horizon their leader's radio reaches to.
    //
    // Only members still on the field: a marker on a burnt-out crew's last hex
    // would be a lie about who is left to obey.
    if let Some(formation) = battle.formation() {
        let members: Vec<(Hex, Color)> = formation
            .members
            .iter()
            .filter_map(|id| battle.state.unit(*id).map(|u| (*id, u.pos)))
            .map(|(id, pos)| {
                let color = match formation.in_contact(id) {
                    true => FORMATION_MARKER,
                    false => FORMATION_CUT_OFF,
                };
                (pos, color)
            })
            .collect();
        // The reach comes from the engine's own accessor, so what is drawn is
        // the graph edge rather than the game crate's opinion of it — see
        // `BattleState::radio_reach`. Nothing is drawn where nothing is
        // priced: a mod with no command block answers `None` and the ring
        // simply does not exist, which is the additivity rule again.
        let ring: Vec<Hex> = formation
            .leader
            .and_then(|id| {
                let at = battle.state.unit(id)?.pos;
                let reach = battle.state.radio_reach(&mods.0, id)?;
                Some(at.ring(reach).filter(|h| map.get(*h).is_some()).collect())
            })
            .unwrap_or_default();
        for hex in ring {
            let overlay = HexOverlay::face_over(hex);
            commands.spawn((
                Sprite {
                    image: art.face.clone(),
                    color: NET_RING,
                    ..default()
                },
                Transform::from_translation(overlay.translation(
                    &map,
                    view.rotation(),
                    view.center(),
                )),
                overlay,
                NetRing,
                BattleScope,
            ));
        }
        for (hex, color) in members {
            let overlay = HexOverlay::face(hex);
            commands.spawn((
                Sprite {
                    image: art.face.clone(),
                    color,
                    ..default()
                },
                Transform::from_translation(overlay.translation(
                    &map,
                    view.rotation(),
                    view.center(),
                )),
                overlay,
                FormationHighlight,
                BattleScope,
            ));
        }
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
        let overlay = HexOverlay::face(hex);
        commands.spawn((
            Sprite {
                image: art.face.clone(),
                color,
                ..default()
            },
            Transform::from_translation(overlay.translation(&map, view.rotation(), view.center())),
            overlay,
            PlanHighlight,
            BattleScope,
        ));
    }
}

/// Tint each objective hex with whoever holds it.
///
/// Separate from `update_highlights` because these are not highlights: they
/// do not depend on selection or the move range, and rebuilding them on every
/// `range_dirty` pass would respawn a hundred sprites to change a colour.
fn update_objective_markers(
    battle: Res<Battle>,
    mut markers: Query<(&ObjectiveMarker, &mut Sprite)>,
) {
    for (marker, mut sprite) in &mut markers {
        let objective = battle.state.map.objectives().get(marker.index);
        // An exit is never held, so it would sit on the neutral colour
        // forever and read as ground nobody had bothered to take.
        if objective.is_some_and(|o| o.kind == ObjectiveKind::Exit) {
            sprite.color = OBJECTIVE_EXIT;
            continue;
        }
        sprite.color = match battle
            .state
            .objective_held
            .get(marker.index)
            .copied()
            .flatten()
        {
            Some(side) => map_render::side_color(side).with_alpha(0.45),
            None => OBJECTIVE_NEUTRAL,
        };
    }
}

fn update_panel(
    battle: Res<Battle>,
    mods: Res<Mods>,
    art: Res<ArtCache>,
    log: Res<BattleLog>,
    view: map_render::View,
    mut hud: BattleHud,
    mut warned: Local<bool>,
) {
    let state = &battle.state;
    let registry = &mods.0;
    let view_side = battle.view_side();

    map_render::warn_if_duplicated(hud.banner.iter().count(), "battle banner", &mut warned);
    if let Ok(mut text) = hud.banner.single_mut() {
        let scale = &registry.scale;
        text.0 = match state.resolving_tick() {
            // The elapsed clock is the point of the tick counter: a round is
            // a minute of battle, and seeing "t+25 s" while the shells are
            // still in the air is what makes the ranges believable.
            Some(tick) => format!(
                "Round {} - resolving {} ({}/{})",
                state.round,
                scale.format_duration(tick + 1),
                tick + 1,
                scale.ticks_per_round
            ),
            None if battle.human_side().is_some_and(|s| state.has_committed(s)) => {
                format!("Round {} - waiting on the other side", state.round)
            }
            None => format!("Round {} - planning", state.round),
        };
        // The score belongs next to the round, because on a map with
        // objectives it is the other clock the player is racing: a battle can
        // now be lost while winning the shooting.
        if !state.map.objectives().is_empty() {
            let scores: Vec<String> = state
                .sides
                .iter()
                .enumerate()
                .map(|(i, side)| format!("{} {}", side.name, state.score(i as u8)))
                .collect();
            text.0 = match state.map.victory_score() {
                Some(target) => format!("{}   |   {} (to {target})", text.0, scores.join("  -  ")),
                None => format!("{}   |   {}", text.0, scores.join("  -  ")),
            };
        }
    }
    if let Ok(mut text) = hud.log_text.single_mut() {
        text.0 = log.0.iter().cloned().collect::<Vec<_>>().join("\n");
    }

    // What the panel will talk about is what the screen is drawing, which
    // under command rules is the picture rather than the fog: describing a
    // unit whose sprite is hidden would hand the player through the panel
    // exactly what the picture withheld from the map.
    let rules = command_rules(registry);
    let visible =
        |unit: &tactics_core::battle::Unit| shown_to(state, rules, view_side, unit) == Shown::Real;

    let hovered_tile = view.hovered(&state.map);
    let hovered_unit = hovered_tile
        .and_then(|hex| state.unit_at(hex))
        .filter(|u| visible(u))
        .map(|u| u.id);

    let Ok(mut text) = hud.panel.single_mut() else {
        return;
    };

    // Commanding a formation is a mode: while one is picked the panel is
    // about it and about the ground under the cursor, which is what the
    // mission keys are aimed at.
    if let Some(formation) = battle.formation() {
        text.0 = format_formation(registry, state, formation, hovered_tile);
        if let Some(leader) = formation.leader {
            set_portrait(&mut hud.portrait, &art, state, leader);
        }
        return;
    }

    // Hovering an enemy while something is selected is a question about a
    // shot, so answer that first. Otherwise inspect whatever is under the
    // cursor, falling back to the selection and then the bare tile.
    let attack = battle
        .selected
        .zip(hovered_unit)
        .filter(|(attacker, target)| {
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
            text.0 = format_attack(&registry.scale, &preview);
            set_portrait(&mut hud.portrait, &art, state, target);
            return;
        }
    }

    // A ghost is the only thing on the board with nothing behind it to
    // inspect, so the panel answers for the report instead: who saw her, and
    // how long ago. That is the whole of what the commander knows, and it
    // outranks the selection for the same reason hovering anything else does.
    if let Some(contact) = hovered_tile
        .filter(|_| hovered_unit.is_none())
        .and_then(|hex| {
            state
                .picture(view_side)
                .iter()
                .find(|c| !c.fresh && c.at == hex)
        })
    {
        text.0 = format!(
            "{}\n\n{}",
            format_contact(state, contact),
            format_tile(registry, state, contact.at)
        );
        return;
    }

    let shown = hovered_unit
        .or(battle.selected)
        .and_then(|id| state.unit(id))
        .filter(|u| visible(u));
    if let Some(unit) = shown {
        // Describe the tile under the cursor rather than the unit's own, so
        // terrain can be read without dropping the selection.
        text.0 = format_unit(registry, state, unit, hovered_tile.unwrap_or(unit.pos));
        set_portrait(&mut hud.portrait, &art, state, unit.id);
        return;
    }
    if let Some(hex) = hovered_tile {
        text.0 = format_tile(registry, state, hex);
        return;
    }
    text.0 = "Hover a tile for terrain\n\nLMB: select / set route\nA: engage hovered enemy\nB: blind fire a tile\nV: hold and watch\nC: clear orders\nEnter: commit the round\nF: pick a formation\nQ/E: rotate view".into();
}

/// The formation panel: who these girls are, what they were told to do, what
/// is still on its way to them, and which of them can no longer hear it.
///
/// The mission is spelled out in words rather than as an enum name, because
/// the point of the panel is that a player can read her own last order back
/// and check it against what her platoon is actually doing. An order still in
/// transit is listed separately for the same reason — "she has been told" and
/// "she knows" are different states, and the gap between them is the system.
fn format_formation(
    registry: &tactics_core::data::DataRegistry,
    state: &BattleState,
    formation: &Formation,
    hovered: Option<Hex>,
) -> String {
    let name = |id: UnitId| {
        state
            .units
            .get(id.index())
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "???".into())
    };
    let mut lines = vec![
        formation_name(state, &formation.id),
        match formation.leader {
            Some(leader) => format!("Leader: {}", name(leader)),
            None => "Leader: nobody left".into(),
        },
    ];
    if let Some(net) = format_net(registry, state, formation) {
        lines.push(net);
    }
    lines.push(String::new());
    lines.push(format!(
        "Orders: {}",
        mission_sentence(state, formation.mission.as_ref())
    ));
    if let Some((change, ticks)) = &formation.incoming {
        // An amendment reads differently from a countermand, because the
        // player who queued a leg should not fear it will replace her plan.
        let verb = match change {
            tactics_core::battle::MissionChange::Replace(_) => "In the air",
            tactics_core::battle::MissionChange::Append(_) => "In the air (and then)",
        };
        lines.push(format!(
            "{verb}: {} ({})",
            mission_sentence(state, Some(change.mission())),
            registry.scale.format_duration(*ticks)
        ));
    }
    for leg in &formation.plan {
        lines.push(format!("Then: {}", mission_sentence(state, Some(leg))));
    }
    lines.push(String::new());
    lines.push("Members:".into());
    for id in &formation.members {
        let Some(unit) = state.units.get(id.index()) else {
            continue;
        };
        if !unit.alive {
            // Gone is gone, and the panel says which kind: a crew that drove
            // off by an exit came home, and listing her as lost would be the
            // UI telling the lie the engine is careful not to.
            lines.push(format!(
                "  {} - {}",
                unit.name,
                if unit.exited { "withdrawn" } else { "lost" }
            ));
            continue;
        }
        // Two different silences, and the panel must not conflate them: one
        // says she cannot hear you, the other says you have already spoken and
        // she has not heard it yet. A player who cannot tell them apart cannot
        // tell whether to reissue the order.
        let mut tags: Vec<String> = Vec::new();
        if !formation.in_contact(*id) {
            tags.push("out of contact".into());
        }
        if state.command.waiting_for(*id).is_some() {
            tags.push("orders waiting".into());
        }
        // Who will go where: a standing personal march is a promise about
        // future rounds, and a promise the player cannot read is one she
        // will fight against.
        if let Some(tasking) = unit.tasking {
            tags.push(format!("moving to {}", hex_label(tasking)));
        }
        let tag = if tags.is_empty() {
            String::new()
        } else {
            format!(" - {}", tags.join(", "))
        };
        lines.push(format!("  {}{}", unit.name, tag));
    }
    lines.push(String::new());
    lines.push("G advance / X assault / H hold / R recon".into());
    lines.push("on the hovered hex; W withdraw. (Shift queues)".into());
    lines.push("F next formation, Esc drops it.".into());
    if let Some(hex) = hovered {
        lines.push(String::new());
        lines.push(format!("Hovered {}:", hex_label(hex)));
        lines.push(format_tile(registry, state, hex));
    }
    lines.join("\n")
}

/// The formation's net in numbers: how far the leader's radio carries, how
/// much of that is her crew rather than her hardware, and how far a flag
/// carries to anybody at all.
///
/// The ring drawn on the map says *where* the radio ends; this says why it
/// ends there, which is the half a player can act on — a poor signaller is a
/// crew problem with a crew answer. Both numbers come from the engine
/// ([`BattleState::radio_reach`] and the command block itself) rather than
/// from arithmetic repeated here, so the sentence cannot come apart from the
/// ring beside it.
///
/// `None` for a mod that prices no chain of command: there is no net, so
/// there is no line, and the panel is the one it was before any of this.
/// The visual clause is dropped the same way when `visual_range` is zero,
/// which is what a mod that declares radios and no flags looks like.
fn format_net(
    registry: &tactics_core::data::DataRegistry,
    state: &BattleState,
    formation: &Formation,
) -> Option<String> {
    let rules = registry.command.as_ref()?;
    let leader = formation.leader?;
    let unit = state.unit(leader)?;
    // The set by name, because it is a thing the vehicle carries — and a
    // receive-only set says so instead of showing a range it does not have.
    let set = registry
        .vehicle(&unit.vehicle)
        .and_then(|v| v.radio.as_deref())
        .and_then(|id| registry.radio(id));
    let mut line = match (set, state.radio_reach(registry, leader)) {
        (Some(set), Some(reach)) => {
            let hardware = set.send.unwrap_or(rules.radius);
            let mut line = format!("Net: {} {reach}", set.name);
            if reach != hardware {
                // The difference between hardware and reach is exactly what
                // the crew is worth, credited to the leader by name; the
                // seat actually working the set may be her operator's,
                // which `crew_skill` already knows.
                line.push_str(&format!(
                    " ({:+}, {}'s signals)",
                    reach as i32 - hardware as i32,
                    unit.name
                ));
            }
            line
        }
        (Some(set), None) => format!("Net: {} — receives only", set.name),
        (None, Some(reach)) => format!("Net: radio {reach}"),
        (None, None) => return None,
    };
    if rules.visual_range > 0 {
        line.push_str(&format!(", visual {}", rules.visual_range));
    }
    Some(line)
}

/// A mission as a sentence a person would say, naming ground the way the map
/// file does so a player can find it again.
fn mission_sentence(state: &BattleState, mission: Option<&Mission>) -> String {
    match mission {
        None => "none given".into(),
        Some(Mission::Advance { to }) => format!("advance on {}", hex_label(*to)),
        // Named apart from the advance, because the difference is the whole
        // point: this one does not stop when somebody shoots at it, and the
        // player is entitled to see which of the two her platoon is under.
        Some(Mission::Assault { to }) => format!("assault {}", hex_label(*to)),
        Some(Mission::Hold { at: Some(at) }) => format!("hold {}", hex_label(*at)),
        Some(Mission::Hold { at: None }) => "hold where you are".into(),
        Some(Mission::Recon { toward }) => format!("reconnoitre toward {}", hex_label(*toward)),
        // Named the way the order was given — after the people, not the
        // ground — because that is what a base of fire is about, and the
        // display name is what the player calls that platoon everywhere else
        // in this log.
        Some(Mission::Support { formation }) => format!(
            "stand base of fire for the {}",
            formation_name(state, formation)
        ),
        Some(Mission::Withdraw { via }) => format!(
            "withdraw by {}",
            state
                .map
                .objectives()
                .iter()
                .find(|o| &o.id == via)
                .map(|o| o.name.clone())
                .unwrap_or_else(|| via.clone())
        ),
    }
}

/// One line of the command picture: who was seen, by whom, and how stale the
/// report is.
fn format_contact(state: &BattleState, contact: &Contact) -> String {
    let name = |id: UnitId| {
        state
            .units
            .get(id.index())
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "???".into())
    };
    format!(
        "Last reported here\n{}\nby {}, {}",
        name(contact.unit),
        name(contact.reporter),
        report_age(state, contact)
    )
}

/// A hex in the coordinates a map file writes down, which is what a player
/// can count on the board and an author can find in JSON. The axial pair is
/// an implementation detail nobody outside the engine reads.
fn hex_label(hex: Hex) -> String {
    let [col, row] = tactics_core::hex_to_offset(hex);
    format!("({col},{row})")
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
    let key = unit
        .crew
        .first()
        .and_then(|id| state.roster.get(*id))
        .map(|girl| girl.def.as_str())
        .unwrap_or(&unit.vehicle);
    if let Some(handle) = art.portraits.get(key) {
        image.image = handle.clone();
    }
}

/// The shot the player is contemplating, with the arithmetic spelled out.
///
/// Distances lead with the real-world figure and keep the hex count in
/// parentheses: the metre value is what tells the player whether this is a
/// long shot, the hex count is what they need to count tiles on the board.
fn format_attack(
    scale: &tactics_core::data::Scale,
    preview: &tactics_core::battle::AttackPreview,
) -> String {
    let mut lines = vec![
        format!("Attack: {}", preview.target_name),
        format!("{} - {}", preview.target_vehicle, preview.target_side),
        format!(
            "Condition {}%  at {} ({} hexes)",
            preview.target_condition,
            scale.format_distance(preview.distance),
            preview.distance
        ),
        String::new(),
        preview.weapon_name.clone(),
        // On its own line: the panel is 240 px wide and wraps at roughly
        // thirty characters, so a metric range and a hex range do not fit
        // beside a weapon name.
        format!(
            "  rng {} ({}-{})",
            scale.format_range(preview.weapon_range),
            preview.weapon_range[0],
            preview.weapon_range[1]
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
    match &preview.ammo {
        Some(round) => lines.push(format!(
            "{round}: {}% through {:?} armor {}",
            preview.pen_chance, preview.facing, preview.effective_armor
        )),
        None => lines.push("NO AMMUNITION".into()),
    }
    lines.push(format!(
        "Effect {}  expected {:.1}",
        preview.damage, preview.expected_damage
    ));
    if preview.lethal {
        lines.push("A penetration could finish her.".into());
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
        vehicle
            .map(|v| v.name.clone())
            .unwrap_or_else(|| "?".into()),
        format!(
            "Condition {}%",
            (state.condition(registry, unit) * 100.0).round() as i32
        ),
        format!("Side: {}", state.sides[unit.side as usize].name),
    ];
    let scale = &registry.scale;
    if let Some(v) = vehicle {
        lines.push(format!(
            "Armor F{}/S{}/R{}",
            v.armor.front, v.armor.side, v.armor.rear
        ));
        // The crewed figures, not the vehicle's paper ones: what this unit
        // actually does with these girls aboard is the interesting number,
        // and it is the only place the player can see the crew bonus land.
        let speed = tactics_core::battle::move_points(
            registry,
            &state.roster,
            unit,
            state.terrain_at(unit.pos),
        );
        let vision = tactics_core::battle::stats::vision_range(
            registry,
            &state.roster,
            unit,
            state.terrain_at(unit.pos),
        );
        lines.push(format!("Move {} ({})", scale.format_speed(speed), speed));
        lines.push(format!(
            "Sight {} ({})",
            scale.format_distance(vision as i32),
            vision
        ));
        for weapon in v.weapons.iter().filter_map(|w| registry.weapon(w)) {
            lines.push(format!("  {} dmg {}", weapon.name, weapon.damage));
            lines.push(format!(
                "    {} ({}-{})",
                scale.format_range(weapon.range),
                weapon.range[0],
                weapon.range[1]
            ));
            lines.push(format!(
                "    a shot every {}",
                scale.format_duration(weapon.reload(scale))
            ));
        }
    }
    lines.push("Crew:".into());
    for c in &unit.crew {
        if let Some(girl) = state.roster.get(*c) {
            // Her strongest training, named. Words rather than a stat block:
            // girls read as people when described and as units when
            // tabulated, and the exact numbers belong behind a toggle.
            let mut best: Vec<(&String, &i32)> = girl.skills.iter().collect();
            best.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            let summary = best
                .iter()
                .take(2)
                .map(|(id, level)| {
                    let name = registry
                        .skill(id)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| (*id).clone());
                    format!("{name} {level}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            if summary.is_empty() {
                lines.push(format!("  {}", girl.name));
            } else {
                lines.push(format!("  {} ({summary})", girl.name));
            }
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
    let scale = &registry.scale;
    let mut lines = vec![format!(
        "{} (elev {})",
        terrain.name,
        scale.format_elevation(tile.elevation)
    )];
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
        // Sight blocking is priced in elevation steps, so it is a height:
        // forest stands 20 m over the ground it grows on.
        lines.push(format!(
            "Blocks sight (+{})",
            scale.format_elevation(terrain.vision_block)
        ));
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
    // Costs are multipliers on a vehicle's speed rather than speeds
    // themselves, so they stay in movement points: "trk2" is half pace.
    lines.push(format!("Move cost {}", costs.join(" ")));
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
        let mut survivors: Vec<(ArmyId, Vec<ArmyUnit>)> = Vec::new();
        let mut slot_of = HashMap::new();
        for army in &field.origins {
            slot_of.entry(*army).or_insert_with(|| {
                survivors.push((*army, Vec::new()));
                survivors.len() - 1
            });
        }
        // `surviving_units`, not `alive_units`: a crew that drove off the map
        // by an exit is off the board but came home, and reading `alive` here
        // would hand the campaign a withdrawal as a burnt-out vehicle.
        for unit in battle.state.surviving_units() {
            let Some(army) = field.origins.get(unit.id.index()) else {
                continue;
            };
            let Some(&slot) = slot_of.get(army) else {
                continue;
            };
            survivors[slot].1.push(ArmyUnit {
                vehicle: unit.vehicle.clone(),
                crew: unit.crew.clone(),
                name: Some(unit.name.clone()),
            });
        }

        // Everyone who was aboard something that burned. The battle reports
        // who and what killed it; the campaign decides what that cost them,
        // because whether this game kills its characters is a campaign rule.
        let mut losses = Vec::new();
        for unit in battle.state.lost_units() {
            for girl in &unit.crew {
                losses.push(CrewLoss {
                    girl: *girl,
                    vehicle: unit.vehicle.clone(),
                    killed_by: unit.last_hit_by,
                });
            }
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
            losses,
        });
    }

    for entity in &scoped {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<Battle>();
    commands.remove_resource::<PendingBattle>();
    next.set(AppState::Overworld);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> tactics_core::data::DataRegistry {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
        tactics_core::data::DataRegistry::load_dir(&root)
            .expect("mods load")
            .0
    }

    /// Deployment puts a side at the shallowest tiles of its own map edge —
    /// which is exactly where a retreat lane lives. Without this rule the
    /// attacker's leading vehicles spawn standing on their own way out and
    /// drive off the map on the first tick, ending the battle before it
    /// starts.
    #[test]
    fn nobody_deploys_onto_their_own_way_off_the_map() {
        let reg = registry();
        let file = reg.map("river_crossing").expect("shipped battle map");
        let map = tactics_core::map::HexMap::from_map_file(file).expect("map parses");
        assert!(
            map.objectives()
                .iter()
                .any(|o| o.kind == ObjectiveKind::Exit),
            "this test is meaningless if the map has no exits"
        );

        let forces: Vec<BattleForce> = [0u8, 1]
            .iter()
            .map(|side| BattleForce {
                army: ArmyId(*side as u32),
                side: *side,
                units: (0..4)
                    .map(|_| ArmyUnit {
                        vehicle: "medium_tank".into(),
                        crew: Vec::new(),
                        name: None,
                    })
                    .collect(),
                mission: None,
            })
            .collect();

        let (placements, _, _) = deploy(&reg, &map, &forces, 0);
        assert_eq!(placements.len(), 8, "everyone was placed");
        for placement in &placements {
            let hex = tactics_core::offset_to_hex(placement.at[0], placement.at[1]);
            for objective in map.objectives() {
                assert!(
                    !(objective.kind == ObjectiveKind::Exit
                        && objective.open_to(placement.side)
                        && objective.contains(hex)),
                    "side {} deployed onto exit `{}` at {:?}",
                    placement.side,
                    objective.id,
                    placement.at
                );
            }
        }
    }

    /// An army arriving on a battlefield is a formation on it, because
    /// otherwise nothing the campaign decided has anybody to say it to: a
    /// mission is given to a formation, and a field battle of flat pools
    /// could inherit no orders at all.
    #[test]
    fn each_army_fills_one_of_the_maps_formations() {
        let reg = registry();
        let file = reg.map("river_crossing").expect("shipped battle map");
        let map = tactics_core::map::HexMap::from_map_file(file).expect("map parses");
        let forces: Vec<BattleForce> = (0..4)
            .map(|i| BattleForce {
                army: ArmyId(i),
                side: (i % 2) as u8,
                units: (0..2)
                    .map(|_| ArmyUnit {
                        vehicle: "medium_tank".into(),
                        crew: Vec::new(),
                        name: None,
                    })
                    .collect(),
                mission: None,
            })
            .collect();

        let (placements, _, _) = deploy(&reg, &map, &forces, 0);
        let named: Vec<&str> = placements
            .iter()
            .filter_map(|p| p.formation.as_deref())
            .collect();
        assert_eq!(named.len(), placements.len(), "nobody is left unattached");
        for def in map.formations() {
            assert_eq!(
                named.iter().filter(|id| **id == def.id).count(),
                2,
                "one army of two vehicles per declared formation: {}",
                def.id
            );
            let leaders = placements
                .iter()
                .filter(|p| p.formation.as_deref() == Some(def.id.as_str()) && p.leads)
                .count();
            assert_eq!(leaders, 1, "exactly one leader in {}", def.id);
        }
    }

    /// The campaign's decision reaches the battlefield: an army that was
    /// falling back fights toward the way out, without the player having to
    /// order every platoon out again by hand.
    #[test]
    fn a_withdrawing_army_hands_its_formations_the_way_out() {
        let reg = registry();
        let file = reg.map("river_crossing").expect("shipped battle map");
        let map = tactics_core::map::HexMap::from_map_file(file).expect("map parses");
        let forces: Vec<BattleForce> = [0u8, 1]
            .iter()
            .map(|side| BattleForce {
                army: ArmyId(*side as u32),
                side: *side,
                units: (0..2)
                    .map(|_| ArmyUnit {
                        vehicle: "medium_tank".into(),
                        crew: Vec::new(),
                        name: None,
                    })
                    .collect(),
                // Only the defender was pulling back.
                mission: (*side == 1).then_some(ArmyMission::Withdraw {
                    to: tactics_core::offset_to_hex(13, 4),
                }),
            })
            .collect();

        let (placements, crews, _) = deploy(&reg, &map, &forces, 0);
        let sides = vec![
            SideState {
                name: "A".into(),
                ai: None,
            },
            SideState {
                name: "B".into(),
                ai: None,
            },
        ];
        let mut state = BattleState::from_placements(
            &reg,
            map,
            sides,
            &placements,
            &crews,
            std::sync::Arc::new(tactics_core::roster::Roster::new()),
            9,
        );
        let lines = inherit_army_missions(&reg, &mut state, &forces, ArmyId(0), ArmyId(1));
        // One army per side, so one formation per side exists: a declaration
        // nobody joined is dropped rather than carried empty.
        assert_eq!(lines.len(), 1, "side 1's formation was told");

        for formation in state.formations() {
            let ordered = formation.latest_mission();
            if formation.side == 1 {
                assert!(
                    matches!(ordered, Some(Mission::Withdraw { via }) if via == "east_road"),
                    "{} should be leaving by its own lane, got {ordered:?}",
                    formation.id
                );
            } else {
                assert!(
                    ordered.is_none(),
                    "{} was told nothing on the map and must be told nothing here",
                    formation.id
                );
            }
        }
    }
}
