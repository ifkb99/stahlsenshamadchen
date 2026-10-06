//! Changing a field of a rule block by the name the json gives it.
//!
//! One walk, shared by the two things that change content before it runs:
//! the harness's `--set path=value` (`crate::harness::overrides`), and a
//! world setting's options (`world_settings` in `mod.json`), which are
//! nothing but `worldgen` fields to set. It lives in `data` rather than in
//! the harness because the second is the game, not an instrument: a
//! player's "woodland: heavy" goes through exactly this.
//!
//! The patch is a serde round trip through the same json a save holds, so
//! every field a mod can declare is a field this can reach, and a field
//! added tomorrow is reachable the same afternoon.

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
