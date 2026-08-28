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
/// `scale`, `casualties`, `morale`, `reaction`, `command` — and
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

/// Read `value` the way json would, falling back to a bare string.
///
/// So `55`, `-10`, `1.5`, `true` and `[1, 2]` all mean what they look like,
/// and `foot` means `"foot"` without the shell-quoting that would otherwise
/// be needed to get a quote past bash.
pub fn parse_value(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|_| serde_json::Value::String(text.to_string()))
}

/// `rungs[2]` -> `("rungs", Some(2))`.
pub fn parse_segment(seg: &str) -> Result<(&str, Option<usize>), String> {
    let Some(open) = seg.find('[') else {
        return Ok((seg, None));
    };
    let rest = &seg[open + 1..];
    let close = rest
        .find(']')
        .ok_or_else(|| format!("`{seg}` opens a [ and never closes it"))?;
    let index: usize = rest[..close]
        .parse()
        .map_err(|_| format!("`{}` is not a list position", &rest[..close]))?;
    Ok((&seg[..open], Some(index)))
}

/// Walk a dotted path into a json value and replace the leaf, returning what
/// was there.
///
/// It refuses to *create* anything. A path naming no existing field is a typo,
/// and a typo that quietly invented a key would produce a sweep whose rows all
/// measured the same game and agreed with each other beautifully. The error
/// lists what was actually at that level, because these names come from serde
/// and are not always what the doc comment beside the field calls them.
pub fn patch_json(
    root: &mut serde_json::Value,
    path: &str,
    new: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let segs: Vec<&str> = path.split('.').filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return Err("no field named".to_string());
    }
    let mut cur = root;
    for (n, seg) in segs.iter().enumerate() {
        let (name, index) = parse_segment(seg)?;
        let obj = cur
            .as_object_mut()
            .ok_or_else(|| format!("nothing under `{seg}`: the level above it holds no fields"))?;
        if !obj.contains_key(name) {
            let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
            keys.sort_unstable();
            return Err(format!(
                "no `{name}` here. This level has: {}",
                keys.join(", ")
            ));
        }
        cur = obj.get_mut(name).expect("checked on the line above");
        if let Some(i) = index {
            let len = cur.as_array().map(|a| a.len());
            let arr = cur
                .as_array_mut()
                .ok_or_else(|| format!("`{name}` is not a list, so `[{i}]` means nothing"))?;
            cur = arr.get_mut(i).ok_or_else(|| {
                format!(
                    "`{name}` has {} entries, so there is no [{i}]",
                    len.unwrap_or(0)
                )
            })?;
        }
        if n + 1 == segs.len() {
            // A number where a string was, or a string where a list was, is
            // caught here rather than by `from_value` below, because serde's
            // message names the Rust type and this one names the path.
            // `null` is always allowed: clearing an optional field is a real
            // thing to want, and `--set terrain.forest.capacity=null` is how
            // you switch a rule off in data to check that its absence is the
            // game you had before it — which is the additivity test this whole
            // project leans on.
            if !cur.is_null()
                && !new.is_null()
                && std::mem::discriminant(&*cur) != std::mem::discriminant(&new)
            {
                return Err(format!(
                    "`{path}` holds {cur}, and {new} is a different kind of value"
                ));
            }
            return Ok(std::mem::replace(cur, new));
        }
    }
    unreachable!("the loop returns on the last segment")
}

/// A json value as a person would rather read it.
///
/// Only really about floats: most of this content is `f32`, json holds `f64`,
/// and the widening prints an `aggression` of 0.85 as 0.8500000238418579. The
/// "was" note exists so somebody can put the number back, and a number nobody
/// would type is no help with that.
pub fn render(value: &serde_json::Value) -> String {
    match value.as_f64() {
        Some(f) if !value.is_i64() && !value.is_u64() => {
            let text = format!("{f:.6}");
            let text = text.trim_end_matches('0').trim_end_matches('.');
            text.to_string()
        }
        _ => value.to_string(),
    }
}

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
             morale, reaction, command. Content: vehicle.<id>, weapon.<id>, ammo.<id>, \
             module.<id>, terrain.<id>, doctrine.<id>."
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
