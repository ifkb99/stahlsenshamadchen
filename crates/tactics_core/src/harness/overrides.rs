//! Changing one number before anything runs.
//!
//! `--set path=value` and `--sweep path=a,b,c` address fields by the name the
//! json uses, and reach every field of every rule block and every content map,
//! because the patch is a serde round trip through the same representation a
//! save file holds rather than a hand-written list of the knobs somebody
//! thought to expose. A field added to `Balance` tomorrow is sweepable the
//! same afternoon with no change here.
//!
//! The claim that makes this legitimate — that a swept override is *the same
//! thing* as hand-editing `mod.json` — is checked in `tests/harness.rs`. If it
//! ever stops holding, this machinery has become a second game.

use crate::data::DataRegistry;

/// One number to change before anything runs, named the way the json names it.
///
/// The path is `<block>.<field>` for the single-value rule blocks — `balance`,
/// `scale`, `casualties`, `morale`, `reaction`, `command`, `planner` — and
/// `<kind>.<id>.<field>` for the content maps, where `kind` is one of
/// `vehicle`, `weapon`, `ammo`, `module`, `terrain`, `doctrine`. Nesting and
/// list indices work throughout: `morale.rungs[2].accuracy` is a path.
#[derive(Clone)]
pub struct Override {
    pub path: String,
    pub value: String,
}

impl Override {
    /// `path=value`, which is how it is written on the command line.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (path, value) = text
            .split_once('=')
            .ok_or_else(|| format!("`{text}` is not `path=value`"))?;
        if path.is_empty() {
            return Err(format!("`{text}` names no field"));
        }
        Ok(Self {
            path: path.to_string(),
            value: value.to_string(),
        })
    }

    /// The short name this override goes by in a table heading. The full path
    /// is printed once, in the legend under the table, because
    /// `balance.partial_penetration_percent=55` is six columns wide on its own.
    pub fn leaf(&self) -> &str {
        self.path.rsplit('.').next().unwrap_or(&self.path)
    }
}

pub use crate::data::patch::{parse_segment, parse_value, patch_json, render};

/// Apply one override to a loaded registry, returning what the number was.
///
/// The round trip through json is deliberate and is the reason this covers
/// every field rather than a hand-written list of the ones somebody thought
/// to expose: the blocks already serialise, because a save file holds them,
/// so the set of tunable numbers here is exactly the set a mod can declare.
/// A field added to `Balance` tomorrow is sweepable the same afternoon with
/// no change to this file.
pub fn apply_override(reg: &mut DataRegistry, ov: &Override) -> Result<String, String> {
    let new = parse_value(&ov.value);
    let (block, rest) = ov
        .path
        .split_once('.')
        .ok_or_else(|| format!("`{}` names a block but no field in it", ov.path))?;

    macro_rules! patch {
        ($slot:expr, $within:expr) => {{
            let mut json = serde_json::to_value(&$slot).map_err(|e| e.to_string())?;
            let was = patch_json(&mut json, $within, new)?;
            $slot = serde_json::from_value(json).map_err(|e| format!("{}: {e}", ov.path))?;
            Ok(render(&was))
        }};
    }

    match block {
        "balance" => patch!(reg.balance, rest),
        "scale" => patch!(reg.scale, rest),
        "casualties" => patch!(reg.casualties, rest),
        "planner" => patch!(reg.planner, rest),
        "morale" => patch!(reg.morale, rest),
        "reaction" => patch!(reg.reaction, rest),
        "command" => match reg.command.as_mut() {
            // Not defaulted, deliberately, and the message says so: "no chain
            // of command" is the absence of the system rather than the system
            // with generous numbers, so there is nothing here to patch.
            None => Err("no `command` block is declared by these mods".to_string()),
            Some(rules) => patch!(*rules, rest),
        },
        "vehicle" | "weapon" | "ammo" | "module" | "terrain" | "doctrine" => {
            let (id, within) = rest
                .split_once('.')
                .ok_or_else(|| format!("`{}` names a {block} but no field on it", ov.path))?;
            macro_rules! entry {
                ($map:expr) => {{
                    match $map.get_mut(id) {
                        None => {
                            let mut ids: Vec<&str> = $map.keys().map(String::as_str).collect();
                            ids.sort_unstable();
                            Err(format!("no {block} `{id}`. There is: {}", ids.join(", ")))
                        }
                        Some(def) => patch!(*def, within),
                    }
                }};
            }
            match block {
                "vehicle" => entry!(reg.vehicles),
                "weapon" => entry!(reg.weapons),
                "ammo" => entry!(reg.ammo),
                "module" => entry!(reg.modules),
                "terrain" => entry!(reg.terrain),
                _ => entry!(reg.doctrines),
            }
        }
        other => Err(format!(
            "`{other}` is not something to set. Blocks: balance, scale, casualties, \
             morale, reaction, command, planner. Content: vehicle.<id>, weapon.<id>, \
             ammo.<id>, module.<id>, terrain.<id>, doctrine.<id>."
        )),
    }
}

/// Load a mod tree and apply every override, loudly.
///
/// Loud because the alternative is a sweep that silently measured one game
/// twice: a bad path here has to stop the run, not warn in a line that scrolls
/// off before the tables arrive.
pub fn configure(root: &std::path::Path, overrides: &[Override]) -> DataRegistry {
    let (mut reg, _) = DataRegistry::load_dir(root).unwrap_or_else(|e| {
        eprintln!("error: could not load mods from {}: {e}", root.display());
        std::process::exit(1);
    });
    for ov in overrides {
        match apply_override(&mut reg, ov) {
            Ok(was) => println!("  set {} = {} (was {was})", ov.path, ov.value),
            Err(e) => {
                eprintln!("error: --set {}={}: {e}", ov.path, ov.value);
                std::process::exit(1);
            }
        }
    }
    reg
}
