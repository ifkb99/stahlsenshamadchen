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
        "  {} characters, {} vehicles, {} weapons, {} terrain, {} doctrines, {} maps",
        registry.characters.len(),
        registry.vehicles.len(),
        registry.weapons.len(),
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
        "  overworld hex {} = {:.0} battle hexes | turn {} h",
        s.format_overworld_distance(1),
        s.battle_hexes_per_overworld_hex(),
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
    for v in vehicles {
        println!(
            "  {:<16} {:>8}  sees {:>7}",
            v.id,
            s.format_speed(v.movement.points),
            s.format_distance(v.vision_range as i32),
        );
    }

    let mut weapons: Vec<_> = registry.weapons.values().collect();
    weapons.sort_by(|a, b| a.id.cmp(&b.id));
    println!("\nweapons");
    for w in weapons {
        println!(
            "  {:<16} {:>18}  a shot every {}",
            w.id,
            s.format_range(w.range),
            s.format_duration(w.reload(s)),
        );
    }

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
