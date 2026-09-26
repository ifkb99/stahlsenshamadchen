//! Dev-only harness for driving the game without a human at the keyboard.
//!
//! The simulation half of this project is verifiable — `tactics_core` is
//! headless, deterministic, and covered by a real suite. The presentation half
//! was not: the only way to see whether a highlight lands on the right hex or
//! a panel reads correctly was for someone to run the game and look at it.
//! That makes every rendering and UI change unreviewable by anyone who is not
//! sitting in front of it, which is most of the interesting work left on the
//! roadmap (prep-phase placement, menus, sprites, the whole stylize pass).
//!
//! So: `STAHL_SCRIPT=<file>` replays a list of timed actions, and `shot`
//! captures the window at any point along the way. Combined with the existing
//! `STAHL_BATTLE=<map_id>` boot shortcut, an arbitrary screen becomes a single
//! reproducible command.
//!
//! ```sh
//! STAHL_DEBUG=1 STAHL_BATTLE=river_crossing \
//!   STAHL_SCRIPT=scripts/dev/select-and-move.txt \
//!   cargo run -p stahlsenshamädchen
//! ```
//!
//! # Why input is injected rather than synthesized
//!
//! Key and button presses are written straight into `ButtonInput`, which is
//! the same resource the real handlers read, so every `just_pressed` branch in
//! `battle::handle_input` runs exactly as it does for a human. What is *not*
//! done that way is the cursor. Picking reads `Window::cursor_position`, and
//! writing to that field makes `bevy_winit` warp the operating system pointer
//! (`bevy_winit/src/system.rs`) — which would fight the user for control of
//! their own mouse and silently fail whenever the window is unfocused or the
//! session is Wayland. Instead the scripted cursor is a resource that
//! [`crate::map_render::View`] consults ahead of the window, so a script
//! names the hex it means and the real pointer is left alone.
//!
//! Addressing a hex rather than a pixel also makes scripts independent of
//! window size, zoom, camera position and view rotation — a script written
//! today still selects the same unit after the camera default changes. `pixel`
//! exists for the cases where that is genuinely what you mean, such as
//! clicking UI chrome.
//!
//! # Script format
//!
//! One action per line; `#` starts a comment. At most one action runs per
//! frame, in order, which is what keeps a press and its release in separate
//! frames the way real input arrives.
//!
//! | Action | Meaning |
//! | --- | --- |
//! | `at <secs>` | block until this much app time has elapsed |
//! | `wait <secs>` | block for this long, relative to now |
//! | `hex <q>,<r>` | put the scripted cursor over an axial hex |
//! | `focus <q>,<r>` | centre the camera on an axial hex |
//! | `pixel <x>,<y>` | put the scripted cursor at a window position |
//! | `cursor off` | hand the cursor back to the real mouse |
//! | `key <name>` | tap a key for one frame (`Enter`, `V`, `KeyV`, `Digit1`, …) |
//! | `hold <name>` / `release <name>` | for held keys, e.g. panning with `D` |
//! | `click <left\|right\|middle>` | tap a mouse button for one frame |
//! | `shot <path>` | capture the window; the script waits for it to land |
//! | `log <text>` | print a marker, to correlate stdout with screenshots |
//! | `until <predicate> [<secs>]` | block until the game says so, or give up |
//! | `press <key> until <predicate> [<secs>]` | tap the key each time the game is idle, until the predicate holds or the deadline passes |
//!
//! Predicates: `idle`, `waiting`, `over`, `turn <cmp> <n>`,
//! `score <side> <cmp> <n>`, `unit "<name>" alive|dead|aboard|afoot`,
//! `log "<text>"`, `selected "<name>"`, `danger <cmp> <n>`.
//! | `expect <predicate>` | assert now; a failure makes the run exit nonzero |
//! | `quit` | exit once every pending screenshot has been written |
//!
//! A script that ends without `quit` leaves the game running normally, which
//! is useful for setting up a state by hand and then taking over.
//!
//! # Waiting on the game rather than on the clock
//!
//! `wait` blocks on a stopwatch, and for a long time that was the only way to
//! say "let the round finish" — every tour in `scripts/dev/` guessed a number
//! of seconds and hoped. A guess that is too short photographs a half-played
//! animation and the screenshot lies about what the state is; a guess that is
//! too long makes every tour slow to keep the margin. Neither failure is
//! visible in the output, which is the worst property a test harness can have.
//!
//! `until` blocks on a [`Predicate`] over [`ScriptFacts`] instead — what the
//! game actually says about itself — so `until idle` means the round has
//! finished and nothing is animating, however long that took on this machine.
//! `expect` asks the same questions as an assertion, so a tour can state what
//! it believes and fail loudly when that stops being true.
//!
//! The predicate vocabulary is deliberately small and closed, for the reason
//! the whole format is: a script that could compute would be a program nobody
//! reviews, and a typo in a closed vocabulary is a parse warning rather than
//! silent nonsense.
//!
//! | Predicate | True when |
//! | --- | --- |
//! | `idle` | not resolving, not animating: the frame is settled |
//! | `waiting` | the screen is holding for an answer: a prompt or a report |
//! | `over` | the battle has been decided |
//! | `turn >= <n>` | the round (or campaign day) has reached `n` (`>= > == <= <`) |
//! | `score <side> >= <n>` | that side's objective points |
//! | `unit "<name>" alive\|dead\|aboard\|afoot` | what became of her |
//! | `log "<text>"` | that text has appeared in the on-screen log |
//! | `danger >= <n>` | the danger overlay is up and tinting `n` tiles |
//!
//! Facts are published by whichever screen is on ([`ScriptFacts`]), so a
//! script that asks a battle question on the campaign map simply never comes
//! true and times out saying so.

