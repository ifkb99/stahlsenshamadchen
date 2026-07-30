//! Modder tool: load and validate every mod without launching the game.
//!
//! Usage: `validate-mods [path-to-mods-dir]` (default: `assets/mods`)

use std::path::PathBuf;
use std::process::ExitCode;
use tactics_core::data::DataRegistry;

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
