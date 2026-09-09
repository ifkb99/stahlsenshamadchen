//! The battle screen: renders a `tactics_core` battle, feeds it player
//! orders, animates the resulting events, and drives AI sides.

use crate::camera::CameraFocus;
use crate::iso::{self, ArtCache, ViewCenter};
use crate::map_render::{self, CurrentMap, FogOverlay, HexOverlay};
use crate::mods::Mods;
use crate::{AppState, ScreenSet};

mod panel;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use panel::{
    format_attack, format_contact, format_danger, format_formation, format_tile, format_unit,
    formation_name, hex_label, mission_sentence, report_age, unit_name,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tactics_core::Hex;
use tactics_core::ai::{AiConfig, AiDriver, SideCommand, make_battle_planner};
use tactics_core::battle::{
    BattleState, CrewCondition, EndReason, Event as BattleEvent, FireIntent, Formation,
    FormationId, Latitude, Mission, Order, SideState, Unit, UnitId, reachable,
};
use tactics_core::map::{ObjectiveKind, UnitPlacement};
use tactics_core::overworld::ArmyId;
use tactics_core::overworld::{ArmyMission, ArmyUnit, CrewLoss};
use tactics_core::roster::{CadetId, Roster};

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
        /// The campaign's cadets, so the crews that fight are the same people
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
    /// Cadets who were aboard a vehicle that was destroyed. What became of
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
    /// Whether the danger overlay is up: the move range tinted by what the
    /// spotted enemy could put on the selected crew if she stood there.
    ///
    /// A toggle rather than an always-on tint because the two questions are
    /// different — *where can I get to* and *what would it cost me* — and
    /// painting both at once in one set of hexes makes neither readable.
    show_danger: bool,
    /// What each reachable tile would cost her, in substance points a round,
    /// summed over every gun that bears.
    ///
    /// Cached beside `move_range` and rebuilt on exactly the same signal.
    /// One `fire_on` is a walk over every spotted enemy asking the resolver
    /// two questions apiece; doing that for a hundred reachable tiles every
    /// frame would price the overlay at roughly a round of AI planning per
    /// sixteen milliseconds, for an answer that cannot change while the
    /// player is holding still. Nothing on this map moves during planning
    /// except by an order, and every order already raises `range_dirty`.
    danger: HashMap<Hex, f32>,
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

    /// Whether a keystroke would actually be acted on this frame.
    ///
    /// `accepting_orders` is not the whole answer — sprites finishing a walk
    /// hold the keyboard too, and the side may have committed already — and
    /// the difference matters to exactly two callers who must never disagree:
    /// [`handle_input`], which ignores a key it is not listening for, and the
    /// dev harness's `idle` fact, which is a script's way of asking "are you
    /// listening yet". They disagreed once, and the symptom was silent and
    /// nasty: `until idle` came true while unit sprites were still walking,
    /// the script pressed Enter into a game that was not listening, and four
    /// commits advanced the battle by one round while every screenshot after
    /// them quietly described the wrong turn.
    ///
    /// So: one predicate, both callers. Anything new that would make
    /// `handle_input` refuse a keystroke belongs in here, not beside it.
    fn listening(&self, movers_idle: bool) -> bool {
        movers_idle
            && self.accepting_orders()
            && self
                .human_side()
                .is_some_and(|side| !self.state.has_committed(side))
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
    // A passenger is not on the field, and this has to be asked before the
    // own-side shortcut below or your own platoon is drawn standing on top of
    // the taxi carrying her. `pos` mirrors the carrier's, so the two sprites
    // land on the same hex and the one on top wins — which is exactly the
    // picture the scripted tour caught on `battle_plains`, where a mounted
    // start means a rifle platoon is aboard from the first frame. The engine
    // has been careful about this all along (`unit_at` filters `aboard`); the
    // renderer simply never asked.
    //
    // Hidden rather than a fourth `Shown` variant: there is nothing to draw
    // and nothing to draw it at, which is what `Hidden` already means. Where
    // she *is* gets said in the two places that can say it in words — the
    // carrier's panel ("Carrying: …") and the formation roll call ("riding
    // in …") — because a sprite cannot express "inside that one".
    if unit.aboard.is_some() {
        return Shown::Hidden;
    }
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

/// The move range's own blue: what the overlay paints on a tile nothing
/// spotted can reach. Blue rather than the bottom of the warm ramp, which is
/// what makes the overlay readable at a glance — the eye is looking for the
/// tiles that are *not* blue.
const DANGER_NONE: Color = Color::srgba(0.35, 0.55, 1.0, 0.4);

/// The two ends of the danger ramp, yellow to red, in the same order as the
/// sentences in [`panel::DANGER_LEGEND`]; [`danger_color`] is the join. The
/// warm tints run at a higher alpha than the blue because they are the
/// answer to a question the player asked, and a warning that has to be
/// hunted for is not one.
const DANGER_LOW: Color = Color::srgba(0.95, 0.85, 0.25, 0.45);
const DANGER_HIGH: Color = Color::srgba(1.0, 0.2, 0.2, 0.6);

/// The colour for a tile at a given point on the ramp
/// ([`panel::danger_tint`]): blue at exactly nothing, and otherwise the
/// straight mix of the two ends. A gradient rather than bands, on the
/// designer's call: distance is most of what decides expected fire, and the
/// bands threw that smoothness away — see `panel::danger_tint` for the scale.
fn danger_color(tint: f32) -> Color {
    if tint <= 0.0 {
        return DANGER_NONE;
    }
    let (low, high) = (DANGER_LOW.to_srgba(), DANGER_HIGH.to_srgba());
    let t = tint.clamp(0.0, 1.0);
    let mix = |a: f32, b: f32| a + (b - a) * t;
    Color::srgba(
        mix(low.red, high.red),
        mix(low.green, high.green),
        mix(low.blue, high.blue),
        mix(low.alpha, high.alpha),
    )
}

/// Overlay showing what your own units have been ordered to do this round.
#[derive(Component)]
struct PlanHighlight;

/// Overlay under every member of the formation being commanded, so a mission
/// is visibly given to *these four vehicles* rather than to a name in a list.
#[derive(Component)]
struct FormationHighlight;

/// The formation marker's colour: violet, because it belongs to neither
/// side's palette nor to the amber of ground worth taking. It says "these are
/// the cadets you are talking to", which is not a fact about the map.
const FORMATION_MARKER: Color = Color::srgba(0.65, 0.5, 1.0, 0.5);

/// The same marker under a cadet who cannot hear a word of it. Kept at the
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
        // Each phase says what it is for; the order between phases is
        // declared once, in `main.rs`, so nothing here depends on the order
        // these calls happen to be written in. Systems are still chained
        // *within* a phase, because within a phase adjacency is a real
        // constraint and a short list is where it can be seen.
        app.init_resource::<BattleLog>()
            .add_systems(OnEnter(AppState::Battle), setup_battle)
            .add_systems(
                Update,
                // The queue first: `advance_resolution` and `handle_input`
                // both refuse to act while anything is still animating, so
                // the drain has to happen before either asks.
                (drive_movers, pump_events)
                    .chain()
                    .in_set(ScreenSet::Animate)
                    .run_if(in_state(AppState::Battle)),
            )
            .add_systems(
                Update,
                (drive_ai, advance_resolution)
                    .chain()
                    .in_set(ScreenSet::Simulate)
                    .run_if(in_state(AppState::Battle)),
            )
            .add_systems(
                Update,
                handle_input
                    .in_set(ScreenSet::Input)
                    .run_if(in_state(AppState::Battle)),
            )
            .add_systems(
                Update,
                sync_units
                    .in_set(ScreenSet::Sync)
                    .run_if(in_state(AppState::Battle)),
            )
            .add_systems(
                Update,
                (
                    update_fog,
                    update_highlights,
                    update_objective_markers,
                    update_panel,
                    update_flashes,
                )
                    .chain()
                    .in_set(ScreenSet::Present)
                    .run_if(in_state(AppState::Battle)),
            )
            .add_systems(
                Update,
                finish_battle
                    .in_set(ScreenSet::Lifecycle)
                    .run_if(in_state(AppState::Battle)),
            );
        // Only a dev build answers questions about itself. The publisher
        // walks every unit once a frame, which is nothing next to rendering
        // them but is pure waste in a build nobody is scripting.
        if crate::devtools::debug_enabled() {
            app.init_resource::<crate::devtools::ScriptFacts>()
                .add_systems(
                    Update,
                    publish_script_facts
                        .in_set(ScreenSet::Facts)
                        .run_if(in_state(AppState::Battle)),
                );
        }
    }
}

