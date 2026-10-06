//! Modder tool: load and validate every mod without launching the game.
//!
//! Usage: `validate-mods [path-to-mods-dir]` (default: `assets/mods`)

use std::path::PathBuf;
use std::process::ExitCode;
use tactics_core::data::DataRegistry;
use tactics_core::map::{MapKind, MapShape};

fn main() -> ExitCode {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("assets/mods"));

    println!("validating mods under {}", root.display());
    let (registry, report) = match DataRegistry::load_dir(&root) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("fatal: {err}");
            return ExitCode::FAILURE;
        }
    };

    println!(
        "loaded {} mod(s): {}",
        registry.mods.len(),
        registry
            .mods
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!(
        "  {} characters, {} vehicles, {} weapons, {} ammo, {} modules, {} terrain, {} doctrines, {} maps",
        registry.characters.len(),
        registry.vehicles.len(),
        registry.weapons.len(),
        registry.ammo.len(),
        registry.modules.len(),
        registry.terrain.len(),
        registry.doctrines.len(),
        registry.maps.len()
    );

    report_scale(&registry);

    for warning in &report.warnings {
        println!("warning: {warning}");
    }
    for error in &report.errors {
        println!("error: {error}");
    }

    if report.is_ok() {
        println!("OK");
        ExitCode::SUCCESS
    } else {
        println!(
            "{} error(s), {} warning(s)",
            report.errors.len(),
            report.warnings.len()
        );
        ExitCode::FAILURE
    }
}

/// The ammunition roster: every kind of round, what it does, and which guns
/// can chamber it.
///
/// The last column is the one worth printing. A round nothing fires is dead
/// content that validation cannot object to — it is perfectly well-formed —
/// and the only way to notice it is to see the blank beside its name. The
/// same table read the other way says which of a gun's rounds a modder
/// actually wrote.
fn report_ammo(registry: &DataRegistry) {
    let mut ammo: Vec<_> = registry.ammo.values().collect();
    ammo.sort_by(|a, b| a.id.cmp(&b.id));
    if ammo.is_empty() {
        return;
    }
    println!("\nammunition");
    println!(
        "  {:<16} {:<11} {:>10} {:>6} {:>11}  fired by",
        "id", "class", "near->far", "blast", "velocity"
    );
    for a in ammo {
        let mut fired_by: Vec<&str> = registry
            .weapons
            .values()
            .filter(|w| w.ammo.iter().any(|id| id == &a.id))
            .map(|w| w.id.as_str())
            .collect();
        // The weapon map is a HashMap, so sort before printing: a table that
        // reorders itself between runs is a table nobody can diff.
        fired_by.sort_unstable();
        println!(
            "  {:<16} {:<11} {:>10} {:>6} {:>7} m/s  {}",
            a.id,
            a.class.as_str(),
            format!("{} -> {}", a.penetration[0], a.penetration[1]),
            a.blast,
            a.velocity,
            if fired_by.is_empty() {
                "nothing".to_string()
            } else {
                fired_by.join(", ")
            },
        );
    }
}

/// The module roster: every piece of breakable hardware, how big a target it
/// is, how much it takes, and who carries it.
///
/// The last column carries the same weight as the ammunition table's "fired
/// by". A module nothing carries is well-formed content no validator can
/// object to, and the blank beside its name is the only way to see it. Read
/// the other way the column is the answer to "which of my vehicles will lose
/// their tracks", including the ones that inherit the standard set by naming
/// nothing — this asks `modules_for`, so what it prints is what will spawn
/// rather than what was typed.
fn report_modules(registry: &DataRegistry) {
    let mut modules: Vec<_> = registry.modules.values().collect();
    modules.sort_by(|a, b| a.id.cmp(&b.id));
    if modules.is_empty() {
        return;
    }
    println!("\nmodules");
    println!(
        "  {:<16} {:<10} {:>5} {:>10}  carried by",
        "id", "effect", "size", "toughness"
    );
    for m in modules {
        let mut carried_by: Vec<&str> = registry
            .vehicles
            .values()
            .filter(|v| registry.modules_for(v).iter().any(|c| c.id == m.id))
            .map(|v| v.id.as_str())
            .collect();
        // The vehicle map is a HashMap, so sort before printing: a table that
        // reorders itself between runs is a table nobody can diff.
        carried_by.sort_unstable();
        println!(
            "  {:<16} {:<10} {:>5} {:>10}  {}",
            m.id,
            m.effect.as_str(),
            m.size,
            m.toughness,
            if carried_by.is_empty() {
                "nothing".to_string()
            } else {
                carried_by.join(", ")
            },
        );
    }
}

