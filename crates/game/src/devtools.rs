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
//! | `quit` | exit once every pending screenshot has been written |
//!
//! A script that ends without `quit` leaves the game running normally, which
//! is useful for setting up a state by hand and then taking over.

use crate::map_render::ScriptedCursor;
use bevy::app::AppExit;
use bevy::input::{ButtonInput, InputSystems};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured, save_to_disk};
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
fn debug_enabled() -> bool {
    std::env::var("STAHL_DEBUG").is_ok()
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
    Quit,
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
    /// Keys and buttons tapped last frame, released at the start of this one.
    tapped_keys: Vec<KeyCode>,
    tapped_buttons: Vec<MouseButton>,
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
            tapped_keys: Vec::new(),
            tapped_buttons: Vec::new(),
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
        "quit" => Some(Action::Quit),
        _ => None,
    }
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
        Action::Quit => {
            // Hold the door until every screenshot observer has fired.
            if pending.0 > 0 {
                return;
            }
            info!("dev script: finished, exiting");
            exit.write(AppExit::Success);
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
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(path.clone()))
                .observe(
                    move |_: On<ScreenshotCaptured>, mut pending: ResMut<PendingShots>| {
                        pending.0 = pending.0.saturating_sub(1);
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