/// Tell the script runner what this screen knows about itself.
///
/// Runs last in the frame and reads only what is already settled, so an
/// `until idle` sees the state the next screenshot would photograph rather
/// than a half-applied one. The facts themselves are deliberately thin — see
/// [`crate::devtools::ScriptFacts`] — and the translation lives here rather
/// than in the runner because `Battle` is this module's business and the
/// campaign map will answer the same questions in its own terms.
fn publish_script_facts(
    battle: Option<Res<Battle>>,
    log: Res<BattleLog>,
    movers: Query<&Mover>,
    mut facts: ResMut<crate::devtools::ScriptFacts>,
) {
    // Optional, and not defensively: `finish_battle` removes `Battle`, and
    // the explicit `.after(finish_battle)` ordering makes Bevy insert a sync
    // point between the two — so on the frame a battle ends this system runs
    // with the resource already gone. A required `Res` panics there, which is
    // what happens to a scripted campaign tour the moment its battle is over,
    // and it took three runs of one to find because a panic in a task pool
    // prints no system name.
    let Some(battle) = battle else {
        return;
    };
    // Built whole and assigned, rather than written field by field. Every
    // screen answers for every fact, even the ones it has no notion of —
    // `ScriptFacts` is one resource shared by all of them, so a field left
    // alone here would still be holding the campaign map's last answer and a
    // script would wait on a prompt dismissed two screens ago. Writing the
    // struct out makes that structural instead of remembered: the literal is
    // deliberately **exhaustive**, with no `..default()`, so a field added to
    // `ScriptFacts` tomorrow fails to compile in every publisher until each
    // screen has said what it answers. Same bargain `Mission::slot`'s
    // exhaustive match makes, for the same reason — the failure mode of the
    // alternative is silent.
    *facts = crate::devtools::ScriptFacts {
        turn: battle.state.round,
        // "Idle" means the game is *waiting for the player*: the only moment
        // a script's keystroke does what a person's would, and the only
        // moment a screenshot shows a settled board. That is exactly
        // `listening`, and it is a method on `Battle` rather than a copy of
        // the conditions here precisely so the two cannot drift — see the
        // note on it for what drifting cost.
        //
        // A battle that has ended is idle too — nothing is moving and nothing
        // more will — or every tour that fights to a finish would hang on its
        // last `until`.
        idle: battle.state.is_over() || battle.listening(movers.is_empty()),
        waiting: false,
        over: battle.state.is_over(),
        score: battle.state.score.clone(),
        units: battle
            .state
            .units
            .iter()
            .map(|unit| crate::devtools::UnitFact {
                name: unit.name.clone(),
                alive: unit.alive(),
                aboard: unit.aboard.is_some(),
            })
            .collect(),
        log: log.0.iter().cloned().collect(),
        selected: battle
            .selected
            .and_then(|id| battle.state.unit(id))
            .map(|unit| unit.name.clone()),
        // Counted off the cached map rather than recomputed, so what a tour
        // asserts is the same arithmetic the tiles were painted from — a
        // fact derived twice is a fact that can disagree with the screen.
        danger: battle
            .show_danger
            .then(|| battle.danger.values().filter(|d| **d > 0.0).count() as u32),
    };
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

/// Why a battle could not be staged at all.
///
/// Every arm of this used to be an `expect`, which meant a mod that dropped a
/// vehicle between one save and the next took the whole game down rather than
/// declining one fight. The campaign asks the same question through
/// [`field_battle_problem`] *before* it commits its armies, so in practice
/// nobody should ever see one of these; it exists so that the day somebody
/// does, they see a line in the log.
#[derive(Debug, thiserror::Error)]
pub enum StagingError {
    #[error("no map `{0}` in the loaded mods")]
    MissingMap(String),
    #[error("map `{0}` will not parse: {1}")]
    Map(String, tactics_core::map::MapError),
    #[error("{0}")]
    Setup(#[from] tactics_core::battle::BattleSetupError),
}

/// Build the battle two armies have caused, on the map the terrain picked.
///
/// Split out of `setup_battle` so the campaign can ask whether a fight is
/// stageable *before* it commits armies to it — see [`field_battle_problem`].
/// Returns the state and, for each unit, the army it came from, so casualties
/// go home to the right one.
#[allow(clippy::type_complexity)]
fn stage_field_battle(
    registry: &tactics_core::data::DataRegistry,
    map_id: &str,
    sides: &[SideState],
    attacker_side: u8,
    forces: &[BattleForce],
    roster: &Arc<Roster>,
    seed: u64,
) -> Result<(BattleState, Vec<ArmyId>), StagingError> {
    let file = registry
        .map(map_id)
        .ok_or_else(|| StagingError::MissingMap(map_id.to_string()))?;
    let map = tactics_core::map::HexMap::from_map_file(file)
        .map_err(|e| StagingError::Map(map_id.to_string(), e))?;
    let (placements, crews, origins) = deploy(registry, &map, forces, attacker_side);
    // The campaign's own roster, so these are the same cadets who will carry
    // whatever happens here back out again.
    let state = BattleState::from_placements(
        registry,
        map,
        sides.to_vec(),
        &placements,
        &crews,
        roster.clone(),
        seed,
    )?;
    Ok((state, origins))
}

/// Whether the campaign can stage this fight, as a sentence for the log.
///
/// `None` means it can. The campaign calls this before `commit_to_battle`,
/// because refusing a battle is survivable and losing the whole run to a
/// panic in `setup_battle` is not. It stages the battle and throws it away,
/// which costs about a millisecond of grid building once per clash; doing
/// anything cleverer would mean a second copy of the rules about what a valid
/// order of battle is, and a second copy is how the two answers drift.
pub fn field_battle_problem(
    registry: &tactics_core::data::DataRegistry,
    map_id: &str,
    sides: &[SideState],
    attacker_side: u8,
    forces: &[BattleForce],
    roster: &Arc<Roster>,
) -> Option<String> {
    stage_field_battle(registry, map_id, sides, attacker_side, forces, roster, 0)
        .err()
        .map(|e| e.to_string())
}

// A Bevy system's arguments are its dependency list, not a signature anybody
// designed: this one now also needs the state machine, because a battle that
// cannot be staged has to hand the player back to the campaign instead of
// panicking.
#[allow(clippy::too_many_arguments)]
fn setup_battle(
    mut commands: Commands,
    mods: Res<Mods>,
    art: Res<ArtCache>,
    pending: Option<Res<PendingBattle>>,
    view: map_render::View,
    mut log: ResMut<BattleLog>,
    mut focus: ResMut<CameraFocus>,
    mut next: ResMut<NextState<AppState>>,
) {
    let registry = &mods.0;
    let pending = pending
        .map(|p| p.clone())
        .unwrap_or(PendingBattle::Scenario {
            map_id: "river_crossing".into(),
        });

    let staged = match &pending {
        PendingBattle::Scenario { map_id } => BattleState::from_map(registry, map_id, seed())
            .map(|state| (state, None))
            .map_err(StagingError::from),
        PendingBattle::Field {
            map_id,
            roster,
            attacker,
            defender,
            sides,
            attacker_side,
            forces,
        } => stage_field_battle(
            registry,
            map_id,
            sides,
            *attacker_side,
            forces,
            roster,
            seed(),
        )
        .map(|(state, origins)| {
            (
                state,
                Some(FieldBattle {
                    attacker: *attacker,
                    defender: *defender,
                    origins,
                }),
            )
        }),
    };

    // Nothing has been spawned yet, so backing out is just declining to enter:
    // the campaign is still where it was, and it hears why on its own log.
    let (mut state, field) = match staged {
        Ok(staged) => staged,
        Err(e) => {
            log.push(format!("This battle cannot be staged: {e}"));
            next.set(AppState::Overworld);
            return;
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
         M mount the hovered ride, U unload, \
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
        show_danger: false,
        danger: HashMap::new(),
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
                latitude: Latitude::Delegated,
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
) -> (Vec<UnitPlacement>, Vec<Vec<CadetId>>, Vec<ArmyId>) {
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
                // The crew travels alongside as cadet handles rather than in
                // the placement: a `UnitPlacement` names crew by definition
                // id, which is the thing this whole refactor is getting away
                // from.
                placements.push(UnitPlacement {
                    aboard_at: None,
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
            // passes: the cadet who inherits a formation mid-battle needs the
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
            // Wider than it was, because the panel now says what an order
            // means rather than only naming it, and a promise that wraps to
            // three lines is one nobody reads.
            width: Val::Px(300.0),
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
    drained.retain(|event| event.heard_by(&battle.state, view_side));
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
    // The log is radio traffic, not an omniscient narrator. One of ours
    // speaks with her call sign in front of her and says what she is doing;
    // anything else is a spot report — something the net heard about, with
    // nobody's voice on it.
    //
    // This is presentation and only presentation. The events carry the same
    // ids, hexes and flags they always did, so the script harness, the
    // replay and every consumer downstream read exactly what they read
    // before; what changed is who is speaking. Keeping that line clean is
    // what lets the voice be rewritten again later without anybody having to
    // check whether the engine still works.
    let sides: Vec<u8> = battle.state.units.iter().map(|u| u.side).collect();
    let mine = |id: UnitId| sides.get(id.index()).is_none_or(|s| *s == view_side);
    let traffic = |id: UnitId, said: &str| format!("{}: {said}", name(id));
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
                log.push(if mine(*unit) {
                    traffic(*unit, "ambush! we're in it —")
                } else {
                    format!("{} has driven into somebody.", name(*unit))
                });
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
                    "firing blind"
                } else if *opportunity {
                    "target of opportunity — engaging"
                } else {
                    "engaging"
                };
                let weapon_name = registry
                    .weapon(weapon)
                    .map(|w| w.name.clone())
                    .unwrap_or_default();
                log.push(if mine(*attacker) {
                    traffic(*attacker, &format!("{verb}, {weapon_name}."))
                } else {
                    format!("{} is firing.", name(*attacker))
                });
                spawn_puff(
                    &mut commands,
                    *at,
                    view.rotation(),
                    view.center(),
                    Color::srgb(1.0, 0.9, 0.4),
                );
            }
            // A shellburst is the one piece of news nobody can keep quiet: it
            // is a column of earth either army can see, so this is filtered
            // by nothing above and named without a shooter. What it did to
            // whoever was underneath arrives as the ordinary hit and module
            // lines that follow it in the same tick.
            BattleEvent::ShellLanded { at, ammo, .. } => {
                let round = registry
                    .ammo(ammo)
                    .map(|a| a.name.clone())
                    .unwrap_or_else(|| ammo.clone());
                log.push(format!("Shellfire — {round} at {}.", hex_label(*at)));
                spawn_puff(
                    &mut commands,
                    *at,
                    view.rotation(),
                    view.center(),
                    Color::srgb(1.0, 0.55, 0.15),
                );
            }
            BattleEvent::ShotHit {
                attacker,
                target,
                facing,
                ..
            } => {
                log.push(if mine(*attacker) {
                    traffic(
                        *attacker,
                        &format!("through {}'s {facing:?} plate.", name(*target)),
                    )
                } else {
                    traffic(*target, &format!("we're hit — {facing:?}."))
                });
                if let Some(entity) = entity_of(*target) {
                    commands
                        .entity(entity)
                        .insert(Flash(Timer::from_seconds(0.35, TimerMode::Once)));
                }
            }
            // A round that went past its target and found somebody else. The
            // `ShotMissed` above it already said she was missed; this says
            // where the round actually ended up, and without it the hit that
            // follows names a unit nobody fired at.
            BattleEvent::ShotStrayed {
                attacker,
                intended,
                onto,
            } => {
                log.push(if mine(*attacker) {
                    traffic(
                        *attacker,
                        &format!("over — round went into {}.", name(*onto)),
                    )
                } else {
                    format!(
                        "{} shoots past {} and into {}.",
                        name(*attacker),
                        name(*intended),
                        name(*onto)
                    )
                });
            }
            BattleEvent::ShotMissed { attacker, at } => {
                log.push(if mine(*attacker) {
                    traffic(*attacker, "miss.")
                } else {
                    format!("{} misses.", name(*attacker))
                });
                spawn_puff(
                    &mut commands,
                    *at,
                    view.rotation(),
                    view.center(),
                    Color::srgba(0.8, 0.8, 0.8, 0.8),
                );
            }
            BattleEvent::UnitDestroyed { unit, at } => {
                log.push(if mine(*unit) {
                    format!("{} is off the net.", name(*unit))
                } else {
                    format!("{} is finished.", name(*unit))
                });
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
            // No audience check here any more: `heard_by` has already dropped
            // any spot that was not this side's, and asking a second time in
            // a second vocabulary is how the two answers came to disagree.
            BattleEvent::UnitSpotted { unit, .. } => {
                log.push(format!("Contact — {}.", name(*unit)));
            }
            // Said in the log, because a cadet doing something other than what
            // she was told has to be attributable or it reads as a bug.
            BattleEvent::MoraleChanged { unit, rung, obeys } => {
                let who = battle
                    .state
                    .unit(*unit)
                    .map(|u| u.name.clone())
                    .unwrap_or_else(|| "A crew".into());
                log.push(if *obeys {
                    format!("{who}: {rung}.")
                } else {
                    format!("{who}: {rung} — not going forward.")
                });
            }
            BattleEvent::SetOut { unit, doing, .. } => {
                let who = battle
                    .state
                    .unit(*unit)
                    .map(|u| u.name.clone())
                    .unwrap_or_else(|| "A crew".into());
                // The first thing this AI has ever done that can be said in a
                // sentence. A vehicle crossing the map with nothing in the log
                // behind it reads as the game moving her for no reason.
                log.push(format!("{who}: {doing}."));
            }
            BattleEvent::Defied {
                unit, rung, doing, ..
            } => {
                let who = battle
                    .state
                    .unit(*unit)
                    .map(|u| u.name.clone())
                    .unwrap_or_else(|| "A crew".into());
                // Her name, her rung, and what she is doing about it. The
                // last part is the one that matters: a tank reversing out of
                // the line with nothing in the log to explain it is
                // indistinguishable from the game malfunctioning.
                log.push(format!("{who}: {rung} — {doing}."));
            }
            // The mid-round drill. Same sentence shape as the planning-table
            // drill's line, so the player learns one idiom for "she decided
            // this herself" wherever in the round it happens.
            BattleEvent::TookCover { unit, .. } => {
                log.push(traffic(*unit, "under fire — breaking for cover."));
            }
            BattleEvent::CrewHit { unit, cadet, out } => {
                let who = battle
                    .state
                    .roster
                    .get(*cadet)
                    .map(|g| g.name.clone())
                    .unwrap_or_else(|| "somebody".into());
                // A platoon's leaders have no station to slump at: they are on
                // their feet with their sections, and the log should not tell
                // an infantry casualty as a story about a vehicle interior.
                // `units.get` rather than `unit()`, which filters on `alive`:
                // the log is drained after the round has resolved, so a
                // platoon killed by this very burst would otherwise be
                // described as a tank crew on the way out.
                let afoot = battle
                    .state
                    .units
                    .get(unit.index())
                    .is_some_and(|u| u.troops(&mods.0).is_some());
                log.push(traffic(
                    *unit,
                    &match (*out, afoot) {
                        (true, true) => format!("{who} is down."),
                        (false, true) => format!("{who} is hit, still up."),
                        (true, false) => format!("{who} is out at her station."),
                        (false, false) => format!("{who} is hit."),
                    },
                ));
            }
            BattleEvent::ModuleHit {
                unit,
                module,
                destroyed,
            } => {
                // Troops are the one module that is people rather than
                // hardware, and "her Rifle Sections is damaged" reads like a
                // broken gearbox for the thing on the field that bleeds.
                let module_def = mods.0.module(module);
                if module_def.is_some_and(|m| m.effect == tactics_core::data::ModuleEffect::Troops)
                {
                    log.push(traffic(
                        *unit,
                        if *destroyed {
                            "no sections left."
                        } else {
                            "taking casualties."
                        },
                    ));
                } else {
                    let what = module_def
                        .map(|m| m.name.clone())
                        .unwrap_or_else(|| module.clone());
                    log.push(traffic(
                        *unit,
                        &format!("{what} {}.", if *destroyed { "gone" } else { "damaged" }),
                    ));
                }
            }
            BattleEvent::BrewedUp { unit } => {
                log.push(format!("{} is burning.", name(*unit)));
            }
            BattleEvent::Abandoned { unit } => {
                log.push(format!("{} — crew are out and clear.", name(*unit)));
            }
            BattleEvent::Mounted { unit, into } => {
                log.push(traffic(*unit, &format!("mounts up in {}.", name(*into))));
            }
            BattleEvent::Dismounted { unit, .. } => {
                log.push(traffic(*unit, "dismounting."));
            }
            // The armor holding is news the player must hear, or the shot
            // reads as the game eating a hit.
            BattleEvent::ShotBounced { target, facing, .. } => {
                log.push(if mine(*target) {
                    traffic(*target, &format!("that one bounced — {facing:?}."))
                } else {
                    format!("No effect on {} — {facing:?} plate held.", name(*target))
                });
            }
            BattleEvent::WeaponDry { unit, weapon } => {
                log.push(traffic(*unit, &format!("that was our last {weapon}.")));
            }
            // Withdrawing is not dying, and the screen has to say so plainly:
            // the sprite vanishes either way, and a player who reads a
            // successful withdrawal as a loss has been told a lie by the UI.
            BattleEvent::UnitExited { unit, at, .. } => {
                log.push(traffic(*unit, "clear of the field."));
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
                match battle.state.units.get(unit.index()).and_then(|u| u.march()) {
                    Some(march) => log.push(format!(
                        "{} has her orders and is on her way to {}.",
                        name(*unit),
                        hex_label(march.to)
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
/// Whether this side's net carries this piece of news.
///
/// Command traffic is a side's own business: the enemy commander's orders,
/// her formations' contact troubles, her radio queue and **where her crews
/// have decided to go** must not read out in the player's log — that is her
/// net, and listening to it is the electronic-warfare future, not a freebie.
/// Fighting events — shots, spots, wrecks, brew-ups — stay side-blind,
/// because they are things anybody on the field can see.
///
/// One function, two callers, and they must not drift: the log filters with
/// it *after* draining, and [`drive_ai`] filters with it *before* queueing.
/// The second is not tidiness. Planning events go through the same paced
/// animation queue as combat, and `accepting_orders` is false while that
/// queue has anything in it — so an event nobody will print still costs the
/// player a beat of not being able to give orders. One `SetOut` a unit put
/// nine of them in front of every planning phase, and the symptom was the
/// infantry tour clicking on a game that was not listening.
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
        let side = battle.view_side();
        for decision in decisions {
            // Filtered before it is queued, not after it is drained: an event
            // the player will never be shown must not cost her a beat of the
            // paced animation queue, because `accepting_orders` is false
            // while that queue is not empty.
            battle.anim.extend(
                decision
                    .events
                    .into_iter()
                    .filter(|e| e.heard_by(&battle.state, side)),
            );
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
    if !battle.listening(movers.is_empty()) {
        return;
    }
    let Some(side) = battle.human_side() else {
        return;
    };

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
                    latitude: Latitude::Delegated,
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

    // D shows what the ground would cost her: the move range tinted by the
    // fire every *spotted* enemy could put on each tile she can reach.
    //
    // `D` doubles as the camera's pan-right key, which is the bargain `A` and
    // `W` already make on this screen: a tap does the thing, a hold moves the
    // camera. It is worth paying again here because the mnemonic is the whole
    // of a toggle's discoverability and there is no other free letter that
    // says "danger".
    //
    // A toggle rather than a modal because it answers a question the player
    // asks *while* choosing — she wants the tint under the cursor she is
    // already moving — and because leaving it on across rounds is a
    // legitimate way to play. Announced in the log for the same reason every
    // silent state change in this game is: a key that changes a colour the
    // player was not looking at is indistinguishable from a key that does
    // nothing.
    if keys.just_pressed(KeyCode::KeyD) {
        battle.show_danger = !battle.show_danger;
        battle.range_dirty = true;
        log.push(if battle.show_danger {
            "Danger overlay on."
        } else {
            "Danger overlay off."
        });
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
                // Two modifiers, each qualifying the order rather than
                // changing it, and they compose: Shift queues instead of
                // replacing ("…and then this"), Ctrl means it ("and I am not
                // asking"). The engine refuses a leg behind a stand-fast or a
                // retreat, and that refusal reaches the log like any other.
                //
                // Ctrl rather than a key of its own because there is no key
                // left that would not lie: `X` is already the assault, which
                // is the *other* axis — press on through fire — and a player
                // who pressed it expecting insistence would get a different
                // order. Latitude has no verb of its own at formation scale,
                // so it takes a modifier.
                let queue = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
                let insist =
                    keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
                let latitude = if insist {
                    Latitude::Binding
                } else {
                    Latitude::Delegated
                };
                let formation = FormationId(index as u32);
                let order = if queue {
                    Order::QueueMission {
                        formation,
                        mission,
                        latitude,
                    }
                } else {
                    Order::SetMission {
                        formation,
                        mission,
                        latitude,
                    }
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

    // X = press on: march to the hovered ground and do not break off for
    // cover on the way.
    //
    // Deliberately the same key that orders a *formation* to assault, because
    // it is deliberately the same sentence: `Advance`/`Assault` and
    // `Delegated`/`Binding` are one distinction at two scales — take that
    // ground using your judgment, versus take that ground and I mean it. A
    // player who has learned what X costs a platoon has already learned what
    // it costs a crew, and there is no second idiom to teach. The two never
    // collide: picking a formation drops the unit selection and vice versa,
    // so only one of them can be listening.
    //
    // Any tile on the map is fair game rather than only this round's reach,
    // which is what a standing destination means — she marches for it over as
    // many rounds as the ground demands. The engine refuses one off the map.
    if keys.just_pressed(KeyCode::KeyX) {
        if let (Some(unit), Some(hex)) = (battle.selected, hovered) {
            let who = unit_name(&battle.state, unit);
            set_intent_saying(
                registry,
                &mut battle,
                &Order::Radio {
                    unit,
                    to: Some(hex),
                    fire: None,
                    latitude: Latitude::Binding,
                },
                &mut log,
                format!("{who} will press on to {} through fire.", hex_label(hex)),
            );
        }
        return;
    }

    // M = mount: the selected foot unit boards the friendly transport under
    // the cursor. Deliberately the same grammar as `A` — pick your cadet,
    // point at the thing you mean, press the key — because "board that
    // halftrack" and "shoot that tank" are the same kind of sentence and the
    // player should not have to learn a second idiom for it. `Order::Mount`
    // is a standing march, so she walks there over as many rounds as it takes
    // and climbs in the tick she arrives alongside; every reason she might
    // not be able to (not on foot, wrong side, no room, no lift) is the
    // engine's refusal and reaches the log by the ordinary road.
    if keys.just_pressed(KeyCode::KeyM) {
        if let Some(unit) = battle.selected {
            // Resolved to a pair before anything mutable happens, so the
            // borrow of the carrier ends here rather than spanning the order.
            let ride = hovered
                .and_then(|hex| battle.state.unit_at(hex).filter(|u| u.side == side))
                .map(|c| (c.id, c.name.clone()));
            match ride {
                Some((into, carrier)) => {
                    let who = unit_name(&battle.state, unit);
                    set_intent_saying(
                        registry,
                        &mut battle,
                        &Order::Mount { unit, into },
                        &mut log,
                        format!("{who} makes for {carrier} and mounts up on arrival."),
                    );
                }
                None => log.push("Hover one of your own vehicles, then press M to mount."),
            }
        }
        return;
    }

    // U = unload, read two ways that can never be confused for each other: a
    // passenger gets off, and a loaded carrier puts everybody off. It is one
    // `Dismount` per passenger rather than an order aimed at the vehicle,
    // because the order is about a cadet deciding to be on the ground — a
    // carrier is not a thing that can be told to empty itself.
    if keys.just_pressed(KeyCode::KeyU) {
        if let Some(unit) = battle.selected {
            let riders: Vec<UnitId> = if battle.state.unit(unit).is_some_and(|u| u.aboard.is_some())
            {
                vec![unit]
            } else {
                battle.state.passengers(unit)
            };
            if riders.is_empty() {
                log.push("She is not riding anything and nobody is riding her.");
            }
            for rider in riders {
                let who = unit_name(&battle.state, rider);
                set_intent_saying(
                    registry,
                    &mut battle,
                    &Order::Dismount { unit: rider },
                    &mut log,
                    format!("{who} gets off at the next opportunity."),
                );
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
                    latitude: Latitude::Delegated,
                },
                &mut log,
            );
        }
        return;
    }

    // Click own unit: select it. Every unit can be given orders during
    // planning, including ones that already have some.
    //
    // Clicking a hex that holds more than one of her crews steps to the next
    // one rather than re-selecting the same one, because since stacking
    // landed the second occupant was **unreachable by mouse entirely**:
    // `unit_at` answers with whoever comes first in id order, and a platoon
    // that dismounts onto her carrier's own hex — which is now the ordinary
    // case, tried before the neighbours — sits behind the carrier forever.
    // The infantry tour caught it, having been the only thing that ever tried
    // to re-mount a platoon.
    //
    // `occupants` rather than `unit_at` is the honest question here, and it
    // walks `units` in id order, so the cycle is the same on every machine.
    let mine: Vec<tactics_core::battle::UnitId> = battle
        .state
        .occupants(hex)
        .filter(|u| u.side == side)
        .map(|u| u.id)
        .collect();
    if !mine.is_empty() {
        let id = match battle
            .selected
            .and_then(|cur| mine.iter().position(|&id| id == cur))
        {
            // Already on one of them: the next, wrapping. Two crews on a hex
            // is a toggle, which is what a player expects of a second click.
            Some(i) => mine[(i + 1) % mine.len()],
            None => mine[0],
        };
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
                latitude: Latitude::Delegated,
            },
            &mut log,
        );
    }
}

/// Apply one planning order and refresh the overlays that show it.
///
/// The player's direct orders travel as [`Order::Radio`] rather than
/// `SetMove`/`SetFire`, because they are the *commander* speaking and a
/// commander needs a wire. The engine decides what that costs: a cadet on the
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

/// [`set_intent`], plus a line of acknowledgement when the engine takes the
/// order.
///
/// Most orders announce themselves: a radio order comes back as an
/// acknowledgement, a mission as an assignment, a route as a drawn path.
/// Mount and dismount produce no event until the tick they actually happen
/// on, and they change nothing on the map in the meantime — a passenger who
/// will step off next tick looks exactly like a passenger who will not. To a
/// player that is indistinguishable from a key that does not work, which is
/// the same bargain every silent deviation in this game has to make. Only on
/// success: a refusal has already printed its own reason.
fn set_intent_saying(
    registry: &tactics_core::data::DataRegistry,
    battle: &mut Battle,
    order: &Order,
    log: &mut BattleLog,
    said: String,
) {
    match battle.state.apply(registry, order) {
        Ok(events) => {
            battle.anim.extend(events);
            battle.range_dirty = true;
            log.push(said);
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
        if let Order::SetMove { unit, .. } = decision.order
            && decision.drill
        {
            drilled.push(unit);
        }
    });
    anim.extend(events);
    for error in refused {
        log.push(format!("Your staff fumbled an order: {error}"));
    }
    // Every drill move is said out loud, because a vehicle moving without a
    // visible order behind it is indistinguishable from a bug — the same
    // bargain every deviation in this game makes.
    //
    // This used to guess at which moves were the drill by keeping only units
    // whose formation had no mission, which silently dropped the one case
    // that most needed saying: a crew under the commander's *personal*
    // tasking is normally in a formation that does have a mission, so when
    // her march was broken off for cover the line was filtered away and
    // nothing was reported at all. The player watched a tank she had ordered
    // to a ridge stop in a hedge, every round, for no stated reason. The
    // planner now says which orders were its own idea (`Decision::drill`) so
    // there is nothing left to guess.
    //
    // The two cases read differently on purpose. A crew nobody ordered took
    // cover on her own initiative; a crew who *was* ordered somewhere broke
    // off something she had been told to do, and the difference is the whole
    // reason the player is being told.
    for unit in drilled {
        let Some(u) = state.units.get(unit.index()) else {
            continue;
        };
        let name = u.name.clone();
        log.push(match u.march() {
            Some(march) => format!(
                "{name} breaks off her march to {} and takes cover.",
                hex_label(march.to)
            ),
            None => format!("{name} is under fire and takes cover on her own."),
        });
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
            latitude: Latitude::Delegated,
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
            // Condition — cadets and modules over the full complement — is
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

    // What the ground would cost her, for the whole reach at once. Rebuilt
    // here rather than in the panel because this is the one place that
    // already knows the reach could have changed, and because a per-frame
    // `fire_on` over a hundred tiles is real money — see the note on
    // `Battle::danger`.
    //
    // Only her own crews, and only while the overlay is up: an enemy's
    // reachable ground is not a thing the player is entitled to be shown, and
    // computing the tints for an overlay nobody asked for would pay the cost
    // for nothing.
    battle.danger.clear();
    if battle.show_danger {
        let view_side = battle.view_side();
        if let Some(unit) = battle
            .selected
            .filter(|id| battle.state.unit(*id).is_some_and(|u| u.side == view_side))
        {
            let tiles: Vec<Hex> = battle.move_range.keys().copied().collect();
            let battle = &mut *battle;
            for hex in tiles {
                // Per round and in worth, which is what the evaluator's
                // threat term spends on the same ground. The player and the
                // AI price a tile with one number or the player is playing a
                // different game from her opponent — and since cadence and
                // pressure joined that number, a tint summing single shots of
                // damage would be showing her a game nobody is playing.
                let total: f32 = tactics_core::battle::fire_on(&mods.0, &battle.state, unit, hex)
                    .iter()
                    .map(|bearing| bearing.worth_per_round())
                    .sum();
                battle.danger.insert(hex, total);
            }
        }
    }

    // The danger tint replaces the move-range blue rather than sitting on top
    // of it. Two translucent fills over one tile make a third colour that
    // means neither of them, and the set of hexes is identical anyway — this
    // is the same overlay answering a second question about the same ground.
    let left = battle
        .selected
        .and_then(|id| battle.state.unit(id))
        .map(|u| {
            let (have, full) = battle.state.substance(&mods.0, u);
            if have > 0 { have } else { full }.max(1) as f32
        })
        .unwrap_or(1.0);
    for hex in battle.move_range.keys() {
        let overlay = HexOverlay::face(*hex);
        let color = match battle.danger.get(hex) {
            Some(worth) => danger_color(panel::danger_tint(worth / left)),
            None => DANGER_NONE,
        };
        commands.spawn((
            Sprite {
                image: art.face.clone(),
                color,
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

    // What the ground under the cursor would cost the crew the player has
    // picked, joined onto the two branches that are about ground.
    //
    // Keyed off the *selection* rather than off whoever the panel happens to
    // be describing, because that is whose decision this is — the question is
    // "what could they do to *her* over there", and the crew it is about is
    // the one the player has in hand. Never for an enemy crew, which is the
    // rule the unit panel's order hints already follow and for the same
    // reason: an enemy's exposure is not the player's to read, and offering
    // it would be an offer.
    //
    // Deliberately not on the shot preview or the ghost report. Both are
    // already an answer about a hex somebody *else* is standing on, and
    // "what could be put on you if you stood where that tank is" is a
    // question nobody asked.
    //
    // A formation being commanded makes this empty on its own, without a
    // branch: picking one drops the unit selection.
    //
    // It leads the panel rather than following it, which is a decision about
    // what falls off the bottom rather than about importance. A full crew's
    // description — chassis, armour, speed, sight, every weapon's range and
    // cadence, every cadet's two best skills — already fills the panel on its
    // own, and appending this put the one part that *changes as the mouse
    // moves* below the fold on exactly the crews worth looking at. A
    // datasheet the player can scroll to later loses less by being second
    // than a live answer does by being invisible.
    let danger = battle
        .selected
        .filter(|id| state.unit(*id).is_some_and(|u| u.side == view_side))
        .zip(hovered_tile)
        .map(|(unit, hex)| {
            let mut section = format_danger(registry, state, unit, hex);
            // The legend goes with the overlay and not with the section: the
            // ramp only exists while something is painted on it, and a key
            // to colours nobody can see is furniture.
            if battle.show_danger {
                section.push_str("\n\nOverlay (D), a round of fire:");
                for line in panel::DANGER_LEGEND {
                    section.push_str(&format!("\n  {line}"));
                }
            }
            section.push_str("\n\n");
            section
        })
        .unwrap_or_default();

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
            text.0 = format_attack(registry, &preview);
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
        text.0 = format!(
            "{danger}{}",
            format_unit(
                registry,
                state,
                unit,
                hovered_tile.unwrap_or(unit.pos),
                unit.side == view_side,
            )
        );
        set_portrait(&mut hud.portrait, &art, state, unit.id);
        return;
    }
    if let Some(hex) = hovered_tile {
        text.0 = format!("{danger}{}", format_tile(registry, state, hex));
        return;
    }
    text.0 = "Hover a tile for terrain\n\nLMB: select / set route\nA: engage hovered enemy\nB: blind fire a tile\nV: hold and watch\nM: mount the hovered ride\nU: unload (her, or all aboard)\nC: clear orders\nEnter: commit the round\nF: pick a formation\nQ/E: rotate view".into();
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
        .map(|cadet| cadet.def.as_str())
        .unwrap_or(&unit.vehicle);
    if let Some(handle) = art.portraits.get(key) {
        image.image = handle.clone();
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

/// What a finished field battle owes the campaign: who walked away from it,
/// and who did not.
///
/// Pure over the finished battle and the bookkeeping the clash was staged
/// with, and free of Bevy on purpose. This is the seam where a battle becomes
/// campaign state — everything on the far side of it, a cadet's wound, an
/// army's destruction, a crew's battle count, is written from what this
/// returns — and while it lived inside a system there was no way to call it
/// without a running app, so the one piece of arithmetic that can silently
/// corrupt a campaign was the one piece nothing tested.
fn battle_outcome(state: &BattleState, field: &FieldBattle) -> BattleOutcome {
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
    for unit in state.surviving_units() {
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

    // Everyone the battle hurt, in two kinds.
    //
    // First, everyone who was aboard something that burned. The battle
    // reports who and what killed it; the campaign decides what that
    // cost them, because whether this game kills its characters is a
    // campaign rule.
    let mut losses = Vec::new();
    for unit in state.lost_units() {
        for cadet in &unit.crew {
            losses.push(CrewLoss {
                cadet: *cadet,
                vehicle: unit.vehicle.clone(),
                killed_by: unit.last_hit_by,
                found: None,
            });
        }
    }
    // ...and then everyone who was hurt at her station in a vehicle that
    // came home. This half used to be thrown away at the door: the
    // battle tracked each cadet's condition seat by seat all fight, and
    // then the only casualties the campaign ever heard about were the
    // crews of destroyed vehicles. A gunner knocked out on the first
    // round of a battle her side won was fit again by the time the
    // campaign screen drew, which is the wound system having no teeth in
    // the most literal possible sense.
    //
    // `Absent` is skipped for the reason it exists: she was in the
    // infirmary before this battle started and is not a casualty of it.
    for unit in state.surviving_units() {
        for (seat, cadet) in unit.crew.iter().enumerate() {
            let found = unit.crew_state.get(seat).copied();
            if !matches!(
                found,
                Some(CrewCondition::Wounded) | Some(CrewCondition::Out)
            ) {
                continue;
            }
            losses.push(CrewLoss {
                cadet: *cadet,
                vehicle: unit.vehicle.clone(),
                killed_by: unit.last_hit_by,
                found,
            });
        }
    }
    BattleOutcome {
        attacker: field.attacker,
        defender: field.defender,
        winner: state.over.and_then(|r| r.winner),
        stalemate: matches!(state.over.map(|r| r.reason), Some(EndReason::Stalemate)),
        survivors,
        losses,
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
        commands.insert_resource(battle_outcome(&battle.state, field));
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
        )
        .expect("the staged placements are content the base mod ships");
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

    /// The panels say what an order commits her to, at both scales.
    ///
    /// Screenshots are the usual way to review a panel and they cannot be
    /// captured from a headless shell, so the wording that the whole of step
    /// 2 consists of would otherwise be reviewable by nobody. Asserting on
    /// the strings is not elegant; a promise silently disappearing from the
    /// one page that explains the game is worse.
    #[test]
    fn the_panels_explain_the_orders_they_offer() {
        let reg = registry();
        let mut state = BattleState::from_map(&reg, "river_crossing", 5).expect("battle builds");

        // The formation panel, under an order, offers the menu and reads the
        // standing order back with its price attached.
        let formation = FormationId(0);
        let to = state
            .formations()
            .first()
            .and_then(|f| f.leader)
            .and_then(|id| state.unit(id))
            .map(|u| u.pos)
            .expect("a formation with somebody in it");
        state
            .apply(
                &reg,
                &Order::SetMission {
                    formation,
                    mission: Mission::Assault { to },
                    latitude: Latitude::Delegated,
                },
            )
            .expect("the order lands");
        let panel = format_formation(&reg, &state, &state.formations()[0], None);
        assert!(
            panel.contains(Mission::Assault { to }.promise()),
            "the standing order does not say what it costs:\n{panel}"
        );
        assert!(
            panel.contains(Mission::Advance { to }.promise()),
            "the menu does not offer the alternative:\n{panel}"
        );

        // ...and the unit panel does the same for the per-crew twin, but only
        // for a crew the viewer may actually order.
        let ours = state.units.iter().find(|u| u.side == 0).expect("our tank");
        let theirs = state
            .units
            .iter()
            .find(|u| u.side == 1)
            .expect("their tank");
        let mine = format_unit(&reg, &state, ours, ours.pos, true);
        assert!(
            mine.contains(Latitude::Binding.promise()),
            "the key that presses her on is explained nowhere:\n{mine}"
        );
        let hers = format_unit(&reg, &state, theirs, theirs.pos, false);
        assert!(
            !hers.contains(Latitude::Binding.promise()),
            "the panel is offering to give the enemy orders:\n{hers}"
        );
    }

    /// A formation's orders read back with how hard they were meant.
    ///
    /// The same argument as the promises above and one step further: the
    /// verb and the latitude are two decisions, and a panel that shows one
    /// of them leaves the player unable to tell why two formations under the
    /// same order behave differently. Asserted on both branches — a standing
    /// order and one still on the wire — because under a signals net the
    /// second is the only place she can read her decision back before it is
    /// too late to change it.
    #[test]
    fn a_formation_panel_says_how_hard_its_orders_were_meant() {
        let reg = registry();
        let to = |state: &BattleState| {
            state
                .formations()
                .first()
                .and_then(|f| f.leader)
                .and_then(|id| state.unit(id))
                .map(|u| u.pos)
                .expect("a formation with somebody in it")
        };
        let panel = |latitude: Latitude| {
            let mut state = BattleState::from_map(&reg, "river_crossing", 5).expect("battle");
            let to = to(&state);
            state
                .apply(
                    &reg,
                    &Order::SetMission {
                        formation: FormationId(0),
                        mission: Mission::Advance { to },
                        latitude,
                    },
                )
                .expect("the order lands");
            format_formation(&reg, &state, &state.formations()[0], None)
        };

        let insisted = panel(Latitude::Binding);
        assert!(
            insisted.contains(Latitude::Binding.mission_promise()),
            "an order the player insisted on reads back as an ordinary one:\n{insisted}"
        );
        let asked = panel(Latitude::Delegated);
        assert!(
            asked.contains(Latitude::Delegated.mission_promise()),
            "and an ordinary one says so rather than saying nothing:\n{asked}"
        );
        // The negative half is asserted on the *delegated* clause rather
        // than the binding one, because the menu line below advertises what
        // insisting buys and would satisfy a naive `contains` on every
        // panel. What must not appear is the promise that contradicts the
        // order actually given.
        assert!(
            !insisted.contains(Latitude::Delegated.mission_promise()),
            "an insisted order still says her doctrine may bend it:\n{insisted}"
        );
        // The modifier that buys it is offered in the same place the verbs
        // are, or it is a feature only a reader of the source can find.
        assert!(
            asked.contains("Ctrl"),
            "nothing on the page says how to insist:\n{asked}"
        );
    }

    // ---------------------------------------------------------------
    // The campaign seam: a whole field battle, fought headlessly, and
    // handed back to the roster it was crewed from.
    //
    // Everything below drives the real path — `field_battle_problem`,
    // `stage_field_battle`, `inherit_army_missions`, `battle_outcome`,
    // `OverworldState::apply_battle_result` — because a parallel staging
    // would be a second answer to what an order of battle is, and the
    // whole point of testing this seam is that the two halves agree.
    // ---------------------------------------------------------------

    use tactics_core::battle::Fate;
    use tactics_core::overworld::{OverworldEvent, OverworldState};
    use tactics_core::roster::CadetStatus;

    /// The campaign the base mod ships, with all forty-nine named cadets
    /// enlisted into the roster its armies are crewed from.
    fn campaign(reg: &tactics_core::data::DataRegistry) -> OverworldState {
        OverworldState::from_map(reg, "frontier", 11).expect("the shipped campaign map builds")
    }

    /// The armies committed to a clash, exactly as `launch_battle` commits
    /// them: principals first, each one's live unit list and whatever it was
    /// already trying to do.
    fn committed(state: &OverworldState, armies: &[ArmyId]) -> Vec<BattleForce> {
        armies
            .iter()
            .filter_map(|id| state.army(*id))
            .map(|army| BattleForce {
                army: army.id,
                side: army.side,
                units: army.units.clone(),
                mission: army.mission.clone(),
            })
            .collect()
    }

    /// A battle that has been fought to a finish, with the bookkeeping that
    /// says which army each hull marched in with.
    struct Fought {
        state: BattleState,
        outcome: BattleOutcome,
        forces: Vec<BattleForce>,
        rounds: usize,
    }

    /// Stage the clash the campaign would stage, fight it out with nobody
    /// watching, and do the accounting.
    ///
    /// Both sides are given a planner: the campaign hands side 0 to the
    /// player and a headless test has no player, so her seat is filled the
    /// same way `STAHL_AUTOPLAY` fills it.
    fn fight(
        reg: &tactics_core::data::DataRegistry,
        campaign: &OverworldState,
        map_id: &str,
        attacker: ArmyId,
        defender: ArmyId,
        seed: u64,
    ) -> Fought {
        let sides: Vec<SideState> = campaign
            .sides
            .iter()
            .map(|s| SideState {
                name: s.name.clone(),
                ai: s.ai.clone(),
            })
            .collect();
        let attacker_side = campaign.army(attacker).expect("the attacker exists").side;
        let forces = committed(campaign, &[attacker, defender]);
        let roster = std::sync::Arc::new(campaign.roster.clone());

        // The campaign asks before it commits; so does this.
        assert!(
            field_battle_problem(reg, map_id, &sides, attacker_side, &forces, &roster).is_none(),
            "the shipped campaign cannot stage its own opening clash on {map_id}"
        );
        let (mut state, origins) =
            stage_field_battle(reg, map_id, &sides, attacker_side, &forces, &roster, seed)
                .expect("the clash the campaign just approved");
        let field = FieldBattle {
            attacker,
            defender,
            origins,
        };
        inherit_army_missions(reg, &mut state, &forces, attacker, defender);

        let mut ai = AiDriver::new();
        for side in 0..state.sides.len() as u8 {
            let cfg = state.sides[side as usize].ai.clone().unwrap_or(AiConfig {
                planner: "utility".into(),
                difficulty: 3,
                doctrine: Some("bounding_overwatch".into()),
            });
            ai.insert(side, make_battle_planner(&cfg, seed ^ side as u64, reg));
        }

        let mut rounds = 0;
        while !state.is_over() && rounds < 60 {
            ai.plan_round(reg, &mut state);
            state.resolve_round(reg);
            rounds += 1;
        }
        let outcome = battle_outcome(&state, &field);
        Fought {
            state,
            outcome,
            forces,
            rounds,
        }
    }

    /// Every cadet the battle was handed comes back out of it exactly once.
    ///
    /// The seam between a battle and a campaign is arithmetic nobody watches:
    /// a survivor list built off unit indices, a loss list built off two
    /// different questions ("did her vehicle come home" and "was she hurt in
    /// the seat"), and a roster written from both. A cadet dropped here is a
    /// person who quietly stops existing, and a cadet counted twice is one
    /// whose wound is rolled for twice; neither shows up as a crash.
    ///
    /// The one deliberate overlap is a cadet hurt at her station in a vehicle
    /// that came home: she is *both* aboard a survivor and reported, and
    /// `CrewLoss::found` is what says so. A cadet pulled out of a wreck
    /// (`found: None`) must never be both.
    #[test]
    fn every_cadet_who_marched_into_a_field_battle_is_accounted_for_when_it_ends() {
        let reg = registry();
        let mut base = campaign(&reg);
        let (attacker, defender) = (ArmyId(0), ArmyId(2));
        // One vehicle nobody was assigned to, which is ordinary campaign
        // state — an army can hold a chassis it has no cadets for. The
        // battle crews it anonymously out of its *own* copy of the roster,
        // so this is what makes the "the campaign never enlisted her" check
        // below ask a real question rather than an empty one.
        base.army_mut(attacker)
            .expect("the attacker exists")
            .units
            .push(ArmyUnit {
                vehicle: "light_tank".into(),
                crew: Vec::new(),
                name: Some("Spare".into()),
            });

        let mut winners = Vec::new();
        let mut station_wounds = 0;
        let mut exits = 0;
        for seed in [0u64, 6, 10, 11] {
            let fought = fight(&reg, &base, "battle_plains", attacker, defender, seed);
            assert!(
                fought.state.is_over(),
                "seed {seed} was still being fought after {} rounds",
                fought.rounds
            );
            winners.push(fought.outcome.winner);

            let marched: Vec<CadetId> = fought
                .forces
                .iter()
                .flat_map(|f| f.units.iter().flat_map(|u| u.crew.iter().copied()))
                .collect();
            let hulls: usize = fought.forces.iter().map(|f| f.units.len()).sum();
            assert_eq!(
                fought.state.units.len(),
                hulls,
                "seed {seed}: somebody was left in the assembly area"
            );

            // The vehicle count is conserved: a hull is either a survivor or
            // a wreck, and there is no third place for one to go.
            let survivors: usize = fought.outcome.survivors.iter().map(|(_, u)| u.len()).sum();
            let lost = fought.state.lost_units().count();
            assert_eq!(
                survivors + lost,
                hulls,
                "seed {seed}: {survivors} survivors + {lost} wrecks is not the {hulls} that marched in"
            );

            // A crew that drove off the map by an exit came home. This is the
            // case that was wrong once — `alive_units` here would have handed
            // the campaign a withdrawal as a burnt-out vehicle.
            for unit in fought.state.units.iter() {
                if !matches!(unit.fate, Fate::Exited) {
                    continue;
                }
                exits += 1;
                for cadet in &unit.crew {
                    assert!(
                        fought
                            .outcome
                            .survivors
                            .iter()
                            .any(|(_, units)| units.iter().any(|u| u.crew.contains(cadet))),
                        "seed {seed}: {cadet:?} took an exit and was not reported home"
                    );
                    assert!(
                        !fought
                            .outcome
                            .losses
                            .iter()
                            .any(|l| l.cadet == *cadet && l.found.is_none()),
                        "seed {seed}: {cadet:?} drove off the map and was written off as a wreck"
                    );
                }
            }

            // Now hand it to the campaign, which is where it becomes state
            // somebody has to live with.
            let mut after = base.clone();
            let before: HashMap<CadetId, u32> = after
                .roster
                .iter()
                .map(|cadet| (cadet.id, cadet.battles))
                .collect();
            after.commit_to_battle(&[]);
            let events = after.apply_battle_result(
                &reg,
                fought.outcome.attacker,
                fought.outcome.defender,
                &fought.outcome.survivors,
                &fought.outcome.losses,
            );

            for cadet in &marched {
                let aboard = after
                    .armies
                    .iter()
                    .any(|a| a.units.iter().any(|u| u.crew.contains(cadet)));
                let reported: Vec<&CrewLoss> = fought
                    .outcome
                    .losses
                    .iter()
                    .filter(|l| l.cadet == *cadet)
                    .collect();
                assert!(
                    aboard || !reported.is_empty(),
                    "{cadet:?} marched out at seed {seed} and is in nobody's account"
                );
                assert!(
                    reported.len() <= 1,
                    "{cadet:?} was reported {} times at seed {seed}",
                    reported.len()
                );
                if let Some(loss) = reported.first() {
                    if loss.found.is_none() {
                        assert!(
                            !aboard,
                            "{cadet:?} was pulled out of a wreck and is still crewing at seed {seed}"
                        );
                    } else {
                        // Hurt at her station in a vehicle that came home:
                        // the one cadet who is legitimately in both lists,
                        // and the wound has to outlive the battle.
                        station_wounds += 1;
                        assert!(
                            aboard,
                            "{cadet:?} came home in her own tank and left the army"
                        );
                        let status = after.roster.get(*cadet).expect("she is on the roll").status;
                        assert!(
                            !status.is_ready(),
                            "{cadet:?} was found {:?} at her station and the campaign says she is fine",
                            loss.found
                        );
                        assert!(
                            matches!(status, CadetStatus::Wounded { .. } | CadetStatus::Dead),
                            "a station casualty is treated, not adrift: {status:?}"
                        );
                    }
                    assert!(
                        events.iter().any(|e| matches!(
                            e,
                            OverworldEvent::CrewCasualty { cadet: c, .. } if c == cadet
                        )),
                        "{cadet:?} was a casualty at seed {seed} and nobody was told"
                    );
                }
            }

            // Every survivor has one more battle behind her, and nobody else
            // does: a crew that did not fight cannot be credited with it.
            for (_, units) in &fought.outcome.survivors {
                for unit in units {
                    for cadet in &unit.crew {
                        if let Some(now) = after.roster.get(*cadet) {
                            assert_eq!(
                                now.battles,
                                before[cadet] + 1,
                                "{cadet:?} survived seed {seed} and was not credited with it"
                            );
                        }
                    }
                }
            }
            for unit in fought.state.lost_units() {
                for cadet in &unit.crew {
                    if let Some(now) = after.roster.get(*cadet) {
                        assert_eq!(
                            now.battles, before[cadet],
                            "{cadet:?} did not come home from seed {seed} and was credited with it"
                        );
                    }
                }
            }
            for cadet in after.roster.iter() {
                if !marched.contains(&cadet.id) {
                    assert_eq!(
                        cadet.battles, before[&cadet.id],
                        "{:?} stayed at the academy and was credited with a battle",
                        cadet.id
                    );
                }
            }

            // The academy's rolls are the academy's: an anonymous crew
            // enlisted into the battle's own copy of the roster must not come
            // back holding a handle the campaign cannot resolve.
            for army in &after.armies {
                for unit in &army.units {
                    for cadet in &unit.crew {
                        assert!(
                            after.roster.get(*cadet).is_some(),
                            "{} holds {cadet:?}, whom the campaign never enlisted",
                            army.name
                        );
                    }
                }
            }

            // An army with nothing left is destroyed, and said to be.
            for army in &after.armies {
                if army.units.is_empty() && [attacker, defender].contains(&army.id) {
                    assert!(
                        !army.alive,
                        "{} lost every vehicle and is still on the map",
                        army.name
                    );
                    assert!(
                        events
                            .iter()
                            .any(|e| matches!(e, OverworldEvent::ArmyDestroyed { army: a } if *a == army.id)),
                        "{} was wiped out at seed {seed} and nobody was told",
                        army.name
                    );
                }
            }
        }
        // Not invariants of the seam but of this test being worth running.
        // Four seeds on `battle_plains` are fought out because one battle is
        // one shape of ending: these four are 5 wrecks / 7 / 9 / 6 with two
        // won by each side, so the accounting is checked against a rout in
        // both directions rather than against one lucky afternoon.
        assert!(
            winners.contains(&Some(0)) && winners.contains(&Some(1)),
            "every seed was won by the same side, so a defeat's accounting went unchecked: {winners:?}"
        );
        assert!(
            station_wounds >= 2,
            "no cadet was hurt at her station and carried home; the half of the \
             loss list that is not a wreck went untested (exits seen: {exits})"
        );
    }

    /// A campaign that fights the same battle twice comes out of it in the
    /// same place.
    ///
    /// The casualty rolls run on the campaign's own rng, and the seam sorts
    /// the losses by cadet id before spending it for exactly this reason: the
    /// battle reports them in whatever order its units happen to sit in, and
    /// a replay that drew them in that order would diverge from the day it
    /// replays.
    #[test]
    fn the_same_battle_leaves_the_campaign_in_the_same_state_twice() {
        let reg = registry();
        let base = campaign(&reg);
        let apply = |seed: u64| -> String {
            let fought = fight(&reg, &base, "battle_plains", ArmyId(0), ArmyId(2), seed);
            let mut after = base.clone();
            after.apply_battle_result(
                &reg,
                fought.outcome.attacker,
                fought.outcome.defender,
                &fought.outcome.survivors,
                &fought.outcome.losses,
            );
            serde_json::to_string(&after).expect("a campaign serialises")
        };
        for seed in [0u64, 6] {
            assert_eq!(
                apply(seed),
                apply(seed),
                "seed {seed} left the campaign somewhere else the second time"
            );
        }
    }

    /// An army caught pulling back gets its crews home rather than losing
    /// them: a vehicle that takes an exit it is entitled to is a survivor,
    /// and the army that owns her is not destroyed for having left.
    #[test]
    fn a_crew_that_drives_off_the_map_comes_home_to_her_army() {
        let reg = registry();
        let mut base = campaign(&reg);
        // Through the campaign's own order, so the mission is one an army
        // could really be carrying when it is caught.
        let falling_back = ArmyId(0);
        base.apply(
            &reg,
            &tactics_core::overworld::OverworldOrder::SetMission {
                army: falling_back,
                mission: ArmyMission::Withdraw {
                    to: tactics_core::offset_to_hex(0, 1),
                },
            },
        )
        .expect("her own headquarters can reach her on day one");

        let mut exited = 0;
        for seed in [5u64, 23, 44] {
            let fought = fight(&reg, &base, "river_crossing", ArmyId(2), falling_back, seed);
            for unit in fought.state.units.iter() {
                if !matches!(unit.fate, Fate::Exited) {
                    continue;
                }
                exited += 1;
                let mut after = base.clone();
                let home = fought
                    .outcome
                    .survivors
                    .iter()
                    .find(|(id, _)| *id == falling_back)
                    .map(|(_, units)| units.len())
                    .unwrap_or(0);
                assert!(
                    home > 0,
                    "seed {seed}: a crew took the exit and her army came home empty"
                );
                let events = after.apply_battle_result(
                    &reg,
                    fought.outcome.attacker,
                    fought.outcome.defender,
                    &fought.outcome.survivors,
                    &fought.outcome.losses,
                );
                assert!(
                    !events.iter().any(
                        |e| matches!(e, OverworldEvent::ArmyDestroyed { army } if *army == falling_back)
                    ),
                    "seed {seed}: an army that withdrew was written off as destroyed"
                );
                for cadet in &unit.crew {
                    assert!(
                        !fought
                            .outcome
                            .losses
                            .iter()
                            .any(|l| l.cadet == *cadet && l.found.is_none()),
                        "seed {seed}: {cadet:?} left by the road and was counted as a casualty"
                    );
                }
                break;
            }
        }
        assert!(
            exited > 0,
            "no crew took an exit in three seeds; this test proved nothing"
        );
    }
}