/// Restate the loaded content in the units a player would recognise.
///
/// The validator can only catch numbers that are self-contradictory; it
/// cannot catch a gun that is technically legal and reaches nine kilometres.
/// Printing the whole roster in metres and km/h is how a modder notices that
/// sort of thing, and it is the check the scale contract went years without.
fn report_scale(registry: &DataRegistry) {
    let s = &registry.scale;
    println!("\nscale contract");
    println!(
        "  hex {} | round {} in {} ticks of {} | elevation {} per level",
        s.format_distance(1),
        s.format_duration(s.ticks_per_round),
        s.ticks_per_round,
        s.format_duration(1),
        s.format_elevation(1),
    );
    println!(
        "  overworld hex {} = {:.0} battle hexes | level {} m | turn {} h",
        s.format_overworld_distance(1),
        s.battle_hexes_per_overworld_hex(),
        s.overworld_elevation_meters,
        s.overworld_turn_hours,
    );
    println!(
        "  crew: +{}% sight per observation, +{}% speed per driving, {:+} hit per gunnery",
        registry.balance.vision_per_observation,
        registry.balance.speed_per_driving,
        registry.balance.accuracy_per_gunnery,
    );

    let mut vehicles: Vec<_> = registry.vehicles.values().collect();
    vehicles.sort_by(|a, b| a.id.cmp(&b.id));
    println!("\nvehicles");
    println!(
        "  {:<16} {:>8} {:>9} {:>8} {:>5}",
        "id", "speed", "sees", "conceal", "lift"
    );
    for v in vehicles {
        // Both new columns print a dash at zero rather than the number,
        // because zero is what every vehicle written before infantry existed
        // says and a column of noughts would bury the handful of rows where
        // these fields mean something.
        println!(
            "  {:<16} {:>8} {:>9} {:>8} {:>5}",
            v.id,
            s.format_speed(v.movement.points),
            s.format_distance(v.vision_range as i32),
            if v.concealment == 0 {
                "-".to_string()
            } else {
                format!("{}%", v.concealment)
            },
            if v.capacity == 0 {
                "-".to_string()
            } else {
                v.capacity.to_string()
            },
        );
    }

    let mut weapons: Vec<_> = registry.weapons.values().collect();
    weapons.sort_by(|a, b| a.id.cmp(&b.id));
    println!("\nweapons");
    for w in weapons {
        println!(
            "  {:<16} {:>18}  a shot every {:<7} {}",
            w.id,
            s.format_range(w.range),
            s.format_duration(w.reload(s)),
            w.ammo.join(", "),
        );
    }

    report_ammo(registry);

    let mut vehicles: Vec<_> = registry.vehicles.values().collect();
    vehicles.sort_by(|a, b| a.id.cmp(&b.id));
    println!("\nstowage");
    for v in vehicles {
        // `stowage` is a BTreeMap, so this listing is in key order whatever
        // the hash seed is — the same reason the field is one.
        let racks: Vec<String> = v
            .stowage
            .iter()
            .map(|(id, n)| format!("{n} {id}"))
            .collect();
        println!(
            "  {:<16} {}",
            v.id,
            if racks.is_empty() {
                "-".to_string()
            } else {
                racks.join(", ")
            }
        );
    }

    report_modules(registry);

    let mut maps: Vec<_> = registry.maps.values().collect();
    maps.sort_by(|a, b| a.id.cmp(&b.id));
    println!("\nmaps");
    for m in maps {
        let cols = m.rows.iter().map(|r| r.chars().count()).max().unwrap_or(0);
        let rows = m.rows.len();
        let tiles: usize = m
            .rows
            .iter()
            .map(|r| r.chars().filter(|c| *c != ' ').count())
            .sum();
        // Battle maps measure in battle hexes, overworld maps in overworld
        // hexes: the same grid at two very different zooms. A tile-shaped
        // battle map is a hexagon, so quoting its bounding box would suggest
        // a rectangle that is not there.
        // A generated campaign has no rows: its size is its world's radius,
        // in campaign hexes, and what that comes to in tiles.
        if let Some(world) = &m.world {
            let radius = world
                .rules
                .as_ref()
                .or(registry.worldgen.as_ref())
                .map_or(0, |r| r.radius);
            let hexes = 3 * radius * (radius + 1) + 1;
            println!(
                "  {:<16} {:?} generated, radius {radius} ({hexes} campaign hexes, {} tiles), seed {}",
                m.id,
                m.kind,
                hexes * s.battle_map_tiles(),
                world
                    .seed
                    .map_or("from the campaign".to_string(), |x| x.to_string()),
            );
            continue;
        }
        let size = match (m.kind, m.shape()) {
            (MapKind::Battle, MapShape::Tile) => format!(
                "hexagon r{}, {} across, {tiles} tiles",
                s.battle_map_radius(),
                s.format_distance(cols as i32),
            ),
            (MapKind::Battle, MapShape::Free) => format!(
                "free {cols}x{rows}, {} x {}, {tiles} tiles",
                s.format_distance(cols as i32),
                s.format_distance(rows as i32),
            ),
            (MapKind::Overworld, _) => format!(
                "{cols}x{rows}, {} x {}",
                s.format_overworld_distance(cols as u32),
                s.format_overworld_distance(rows as u32),
            ),
        };
        println!("  {:<16} {:?} {size}", m.id, m.kind);
    }
    println!();
}