use crate::map_render::ScriptedCursor;
use bevy::app::AppExit;
use bevy::input::{ButtonInput, InputSystems};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
use std::collections::HashSet;
use tactics_core::Hex;

pub struct DevToolsPlugin;

impl Plugin for DevToolsPlugin {
    fn build(&self, app: &mut App) {
        // Everything here is opt-in through the environment, so a normal run
        // pays for nothing but a resource that is never inserted.
        if !debug_enabled() {
            return;
        }
        app.init_resource::<ScriptedCursor>()
            .init_resource::<PendingShots>()
            .add_systems(Update, dev_screenshot);

        if let Ok(path) = std::env::var("STAHL_SCRIPT") {
            match Script::load(&path) {
                Ok(script) => {
                    info!("dev script: {} action(s) from {path}", script.actions.len());
                    app.insert_resource(script);
                    // Ordering here is the whole trick. Bevy's own input
                    // systems clear `just_pressed` at the top of every
                    // `PreUpdate`, so a press injected before them is wiped
                    // before any handler sees it — which is exactly what
                    // happened the first time this ran, and the screenshots
                    // showed a click that selected nothing. Running after
                    // `InputSystems` puts the press in the same window a real
                    // one occupies: visible for the rest of the frame, gone by
                    // the next.
                    app.add_systems(PreUpdate, run_script.after(InputSystems));
                }
                Err(err) => error!("dev script {path}: {err}"),
            }
        }
    }
}

/// Dev tooling is gated behind `STAHL_DEBUG` so that a release build handed to
/// a player has no scripted input path at all.
///
/// Crate-visible because the screens gate their fact publishers on it too: a
/// player's build should not be walking every unit once a frame to fill in a
/// resource nobody reads.
pub(crate) fn debug_enabled() -> bool {
    std::env::var("STAHL_DEBUG").is_ok()
}

/// What the screen currently on knows about itself, in the small vocabulary a
/// script is allowed to ask about.
///
/// **Published by the screens, read by the runner** — deliberately that way
/// round. `run_script` is screen-agnostic and must stay so, the battle screen's
/// own `Battle` resource is private to its module, and the campaign map will
/// want to answer the same questions in its own terms. One flat resource that
/// each screen fills in leaves the dependency pointing from the game at the
/// dev tooling, which is the direction that costs nothing when the dev tooling
/// is compiled out of a player's build.
///
/// Everything here is a *fact about now*. Nothing accumulates, because a
/// resource that remembered would have to be reset on every screen change and
/// would quietly answer for the last battle. The one thing a script wants
/// history for — has this line ever appeared in the log — is accumulated by
/// the runner instead, from the rolling window below.
///
/// **Every publisher assigns this whole struct, and the literal is exhaustive
/// — no `..default()`.** One resource shared by every screen means a field a
/// publisher leaves alone is still holding the *previous* screen's answer,
/// which is a fact about a screen the player has left; the campaign map used
/// to do exactly that with [`Self::selected`], so a tour could have waited on
/// a selection made in a battle two screens ago. Adding a field here should
/// therefore break the build in every publisher until each screen has said
/// what it answers, the same way a new [`tactics_core::battle::Mission`]
/// without a promise breaks `Mission::slot`.
#[derive(Resource, Default)]
pub(crate) struct ScriptFacts {
    /// Rounds on the battle screen, days on the campaign map. One name
    /// because a script asking "how far in are we" means the same thing on
    /// both and should not need two words for it.
    pub turn: u32,
    /// Nothing resolving and nothing animating: what the screen is drawing is
    /// what the state says, and a screenshot taken now will not lie.
    pub idle: bool,
    /// The battle has been decided. Always false where the question has no
    /// meaning.
    pub over: bool,
    /// The screen is holding the player behind something she has to answer or
    /// dismiss — a muster prompt, an after-action report. The complement of
    /// [`Self::idle`] rather than a second name for its negation: a screen can
    /// be neither (mid-animation) but never both.
    pub waiting: bool,
    /// Objective points by side.
    pub score: Vec<u32>,
    pub units: Vec<UnitFact>,
    /// The on-screen log as it stands. A rolling window of the last few
    /// lines, not a transcript — see [`Script::seen_log`].
    pub log: Vec<String>,
    /// Who the player has selected, by name, if anyone.
    ///
    /// Worth a fact of its own because selection is the one piece of UI state
    /// a script *drives* and could not previously *check*: a click that
    /// selected nothing, or selected the wrong crew of two sharing a hex,
    /// looked exactly like a click that worked until several actions later
    /// when the keystroke it was setting up did nothing. That is how the
    /// stacked-hex selection bug survived — see the click handler in
    /// `battle::handle_input`.
    pub selected: Option<String>,
    /// How many tiles of the selected crew's reach the danger overlay is
    /// currently painting as under fire, or `None` where the overlay is not
    /// up at all.
    ///
    /// Two facts in one field on purpose, because a tour needs both and they
    /// are useless apart. `Some(0)` and `None` are different states — the
    /// overlay is on and the ground is clear, versus nobody asked — and a
    /// bare boolean would let a tour pass while the overlay painted every
    /// tile the same colour, which is the exact failure a screenshot is
    /// least likely to catch.
    pub danger: Option<u32>,
}

/// One unit, as a script may ask about her.
pub(crate) struct UnitFact {
    pub name: String,
    /// False for a crew that was destroyed *or* that drove off by an exit.
    /// The distinction matters to the game and not to a script, which only
    /// ever asks "is she still out there".
    pub alive: bool,
    /// Riding in something. The state a `Mounted` event leaves behind, and
    /// the reason a script can wait for a mount without an event log.
    pub aboard: bool,
}

/// Screenshots requested but not yet written to disk. `quit` waits on this so
/// a script never races the GPU readback and truncates its own output.
#[derive(Resource, Default)]
struct PendingShots(usize);

/// One step of a script. Actions that block (`at`, `wait`, `shot`-then-`quit`)
/// report not-done and are retried on the next frame.
#[derive(Debug, Clone)]
enum Action {
    At(f32),
    Wait(f32),
    Cursor(Option<ScriptedCursor>),
    Key {
        code: KeyCode,
        hold: Hold,
    },
    Click {
        button: MouseButton,
    },
    Shot(String),
    Log(String),
    /// Put the camera on a hex, in one step. Panning by holding an arrow key
    /// also works and is what a player does, but it is a *rate* — how far it
    /// travels depends on the zoom level and on how many seconds of app time
    /// the script happened to spend holding it, which made framing drift
    /// between runs of the same script on the same machine. Naming the hex
    /// makes it exact, and matches the rest of the format: scripts address
    /// the world, never the screen.
    Focus(Hex),
    /// Block until the game says so. The deadline is not optional in spirit —
    /// a predicate that never comes true would hang the run forever with no
    /// output, which is exactly the failure `until` exists to replace — so an
    /// omitted one gets [`DEFAULT_TIMEOUT`].
    Until {
        predicate: Predicate,
        secs: f32,
    },
    /// Tap a key every time the game will take one, until the predicate
    /// holds. The loop the format never had: "press Enter until the battle
    /// is over" used to be twenty `key Enter` / `until idle` pairs, which is
    /// a guess about how many rounds a battle takes, and a tour built on
    /// that guess broke the day a content change moved the fight to a
    /// different map. The deadline is the whole loop's, not one tap's.
    PressUntil {
        code: KeyCode,
        predicate: Predicate,
        secs: f32,
    },
    Expect(Predicate),
    Quit,
}

/// How long an `until` waits before giving up and saying so. Generous,
/// because the thing it usually waits for is a round of twelve ticks
/// animating on a machine that may be building at the same time; finite,
/// because a script that hangs teaches nobody anything.
const DEFAULT_TIMEOUT: f32 = 30.0;

/// A question a script may ask about the running game.
#[derive(Debug, Clone, PartialEq)]
enum Predicate {
    Idle,
    Waiting,
    Over,
    Turn {
        op: Cmp,
        n: u32,
    },
    Score {
        side: usize,
        op: Cmp,
        n: u32,
    },
    Unit {
        name: String,
        is: UnitIs,
    },
    Log(String),
    Selected(String),
    /// How many reachable tiles the danger overlay is painting as under
    /// fire. False whenever the overlay is not up, so `until danger >= 1`
    /// waits for the overlay *and* for it to have found something rather
    /// than coming true on an empty answer.
    Danger {
        op: Cmp,
        n: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cmp {
    Ge,
    Gt,
    Eq,
    Le,
    Lt,
}

impl Cmp {
    fn holds(self, left: u32, right: u32) -> bool {
        match self {
            Cmp::Ge => left >= right,
            Cmp::Gt => left > right,
            Cmp::Eq => left == right,
            Cmp::Le => left <= right,
            Cmp::Lt => left < right,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnitIs {
    Alive,
    Dead,
    Aboard,
    /// On the ground under her own power: alive and riding nothing. The
    /// answer to "has she got off yet", which is not the same as `alive`.
    Afoot,
}

impl Predicate {
    /// Whether this holds, given what the screen last published and what the
    /// runner has seen go by in the log.
    ///
    /// A unit named by a script that no screen has published is *not* a
    /// failure of the predicate, it is a script that has not caught up yet:
    /// `alive` is false and `dead` is true for somebody who does not exist,
    /// which is the reading that makes `until unit "X" dead` terminate when
    /// she is removed and `expect unit "X" alive` fail loudly on a typo.
    fn holds(&self, facts: &ScriptFacts, seen_log: &HashSet<String>) -> bool {
        match self {
            Predicate::Idle => facts.idle,
            Predicate::Waiting => facts.waiting,
            Predicate::Over => facts.over,
            Predicate::Turn { op, n } => op.holds(facts.turn, *n),
            Predicate::Score { side, op, n } => {
                op.holds(facts.score.get(*side).copied().unwrap_or(0), *n)
            }
            Predicate::Unit { name, is } => {
                let unit = facts.units.iter().find(|u| u.name == *name);
                match is {
                    UnitIs::Alive => unit.is_some_and(|u| u.alive),
                    UnitIs::Dead => !unit.is_some_and(|u| u.alive),
                    UnitIs::Aboard => unit.is_some_and(|u| u.alive && u.aboard),
                    UnitIs::Afoot => unit.is_some_and(|u| u.alive && !u.aboard),
                }
            }
            Predicate::Log(text) => seen_log.iter().any(|line| line.contains(text.as_str())),
            Predicate::Selected(name) => facts.selected.as_deref() == Some(name.as_str()),
            // `None` is the overlay being off, and a question about a
            // thing that is not on screen is false rather than zero.
            Predicate::Danger { op, n } => facts.danger.is_some_and(|d| op.holds(d, *n)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    /// Press this frame, release the next: an ordinary keystroke.
    Tap,
    Press,
    Release,
}

#[derive(Resource)]
struct Script {
    actions: Vec<Action>,
    /// Index of the next action to run.
    cursor: usize,
    /// When the currently blocking `wait` started, in elapsed seconds.
    waiting_since: Option<f32>,
    /// When a `press … until` last tapped its key, so it does not tap again
    /// on the frame after, before the game has had a chance to stop being
    /// idle — a keystroke is read the frame after it is pressed.
    pressed_at: Option<f32>,
    /// Keys and buttons tapped last frame, released at the start of this one.
    tapped_keys: Vec<KeyCode>,
    tapped_buttons: Vec<MouseButton>,
    /// Every distinct log line this run has seen go past.
    ///
    /// [`ScriptFacts::log`] is the screen's rolling window of the last few
    /// lines, so a script that asked it directly could only ever match what
    /// happened in the last moment. Accumulating here gives `log "..."` the
    /// meaning a script wants — *has this ever happened* — without making the
    /// game keep a transcript it has no use for. A burst long enough to push
    /// a line through the window inside one frame would be missed; the log is
    /// paced by the animation timer, so in practice they arrive a few a
    /// second.
    seen_log: HashSet<String>,
    /// How many `expect`s have failed. `quit` reads it and exits nonzero,
    /// which is what makes a tour something CI can run rather than something
    /// a person has to look at.
    failures: usize,
}

impl Script {
    fn load(path: &str) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let mut actions = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            match parse_action(line) {
                Some(action) => actions.push(action),
                // A typo in a script should be loud but not fatal: the rest of
                // the run is still worth seeing.
                None => warn!("dev script {path}:{}: cannot parse {line:?}", n + 1),
            }
        }
        Ok(Self {
            actions,
            cursor: 0,
            waiting_since: None,
            pressed_at: None,
            tapped_keys: Vec::new(),
            tapped_buttons: Vec::new(),
            seen_log: HashSet::new(),
            failures: 0,
        })
    }
}

fn parse_action(line: &str) -> Option<Action> {
    let mut parts = line.splitn(2, char::is_whitespace);
    let verb = parts.next()?;
    let rest = parts.next().unwrap_or("").trim();
    match verb {
        "at" => rest.parse().ok().map(Action::At),
        "wait" => rest.parse().ok().map(Action::Wait),
        "hex" => parse_pair(rest)
            .map(|(q, r)| Action::Cursor(Some(ScriptedCursor::Hex(Hex::new(q as i32, r as i32))))),
        "pixel" => parse_pair(rest)
            .map(|(x, y)| Action::Cursor(Some(ScriptedCursor::Pixel(Vec2::new(x, y))))),
        "cursor" if rest == "off" => Some(Action::Cursor(None)),
        "key" => parse_key(rest).map(|code| Action::Key {
            code,
            hold: Hold::Tap,
        }),
        "hold" => parse_key(rest).map(|code| Action::Key {
            code,
            hold: Hold::Press,
        }),
        "release" => parse_key(rest).map(|code| Action::Key {
            code,
            hold: Hold::Release,
        }),
        "click" => parse_button(rest).map(|button| Action::Click { button }),
        "shot" if !rest.is_empty() => Some(Action::Shot(rest.to_string())),
        "log" => Some(Action::Log(rest.to_string())),
        "focus" => parse_pair(rest).map(|(q, r)| Action::Focus(Hex::new(q as i32, r as i32))),
        // The timeout rides on the end of the same line rather than taking a
        // keyword, because it is the only number an `until` can carry and a
        // keyword would be ceremony. `until idle` and `until idle 90` both
        // read as English.
        "until" => {
            let (body, secs) = match rest.rsplit_once(char::is_whitespace) {
                Some((head, tail)) => match tail.parse::<f32>() {
                    Ok(secs) => (head.trim(), secs),
                    Err(_) => (rest, DEFAULT_TIMEOUT),
                },
                None => (rest, DEFAULT_TIMEOUT),
            };
            parse_predicate(body).map(|predicate| Action::Until { predicate, secs })
        }
        "expect" => parse_predicate(rest).map(Action::Expect),
        // `press Enter until waiting 300`: the key, the word, then exactly
        // what an `until` line would say.
        "press" => {
            let (key, tail) = rest.split_once(char::is_whitespace)?;
            let body = tail.trim().strip_prefix("until")?.trim();
            let code = parse_key(key)?;
            let (body, secs) = match body.rsplit_once(char::is_whitespace) {
                Some((head, tail)) => match tail.parse::<f32>() {
                    Ok(secs) => (head.trim(), secs),
                    Err(_) => (body, DEFAULT_TIMEOUT),
                },
                None => (body, DEFAULT_TIMEOUT),
            };
            parse_predicate(body).map(|predicate| Action::PressUntil {
                code,
                predicate,
                secs,
            })
        }
        "quit" => Some(Action::Quit),
        _ => None,
    }
}

/// `idle`, `turn >= 3`, `unit "Grenadier 2" aboard`, `log "brews up"`.
///
/// Quoted names are taken whole, so a unit called `Anvil 1` needs no
/// escaping; everything else is whitespace-separated words. Anything that
/// does not parse returns `None` and the loader warns with the line, which is
/// the same forgiveness the rest of the format has.
fn parse_predicate(text: &str) -> Option<Predicate> {
    let text = text.trim();
    let (head, rest) = match text.split_once(char::is_whitespace) {
        Some((head, rest)) => (head, rest.trim()),
        None => (text, ""),
    };
    match head {
        "idle" if rest.is_empty() => Some(Predicate::Idle),
        "waiting" if rest.is_empty() => Some(Predicate::Waiting),
        "over" if rest.is_empty() => Some(Predicate::Over),
        "turn" => {
            let (op, n) = parse_comparison(rest)?;
            Some(Predicate::Turn { op, n })
        }
        "score" => {
            let (side, rest) = rest.split_once(char::is_whitespace)?;
            let (op, n) = parse_comparison(rest)?;
            Some(Predicate::Score {
                side: side.trim().parse().ok()?,
                op,
                n,
            })
        }
        "unit" => {
            let (name, state) = parse_quoted(rest)?;
            Some(Predicate::Unit {
                name,
                is: match state.trim() {
                    "alive" => UnitIs::Alive,
                    "dead" => UnitIs::Dead,
                    "aboard" => UnitIs::Aboard,
                    "afoot" => UnitIs::Afoot,
                    _ => return None,
                },
            })
        }
        "log" => parse_quoted(rest).map(|(text, _)| Predicate::Log(text)),
        "selected" => parse_quoted(rest).map(|(name, _)| Predicate::Selected(name)),
        "danger" => {
            let (op, n) = parse_comparison(rest)?;
            Some(Predicate::Danger { op, n })
        }
        _ => None,
    }
}

/// `>= 3`, `== 0`, `< 12`.
fn parse_comparison(text: &str) -> Option<(Cmp, u32)> {
    let (op, n) = text.trim().split_once(char::is_whitespace)?;
    let op = match op.trim() {
        ">=" => Cmp::Ge,
        ">" => Cmp::Gt,
        "==" | "=" => Cmp::Eq,
        "<=" => Cmp::Le,
        "<" => Cmp::Lt,
        _ => return None,
    };
    Some((op, n.trim().parse().ok()?))
}

/// A `"quoted string"` and whatever follows it.
fn parse_quoted(text: &str) -> Option<(String, &str)> {
    let rest = text.trim().strip_prefix('"')?;
    let (inside, after) = rest.split_once('"')?;
    Some((inside.to_string(), after))
}

/// `3,-1` or `3 -1`; used for both hex coordinates and pixel positions.
fn parse_pair(text: &str) -> Option<(f32, f32)> {
    let (a, b) = text
        .split_once(',')
        .or_else(|| text.split_once(char::is_whitespace))?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// Accepts both the Bevy spelling (`KeyV`, `Digit1`, `ArrowUp`) and the bare
/// one a person would actually type in a script (`V`, `1`, `Up`).
fn parse_key(name: &str) -> Option<KeyCode> {
    let name = name.trim();
    if let Some(letter) = name.strip_prefix("Key").unwrap_or(name).chars().next()
        && name.trim_start_matches("Key").len() == 1
        && letter.is_ascii_alphabetic()
    {
        return Some(letter_key(letter.to_ascii_uppercase()));
    }
    let digits = name.strip_prefix("Digit").unwrap_or(name);
    if digits.len() == 1
        && let Some(d) = digits.chars().next().filter(char::is_ascii_digit)
    {
        return Some(digit_key(d));
    }
    if let Some(n) = name.strip_prefix('F').and_then(|n| n.parse::<u8>().ok())
        && (1..=12).contains(&n)
    {
        return Some(function_key(n));
    }
    match name {
        "Enter" | "Return" => Some(KeyCode::Enter),
        "Escape" | "Esc" => Some(KeyCode::Escape),
        "Space" => Some(KeyCode::Space),
        "Tab" => Some(KeyCode::Tab),
        "ArrowUp" | "Up" => Some(KeyCode::ArrowUp),
        "ArrowDown" | "Down" => Some(KeyCode::ArrowDown),
        "ArrowLeft" | "Left" => Some(KeyCode::ArrowLeft),
        "ArrowRight" | "Right" => Some(KeyCode::ArrowRight),
        // The world clock's speed keys (WORLD.md W5.0).
        "BracketLeft" | "[" => Some(KeyCode::BracketLeft),
        "BracketRight" | "]" => Some(KeyCode::BracketRight),
        _ => None,
    }
}

fn letter_key(c: char) -> KeyCode {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO,
        KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV, KeyW, KeyX, KeyY, KeyZ,
    ];
    LETTERS[(c as u8 - b'A') as usize]
}

fn function_key(n: u8) -> KeyCode {
    use KeyCode::*;
    const FN: [KeyCode; 12] = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    FN[(n - 1) as usize]
}

fn digit_key(c: char) -> KeyCode {
    use KeyCode::*;
    const DIGITS: [KeyCode; 10] = [
        Digit0, Digit1, Digit2, Digit3, Digit4, Digit5, Digit6, Digit7, Digit8, Digit9,
    ];
    DIGITS[(c as u8 - b'0') as usize]
}

fn parse_button(name: &str) -> Option<MouseButton> {
    match name.trim() {
        "left" | "lmb" => Some(MouseButton::Left),
        "right" | "rmb" => Some(MouseButton::Right),
        "middle" | "mmb" => Some(MouseButton::Middle),
        _ => None,
    }
}

/// Advance the script by at most one action per frame.
#[allow(clippy::too_many_arguments)]
fn run_script(
    mut commands: Commands,
    mut script: ResMut<Script>,
    mut cursor: ResMut<ScriptedCursor>,
    mut pending: ResMut<PendingShots>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut buttons: ResMut<ButtonInput<MouseButton>>,
    mut exit: MessageWriter<AppExit>,
    mut focus: ResMut<crate::camera::CameraFocus>,
    rotation: Res<crate::iso::ViewRotation>,
    center: Res<crate::iso::ViewCenter>,
    facts: Option<Res<ScriptFacts>>,
    time: Res<Time>,
) {
    // Release last frame's taps first, so a keystroke occupies exactly one
    // frame the way a real one does.
    for code in script.tapped_keys.drain(..) {
        keys.release(code);
    }
    for button in script.tapped_buttons.drain(..) {
        buttons.release(button);
    }

    // Sweep up whatever the log has said since the last frame, whether or not
    // a predicate is waiting on it — a script that only started watching when
    // it reached the `until` would miss the line it was waiting for.
    let facts = facts.map(|f| f.into_inner());
    if let Some(facts) = &facts {
        for line in &facts.log {
            if !script.seen_log.contains(line) {
                script.seen_log.insert(line.clone());
            }
        }
    }

    let now = time.elapsed_secs();
    let Some(action) = script.actions.get(script.cursor).cloned() else {
        return;
    };

    // Actions that block return early without advancing the cursor.
    match action {
        Action::At(t) => {
            if now < t {
                return;
            }
        }
        Action::Wait(secs) => {
            let started = *script.waiting_since.get_or_insert(now);
            if now - started < secs {
                return;
            }
            script.waiting_since = None;
        }
        Action::Until { predicate, secs } => {
            let started = *script.waiting_since.get_or_insert(now);
            let met = facts
                .as_ref()
                .is_some_and(|f| predicate.holds(f, &script.seen_log));
            if met {
                script.waiting_since = None;
            } else if now - started < secs {
                return;
            } else {
                // Loud, and counted as a failure: a timed-out `until` means
                // the script asked for something that never happened, and
                // every action after it is running against a game in a state
                // it did not expect. Carrying on anyway is deliberate — the
                // screenshots after the failure are usually what explains it.
                script.failures += 1;
                match facts.is_some() {
                    true => warn!("dev script: SCRIPT FAIL: gave up waiting for {predicate:?}"),
                    false => warn!(
                        "dev script: SCRIPT FAIL: gave up waiting for {predicate:?} — no screen \
                         is publishing facts, so nothing could ever have answered it"
                    ),
                }
                script.waiting_since = None;
            }
        }
        Action::PressUntil {
            code,
            predicate,
            secs,
        } => {
            let started = *script.waiting_since.get_or_insert(now);
            let met = facts
                .as_ref()
                .is_some_and(|f| predicate.holds(f, &script.seen_log));
            if met {
                script.waiting_since = None;
                script.pressed_at = None;
            } else if now - started < secs {
                // A tap only when the game would take one — listening, or
                // held behind a page a keystroke dismisses — and never on
                // the frame right after the last: the press is read next
                // frame, so `idle` is still true for one frame after a tap
                // that is about to start a round.
                let idle = facts.as_ref().is_some_and(|f| f.idle || f.waiting);
                let settled = script.pressed_at.is_none_or(|t| now - t > 0.2);
                if idle && settled {
                    keys.press(code);
                    script.tapped_keys.push(code);
                    script.pressed_at = Some(now);
                }
                return;
            } else {
                script.failures += 1;
                warn!("dev script: SCRIPT FAIL: gave up pressing {code:?} for {predicate:?}");
                script.waiting_since = None;
                script.pressed_at = None;
            }
        }
        Action::Expect(predicate) => {
            let met = facts
                .as_ref()
                .is_some_and(|f| predicate.holds(f, &script.seen_log));
            if met {
                info!("dev script: ok, {predicate:?}");
            } else {
                script.failures += 1;
                warn!("dev script: SCRIPT FAIL: expected {predicate:?}");
            }
        }
        Action::Quit => {
            // Hold the door until every screenshot observer has fired.
            if pending.0 > 0 {
                return;
            }
            // A tour is a test now, so it has to be able to fail the way a
            // test does. `AppExit::Error` is what makes `cargo run` return
            // nonzero and a CI step go red; without it a script that watched
            // every assertion fail would still exit 0 and be believed.
            match script.failures {
                0 => {
                    info!("dev script: finished, exiting");
                    exit.write(AppExit::Success);
                }
                failures => {
                    error!("dev script: finished with {failures} failure(s)");
                    exit.write(AppExit::error());
                }
            }
        }
        Action::Cursor(target) => *cursor = target.unwrap_or(ScriptedCursor::None),
        Action::Key { code, hold } => match hold {
            Hold::Tap => {
                keys.press(code);
                script.tapped_keys.push(code);
            }
            Hold::Press => keys.press(code),
            Hold::Release => keys.release(code),
        },
        Action::Click { button } => {
            buttons.press(button);
            script.tapped_buttons.push(button);
        }
        Action::Shot(path) => {
            pending.0 += 1;
            let named = path.clone();
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path.clone()))
                .observe(
                    move |captured: On<ScreenshotCaptured>,
                          mut pending: ResMut<PendingShots>,
                          mut script: ResMut<Script>| {
                        pending.0 = pending.0.saturating_sub(1);
                        // A capture taken before the window has a swapchain
                        // comes back as a single pixel and is written to disk
                        // as a valid, useless PNG. Nothing about that is
                        // visible in a script's output — the file exists, the
                        // run exits 0 — and it is the same class of lie as a
                        // stopwatch that guessed too short. Counting it as a
                        // failure is the whole fix; the run carries on,
                        // because the later shots are usually fine and worth
                        // having.
                        let size = captured.image.texture_descriptor.size;
                        if size.width <= 1 || size.height <= 1 {
                            script.failures += 1;
                            warn!(
                                "dev script: SCRIPT FAIL: {named} captured at \
                                 {}x{} — the window had no surface to \
                                 photograph",
                                size.width, size.height
                            );
                        }
                    },
                );
        }
        Action::Log(text) => info!("dev script: {text}"),
        // Elevation zero, exactly as `overworld::center_camera` does it: the
        // camera looks at a column of world, and which tier of that column
        // the tile happens to sit on is not worth a map lookup here.
        Action::Focus(hex) => {
            let (pos, _) = crate::iso::project(hex, 0, rotation.0, center.0);
            focus.0 = pos;
        }
    }

    script.cursor += 1;
}

/// Dev tool: `STAHL_SCREENSHOT=out.png` captures the window a few seconds in
/// (delay adjustable with `STAHL_SCREENSHOT_AT=<seconds>`). Kept separate from
/// the script runner because the one-shot case is worth having without writing
/// a file first.
fn dev_screenshot(mut commands: Commands, time: Res<Time>, mut done: Local<bool>) {
    let at = std::env::var("STAHL_SCREENSHOT_AT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(4.0);
    if *done || time.elapsed_secs() < at {
        return;
    }
    *done = true;
    if let Ok(path) = std::env::var("STAHL_SCREENSHOT") {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_line_parses_into_the_action_it_names() {
        assert!(matches!(parse_action("quit"), Some(Action::Quit)));
        assert!(matches!(parse_action("wait 0.5"), Some(Action::Wait(t)) if t == 0.5));
        assert!(matches!(
            parse_action("click left"),
            Some(Action::Click {
                button: MouseButton::Left
            })
        ));
        assert!(matches!(
            parse_action("hex 3,-1"),
            Some(Action::Cursor(Some(ScriptedCursor::Hex(h)))) if h == Hex::new(3, -1)
        ));
        assert!(matches!(
            parse_action("focus -7,20"),
            Some(Action::Focus(h)) if h == Hex::new(-7, 20)
        ));
        assert!(matches!(
            parse_action("cursor off"),
            Some(Action::Cursor(None))
        ));
        // Comments and unknown verbs are the two ways a line can be dropped.
        assert!(parse_action("frobnicate 3").is_none());
    }

    fn facts() -> ScriptFacts {
        ScriptFacts {
            turn: 3,
            idle: true,
            waiting: false,
            over: false,
            score: vec![7, 0],
            units: vec![
                UnitFact {
                    name: "Grenadier 1".into(),
                    alive: true,
                    aboard: false,
                },
                UnitFact {
                    name: "Grenadier 2".into(),
                    alive: true,
                    aboard: true,
                },
                UnitFact {
                    name: "Anvil 3".into(),
                    alive: false,
                    aboard: false,
                },
            ],
            log: Vec::new(),
            selected: Some("Grenadier 1".into()),
            danger: Some(4),
        }
    }

    #[test]
    fn a_predicate_parses_into_the_question_it_asks() {
        assert_eq!(parse_predicate("idle"), Some(Predicate::Idle));
        assert_eq!(parse_predicate("waiting"), Some(Predicate::Waiting));
        assert_eq!(
            parse_predicate("turn >= 4"),
            Some(Predicate::Turn { op: Cmp::Ge, n: 4 })
        );
        assert_eq!(
            parse_predicate("score 1 == 0"),
            Some(Predicate::Score {
                side: 1,
                op: Cmp::Eq,
                n: 0
            })
        );
        // The quotes are what let a call sign contain a space, which every
        // one of them does.
        assert_eq!(
            parse_predicate("unit \"Grenadier 2\" aboard"),
            Some(Predicate::Unit {
                name: "Grenadier 2".into(),
                is: UnitIs::Aboard
            })
        );
        assert_eq!(
            parse_predicate("log \"brews up\""),
            Some(Predicate::Log("brews up".into()))
        );
        assert_eq!(
            parse_predicate("selected \"Grenadier 2\""),
            Some(Predicate::Selected("Grenadier 2".into()))
        );
        assert_eq!(
            parse_predicate("danger >= 1"),
            Some(Predicate::Danger { op: Cmp::Ge, n: 1 })
        );
        // ...and it is false, not zero, when the overlay is not up at all,
        // so `until danger >= 1` waits for the overlay rather than coming
        // true on a screen that never drew one.
        let mut off = facts();
        off.danger = None;
        assert!(!Predicate::Danger { op: Cmp::Ge, n: 0 }.holds(&off, &HashSet::new()));
        assert!(Predicate::Danger { op: Cmp::Ge, n: 4 }.holds(&facts(), &HashSet::new()));
        // A closed vocabulary: anything else is a warning at load, not a
        // silently-false question at run time.
        assert!(parse_predicate("vibes good").is_none());
        assert!(parse_predicate("turn ~ 4").is_none());
        assert!(parse_predicate("unit Grenadier 2 aboard").is_none());
        assert!(parse_predicate("idle please").is_none());
    }

    #[test]
    fn press_until_reads_the_key_the_predicate_and_the_deadline() {
        assert!(matches!(
            parse_action("press Enter until log \"Battle \" 400"),
            Some(Action::PressUntil {
                code: KeyCode::Enter,
                predicate: Predicate::Log(ref text),
                secs,
            }) if text == "Battle " && secs == 400.0
        ));
        assert!(matches!(
            parse_action("press Enter until waiting"),
            Some(Action::PressUntil {
                predicate: Predicate::Waiting,
                secs,
                ..
            }) if secs == DEFAULT_TIMEOUT
        ));
        assert!(
            parse_action("press Enter waiting").is_none(),
            "the word is not optional"
        );
        assert!(
            parse_action("press until waiting").is_none(),
            "nor is the key"
        );
    }

    #[test]
    fn until_takes_its_deadline_off_the_end_of_the_line() {
        assert!(matches!(
            parse_action("until idle"),
            Some(Action::Until { secs, .. }) if secs == DEFAULT_TIMEOUT
        ));
        assert!(matches!(
            parse_action("until idle 90"),
            Some(Action::Until { secs, .. }) if secs == 90.0
        ));
        // A trailing word that is not a number belongs to the predicate, so
        // the state a unit is in is never mistaken for a timeout.
        assert!(matches!(
            parse_action("until unit \"Grenadier 2\" afoot"),
            Some(Action::Until { predicate: Predicate::Unit { is: UnitIs::Afoot, .. }, secs })
                if secs == DEFAULT_TIMEOUT
        ));
        assert!(matches!(
            parse_action("expect over"),
            Some(Action::Expect(Predicate::Over))
        ));
    }

    #[test]
    fn a_predicate_answers_from_what_the_screen_published() {
        let facts = facts();
        let seen: HashSet<String> = ["Grenadier 2 mounts up in Grenadier 1.".to_string()]
            .into_iter()
            .collect();
        let holds = |text: &str| {
            parse_predicate(text)
                .unwrap_or_else(|| panic!("{text}"))
                .holds(&facts, &seen)
        };

        assert!(holds("idle"));
        assert!(!holds("over"));
        assert!(holds("turn >= 3") && holds("turn == 3") && !holds("turn > 3"));
        assert!(holds("score 0 >= 7") && holds("score 1 == 0"));
        // A side nobody published scores nothing rather than panicking.
        assert!(holds("score 9 == 0"));

        // Selection: the fact a script drives and could not previously check.
        assert!(holds("selected \"Grenadier 1\""));
        assert!(!holds("selected \"Grenadier 2\""));

        assert!(holds("unit \"Grenadier 2\" aboard"));
        assert!(!holds("unit \"Grenadier 2\" afoot"));
        assert!(holds("unit \"Grenadier 1\" afoot"));
        assert!(holds("unit \"Anvil 3\" dead") && !holds("unit \"Anvil 3\" alive"));
        // A dead crew is neither aboard nor afoot: both readings are about
        // somebody who is still out there.
        assert!(!holds("unit \"Anvil 3\" afoot") && !holds("unit \"Anvil 3\" aboard"));
        // Somebody no screen has published reads as gone, so a script waiting
        // for a death terminates and one asserting life fails loudly.
        assert!(holds("unit \"Nobody\" dead") && !holds("unit \"Nobody\" alive"));

        // Matched as a substring of a line the log has ever carried, not of
        // the line it happens to be showing now.
        assert!(holds("log \"mounts up\""));
        assert!(!holds("log \"brews up\""));
    }

    #[test]
    fn keys_accept_both_the_bevy_spelling_and_the_human_one() {
        assert_eq!(parse_key("V"), Some(KeyCode::KeyV));
        assert_eq!(parse_key("KeyV"), Some(KeyCode::KeyV));
        assert_eq!(parse_key("1"), Some(KeyCode::Digit1));
        assert_eq!(parse_key("Digit1"), Some(KeyCode::Digit1));
        assert_eq!(parse_key("Enter"), Some(KeyCode::Enter));
        assert_eq!(parse_key("Up"), Some(KeyCode::ArrowUp));
        assert_eq!(parse_key("F5"), Some(KeyCode::F5));
        assert_eq!(parse_key("F12"), Some(KeyCode::F12));
        assert_eq!(parse_key("F13"), None);
        assert_eq!(parse_key("Nonsense"), None);
    }
}
