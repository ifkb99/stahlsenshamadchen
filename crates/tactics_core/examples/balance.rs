//! What the numbers in `assets/mods` actually do.
//!
//! ```sh
//! cargo run --release -p tactics_core --example balance          # instant
//! cargo run --release -p tactics_core --example balance -- --sim # + fought out
//! cargo run --release -p tactics_core --example balance -- --sim --games 40
//! cargo run --release -p tactics_core --example balance -- --help # every flag
//! ```
//!
//! Three passes, then, because there is a third question under the first two.
//!
//! The **analytic** pass answers "what did that number just do" without playing
//! anything: it stands two vehicles on an empty field and asks the real combat
//! code what would happen. It runs in well under a second, so it belongs in the
//! tight loop where you are editing a json file and want to know whether the 88
//! can still hurt a Löwe from the front.
//!
//! The **simulated** pass (`--sim`) fights whole battles and reports what
//! actually happened — who won, what killed them, what it cost the cadets, how
//! many shells went out. It is slower and noisier, and it is the check at the
//! end, because the analytic numbers can all look reasonable while the fights
//! they produce are terrible.
//!
//! Both passes are organised around the **kill chain** the ballistics rewrite
//! installed, because that is now the shape of the game: a round is chambered,
//! it hits or it does not, it gets through the plate or it does nothing at all,
//! and what it finds behind the plate is cadets and modules rather than a hit
//! point pool. Every number below therefore comes out of `preview_attack`,
//! `chambered`, `flight_ticks` and `resolve_round` rather than a formula
//! written here — the tables are forced through the engine by loading the
//! round under test into the racks and asking, not by multiplying the same
//! numbers a second time in a second place. A harness that computes its own
//! penetration would eventually be measuring a second, imaginary game.
//!
//! The one exception is labelled where it appears: the "shots to knock out"
//! table folds the brew-up and blast-overmatch rolls into a closed form,
//! assembled from the real constants but not run through the resolver. The
//! note under that table says so, and the `--sim` kill-cause table is what
//! checks it.
//!
//! The **comparative** pass (`--sweep`) is the third speed, and it exists
//! because neither of the first two answers the question actually being asked
//! while a number is being tuned. That question is never "what does 55 do", it
//! is "what does 55 do *that 40 did not*", and until now the way to answer it
//! was to edit `mod.json`, run, revert, run again, and compare two screens of
//! scrollback from memory. That method has two failure modes and the whole
//! ballistics rewrite met both: a revert that was never made, and a difference
//! well inside the noise of the sample read as a result anyway. A sweep runs
//! every value at once, folds each one's battles in seed order, and prints the
//! baseline on the row above the difference — and `--sweep seed=0,1000` in
//! the same table is what the noise floor looks like, so a difference can be
//! held against it instead of against an intuition.
//!
//! ```sh
//! # what does the partial-penetration floor do?
//! balance --sim --games 36 --sweep balance.partial_penetration_percent=40,55,70
//! # ...and how much of that was the dice?
//! balance --sim --games 36 --sweep seed=0,1000,2000
//! # two versions of the content, rather than two numbers in one version
//! balance --sim --sweep mods=assets/mods,../old/assets/mods
//! ```
//!
//! `--set` is the same machinery for a single run, and `--jobs` caps how much
//! of the machine one invocation takes, so several of these can be run beside
//! each other. **No printed number moves with `--jobs`**: results are folded in
//! seed order regardless of which core finished first, and that is checked by
//! running the same batch at several widths, not hoped for.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use tactics_core::Hex;
use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, make_battle_planner};
use tactics_core::battle::{
    AttackPreview, BattleState, Destruction, EndReason, Event, Order, SideState, UnitId,
    blast_overmatches, flight_ticks, hit_breakdown, preview_attack,
};
use tactics_core::data::{ArmorFacing, DataRegistry, ModuleEffect, TerrainDef, WeaponDef};
use tactics_core::force;
use tactics_core::harness::arena::{ARENAS, Arena, DEFAULT_ARENA, arena_named};
use tactics_core::harness::overrides::{Override, configure};
use tactics_core::harness::parallel::{JOBS, run_all, thread_budget};
use tactics_core::harness::tally::Tally;
use tactics_core::map::{Facing, HexMap, MapFile, MapKind, UnitPlacement};
use tactics_core::roster::{CadetId, Roster};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return;
    }
    let sim = args.iter().any(|a| a == "--sim");
    // Opt-in and separately sized, because MCTS costs seconds per order and
    // would otherwise make the content-iteration loop unusable. Its default
    // sample is deliberately small: this table answers a yes/no question
    // about which brain is better, not a balance question about a number.
    let brains = args.iter().any(|a| a == "--brains");
    let brain_games: usize = flag(&args, "--brain-games")
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let brain_difficulty: u8 = flag(&args, "--brain-difficulty")
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let games: usize = flag(&args, "--games")
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);
    // Requisition points each side spends on its own army in the mustered
    // section. 60 is about what the shipped maps field a side — three tanks,
    // a gun section and a grenadier section — so the table starts life
    // comparable to the scenarios beside it.
    let budget: i32 = flag(&args, "--points")
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    // How far to shift every table's battles. An offset rather than a base,
    // so that 0 — the default — is exactly the sample every number this
    // project has quoted was measured on, and any other value moves all four
    // fought-out tables together. Exposed because two runs of the same
    // configuration at different seeds are two *samples*, and the honest way
    // to ask whether a difference between variants is real is to re-draw it,
    // which needs the seeds to be somebody's choice rather than a constant
    // buried in a function.
    let seed: u64 = flag(&args, "--seed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    if let Some(n) = flag(&args, "--jobs").and_then(|v| v.parse::<usize>().ok()) {
        JOBS.store(n.max(1), std::sync::atomic::Ordering::Relaxed);
    }
    let arena = match flag(&args, "--arena") {
        None => DEFAULT_ARENA,
        Some(name) => arena_named(name).unwrap_or_else(|| {
            eprintln!(
                "error: --arena {name}: no such arena. There is: {}",
                ARENAS.iter().map(|a| a.id).collect::<Vec<_>>().join(", ")
            );
            std::process::exit(1);
        }),
    };
    let cfg = Run {
        games,
        seed,
        budget,
        sim,
        verbose: args.iter().any(|a| a == "--verbose"),
        csv: args.iter().any(|a| a == "--csv"),
        absolute: args.iter().any(|a| a == "--absolute"),
        only: Only::parse(flag(&args, "--only")),
        arena,
    };

    if cfg!(debug_assertions) {
        eprintln!("note: debug build. Fine for the analytic pass, slow for --sim.");
    }

    let root = match flag(&args, "--mods") {
        Some(dir) => std::path::PathBuf::from(dir),
        None => std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods"),
    };

    let overrides: Vec<Override> = flag_all(&args, "--set")
        .iter()
        .map(|text| {
            Override::parse(text).unwrap_or_else(|e| {
                eprintln!("error: --set {text}: {e}");
                std::process::exit(1);
            })
        })
        .collect();
    let axes: Vec<Axis> = flag_all(&args, "--sweep")
        .iter()
        .map(|text| {
            let ov = Override::parse(text).unwrap_or_else(|e| {
                eprintln!("error: --sweep {text}: {e}");
                std::process::exit(1);
            });
            Axis {
                path: ov.path,
                values: ov.value.split(',').map(str::to_string).collect(),
            }
        })
        .collect();

    if !axes.is_empty() {
        sweep(&axes, &overrides, &root, &cfg);
        return;
    }

    let registry = configure(&root, &overrides);

    analytic(&registry, &cfg);
    if sim {
        if cfg.only.wants("sim") {
            simulate(&registry, games, seed);
        }
        for grid in fought_grids(&registry, &cfg, seed, budget) {
            print_grid(&grid);
        }
    }
    if brains {
        brains_table(&registry, cfg.arena, brain_games, brain_difficulty);
    } else {
        println!("\n(pass --sim to fight {games} battles and see what these numbers do)");
    }
}

/// Everything that answers without fighting anything.
fn analytic(reg: &DataRegistry, cfg: &Run) {
    let mut duels = Duels::new(reg);
    if cfg.only.wants("roster") {
        roster_table(reg);
    }
    if cfg.only.wants("detect") {
        detect_table(reg);
    }
    if cfg.only.wants("hit") {
        hit_table(reg);
    }
    if cfg.only.wants("pen") {
        penetration_table(reg, &mut duels);
    }
    if cfg.only.wants("kills") {
        kill_chain_table(reg, &mut duels);
    }
    if cfg.only.wants("flight") {
        flight_table(reg);
    }
    if cfg.only.wants("flags") {
        flags(reg, &mut duels);
    }
}

const USAGE: &str = "\
what the numbers in assets/mods actually do

  cargo run --release -p tactics_core --example balance [-- FLAGS]

what to run
  --sim                  fight whole battles as well as previewing shots
  --games N              battles in the fought-out pass (default 12; read the
                         doctrine table at 36 or not at all)
  --seed N               shift every table's battles by N (default 0, the
                         sample this project's numbers were measured on). Two
                         runs at different seeds are two samples of one game.
  --points N             requisition budget per side in the mustered table (60)
  --brains               which planner is better; --brain-games, --brain-difficulty
  --only A,B             print only these tables. One of: roster, detect, hit,
                         pen, kills, flight, flags, sim, delegation, mustered,
                         skill, ground

which game
  --arena NAME           battlefield the skill, brains and mustered tables fight
                         on, and the arena row the ground table adds. One of:
                         skill_arena (default — the sample every quoted number
                         was measured on), ridge_arena (bigger, and built so a
                         wrong choice of ground is punished: a bare crest whose
                         thirteen hexes see between 68 and 148 tiles, woods a
                         gun on the ridge sees into and woods it does not)
  --mods DIR             mod tree to load (default assets/mods)
  --set PATH=VALUE       change one number before anything runs. Repeatable.
                         Blocks: balance, scale, casualties, morale, reaction,
                         command, planner. Content: vehicle.<id>, weapon.<id>,
                         ammo.<id>, module.<id>, terrain.<id>, doctrine.<id>.
                         Nesting and list indices work: morale.rungs[2].accuracy
  --sweep PATH=A,B,C     run once per value and put the results side by side.
                         Repeatable; the axes multiply. Two axis names are
                         not fields: `--sweep mods=a,b` compares two versions
                         of the content, `--sweep seed=0,1000` fights the same
                         game twice (how the table shows the noise floor that
                         every other difference has to clear), and
                         `--sweep points=60,100` buys each doctrine a bigger
                         army instead of changing a number in one.

how, and how much of it
  --jobs N               battles in the air at once (default: every core).
                         The numbers do not move with this — results are
                         folded in seed order — so it is safe to run several
                         of these at once at a fraction of the machine each.
  --verbose              in a sweep, print each variant's full report too
  --absolute             in a sweep, print each variant's own numbers rather
                         than its differences from the first. What a range
                         wants; the differences are what a tuning question wants
  --csv                  print the comparison digest and every swept table as
                         csv as well, long-form: table,row,variant,column,value

  A swept table with three or more variants also gets a `spread` line per row:
  the widest gap between variants in that column. Under a `--sweep seed=` that
  line is the noise floor, and it is what every other difference has to clear.

examples
  --sim --sweep balance.partial_penetration_percent=40,55,70 --games 36
  --sim --sweep weapon.howitzer_105.dispersion=0,4,8 --jobs 4
  --set balance.moving_target_per_hex=0 --set balance.firing_on_the_move_per_hex=0
  --sim --sweep mods=assets/mods,../old/assets/mods
  --sim --sweep planner.horizon_rounds=2,4,6 --games 36  # how the AI thinks
  --only delegation --sim --games 36 --set planner.devolved=1.1 \
\n      --sweep planner.order_worth=0,0.25,1     # what is an order worth?
  --sim --sweep seed=0,1000,2000 --games 36         # what is the noise floor?
  --sim --only skill --absolute --sweep seed=0,1000,2000,3000   # ...for one table
  --sim --games 36 --arena ridge_arena --only skill,ground --absolute \\
\n      --sweep seed=0,1000,2000,3000    # does ground make skill tell?";

/// Every occurrence of a repeatable `--flag value`.
fn flag_all<'a>(args: &'a [String], name: &str) -> Vec<&'a str> {
    args.iter()
        .enumerate()
        .filter(|(_, a)| a.as_str() == name)
        .filter_map(|(i, _)| args.get(i + 1).map(String::as_str))
        .collect()
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).map(|s| s.as_str())
}

fn heading(title: &str) {
    println!("\n{title}");
    println!("{}", "-".repeat(title.len().max(60)));
}

/// Column heading that fits, without mangling short names.
fn short(id: &str) -> String {
    if id.len() <= 11 {
        id.to_string()
    } else {
        id.chars().take(10).chain(std::iter::once('.')).collect()
    }
}

/// Every vehicle in the roster, sorted, so every table has the same columns
/// in the same order.
fn vehicle_ids(reg: &DataRegistry) -> Vec<String> {
    let mut ids: Vec<String> = reg.vehicles.keys().cloned().collect();
    ids.sort();
    ids
}

/// Every gun paired with every round it can chamber, sorted.
///
/// A weapon whose mod declares no ammunition is deliberately absent: it fires
/// on the legacy path, whose penetration is derived inside `chambered` from
/// the weapon's own numbers and is not reachable from here without writing
/// that derivation down a second time. [`legacy_guns`] names them instead, so
/// they are reported as untabulated rather than silently missing.
fn gun_rounds(reg: &DataRegistry) -> Vec<(String, String)> {
    let mut ids: Vec<&String> = reg.weapons.keys().collect();
    ids.sort();
    let mut out = Vec::new();
    for id in ids {
        let weapon = &reg.weapons[id];
        for ammo in &weapon.ammo {
            if reg.ammo(ammo).is_some() {
                out.push((id.clone(), ammo.clone()));
            }
        }
    }
    out
}

fn legacy_guns(reg: &DataRegistry) -> Vec<String> {
    let mut ids: Vec<String> = reg
        .weapons
        .values()
        .filter(|w| w.ammo.is_empty())
        .map(|w| w.id.clone())
        .collect();
    ids.sort();
    ids
}

/// The three ranges every gun is tabulated at: the near edge of its band, the
/// middle of it, and the end of its reach. Kinetic penetration falls off
/// across exactly this span, so the near and far rows are the two ends of the
/// only curve in the data.
fn bands(weapon: &WeaponDef) -> [i32; 3] {
    let near = (weapon.range[0] as i32).max(1);
    let far = (weapon.range[1] as i32).max(near);
    [near, (near + far) / 2, far]
}

/// Which arc a shot lands on, and the target facing that produces it when the
/// attacker stands due west. Front is nose-on, rear is tail-on, and either
/// eastward diagonal presents a flank.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Arc {
    Front,
    Side,
    Rear,
}

impl Arc {
    fn facing(self) -> Facing {
        match self {
            Arc::Front => Facing::West,
            Arc::Side => Facing::NorthEast,
            Arc::Rear => Facing::East,
        }
    }

    fn expected(self) -> ArmorFacing {
        match self {
            Arc::Front => ArmorFacing::Front,
            Arc::Side => ArmorFacing::Side,
            Arc::Rear => ArmorFacing::Rear,
        }
    }

    fn index(self) -> u8 {
        match self {
            Arc::Front => 0,
            Arc::Side => 1,
            Arc::Rear => 2,
        }
    }
}

// --- the analytic pass ----------------------------------------------------

/// A memo over duels, because the tables below ask the same question from
/// several directions and each answer costs a whole battlefield.
///
/// The cache is keyed by everything that changes the answer and read through
/// sorted iteration everywhere it is printed, so hash order never reaches the
/// output.
struct Duels<'r> {
    reg: &'r DataRegistry,
    shots: HashMap<(String, String, String, u8, i32), Option<AttackPreview>>,
    facts: HashMap<String, Option<Facts>>,
}

/// What a vehicle is made of, as the engine counts it. Read off a spawned
/// unit rather than off the definition so that the substance pool here is
/// literally `BattleState::substance` and the interior weights are literally
/// the modules the engine stamped aboard — including the standard four a
/// chassis that declares none inherits.
#[derive(Clone, Copy)]
struct Facts {
    /// Full substance complement: two points per cadet plus every module's
    /// toughness.
    substance: u32,
    /// Cadets aboard — the seats a behind-armor roll can find.
    seats: u32,
    /// Total weight a behind-armor effect roll draws from.
    interior: u32,
    /// The ammunition stowage's share of that weight, which is the only
    /// module whose destruction is usually the end of the vehicle.
    rack: u32,
    /// The thinnest plate on the hull, which is what blast overmatch is
    /// measured against.
    thinnest: i32,
}

impl<'r> Duels<'r> {
    fn new(reg: &'r DataRegistry) -> Self {
        Self {
            reg,
            shots: HashMap::new(),
            facts: HashMap::new(),
        }
    }

    /// One prospective shot, as the engine previews it: `weapon` firing
    /// `ammo` at `vehicle` from `dist` hexes, striking `arc`.
    fn shot(
        &mut self,
        weapon: &str,
        ammo: &str,
        vehicle: &str,
        arc: Arc,
        dist: i32,
    ) -> Option<AttackPreview> {
        let key = (
            weapon.to_string(),
            ammo.to_string(),
            vehicle.to_string(),
            arc.index(),
            dist,
        );
        if let Some(hit) = self.shots.get(&key) {
            return hit.clone();
        }
        let value = duel(self.reg, weapon, ammo, vehicle, arc, dist);
        self.shots.insert(key, value.clone());
        value
    }

    fn facts(&mut self, vehicle: &str) -> Option<Facts> {
        if let Some(known) = self.facts.get(vehicle) {
            return *known;
        }
        let value = facts_for(self.reg, vehicle);
        self.facts.insert(vehicle.to_string(), value);
        value
    }
}

/// Stand two vehicles on an empty field, load exactly one kind of round into
/// the attacker, and ask the real combat code what one shot would do.
///
/// Forcing the round is what makes this a table *per round* rather than per
/// gun: `chambered` fires the first ammunition in the weapon's list with
/// anything left in the racks, so emptying every other rack through the
/// engine's own `set_loadout` leaves the round under test as the only thing
/// the gun can put downrange. Nothing here reimplements penetration; it
/// arranges the world so that `preview_attack` answers the question asked.
///
/// The two vehicles stand in the same map row, which makes the shot square on
/// to the struck hex face — obliquity exactly 1.00 — so the plate in the
/// table is the plate on the datasheet and the reader is comparing armor
/// rather than geometry.
fn duel(
    reg: &DataRegistry,
    weapon: &str,
    ammo: &str,
    target_vehicle: &str,
    arc: Arc,
    dist: i32,
) -> Option<AttackPreview> {
    // Sorted rather than "whichever the hash hands over first": the choice of
    // firing platform must not move between runs.
    let mut carriers: Vec<&String> = reg
        .vehicles
        .values()
        .filter(|v| v.weapons.iter().any(|w| w == weapon))
        .map(|v| &v.id)
        .collect();
    carriers.sort();
    let attacker_vehicle = reg.vehicle(carriers.first()?)?;
    let weapon_index = attacker_vehicle.weapons.iter().position(|w| w == weapon)?;

    let mut state = two_unit_field(
        reg,
        &attacker_vehicle.id,
        target_vehicle,
        arc.facing(),
        dist,
    )?;
    let loaded: Vec<String> = state.unit(UnitId(0))?.ammo.keys().cloned().collect();
    for id in loaded {
        if id != ammo {
            state.set_loadout(reg, UnitId(0), &id, 0).ok()?;
        }
    }
    // One round is all a preview needs, and a gun this vehicle cannot chamber
    // the round in refuses here rather than quietly previewing something else.
    state.set_loadout(reg, UnitId(0), ammo, 1).ok()?;

    let preview = preview_attack(reg, &state, UnitId(0), weapon_index, UnitId(1), false)?;
    debug_assert_eq!(preview.facing, arc.expected());
    Some(preview)
}

/// What the engine says is aboard a fresh vehicle of this type.
fn facts_for(reg: &DataRegistry, vehicle: &str) -> Option<Facts> {
    let state = two_unit_field(reg, vehicle, vehicle, Facing::West, 2)?;
    let unit = state.unit(UnitId(1))?;
    let (_, substance) = state.substance(reg, unit);
    let crew_weight = reg.balance.crew_weight.max(0) as u32;
    let seats = unit.crew.len() as u32;
    let mut interior = crew_weight * seats;
    let mut rack = 0;
    for id in unit.modules.keys() {
        if let Some(module) = reg.module(id) {
            interior += module.size;
            if module.effect == ModuleEffect::Ammo {
                rack += module.size;
            }
        }
    }
    let armor = &reg.vehicle(vehicle)?.armor;
    let thinnest = [ArmorFacing::Front, ArmorFacing::Side, ArmorFacing::Rear]
        .into_iter()
        .map(|f| armor.value(f))
        .min()
        .unwrap_or(0)
        .max(0);
    Some(Facts {
        substance,
        seats,
        interior,
        rack,
        thinnest,
    })
}

/// A strip of grass with one vehicle at each end, `dist` hexes apart.
/// `shape: free` because this is deliberately not a whole overworld tile.
fn two_unit_field(
    reg: &DataRegistry,
    attacker: &str,
    target: &str,
    target_facing: Facing,
    dist: i32,
) -> Option<BattleState> {
    let width = (dist + 3) as usize;
    let row = "g".repeat(width);
    let file: MapFile = serde_json::from_value(serde_json::json!({
        "id": "balance_field",
        "kind": "battle",
        "shape": "free",
        "palette": { "g": "grass" },
        "rows": [row, row, row],
    }))
    .ok()?;
    let map = HexMap::from_map_file(&file).ok()?;

    let placements = vec![
        UnitPlacement {
            aboard_at: None,
            at: [1, 1],
            side: 0,
            vehicle: attacker.to_string(),
            crew: Vec::new(),
            name: Some("attacker".into()),
            facing: Some(Facing::East),
            formation: None,
            leads: false,
        },
        UnitPlacement {
            aboard_at: None,
            at: [1 + dist, 1],
            side: 1,
            vehicle: target.to_string(),
            crew: Vec::new(),
            name: Some("target".into()),
            facing: Some(target_facing),
            formation: None,
            leads: false,
        },
    ];
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
    let (roster, crews) = Roster::stamp_for(reg, &placements);
    Some(
        BattleState::from_placements(
            reg,
            map,
            sides,
            &placements,
            &crews,
            std::sync::Arc::new(roster),
            1,
        )
        .expect("the staged placements are content the base mod ships"),
    )
}

fn roster_table(reg: &DataRegistry) {
    heading("vehicles");
    println!(
        "{:<16} {:>12} {:>9} {:>9} {:>8} {:>6} {:>7} {:>5} {:>9}",
        "id", "armour F/S/R", "substance", "speed", "sight", "safety", "profile", "cost", "rounds"
    );
    for id in vehicle_ids(reg) {
        let v = &reg.vehicles[&id];
        // Substance is the hit-point pool's successor and is derived, not
        // declared: two points per seat plus every module's toughness.
        let substance: u32 = 2 * v.crew_slots.len() as u32
            + reg.modules_for(v).iter().map(|m| m.toughness).sum::<u32>();
        let rounds: u32 = v.stowage.values().sum();
        println!(
            "{:<16} {:>12} {:>9} {:>9} {:>8} {:>6} {:>7} {:>5} {:>9}",
            id,
            format!("{}/{}/{}", v.armor.front, v.armor.side, v.armor.rear),
            substance,
            reg.scale.format_speed(v.movement.points),
            reg.scale.format_distance(v.vision_range as i32),
            v.safety,
            if v.profile == 0 {
                "-".to_string()
            } else {
                format!("{:+}", v.profile)
            },
            v.cost,
            rounds,
        );
    }
    println!(
        "\n  `substance` is what she is made of — two points per cadet plus every\n  \
         module's toughness — which is what a penetration spends itself against\n  \
         now that there are no hit points. `rounds` is everything in the racks.\n  \
         `profile` is percentage points off an attacker's hit chance — how big\n  \
         a target she is, which is a different question from how hard she is\n  \
         to find (that is `concealment`, and it scales the spotter's range)."
    );
}

/// What the gunner is up against before the plate is ever consulted.
///
/// The twin of the penetration table, and it exists because the arc that
/// added these terms is an argument that the number in front of the plate
/// deserves the same scrutiny as the number behind it. Every cell is
/// `hit_breakdown` — the resolver's own arithmetic, listening in — against a
/// medium tank on open grass, so the columns differ in exactly one thing
/// each and the reader is comparing circumstances rather than terrain.
///
/// The states are stamped onto real units rather than modelled: `moved` is
/// the field the resolver reads, and `pressure` is set to the rung's own
/// threshold, so a column labelled "breaking" is a crew the morale ladder
/// would actually call breaking.
/// What the detection numbers do: how long a crew takes to pick somebody out
/// of ground she is already looking straight at.
///
/// The twin of the `to hit` table one section down, and built the same way —
/// every cell is the engine's own [`Balance::detection_chance`], never a
/// formula retyped here, so the table cannot drift from what the spotting
/// pass rolls against.
///
/// Rows are ground rather than vehicles because the ground is where the
/// number lives. What a *vehicle* contributes is her `concealment`, and that
/// does something else entirely: it shortens the range at which she can be
/// found at all, which the roster table already prints and which moves these
/// columns rather than these cells — a crew hidden at 50% meets the `far`
/// column at half the distance the same crew in the open would.
fn detect_table(reg: &DataRegistry) {
    heading("detection: how long a crew takes to find what she is looking at");
    // Only the ratio of distance to reach enters the arithmetic, so a
    // notional hundred-hex reach makes every column an exact percentage of
    // it and keeps the table independent of whose eyes are being used.
    const REACH: u32 = 100;
    // The gradient is the rule, exactly as it is for the gunnery terms: one
    // hex a round is a walking pace and five is thirty km/h, and a table
    // with one "moving" column would price them the same.
    const BOUNDS: [u32; 3] = [1, 3, 5];
    let ticks = reg.scale.ticks_per_round;

    let mut grounds: Vec<&TerrainDef> = reg
        .terrain
        .values()
        // Ground nothing can stand on cannot hide anybody on it.
        .filter(|t| !t.move_cost.is_empty())
        .collect();
    grounds.sort_by(|a, b| a.id.cmp(&b.id));

    print!(
        "{:<16} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "ground", "conceal", "close", "half", "3/4", "far"
    );
    for hexes in BOUNDS {
        print!(" {:>8}", format!("far {hexes}h"));
    }
    println!(" {:>10} {:>10}", "ticks far", "rounds");

    for ground in grounds {
        let chance = |at: u32, moved: u32| {
            reg.balance
                .detection_chance(ground.concealment, at as i32, REACH, moved)
        };
        let far = chance(REACH, 0);
        print!(
            "{:<16} {:>8} {:>8} {:>8} {:>8} {:>8}",
            short(&ground.id),
            if ground.concealment == 0 {
                "-".to_string()
            } else {
                format!("{}%", ground.concealment)
            },
            chance(0, 0),
            chance(REACH / 2, 0),
            chance(REACH * 3 / 4, 0),
            far
        );
        for hexes in BOUNDS {
            print!(" {:>8}", chance(REACH, hexes));
        }
        // Expected ticks for ONE crew to find a halted target at the edge of
        // her reach, which is `1 / p` — the mean of the geometric
        // distribution the per-tick roll actually is. A dash where the
        // numbers reach zero, because "never" is not a large number.
        if far <= 0 {
            println!(" {:>10} {:>10}", "never", "never");
        } else {
            let t = 100.0 / far as f32;
            println!(" {:>10.1} {:>10.1}", t, t / ticks as f32);
        }
    }

    println!(
        "\n  every cell is `Balance::detection_chance` in 100, rolled once per\n  \
         tick by the best-placed crew who has her ground in view — one roll, never\n  \
         one per pair of eyes, so the ticks column is the real wait and not a\n  \
         number divided by however many crews happen to be looking.\n  \
         \n  \
         `close`, `half`, `3/4` and `far` are distance as a share of that\n  \
         spotter's reach, and `far Nh` is the target having crossed N hexes this\n  \
         round — the same `moved` field the gunner reads, so a crew who dashed for\n  \
         cover pays for the dash twice. Inside {}% of a crew's reach she simply\n  \
         sees what is in front of her, and everything the ground and the distance\n  \
         are worth fades in over what is left of it.\n  \
         \n  \
         A round is {ticks} ticks. Firing bypasses all of this — `revealed` is\n  \
         checked before a die is thrown — and so does a contact this side already\n  \
         holds: a search buys an acquisition, never the watching afterwards.\n  \
         detection_base {}, certain {}%, at_range {}, per_hex_moved {}.",
        reg.balance.detection_certain_percent,
        reg.balance.detection_base,
        reg.balance.detection_certain_percent,
        reg.balance.detection_at_range_percent,
        reg.balance.detection_per_hex_moved,
    );
}

fn hit_table(reg: &DataRegistry) {
    heading("to hit: what the gunner is up against, P(hit) %");
    // A representative target: the hit half does not read armour at all, so
    // the only thing the target contributes is her profile, and that gets a
    // section of its own below.
    const TARGET: &str = "medium_tank";
    let rungs: Vec<(String, u32)> = reg
        .morale
        .rungs
        .iter()
        .filter(|r| r.accuracy != 0)
        .map(|r| (r.name.to_lowercase(), r.at_pressure))
        .collect();
    // The attacker's own motion is shown as a gradient rather than as one
    // "moving" column, because the penalty is per hex and the whole point of
    // that shape is that a crawl and a dash are different decisions.
    const BOUNDS: [u32; 3] = [1, 3, 5];
    const TARGET_BOUND: u32 = 3;
    print!("{:<24} {:>10} {:>8}", "gun", "range", "halted");
    for hexes in BOUNDS {
        print!(" {:>8}", format!("moves {hexes}h"));
    }
    print!(" {:>8}", format!("tgt {TARGET_BOUND}h"));
    for (name, _) in &rungs {
        print!(" {:>10}", name);
    }
    print!(" {:>8}", "blind");
    println!();

    let mut guns: Vec<&String> = reg.weapons.values().map(|w| &w.id).collect();
    guns.sort();
    for gun in guns {
        let Some(weapon) = reg.weapon(gun) else {
            continue;
        };
        for (label, dist) in ["near", "mid", "max"].into_iter().zip(bands(weapon)) {
            // Each cell rebuilds the field, because each one stamps a
            // different state onto the units and a shared state would leak
            // one column into the next.
            let chance = |moved_attacker: u32, moved_target: u32, pressure: u32, blind: bool| {
                let mut state =
                    two_unit_field(reg, gun_carrier(reg, gun)?, TARGET, Facing::West, dist)?;
                let weapon_index = reg
                    .vehicle(&state.units[0].vehicle)?
                    .weapons
                    .iter()
                    .position(|w| w == gun)?;
                state.units[0].moved = moved_attacker;
                state.units[0].pressure = pressure;
                state.units[1].moved = moved_target;
                let w = reg.weapon(&reg.vehicle(&state.units[0].vehicle)?.weapons[weapon_index])?;
                Some(
                    hit_breakdown(
                        reg,
                        &state,
                        UnitId(0),
                        state.units[0].pos,
                        w,
                        UnitId(1),
                        state.units[1].pos,
                        blind,
                    )
                    .total,
                )
            };
            let cell = |v: Option<i32>| match v {
                Some(n) => format!("{n}"),
                None => "-".to_string(),
            };
            print!("{:<24} {:>10}", short(gun), format!("{label} {dist}h"));
            print!(" {:>8}", cell(chance(0, 0, 0, false)));
            for hexes in BOUNDS {
                print!(" {:>8}", cell(chance(hexes, 0, 0, false)));
            }
            print!(" {:>8}", cell(chance(0, TARGET_BOUND, 0, false)));
            for (_, at) in &rungs {
                print!(" {:>10}", cell(chance(0, 0, *at, false)));
            }
            print!(" {:>8}", cell(chance(0, 0, 0, true)));
            println!();
        }
    }
    println!(
        "\n  every cell is `hit_breakdown` against a {TARGET} on open grass,\n  \
         clamped to {}..={}% like the roll it feeds. `moves Nh` is the gun's\n  \
         own vehicle having crossed N hexes this round and `tgt {TARGET_BOUND}h` is the\n  \
         target having done so — the same `moved` field the resolver reads,\n  \
         and the reason a crawl and a dash are priced differently. The crew\n  \
         columns set pressure to that rung's own threshold rather than naming\n  \
         a number here; a mod whose rungs cost no accuracy prints none.",
        reg.balance.min_hit, reg.balance.max_hit,
    );
}

/// The vehicle the tables fire this gun from: sorted, so the choice of
/// platform cannot move between runs.
fn gun_carrier<'r>(reg: &'r DataRegistry, weapon: &str) -> Option<&'r str> {
    let mut carriers: Vec<&String> = reg
        .vehicles
        .values()
        .filter(|v| v.weapons.iter().any(|w| w == weapon))
        .map(|v| &v.id)
        .collect();
    carriers.sort();
    carriers.first().map(|s| s.as_str())
}

/// The penetration gate, gun by gun and round by round: the odds of getting
/// through, which is the first question the whole model asks.
fn penetration_table(reg: &DataRegistry, duels: &mut Duels) {
    heading(&format!(
        "penetration: P(pen) %, square on (obliquity 1.00), scatter ±{}%",
        reg.balance.pen_scatter
    ));
    let vehicles = vehicle_ids(reg);
    print!("{:<26}", "");
    for v in &vehicles {
        print!("{:>12}", short(v));
    }
    println!();

    for (weapon_id, ammo_id) in gun_rounds(reg) {
        let (Some(weapon), Some(ammo)) = (reg.weapon(&weapon_id), reg.ammo(&ammo_id)) else {
            continue;
        };
        let [near, mid, far] = bands(weapon);
        println!(
            "\n{weapon_id} / {ammo_id}  ({}, pen {}→{} over {}, blast {})",
            ammo.class.as_str(),
            ammo.penetration[0],
            ammo.penetration[1],
            reg.scale.format_range(weapon.range),
            ammo.blast,
        );
        for (label, arc, dist) in [
            ("front  near", Arc::Front, near),
            ("front   mid", Arc::Front, mid),
            ("front   max", Arc::Front, far),
            ("side    mid", Arc::Side, mid),
            ("rear    mid", Arc::Rear, mid),
        ] {
            print!("{:<26}", format!("  {label}  {dist} hex"));
            for vehicle in &vehicles {
                match duels.shot(&weapon_id, &ammo_id, vehicle, arc, dist) {
                    // Two numbers where there used to be one, and the second
                    // is the whole of the partial-penetration band: a gun
                    // that gets through eight times in ten and barely on
                    // most of them is a different gun from one that gets
                    // through eight times in ten and eats the target every
                    // time. Suppressed at 100 so the common case — a round
                    // with real margin — still reads as a single figure.
                    Some(shot) if shot.pen_share >= 100 => {
                        print!("{:>12}", shot.pen_chance)
                    }
                    Some(shot) => {
                        print!("{:>12}", format!("{}·{}%", shot.pen_chance, shot.pen_share))
                    }
                    None => print!("{:>12}", "-"),
                }
            }
            println!();
        }
    }
    println!(
        "\n  every cell is `preview_attack`'s own pen chance with that round forced\n  \
         into the racks, so it is the number the AI plans on and the die the\n  \
         resolver throws. `-` means no vehicle in the roster carries that gun.\n  \
         A cell reading `70·62%` gets through seven times in ten and spends\n  \
         only 62% of its budget when it does — it is beating that plate by a\n  \
         margin too thin to do its worst, which is the partial-penetration\n  \
         band. A bare number is a round with room to spare."
    );
    let legacy = legacy_guns(reg);
    if !legacy.is_empty() {
        println!(
            "  not tabulated: {} — no ammunition declared, so they fire the legacy\n  \
             path off the weapon's own numbers.",
            legacy.join(", ")
        );
    }
}

/// One shot's worth of the whole chain, folded into "how many of these does
/// it take".
struct KillChain {
    /// Expected shots fired until the target is out of the fight.
    shots: f32,
    /// The same, in rounds of the battle clock, at this gun's cadence.
    rounds: f32,
}

/// Expected shots to knock out, at mid range, front plate.
///
/// The chain, in the order the engine walks it: hit chance, penetration
/// chance, and then the behind-armor budget — the ledger damage the round
/// carries, spent as one effect roll per `points_per_effect`, each roll
/// picking a cadet or a module weighted by size. Everything up to and
/// including the budget is read off `preview_attack`; what is *modeled* here
/// is the two catastrophes, because they are rolls rather than expectations:
/// a rack hit lighting the racks, and blast overmatching a thin skin. Both
/// are assembled from the real constants and both are checked by the
/// `--sim` kill-cause table.
///
/// What it deliberately leaves out is the crew's nerve. Bail-out is a morale
/// roll and the commonest end of a real tank; the fought-out numbers will
/// therefore always come in *under* this table, and the gap between them is
/// itself worth watching.
fn kill_chain(
    reg: &DataRegistry,
    shot: &AttackPreview,
    facts: Facts,
    weapon: &WeaponDef,
    overmatches: bool,
) -> KillChain {
    let per_effect = reg.balance.points_per_effect.max(1);
    let hit = shot.hit.total as f32 / 100.0;
    let pen = shot.pen_chance as f32 / 100.0;
    let rolls = ((shot.damage.max(1) + per_effect - 1) / per_effect) as f32;
    // The same test `behind_armor_effects` makes: double the price of a roll
    // arriving at once puts a cadet straight out instead of wounding her.
    let savage = shot.damage >= per_effect * 2;

    let interior = facts.interior.max(1) as f32;
    let crew_share = (reg.balance.crew_weight.max(0) as u32 * facts.seats) as f32 / interior;
    // Each roll takes one substance point — a wound, or one module hit — and
    // a savage roll that lands on a cadet takes both of hers at once.
    let per_pen = rolls * (1.0 + if savage { crew_share } else { 0.0 });
    let per_shot = hit * pen * per_pen;

    // The rack, and the fire it starts. `brewup_percent` is scaled by the
    // round's post-pen potency and by how full the racks still are; at the
    // start of a battle they are full, which is the worst case and the one
    // worth tabulating. The potency is recovered from the ledger damage the
    // preview reports over the weapon's own weight, which is exactly how
    // `Round::loaded` built it.
    let rack_found = 1.0 - (1.0 - facts.rack as f32 / interior).powf(rolls);
    let potency = if weapon.damage > 0 {
        shot.damage as f32 / weapon.damage as f32
    } else {
        1.0
    };
    let brew = rack_found * ((reg.balance.brewup_percent.max(0) as f32 * potency) / 100.0).min(1.0);
    // A bounce is not always nothing: blast comfortably over the thinnest
    // plate wrecks the vehicle without consulting the gate at all.
    let catastrophe = hit * (pen * brew + (1.0 - pen) * if overmatches { 1.0 } else { 0.0 });

    let attrition = if per_shot > 0.0 {
        (facts.substance as f32 / per_shot).ceil()
    } else {
        f32::INFINITY
    };
    // E[min(geometric catastrophe, attrition)] — the sum of the odds of her
    // still being in the fight after each shot.
    let shots = if catastrophe > 0.0 {
        let survive = 1.0 - catastrophe;
        if attrition.is_finite() {
            (1.0 - survive.powf(attrition)) / catastrophe
        } else {
            1.0 / catastrophe
        }
    } else {
        attrition
    };
    let cadence = reg.scale.ticks_per_round as f32 / weapon.reload(&reg.scale).max(1) as f32;
    KillChain {
        shots,
        rounds: shots / cadence.max(0.001),
    }
}

fn kill_chain_table(reg: &DataRegistry, duels: &mut Duels) {
    heading("expected shots to knock out — mid range, front plate");
    let vehicles = vehicle_ids(reg);
    print!("{:<26}", "");
    for v in &vehicles {
        print!("{:>12}", short(v));
    }
    println!();

    for (weapon_id, ammo_id) in gun_rounds(reg) {
        let (Some(weapon), Some(ammo)) = (reg.weapon(&weapon_id), reg.ammo(&ammo_id)) else {
            continue;
        };
        let mid = bands(weapon)[1];
        print!("{:<26}", format!("{weapon_id} / {ammo_id}"));
        for vehicle in &vehicles {
            let cell = match (
                duels.shot(&weapon_id, &ammo_id, vehicle, Arc::Front, mid),
                duels.facts(vehicle),
            ) {
                (Some(shot), Some(facts)) => {
                    let overmatch = ammo.blast > 0 && blast_overmatches(ammo.blast, facts.thinnest);
                    let chain = kill_chain(reg, &shot, facts, weapon, overmatch);
                    if chain.shots.is_finite() && chain.shots < 400.0 {
                        format!("{:.1} ({:.1}r)", chain.shots, chain.rounds)
                    } else {
                        "never".to_string()
                    }
                }
                _ => "-".to_string(),
            };
            print!("{cell:>12}");
        }
        println!();
    }
    println!(
        "\n  cell is `shots (rounds of battle time)` for one gun firing that round\n  \
         at its own cadence. Hit, penetration and the effect budget are engine\n  \
         calls; the brew-up and blast-overmatch terms are MODELED from the same\n  \
         constants the resolver rolls against. Bail-out is not modeled at all,\n  \
         so the fought-out battles should come in under this — the gap is the\n  \
         crew's nerve, and `--sim` counts it."
    );
}

/// How long a shell is in the air, which is how much warning its target gets.
fn flight_table(reg: &DataRegistry) {
    let indirect: Vec<(String, String)> = gun_rounds(reg)
        .into_iter()
        .filter(|(w, _)| reg.weapon(w).is_some_and(|w| w.indirect))
        .collect();
    if indirect.is_empty() {
        return;
    }
    heading("shell flight: how much warning the ground gets");
    println!(
        "{:<26} {:>10} {:>8} {:>8} {:>14} {:>12}",
        "gun / round", "range", "ticks", "seconds", "target moves", "spread"
    );
    // What the quickest thing on the field covers while the shell is up: the
    // lead an artillery order has to guess, in the only unit that matters.
    let fastest = reg
        .vehicles
        .values()
        .map(|v| v.movement.points)
        .max()
        .unwrap_or(1);
    for (weapon_id, ammo_id) in indirect {
        let (Some(weapon), Some(ammo)) = (reg.weapon(&weapon_id), reg.ammo(&ammo_id)) else {
            continue;
        };
        for (label, dist) in bands(weapon)
            .into_iter()
            .zip(["near", "mid", "max"])
            .map(|(d, l)| (l, d))
        {
            let ticks = flight_ticks(&reg.scale, ammo.velocity, dist);
            let seconds = reg.scale.seconds(ticks as u32);
            let hexes = fastest as f32 * ticks as f32 / reg.scale.ticks_per_round as f32;
            // The gun's own error, in the same units the mod writes it in:
            // a percentage of the range flown, resolved through the scale
            // into the hexes the resolver will actually displace by.
            let spread = reg.scale.meters(dist) * weapon.dispersion as f32 / 100.0;
            let radius = (spread / reg.scale.hex_meters).round() as i32;
            println!(
                "{:<26} {:>10} {:>8} {:>8} {:>14} {:>12}",
                format!("{weapon_id} / {ammo_id}"),
                format!("{label} {dist}h"),
                ticks,
                format!("{seconds:.0}s"),
                format!("{hexes:.1} hex"),
                if weapon.dispersion == 0 {
                    "exact".to_string()
                } else {
                    format!("{spread:.0} m / {radius} hex")
                },
            );
        }
    }
    println!(
        "\n  a shell is aimed at ground, so `target moves` is how far the quickest\n  \
         vehicle in the roster ({}) travels before it lands — the lead an\n  \
         artillery order has to guess, and the reason standing still is the\n  \
         mistake it historically was. `spread` is the piece's own dispersion\n  \
         at that range, and the hex radius the resolver displaces the impact\n  \
         by: a different fact from the lead, and the one that decides whether\n  \
         a shell aimed at somebody standing perfectly still arrives on her.",
        reg.scale.format_speed(fastest)
    );
}

/// Things worth a second look. Not failures — this game has not decided all of
/// these — but the shapes that usually mean a number is wrong.
fn flags(reg: &DataRegistry, duels: &mut Duels) {
    heading("worth a look");
    let mut said = false;
    let vehicles = vehicle_ids(reg);
    let pairs = gun_rounds(reg);

    // A gun is judged on its best round at its best range, and on its blast
    // as well as its penetration: a 105 that bounces off a medium still
    // wrecks a scout car, and calling that "cannot hurt" would be the same
    // half-truth the pre-ballistics version of this line told.
    let mut guns: Vec<String> = pairs.iter().map(|(w, _)| w.clone()).collect();
    guns.dedup();
    for weapon_id in &guns {
        let Some(weapon) = reg.weapon(weapon_id) else {
            continue;
        };
        let near = bands(weapon)[0];
        let rounds: Vec<String> = pairs
            .iter()
            .filter(|(w, _)| w == weapon_id)
            .map(|(_, a)| a.clone())
            .collect();
        let best_blast = rounds
            .iter()
            .filter_map(|a| reg.ammo(a))
            .map(|a| a.blast)
            .max()
            .unwrap_or(0);
        let mut helpless = Vec::new();
        for vehicle in &vehicles {
            let Some(facts) = duels.facts(vehicle) else {
                continue;
            };
            let best_pen = rounds
                .iter()
                .filter_map(|a| duels.shot(weapon_id, a, vehicle, Arc::Front, near))
                .map(|s| s.pen_chance)
                .max()
                .unwrap_or(0);
            let overmatches = best_blast > 0 && blast_overmatches(best_blast, facts.thinnest);
            if best_pen == 0 && !overmatches {
                helpless.push(vehicle.clone());
            }
        }
        if !helpless.is_empty() {
            println!(
                "  {weapon_id} cannot meaningfully hurt: {}\n    \
                 (no round it chambers gets through the front plate even at {}, and\n    \
                 its heaviest blast {best_blast} does not overmatch their thinnest plate)",
                helpless.join(", "),
                reg.scale.format_distance(near),
            );
            said = true;
        }
    }

    // A vehicle nothing on the field can touch is not a design achievement,
    // it is a battle that cannot end. Every arc, every round, best range.
    for vehicle in &vehicles {
        let Some(facts) = duels.facts(vehicle) else {
            continue;
        };
        let mut reachable = false;
        for (weapon_id, ammo_id) in &pairs {
            let Some(weapon) = reg.weapon(weapon_id) else {
                continue;
            };
            let near = bands(weapon)[0];
            let blast = reg.ammo(ammo_id).map(|a| a.blast).unwrap_or(0);
            if blast > 0 && blast >= facts.thinnest * 2 {
                reachable = true;
                break;
            }
            for arc in [Arc::Front, Arc::Side, Arc::Rear] {
                if duels
                    .shot(weapon_id, ammo_id, vehicle, arc, near)
                    .is_some_and(|s| s.pen_chance > 0)
                {
                    reachable = true;
                    break;
                }
            }
            if reachable {
                break;
            }
        }
        if !reachable {
            println!(
                "  NOTHING in the roster can hurt {vehicle} from any arc with any round — \
                 she cannot be killed"
            );
            said = true;
        }
    }

    // A round no gun lists is content nobody will ever see fired.
    let mut orphans: Vec<&String> = reg
        .ammo
        .keys()
        .filter(|id| {
            !reg.weapons
                .values()
                .any(|w| w.ammo.iter().any(|a| &a == id))
        })
        .collect();
    orphans.sort();
    for ammo in orphans {
        println!("  {ammo} is declared but no weapon can chamber it");
        said = true;
    }

    // A vehicle that outranges its own eyes needs a spotter, which is a
    // deliberate design point for gun tanks and an accident anywhere else.
    for vehicle in &vehicles {
        let Some(veh) = reg.vehicle(vehicle) else {
            continue;
        };
        let reach = veh
            .weapons
            .iter()
            .filter_map(|w| reg.weapon(w))
            .map(|w| w.range[1])
            .max()
            .unwrap_or(0);
        if reach > veh.vision_range {
            println!(
                "  {vehicle} shoots {} but sees {} — needs a spotter",
                reg.scale.format_distance(reach as i32),
                reg.scale.format_distance(veh.vision_range as i32),
            );
            said = true;
        }
    }
    if !said {
        println!("  nothing stood out");
    }
}

// --- the simulated pass ---------------------------------------------------

/// Every battle map the loaded mods ship, sorted by id.
///
/// The fought-out pass used to play every battle on `river_crossing`, which
/// meant every conclusion it reached was a conclusion about one river, one
/// town and two fords. A gun that only ever gets its range on open ground and
/// a scout that only earns her keep in timber both looked like flat balance
/// facts. Sampling the roster instead spreads the sample over the shapes the
/// game actually ships, so a number that is only good on one of them shows up
/// as variance rather than as truth.
///
/// Sorted because a `HashMap`'s iteration order is not a decision, and the
/// whole table has to be reproducible from the seed.
fn battle_maps(reg: &DataRegistry) -> Vec<&str> {
    let mut ids: Vec<&str> = reg
        .maps
        .values()
        .filter(|m| m.kind == MapKind::Battle)
        .map(|m| m.id.as_str())
        .collect();
    ids.sort_unstable();
    assert!(!ids.is_empty(), "no battle maps to fight on");
    ids
}

/// Where each table's battles start counting, before `--seed` is added.
///
/// Distinct blocks so that two tables in one run never fight literally the
/// same battle and then agree with each other for that reason. The values are
/// the constants each table was born with, kept exactly so that `--seed 0` —
/// the default — is the run every number this project has quoted was measured
/// on. `--seed N` shifts all four together, which is what makes sweeping the
/// seed a sample of the whole report rather than of one table in it.
const FOUGHT_SEED: u64 = 1000;
const DELEGATION_SEED: u64 = 1000;
const MUSTER_SEED: u64 = 4000;
/// The skill-gap table's own block is zero: `arena_duel` adds its 9000 itself,
/// and moving that would change the planner seeds rather than only the ground.
const ARENA_SEED: u64 = 0;
const GROUND_SEED: u64 = 7000;

/// Which ground this battle is fought on. Keyed on the seed rather than the
/// game index so the pairing of map to battle survives changing `--games`.
fn map_for<'a>(maps: &[&'a str], seed: u64) -> &'a str {
    maps[seed as usize % maps.len()]
}

/// Fight a batch of battles and account for all of them together.
fn fight_out(reg: &DataRegistry, games: usize, seed: u64) -> Tally {
    let maps = battle_maps(reg);
    let fought = fight_all(reg, games, |reg, game| {
        fight_one(reg, &maps, FOUGHT_SEED + seed + game)
    });
    let mut t = Tally::default();
    for one in &fought {
        t.merge(one);
    }
    t
}

fn simulate(reg: &DataRegistry, games: usize, seed: u64) {
    let maps = battle_maps(reg);
    heading(&format!(
        "fought out: {games} battles across {}",
        maps.join(", ")
    ));
    report(&fight_out(reg, games, seed), games);
}

/// Fight one battle and account for it on its own.
///
/// Split out of the fought-out pass so that pass can go across the cores like
/// every other table here, and so a sweep can schedule its variants and its
/// battles as one flat list of jobs. A tally is per battle and the tallies are
/// folded in seed order afterwards, which is what keeps the printed numbers
/// independent of which core finished first — see [`fight_all`].
fn fight_one(reg: &DataRegistry, maps: &[&str], seed: u64) -> Tally {
    let mut t = Tally::default();
    let mut state = BattleState::from_map(reg, map_for(maps, seed), seed).expect("battle");
    let mut ai = AiDriver::new();
    ai.insert(0, planner(reg, seed, "massed_armor"));
    ai.insert(1, planner(reg, seed + 1, "elastic_defense"));
    let mut rounds = 0;
    let mut last_hit: HashMap<UnitId, String> = HashMap::new();
    // Final state per cadet, so a cadet wounded and then killed is counted
    // once, as killed.
    let mut cadets: BTreeMap<CadetId, bool> = BTreeMap::new();
    // Which crews have been found at all yet, so a contact broken and remade
    // is not counted as a second crew coming out of hiding.
    let mut found: HashSet<UnitId> = HashSet::new();
    for unit in &state.units {
        for (id, count) in &unit.ammo {
            *t.ammo_aboard.entry(id.clone()).or_default() += count;
        }
        if unit.troops(reg).is_some() {
            t.foot_fielded += 1;
        }
    }
    while !state.is_over() && rounds < 60 {
        ai.plan_round(reg, &mut state);
        rounds += 1;
        // A shell announces itself and then, if anybody was standing on
        // the ground it came down on, the ordinary vocabulary follows it
        // immediately. Watching the next event is how artillery gets
        // credited without the resolver having to say so twice.
        let mut shell_from: Option<UnitId> = None;
        {
            // A census at the top of every round: how deep did anybody stack?
            // Taken from state rather than from events because sharing a hex is
            // a *state* and nothing announces it.
            let mut per_hex: BTreeMap<(i32, i32), usize> = BTreeMap::new();
            for unit in state
                .units
                .iter()
                .filter(|u| u.alive() && u.aboard.is_none())
            {
                *per_hex.entry((unit.pos.x, unit.pos.y)).or_default() += 1;
            }
            let deepest = per_hex.values().copied().max().unwrap_or(0);
            if deepest > 1 {
                t.stacked_rounds += 1;
            }
            t.deepest_stack = t.deepest_stack.max(deepest);
        }
        // Resolved a tick at a time rather than through `resolve_round`,
        // which is exactly this loop — so the simulation is untouched — in
        // order to have a *state* to read when a contact is announced. How
        // far away a crew was when somebody finally picked her out is the
        // number the detection rules are really about, and an event stream
        // drained at the end of the round cannot answer it: by then everyone
        // has driven on. Read at the end of the tick the contact was made
        // in, which is within one tick's driving of where the spotter stood
        // when she made it.
        let mut round_events = Vec::new();
        while !state.is_over() && state.resolving_tick().is_some() {
            let ticked = state.step_tick(reg);
            for event in &ticked {
                if let Event::UnitSpotted { by_side, at, .. } = event {
                    let range = state
                        .units
                        .iter()
                        .filter(|u| u.alive() && u.aboard.is_none() && u.side == *by_side)
                        .map(|u| u.pos.distance_to(*at))
                        .min();
                    if let Some(hexes) = range {
                        t.contact_range.push(hexes as u32);
                    }
                }
            }
            round_events.extend(ticked);
        }
        for event in round_events {
            let landed = std::mem::take(&mut shell_from);
            match event {
                Event::ShotFired { moving, blind, .. } => {
                    t.shots += 1;
                    if moving {
                        t.shots_on_the_move += 1;
                    }
                    if blind {
                        t.shots_blind += 1;
                    }
                }
                Event::UnitSpotted { unit, .. } => {
                    t.spots += 1;
                    // The round THIS crew was first found in, once per crew
                    // and not once per battle. `rounds` was incremented above
                    // before the round was resolved, so it already reads as
                    // this round's number.
                    if found.insert(unit) {
                        t.found_at.push(rounds);
                    }
                }
                Event::ShellLanded { attacker, .. } => {
                    t.shells += 1;
                    shell_from = Some(attacker);
                }
                Event::ShotHit {
                    attacker,
                    target,
                    facing,
                    ..
                } => {
                    t.hits += 1;
                    *t.hits_by_arc.entry(format!("{facing:?}")).or_default() += 1;
                    if landed == Some(attacker) {
                        t.shells_on_target += 1;
                    }
                    // Remembered so a kill can be credited: `UnitDestroyed`
                    // says who died, not who did it, because death is
                    // reaped at the end of a tick and may have several
                    // contributors.
                    if let Some(a) = state.units.get(attacker.index()) {
                        last_hit.insert(target, a.vehicle.clone());
                    }
                }
                Event::ShotMissed { .. } => t.misses += 1,
                Event::ShotStrayed { .. } => t.strays += 1,
                Event::ShotBounced { attacker, .. } => {
                    t.bounces += 1;
                    if landed == Some(attacker) {
                        t.shells_on_target += 1;
                        t.shells_bounced += 1;
                    }
                }
                Event::CrewHit { cadet, out, .. } => {
                    let entry = cadets.entry(cadet).or_insert(false);
                    *entry |= out;
                }
                Event::Mounted { .. } => t.mounts += 1,
                Event::Dismounted { .. } => t.dismounts += 1,
                Event::ModuleHit {
                    module, destroyed, ..
                } => {
                    if destroyed
                        && reg
                            .module(&module)
                            .is_some_and(|m| m.effect == ModuleEffect::Ammo)
                    {
                        t.racks_destroyed += 1;
                    }
                }
                Event::UnitDestroyed { unit, .. } => {
                    if let Some(u) = state.units.get(unit.index()) {
                        *t.deaths.entry(u.vehicle.clone()).or_default() += 1;
                        // Reap carries the destruction into `Fate::Destroyed`
                        // rather than discarding it, so the cause of death is
                        // readable exactly here — and it is now one value
                        // with one name, instead of three flags read in a
                        // precedence this table had to know about. A hull
                        // that took two ends inside one tick is remembered by
                        // whichever `Destruction::supersedes` kept.
                        let cause = match u.destruction() {
                            Some(Destruction::BrewedUp) => "brewed",
                            Some(Destruction::Crushed) => "wrecked by blast",
                            Some(Destruction::Abandoned) => "abandoned",
                            Some(Destruction::CrewSpent) | None => "crew out",
                        };
                        *t.causes.entry(cause).or_default() += 1;
                    }
                    if let Some(killer) = last_hit.get(&unit) {
                        *t.kills.entry(killer.clone()).or_default() += 1;
                    }
                }
                _ => {}
            }
        }
    }
    t.rounds.push(rounds);
    for out in cadets.values() {
        if *out {
            t.girls_out += 1;
        } else {
            t.girls_wounded += 1;
        }
    }
    // What is still in the racks when the shooting stops. Everything
    // missing either went downrange or burned with the rack that held
    // it, and those two are not separable from outside the engine —
    // which is what the caveat under the table says.
    for unit in &state.units {
        for (id, count) in &unit.ammo {
            *t.ammo_left.entry(id.clone()).or_default() += count;
        }
    }
    // Survivors only, and `surviving_units` rather than `alive`: a
    // platoon that drove off by an exit came home with whatever strength
    // she had left, and counting her as a loss would be exactly the lie
    // the engine is careful not to tell.
    for unit in state.surviving_units() {
        if let Some((have, total)) = unit.troops(reg)
            && total > 0
        {
            t.troops_left.push(have as f32 / total as f32);
        }
    }
    match state.over.map(|r| (r.winner, r.reason)) {
        Some((Some(w), _)) => {
            *t.wins
                .entry(state.sides[w as usize].name.clone())
                .or_default() += 1;
        }
        Some((None, EndReason::Stalemate)) => {
            t.stalemates += 1;
            t.draws += 1;
        }
        _ => t.draws += 1,
    }
    t
}

/// Everything the fought-out pass has to say, once the battles are in.
fn report(t: &Tally, games: usize) {
    let mean = t.rounds.iter().sum::<u32>() as f32 / t.rounds.len().max(1) as f32;
    println!("  outcome:");
    let mut names: Vec<_> = t.wins.keys().cloned().collect();
    names.sort();
    for name in names {
        println!("    {:<22} {} wins", name, t.wins[&name]);
    }
    println!(
        "    {:<22} {} ({} by stalemate)",
        "draws", t.draws, t.stalemates
    );
    println!(
        "  length: {mean:.1} rounds mean, {} shortest, {} longest",
        t.rounds.iter().min().copied().unwrap_or(0),
        t.rounds.iter().max().copied().unwrap_or(0)
    );
    // How long it takes to find somebody, which is what the detection roll
    // is for and the one thing a length figure cannot tell you: a battle can
    // be short because it was decided quickly or because it started at once.
    if !t.found_at.is_empty() {
        println!(
            "  contact: a crew is first found in round {:.1} mean, {} acquisition(s) \
             per battle",
            t.found_at.iter().sum::<u32>() as f32 / t.found_at.len() as f32,
            t.spots as usize / t.rounds.len().max(1),
        );
        println!(
            "    made at {:.1} hexes from the nearest crew of the finding side, mean \
             of {}",
            t.contact_range.iter().sum::<u32>() as f32 / t.contact_range.len().max(1) as f32,
            t.contact_range.len(),
        );
    }
    if t.shots > 0 {
        // Four outcomes now, not two: a shot that hits and a shot that gets
        // through are different events, and the gap between them is the
        // whole penetration gate. The remainder is shells that came down on
        // ground nobody was standing on.
        let elsewhere = t.shots.saturating_sub(t.hits + t.bounces + t.misses);
        println!(
            "  gunnery: {} shots — {} penetrated ({:.0}%), {} bounced, {} missed, {} \
             fell on empty ground",
            t.shots,
            t.hits,
            100.0 * t.hits as f32 / t.shots as f32,
            t.bounces,
            t.misses,
            elsewhere,
        );
        println!(
            "    {} of those shots ({:.0}%) were laid from a vehicle under way, {} \
             ({:.0}%) at a map\n    reference nobody could see",
            t.shots_on_the_move,
            100.0 * t.shots_on_the_move as f32 / t.shots as f32,
            t.shots_blind,
            100.0 * t.shots_blind as f32 / t.shots as f32,
        );
        println!(
            "    {} of the {} misses ({:.0}%) still found somebody sharing the target's\n    \
             hex; crews shared ground in {} round(s) and the deepest stack was {}",
            t.strays,
            t.misses,
            100.0 * t.strays as f32 / t.misses.max(1) as f32,
            t.stacked_rounds,
            t.deepest_stack,
        );
    }
    let mut arcs: Vec<_> = t.hits_by_arc.iter().collect();
    arcs.sort();
    if !arcs.is_empty() {
        let total: u32 = t.hits_by_arc.values().sum();
        print!("  hits by arc:");
        for (arc, n) in arcs {
            print!(" {arc} {:.0}%", 100.0 * *n as f32 / total as f32);
        }
        println!();
    }
    if !t.deaths.is_empty() {
        println!("  {:<18} {:>6} {:>6}", "vehicle", "kills", "losses");
        let mut names: Vec<&String> = t.deaths.keys().chain(t.kills.keys()).collect();
        names.sort();
        names.dedup();
        for vehicle in names {
            println!(
                "    {vehicle:<16} {:>6} {:>6}",
                t.kills.get(vehicle).copied().unwrap_or(0),
                t.deaths.get(vehicle).copied().unwrap_or(0)
            );
        }
    }

    if t.foot_fielded > 0 {
        let mean = t.troops_left.iter().sum::<f32>() / t.troops_left.len().max(1) as f32;
        println!(
            "  infantry: {} foot units fielded across the run, {} of them still on \
             the field\n    at the bell with {:.0}% of their rifles left on average; \
             {} mounted and {} dismounted",
            t.foot_fielded,
            t.troops_left.len(),
            100.0 * mean,
            t.mounts,
            t.dismounts,
        );
    }

    // What actually killed them. The single most useful line in this report
    // after the outcome, because it says which half of the model is doing
    // the work: fires, blast, nerve, or the cadets themselves.
    let total_deaths: usize = t.causes.values().sum();
    if total_deaths > 0 {
        print!("  killed by:");
        for (cause, n) in &t.causes {
            print!(
                " {cause} {n} ({:.0}%)",
                100.0 * *n as f32 / total_deaths as f32
            );
        }
        println!();
    }
    println!(
        "  crew cost: {:.1} cadets wounded and {:.1} out per battle ({} and {} across the\n    \
         run, by her state at the end — a cadet wounded and then killed is counted\n    \
         once, as out)",
        t.girls_wounded as f32 / games.max(1) as f32,
        t.girls_out as f32 / games.max(1) as f32,
        t.girls_wounded,
        t.girls_out,
    );

    if !t.ammo_aboard.is_empty() {
        println!("  ammunition per battle (expended or lost with the rack):");
        println!(
            "    {:<12} {:>10} {:>10} {:>8}",
            "round", "aboard", "gone", "share"
        );
        for (id, aboard) in &t.ammo_aboard {
            let left = t.ammo_left.get(id).copied().unwrap_or(0);
            let gone = aboard.saturating_sub(left);
            let per = gone as f32 / games.max(1) as f32;
            let aboard_per = *aboard as f32 / games.max(1) as f32;
            println!(
                "    {:<12} {:>10.0} {:>10.1} {:>7.0}%",
                id,
                aboard_per,
                per,
                if *aboard > 0 {
                    100.0 * gone as f32 / *aboard as f32
                } else {
                    0.0
                },
            );
        }
        println!(
            "    {} ammunition racks were destroyed across the run, and a wrecked\n    \
             rack zeroes what was still in it — so `gone` is rounds fired plus\n    \
             rounds lost, not rounds fired.",
            t.racks_destroyed
        );
    }

    if t.shells > 0 {
        println!(
            "  artillery: {} shells landed, {} on an occupied hex ({:.0}%) — {} through, {} bounced",
            t.shells,
            t.shells_on_target,
            100.0 * t.shells_on_target as f32 / t.shells as f32,
            t.shells_on_target - t.shells_bounced,
            t.shells_bounced,
        );
        println!(
            "    a shell is aimed at ground ticks before it arrives, so the rest\n    \
             landed on an empty hex and paid out only in blast on the neighbours."
        );
    }

    // Outcomes worth noticing, in the same spirit as the analytic flags.
    let stalemate_rate = t.stalemates as f32 / games.max(1) as f32;
    if stalemate_rate > 0.3 {
        println!(
            "\n  NOTE {:.0}% of battles ended in stalemate — the sides are losing each \
             other rather than fighting. Worth looking at vision, map size, or the \
             stalemate timeout before reading anything else here as balance.",
            100.0 * stalemate_rate
        );
    }
}

fn planner(
    reg: &DataRegistry,
    seed: u64,
    doctrine: &str,
) -> Box<dyn AiPlanner<BattleState, Order>> {
    planner_with(reg, seed, "utility", doctrine)
}

fn planner_with(
    reg: &DataRegistry,
    seed: u64,
    kind: &str,
    doctrine: &str,
) -> Box<dyn AiPlanner<BattleState, Order>> {
    make_battle_planner(
        &AiConfig {
            planner: kind.into(),
            difficulty: 3,
            doctrine: Some(doctrine.into()),
        },
        seed,
        reg,
    )
}

/// What a side pays for fighting through its chain of command instead of
/// micromanaging every vehicle. The opponent is held constant — flat utility
/// — and one side at a time switches to the `command` planner, so the drop
/// in its wins is attributable to delegation and nothing else.
///
/// This table exists because delegation is the campaign's default posture:
/// the player is meant to rely on missions, and a delegation that costs
/// battles is one they will rightly refuse to pay for. Zero is the target;
/// the design doc's build order drives it there across chunks 2 through 7.
///
/// It also carries the infantry-employment columns, and they belong here
/// rather than beside the flat run's infantry line because employing
/// infantry is a *commander's* job. The flat pairing is the control: the
/// same forces on the same ground with nobody assigning missions. If the
/// brain has learned anything about what a grenadier section is for, it
/// shows as fewer taxis and platoons lost in the rows below the control
/// than in the control itself.
/// What one battle contributed to the delegation table.
///
/// Every field is a count, including `rounds`, which is a total rather than a
/// mean so that folding stays addition — the mean is taken once, at the end,
/// where the number of battles is known. A mean folded into a mean would be a
/// different number depending on how the battles were split across cores.
#[derive(Default, Clone, Copy)]
struct Delegation {
    wins_side0: usize,
    wins_side1: usize,
    draws: usize,
    rounds: u32,
    taxis_lost: usize,
    foot_lost: usize,
    foot_shots: usize,
}

impl Delegation {
    fn merge(&mut self, o: &Self) {
        self.wins_side0 += o.wins_side0;
        self.wins_side1 += o.wins_side1;
        self.draws += o.draws;
        self.rounds += o.rounds;
        self.taxis_lost += o.taxis_lost;
        self.foot_lost += o.foot_lost;
        self.foot_shots += o.foot_shots;
    }
}

/// Read off the chassis rather than named: a taxi is anything that lifts
/// somebody and a foot unit is anything that walks, so a mod's own transports
/// and its own infantry land in these columns unasked.
fn is_taxi(reg: &DataRegistry, unit: &tactics_core::battle::Unit) -> bool {
    reg.vehicle(&unit.vehicle).is_some_and(|v| v.capacity > 0)
}

fn is_afoot(reg: &DataRegistry, unit: &tactics_core::battle::Unit) -> bool {
    reg.vehicle(&unit.vehicle)
        .is_some_and(|v| v.movement.class == tactics_core::data::MovementClass::Foot)
}

fn delegation_battle(
    reg: &DataRegistry,
    maps: &[&str],
    seed: u64,
    p0: &str,
    p1: &str,
    d0: &str,
) -> Delegation {
    let mut d = Delegation::default();
    let mut state = BattleState::from_map(reg, map_for(maps, seed), seed).expect("battle");
    let mut ai = AiDriver::new();
    ai.insert(0, planner_with(reg, seed, p0, d0));
    ai.insert(1, planner_with(reg, seed + 1, p1, "elastic_defense"));
    let mut rounds = 0;
    while !state.is_over() && rounds < 60 {
        ai.plan_round(reg, &mut state);
        for event in &state.resolve_round(reg) {
            if let Event::ShotFired { attacker, .. } = event
                && state
                    .units
                    .get(attacker.index())
                    .is_some_and(|u| u.side == 0 && is_afoot(reg, u))
            {
                d.foot_shots += 1;
            }
        }
        rounds += 1;
    }
    d.rounds = rounds;
    // Side 0 only, always — the side whose planner the middle row swaps.
    // Counting both sides would average the commanded force together with its
    // flat opponent and hide exactly the difference the table is asking about.
    d.taxis_lost = state
        .lost_units()
        .filter(|u| u.side == 0 && is_taxi(reg, u))
        .count();
    d.foot_lost = state
        .lost_units()
        .filter(|u| u.side == 0 && is_afoot(reg, u))
        .count();
    match state.over.and_then(|r| r.winner) {
        Some(0) => d.wins_side0 += 1,
        Some(1) => d.wins_side1 += 1,
        _ => d.draws += 1,
    }
    d
}

fn delegation_tax(reg: &DataRegistry, games: usize, seed: u64) -> Grid {
    let maps = battle_maps(reg);
    let run = |p0: &str, p1: &str, d0: &str| -> Delegation {
        let fought = fight_all(reg, games, |reg, game| {
            delegation_battle(reg, &maps, DELEGATION_SEED + seed + game, p0, p1, d0)
        });
        let mut total = Delegation::default();
        for one in &fought {
            total.merge(one);
        }
        total
    };
    let per = games.max(1) as f64;
    let rows = [
        ("massed flat", run("utility", "utility", "massed_armor")),
        (
            "massed under command",
            run("command", "utility", "massed_armor"),
        ),
        (
            "elastic under command",
            run("utility", "command", "massed_armor"),
        ),
        // The doctrine that orders a *movement to contact*, and the reason it
        // is here. Every row above it fights an `Assault` or nothing at all,
        // because massed armour's aggression is 0.85 and both of the other
        // shipped doctrines devolve — so `planner.pull_under_fire`, the term
        // this project diagnosed as "the whole problem in miniature", was
        // unreachable in AI-versus-AI play and swept to bit-identical across
        // 576 battles. These two rows are the first ones in this instrument
        // that meet it.
        (
            "bounding flat",
            run("utility", "utility", "bounding_overwatch"),
        ),
        (
            "bounding under command",
            run("command", "utility", "bounding_overwatch"),
        ),
    ]
    .into_iter()
    .map(|(name, t)| {
        (
            name.to_string(),
            vec![
                t.wins_side0 as f64,
                t.wins_side1 as f64,
                t.draws as f64,
                t.rounds as f64 / per,
                t.taxis_lost as f64,
                t.foot_lost as f64,
                t.foot_shots as f64,
            ],
        )
    })
    .collect();

    Grid {
        title: format!(
            "delegation tax: {games} battles per pairing across {}, opponent held constant",
            maps.join(", ")
        ),
        preamble: Vec::new(),
        row_head: "pairing",
        columns: vec![
            col("side 0", 0),
            col("side 1", 0),
            col("draws", 0),
            col("rounds", 1),
            col("taxis", 0),
            col("platoons", 0),
            col("shots", 0),
        ],
        rows,
        note: format!(
            "\n  a side's tax is its win drop against the same flat opponent when it\n  \
             fights through missions instead; zero is the target.\n\n  \
             the last three columns are always side 0's — carriers lost, foot units\n  \
             lost, and rounds fired by anybody on their feet — so a `under command`\n  \
             row is the commanded force and the `flat` row above it is the same\n  \
             force fighting for itself. side 1 is elastic defence throughout.\n\
             {}",
            level_note(games)
        ),
    }
}

/// Does skill win cleanly? Identical forces, identical doctrine, and the only
/// thing that differs is how well each side executes.
///
/// This was a standalone `skillgap` example until the instruments were
/// rebuilt around the kill chain; it lives here because one question about
/// the data deserves one instrument, and because its answer has to be read
/// beside the kill causes and the ammunition economy rather than in another
/// terminal. Every pairing is mirrored — (5,3) and (3,5) — so a result that
/// is really about which side of the river a force deploys on cannot be
/// mistaken for a result about skill.
///
/// The bar, written down by the ballistics doc before B4 starts: a
/// difficulty-5 side against difficulty-1 with equal forces should win most
/// battles at a loss ratio visibly better than 1:2. Skill should buy
/// *cleanliness*, not merely wins.
/// A mirror-symmetric arena for the skill-gap study: identical forces on
/// identical ground, because river_crossing's two sides field different
/// vehicles and the first version of this table measured the map more than
/// the players — side B won every pairing regardless of who was smarter.
/// Forest belts at equal distance from both edges give cover texture, and a
/// single held objective in the middle forces contact the way the shipped
/// scenarios do: two campers on a featureless field would stalemate and the
/// table would measure patience.
/// Two commanders, one budget, and whatever each of them thinks that buys.
///
/// The other tables all fight a *given* order of battle — the map's, or a
/// mirrored one written into this file — which measures how well a doctrine
/// fights the army somebody else handed it. This one asks the question the
/// content actually needs answered: given the same points, does what a
/// doctrine chooses to bring beat what another one chooses to bring?
///
/// Every pairing is fought both ways round on the same mirrored ground, so a
/// result cannot be about which end of the arena a force deploys on — the
/// lesson the skill-gap table learned the hard way. Each side both picks and
/// fights under its own doctrine, because the two halves are the same
/// commander and separating them would measure nothing anybody can act on.
///
/// **Read a lopsided row as a question about `cost`, not as a verdict on a
/// doctrine.** `force::muster` deliberately pays the asking price rather than
/// hunting for value, so a doctrine that wins reliably here is one whose
/// preferred hardware is underpriced — which is exactly the signal this table
/// exists to raise, and exactly what a shopping algorithm that optimised for
/// value-per-point would have hidden.
/// What one mustered-arena battle contributed. Counts, so folding is addition
/// — see [`Delegation`], which says why that matters here.
#[derive(Default, Clone, Copy)]
struct Muster {
    a_wins: usize,
    b_wins: usize,
    draws: usize,
    rounds: u32,
    a_points_lost: i32,
}

fn muster_battle(
    reg: &DataRegistry,
    arena: &Arena,
    seed: u64,
    flip: bool,
    a: &(&str, Vec<String>),
    b: &(&str, Vec<String>),
) -> Muster {
    let mut m = Muster::default();
    let (west, east) = if flip { (&b.1, &a.1) } else { (&a.1, &b.1) };
    let (west_doctrine, east_doctrine) = if flip { (b.0, a.0) } else { (a.0, b.0) };
    let Some(mut state) = muster_arena(reg, arena, seed, west, east) else {
        return m;
    };
    let mut ai = AiDriver::new();
    ai.insert(0, planner(reg, seed, west_doctrine));
    ai.insert(1, planner(reg, seed + 1, east_doctrine));
    let mut rounds = 0;
    while !state.is_over() && rounds < 60 {
        ai.plan_round(reg, &mut state);
        state.resolve_round(reg);
        rounds += 1;
    }
    m.rounds = rounds;
    // A is whichever end she deployed on this game, so the tallies follow the
    // flip rather than the side number.
    let a_side = if flip { 1u8 } else { 0u8 };
    m.a_points_lost = state
        .lost_units()
        .filter(|u| u.side == a_side)
        .filter_map(|u| reg.vehicle(&u.vehicle))
        .map(|v| v.cost)
        .sum::<i32>();
    match state.over.and_then(|r| r.winner) {
        Some(s) if s == a_side => m.a_wins += 1,
        Some(_) => m.b_wins += 1,
        None => m.draws += 1,
    }
    m
}

fn mustered_forces(
    reg: &DataRegistry,
    arena: &Arena,
    games: usize,
    budget: i32,
    seed: u64,
) -> Grid {
    let doctrines = [
        "massed_armor",
        "elastic_defense",
        "recon_pull",
        "bounding_overwatch",
    ];
    let forces: Vec<(&str, Vec<String>)> = doctrines
        .iter()
        .filter_map(|id| {
            let doctrine = reg.doctrine(id)?;
            Some((*id, force::muster(reg, doctrine, budget)))
        })
        .collect();

    // The budget goes in the preamble rather than the title because a sweep
    // can move it, and a comparison prints one title over every variant: a
    // heading claiming "60 points a side" above a `points=100` row would be
    // the table lying about its own axis. A preamble that differs is printed
    // per variant, which is exactly what this needs.
    let mut preamble = vec![format!("  {budget} points a side. the shopping lists:")];
    for (id, army) in &forces {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for vehicle in army {
            *counts.entry(vehicle.as_str()).or_default() += 1;
        }
        preamble.push(format!(
            "    {:<16} {:>3} pts, {} units — {}",
            id,
            force::cost_of(reg, army),
            army.len(),
            counts
                .iter()
                .map(|(v, n)| format!("{n}x {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let per = games.max(1) as f64;
    let mut rows = Vec::new();
    for (i, a) in forces.iter().enumerate() {
        for b in forces.iter().skip(i + 1) {
            // Both orientations, alternating game by game: the arena is
            // mirror-symmetric but resolution order is not, and side B has
            // held a measured edge on this ground since the plateau rule
            // went in.
            let fought = fight_all(reg, games, |reg, game| {
                muster_battle(reg, arena, MUSTER_SEED + seed + game, game % 2 == 1, a, b)
            });
            let mut t = Muster::default();
            for one in &fought {
                t.a_wins += one.a_wins;
                t.b_wins += one.b_wins;
                t.draws += one.draws;
                t.rounds += one.rounds;
                t.a_points_lost += one.a_points_lost;
            }
            rows.push((
                format!("{} vs {}", a.0, b.0),
                vec![
                    t.a_wins as f64,
                    t.b_wins as f64,
                    t.draws as f64,
                    t.rounds as f64 / per,
                    t.a_points_lost as f64 / per,
                ],
            ));
        }
    }

    Grid {
        title: format!(
            "mustered forces: {games} battles per pairing, each doctrine buying its own army"
        ),
        preamble,
        row_head: "pairing (A vs B)",
        columns: vec![
            col("A won", 0),
            col("B won", 0),
            col("draws", 0),
            col("rounds", 1),
            col("A pts lost", 1),
        ],
        rows,
        note: format!(
            "\n  each pairing alternates ends game by game, so neither column is a\n  \
             statement about deployment. `A pts lost` is the requisition value of\n  \
             A's dead per battle — what the win cost, in the same currency the\n  \
             army was bought with.\n{}",
            level_note(games)
        ),
    }
}

/// The skill-gap arena, filled with two bought armies instead of the mirrored
/// four. Same ground, different orders of battle — which is the whole point,
/// and why the placement is separate rather than a parameter: the skill-gap
/// table's four-unit spacing is baked into numbers already recorded in the
/// log, and must not move because this section wanted a ninth slot.
fn muster_arena(
    reg: &DataRegistry,
    arena: &Arena,
    seed: u64,
    west: &[String],
    east: &[String],
) -> Option<BattleState> {
    let map = arena.map()?;
    let mut placements = Vec::new();
    // Deliberately dense, and deliberately the same shape for both armies:
    // these forces differ in size — that is the interesting part — and a
    // spacing rule that scaled with the count would hand the smaller army more
    // room as a hidden bonus. Every hex is one of `arena_deployment`'s mirror
    // pairs walked outward, so the n-th vehicle of one army stands exactly
    // where the n-th of the other's reflection would.
    let pairs = muster_deployment(arena, west.len().max(east.len()));
    for (side, army) in [(0u8, west), (1u8, east)] {
        for (i, vehicle) in army.iter().enumerate() {
            let Some((w, e)) = pairs.get(i) else { continue };
            placements.push(UnitPlacement {
                aboard_at: None,
                at: tactics_core::hex_to_offset(if side == 0 { *w } else { *e }),
                side,
                vehicle: vehicle.clone(),
                crew: Vec::new(),
                name: Some(format!("{vehicle} {i}")),
                facing: None,
                formation: None,
                leads: false,
            });
        }
    }
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
    let (roster, crews) = Roster::stamp_for(reg, &placements);
    Some(
        BattleState::from_placements(
            reg,
            map,
            sides,
            &placements,
            &crews,
            std::sync::Arc::new(roster),
            seed,
        )
        .expect("the staged placements are content the base mod ships"),
    )
}

/// Enough mirror-paired standing room for an army of `wanted` vehicles.
///
/// Walks outward from the arena's own deployment line: the four hexes of
/// `arena_deployment`, then the ring of hexes one step further from the middle,
/// and so on. Sorted at every step so the order is a property of the geometry
/// and not of a hash.
fn muster_deployment(arena: &Arena, wanted: usize) -> Vec<(Hex, Hex)> {
    let centre = arena.centre_hex();
    let mut out = arena.deployment();
    // One step outside the arena's own forming-up ring, whatever that ring is.
    // Derived rather than written down, so that an arena which forms up further
    // out does not silently start this walk inside its own line.
    let mut ring = out
        .iter()
        .map(|(w, _)| w.distance_to(centre))
        .max()
        .unwrap_or(0)
        + 1;
    while out.len() < wanted && ring <= arena.radius as i32 {
        let mut wests: Vec<Hex> = (-(ring)..=ring)
            .map(|k| centre + Hex::new(-ring, k))
            .filter(|h| h.distance_to(centre) <= arena.radius as i32)
            .collect();
        wests.sort_by_key(|h| (h.y, h.x));
        for w in wests {
            if out.iter().all(|(a, _)| *a != w) {
                out.push((w, arena.mirror(w)));
            }
        }
        ring += 1;
    }
    out
}

// --- the arena ------------------------------------------------------------
//
// The skill-gap and brains tables want ground that is worth nothing to either
// side, because they are measuring execution and a ground advantage in the
// arena is a confound rather than a result. That is a harder thing to build
// than it looks, and the first version of this got it wrong in a way nobody
// noticed for months: it was written as 25x13 rows of ASCII with forest at
// columns 8 and 16, which is symmetric *as text*, and the odd-r offset
// conversion shears text into hexes. Measured on the map it actually produced,
// side A had nine forest hexes within six of its deployment and side B had
// four, and side B went on to win 56.6% of 1152 equal-skill battles.
//
// So the arena is now built in hex space, and its symmetry is structural
// rather than checked: a hexagon is closed under point reflection through its
// own centre for the same reason a circle is, every feature is declared once
// and mirrored, and `tests/arena.rs` asserts all of it. A map whose halves
// have to be *compared* to know they match is a map that will drift.

fn symmetric_arena(reg: &DataRegistry, arena: &Arena, seed: u64) -> Option<BattleState> {
    let map = arena.map()?;

    // One pair per chassis, and the pairs line up with `arena_deployment`'s
    // flip pairs — so the force is symmetric across the axis of advance as
    // well as through the centre. See the note on `arena_deployment`: a
    // laterally asymmetric force on laterally symmetric ground puts the
    // left-or-right confound straight back.
    let roster_of = [
        "medium_tank",
        "medium_tank",
        "tank_destroyer",
        "tank_destroyer",
    ];
    let mut placements = Vec::new();
    // The i-th vehicle of each side stands on the i-th mirror pair, so the two
    // forces are one force and its reflection. `arena_deployment` is the only
    // thing that knows where that is, which is what stops the two halves from
    // being written down twice and drifting.
    for ((west, east), vehicle) in arena.deployment().into_iter().zip(roster_of) {
        for (side, hex) in [(0u8, west), (1u8, east)] {
            placements.push(UnitPlacement {
                aboard_at: None,
                at: tactics_core::hex_to_offset(hex),
                side,
                vehicle: vehicle.to_string(),
                crew: Vec::new(),
                name: Some(format!("{vehicle} {side}")),
                facing: None,
                formation: None,
                leads: false,
            });
        }
    }
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
    let (roster, crews) = Roster::stamp_for(reg, &placements);
    Some(
        BattleState::from_placements(
            reg,
            map,
            sides,
            &placements,
            &crews,
            std::sync::Arc::new(roster),
            seed,
        )
        .expect("the staged placements are content the base mod ships"),
    )
}

/// One battle's outcome, reduced to what the tables count.
struct Outcome {
    winner: Option<u8>,
    a_losses: usize,
    b_losses: usize,
    rounds: usize,
}

/// Fight a batch of independent battles across the machine's cores.
///
/// They really are independent: each has its own map, its own `BattleState`,
/// its own planners and its own seeded rng, and they share nothing but the
/// registry, which is read-only for the whole run. So the only thing
/// parallelism can damage is the *table*, and it is guarded rather than
/// hoped for — every battle writes into the slot its seed owns, and the
/// results are folded in seed order afterwards. A balance figure that moved
/// depending on which core finished first would be worse than a slow one,
/// and it would be the kind of wrong that looks like noise.
///
/// `std::thread::scope` rather than a work-stealing pool: the batch is known
/// up front, the battles are within a factor of a few of each other, and it
/// costs no dependency. Chunked by slice rather than by index so the borrow
/// checker proves no two threads touch the same slot.
fn fight_all<T: Send>(
    reg: &DataRegistry,
    games: usize,
    each: impl Fn(&DataRegistry, u64) -> T + Sync,
) -> Vec<T> {
    let seeds: Vec<u64> = (0..games as u64).collect();
    run_all(&seeds, |seed| each(reg, *seed))
}

/// Play one battle between two configured planners on the mirrored arena.
fn arena_duel(
    reg: &DataRegistry,
    arena: &Arena,
    seed: u64,
    a: &AiConfig,
    b: &AiConfig,
) -> Option<Outcome> {
    let mut state = symmetric_arena(reg, arena, 9000 + seed)?;
    let mut ai = AiDriver::new();
    for (side, cfg) in [(0u8, a), (1u8, b)] {
        ai.insert(side, make_battle_planner(cfg, seed * 2 + side as u64, reg));
    }
    let mut rounds = 0;
    while !state.is_over() && rounds < 60 {
        ai.plan_round(reg, &mut state);
        state.resolve_round(reg);
        rounds += 1;
    }
    Some(Outcome {
        winner: state.over.and_then(|r| r.winner),
        a_losses: state.lost_units().filter(|u| u.side == 0).count(),
        b_losses: state.lost_units().filter(|u| u.side == 1).count(),
        rounds,
    })
}

/// Which brain is better, controlled.
///
/// REVIEW.md read "Kuhlmann won 3/3" as evidence that MCTS beats the utility
/// planner, and it cannot be: `playthrough` hands side 0 both MCTS *and*
/// `massed_armor`, on a map whose two sides field different vehicles. Three
/// candidate explanations, one observation. This fights the same mirrored
/// arena the skill-gap table uses — identical forces, identical doctrine,
/// identical difficulty — and varies nothing but which planner is thinking,
/// in both orientations so that a side-of-the-map effect shows up as a
/// disagreement between the two rows rather than as a result.
///
/// The two same-brain rows are the control and are the first thing to read:
/// on a mirrored arena they should sit near parity, and how far they miss it
/// is the noise floor every other row has to beat before it means anything.
fn brains_table(reg: &DataRegistry, arena: &Arena, games: usize, difficulty: u8) {
    heading(&format!(
        "brains: {games} battles per pairing on {}, difficulty {difficulty} both sides",
        arena.name
    ));
    println!(
        "  {:<22} {:>5} {:>5} {:>6} {:>12} {:>12} {:>9}",
        "pairing (A vs B)", "A won", "B won", "draws", "A lost/game", "B lost/game", "rounds"
    );
    for (a, b) in [
        ("utility", "utility"),
        ("mcts", "mcts"),
        ("mcts", "utility"),
        ("utility", "mcts"),
    ] {
        let cfg = |planner: &str| AiConfig {
            planner: planner.into(),
            difficulty,
            doctrine: None,
        };
        let (a_cfg, b_cfg) = (cfg(a), cfg(b));
        let fought = fight_all(reg, games, |reg, seed| {
            arena_duel(reg, arena, seed, &a_cfg, &b_cfg)
        });

        let (mut a_wins, mut b_wins, mut draws) = (0, 0, 0);
        let (mut a_losses, mut b_losses, mut total_rounds) = (0usize, 0usize, 0usize);
        for outcome in fought.into_iter().flatten() {
            total_rounds += outcome.rounds;
            match outcome.winner {
                Some(0) => a_wins += 1,
                Some(_) => b_wins += 1,
                None => draws += 1,
            }
            a_losses += outcome.a_losses;
            b_losses += outcome.b_losses;
        }
        let per = |l: usize| l as f32 / games.max(1) as f32;
        println!(
            "  {:<22} {a_wins:>5} {b_wins:>5} {draws:>6} {:>12.2} {:>12.2} {:>9.1}",
            format!("{a} vs {b}"),
            per(a_losses),
            per(b_losses),
            total_rounds as f32 / games.max(1) as f32,
        );
    }
    println!(
        "\n  the two same-brain rows are the control: read how far they miss\n  \
         parity first, because that is the noise floor the mixed rows have to\n  \
         beat. a real difference shows as BOTH mixed rows favouring the same\n  \
         brain."
    );
}

/// A pairing's five raw figures dressed with the exchange ratio the table
/// reports. Kept apart from the tallying so the summary rows below are built
/// the same way as the ordinary ones and cannot disagree with them.
fn cells(raw: &[f64; 5]) -> Vec<f64> {
    let mut out = raw.to_vec();
    // B's losses per A's loss, as a number rather than the `1:2.1` this used
    // to print, so a sweep can subtract it. The note says which way it reads.
    out.push(if raw[3] > 0.0 {
        raw[4] / raw[3]
    } else {
        f64::INFINITY
    });
    out
}

fn skill_gap(reg: &DataRegistry, arena: &Arena, games: usize, seed: u64) -> Grid {
    let mut rows = Vec::new();
    let mut raw: BTreeMap<(u8, u8), [f64; 5]> = BTreeMap::new();
    for (a, b) in [(5, 5), (1, 1), (5, 3), (3, 5), (5, 1), (1, 5)] {
        let cfg = |difficulty: u8| AiConfig {
            planner: "utility".into(),
            difficulty,
            doctrine: None,
        };
        let (a_cfg, b_cfg) = (cfg(a), cfg(b));
        let fought = fight_all(reg, games, |reg, game| {
            arena_duel(reg, arena, ARENA_SEED + seed + game, &a_cfg, &b_cfg)
        });

        let (mut a_wins, mut b_wins, mut draws) = (0, 0, 0);
        let (mut a_losses, mut b_losses) = (0usize, 0usize);
        for outcome in fought.into_iter().flatten() {
            match outcome.winner {
                Some(0) => a_wins += 1,
                Some(_) => b_wins += 1,
                None => draws += 1,
            }
            a_losses += outcome.a_losses;
            b_losses += outcome.b_losses;
        }
        let per = |l: usize| l as f64 / games.max(1) as f64;
        raw.insert(
            (a, b),
            [
                a_wins as f64,
                b_wins as f64,
                draws as f64,
                per(a_losses),
                per(b_losses),
            ],
        );
        rows.push((format!("{a} vs {b}"), cells(&raw[&(a, b)])));
        // Each pairing is a few hundred battles' worth of planning; let it
        // out as it finishes rather than making the reader wait for the block.
        let _ = std::io::stdout().flush();
    }

    // The three summary rows, and they are the ones to quote.
    //
    // The arena is mirror-symmetric but a battle is not: side 0 and side 1 do
    // not resolve simultaneously, so each of the six rows above measures a
    // skill gap *plus* whatever the ends are worth. Adding the two
    // orientations of a pairing cancels that, and adding the two equal-skill
    // pairings isolates it — which is the only honest way to read either.
    // Quoting one orientation of one pairing is what put figures in CLAUDE.md
    // that the other orientation did not support.
    // `x` is the pairing with the better crew on side B and `y` the one with
    // her on side A, so the strong side's figures are x[1]/y[0] and the weak
    // side's are x[0]/y[1]. Strong first, because the row is named that way.
    let both = |x: &[f64; 5], y: &[f64; 5]| {
        cells(&[
            x[1] + y[0],
            x[0] + y[1],
            x[2] + y[2],
            (x[4] + y[3]) / 2.0,
            (x[3] + y[4]) / 2.0,
        ])
    };
    if let (Some(hi), Some(lo)) = (raw.get(&(5, 5)), raw.get(&(1, 1))) {
        rows.push((
            "the ends (A/B)".to_string(),
            cells(&[
                hi[0] + lo[0],
                hi[1] + lo[1],
                hi[2] + lo[2],
                (hi[3] + lo[3]) / 2.0,
                (hi[4] + lo[4]) / 2.0,
            ]),
        ));
    }
    for (weak, strong) in [(3u8, 5u8), (1u8, 5u8)] {
        if let (Some(x), Some(y)) = (raw.get(&(weak, strong)), raw.get(&(strong, weak))) {
            rows.push((format!("{strong} over {weak}, both ends"), both(x, y)));
        }
    }

    Grid {
        title: format!(
            "skill gap: {games} battles per pairing on {} — same forces, same doctrine, only execution differs",
            arena.name
        ),
        preamble: vec![format!("  {} ({})", arena.blurb, arena.id)],
        row_head: "pairing",
        columns: vec![
            col("A won", 0),
            col("B won", 0),
            col("draws", 0),
            col("A lost", 2),
            col("B lost", 2),
            col("B per A", 1),
        ],
        rows,
        note: format!(
            "\n  `A lost` and `B lost` are vehicles per battle. `B per A` is B's losses\n  \
             for each of A's: a side that wins by outfighting rather than by\n  \
             outlasting shows it there, not in the win column.\n\n  \
             the last three rows are the ones to quote. `the ends` adds the two\n  \
             equal-skill pairings, so its win columns are worth of being side A\n  \
             against worth of being side B and nothing else — the arena is mirrored\n  \
             but a battle is not, and each of the six rows above carries that on\n  \
             top of the skill gap it is for. `both ends` adds a pairing's two\n  \
             orientations, which cancels it: those columns are the better crew\n  \
             against the worse one, and `B per A` there is the worse crew's losses\n  \
             for each of the better crew's.\n{}\n\n  \
             the three summary rows fought {} battles each, and their band is\n  \
             {:.0}–{:.0} to {:.0}–{:.0}.\n\n  \
             read this table at --games 36 or not at all, and re-draw it before\n  \
             quoting it: --only skill --sweep seed=0,1000,2000,3000. This is the\n  \
             table most often quoted at somebody.",
            level_note(games),
            2 * games,
            games as f64 - coin_band(2 * games),
            games as f64 + coin_band(2 * games),
            games as f64 + coin_band(2 * games),
            games as f64 - coin_band(2 * games),
        ),
    }
}

// --- the same game, more than once ----------------------------------------
//
// Everything below exists so that a question of the form "what would this
// number do" can be answered by *running both games*, side by side, instead
// of editing `mod.json`, running, reverting, running again and comparing two
// screens of scrollback from memory. That loop was the actual method used for
// the whole ballistics rewrite and it has two failure modes worth naming: a
// revert that was never made, and a difference small enough to be inside the
// noise of the sample being read as a result anyway. A sweep fixes the first
// by never touching the file, and exposes the second by putting the baseline
// on the row above.

/// One axis of a sweep: a number, and the values to try in it.
struct Axis {
    path: String,
    values: Vec<String>,
}

/// One whole configuration of the game, and what it is called in the table.
struct Variant {
    label: String,
    /// Which mod tree to load. An axis may sweep this, which is how two
    /// *versions* of the content get compared rather than two numbers in one.
    mods: std::path::PathBuf,
    /// Where this variant's battles start counting from. An axis may sweep
    /// this too, and that is the most useful sweep there is: rows that differ
    /// only in their seed are the same game sampled twice, so the spread down
    /// those rows is the noise floor that every *other* difference in the
    /// table has to clear before it means anything. It is the answer to the
    /// question the sample-size note asks, in the table rather than in prose.
    seed: Option<u64>,
    /// The requisition budget each doctrine spends in the mustered table. An
    /// axis because "does massed armour still win at 100 points" is a design
    /// question and not a tuning one: the budget changes what every doctrine
    /// *buys*, so two values of it are two armies rather than two numbers.
    points: Option<i32>,
    overrides: Vec<Override>,
}

/// The headline numbers of a fought-out run, reduced to something a row can
/// hold and a difference can be taken of.
///
/// Deliberately a *summary* and not the whole report: the value of the
/// comparison table is that two runs fit on the screen at once, and the full
/// report is still one `--verbose` away for whichever row turns out to be
/// interesting.
#[derive(Clone, Default)]
struct Digest {
    wins: BTreeMap<String, usize>,
    draws: f64,
    stalemates: f64,
    rounds: f64,
    shots: f64,
    hit: f64,
    bounce: f64,
    miss: f64,
    moving: f64,
    blind: f64,
    contact: f64,
    range: f64,
    on_target: f64,
    wounded: f64,
    out: f64,
    kills: BTreeMap<String, usize>,
    losses: BTreeMap<String, usize>,
}

impl Digest {
    fn of(t: &Tally, games: usize) -> Self {
        let games = games.max(1) as f64;
        let shots = t.shots.max(1) as f64;
        let shells = t.shells.max(1) as f64;
        Self {
            wins: t.wins.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            draws: t.draws as f64,
            stalemates: t.stalemates as f64,
            rounds: t.rounds.iter().sum::<u32>() as f64 / t.rounds.len().max(1) as f64,
            shots: t.shots as f64 / games,
            hit: 100.0 * t.hits as f64 / shots,
            bounce: 100.0 * t.bounces as f64 / shots,
            miss: 100.0 * t.misses as f64 / shots,
            moving: 100.0 * t.shots_on_the_move as f64 / shots,
            blind: 100.0 * t.shots_blind as f64 / shots,
            contact: t.found_at.iter().sum::<u32>() as f64 / t.found_at.len().max(1) as f64,
            range: t.contact_range.iter().sum::<u32>() as f64 / t.contact_range.len().max(1) as f64,
            on_target: 100.0 * t.shells_on_target as f64 / shells,
            wounded: t.girls_wounded as f64 / games,
            out: t.girls_out as f64 / games,
            kills: t.kills.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            losses: t.deaths.iter().map(|(k, v)| (k.clone(), *v)).collect(),
        }
    }
}

/// One column of the comparison table: what it is called, how wide it is, and
/// how to read it off a digest.
///
/// A list rather than a `println!` argument list, because the values table and
/// the differences table have to line up exactly — the second one is only
/// legible sitting under the first — and two format strings maintained by hand
/// would drift the first time a column was added.
struct Column {
    head: &'static str,
    of: fn(&Digest) -> f64,
    dp: usize,
}

const COLUMNS: &[Column] = &[
    Column {
        head: "draw",
        of: |d| d.draws,
        dp: 0,
    },
    Column {
        head: "stale",
        of: |d| d.stalemates,
        dp: 0,
    },
    Column {
        head: "rounds",
        of: |d| d.rounds,
        dp: 1,
    },
    Column {
        head: "shots",
        of: |d| d.shots,
        dp: 0,
    },
    Column {
        head: "hit%",
        of: |d| d.hit,
        dp: 0,
    },
    Column {
        head: "bnc%",
        of: |d| d.bounce,
        dp: 0,
    },
    Column {
        head: "miss%",
        of: |d| d.miss,
        dp: 0,
    },
    Column {
        head: "mov%",
        of: |d| d.moving,
        dp: 0,
    },
    Column {
        head: "bln%",
        of: |d| d.blind,
        dp: 0,
    },
    Column {
        head: "cntct",
        of: |d| d.contact,
        dp: 1,
    },
    Column {
        head: "range",
        of: |d| d.range,
        dp: 1,
    },
    Column {
        head: "arty%",
        of: |d| d.on_target,
        dp: 0,
    },
    Column {
        head: "wnd",
        of: |d| d.wounded,
        dp: 1,
    },
    Column {
        head: "out",
        of: |d| d.out,
        dp: 1,
    },
];

/// Every side name that won anything in any variant, so the win columns are
/// the same columns in every row. Sorted, because a `HashMap`'s order is not
/// a decision anywhere in this project.
fn side_names(digests: &[Digest]) -> Vec<String> {
    let mut names: Vec<String> = digests
        .iter()
        .flat_map(|d| d.wins.keys().cloned())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// The two tables the whole sweep is for: what each variant did, and how that
/// differs from the first one.
///
/// Two tables rather than one with the difference in brackets, because the
/// question being asked is almost always "did anything move at all", and the
/// answer to that is a screenful of `·` or it is not.
fn compare(labels: &[String], digests: &[Digest], games: usize) {
    let sides = side_names(digests);
    let width = labels.iter().map(|l| l.len()).max().unwrap_or(7).max(7);

    let header = |title: &str| {
        heading(title);
        print!("  {:<width$}", "variant");
        for name in &sides {
            print!(" {:>11}", short(name));
        }
        for col in COLUMNS {
            print!(" {:>6}", col.head);
        }
        println!();
    };

    header(&format!("per variant: {games} battles each"));
    for (label, d) in labels.iter().zip(digests) {
        print!("  {label:<width$}");
        for name in &sides {
            print!(" {:>11}", d.wins.get(name).copied().unwrap_or(0));
        }
        for col in COLUMNS {
            print!(" {:>6.*}", col.dp, (col.of)(d));
        }
        println!();
    }

    if digests.len() < 2 {
        return;
    }
    let base = &digests[0];
    header(&format!("difference from `{}`", labels[0]));
    for (label, d) in labels.iter().zip(digests).skip(1) {
        print!("  {label:<width$}");
        let mut moved = false;
        for name in &sides {
            let delta = d.wins.get(name).copied().unwrap_or(0) as i64
                - base.wins.get(name).copied().unwrap_or(0) as i64;
            moved |= delta != 0;
            print!(" {:>11}", signed(delta as f64, 0));
        }
        for col in COLUMNS {
            let delta = (col.of)(d) - (col.of)(base);
            moved |= delta.abs() >= 0.05;
            print!(" {:>6}", signed(delta, col.dp));
        }
        if !moved {
            print!("   <- the same game");
        }
        println!();
    }
    println!(
        "\n  A `·` is no difference at the precision shown. A row that is all dots\n  \
         changed nothing these {games} battles could see, which is a result about the\n  \
         number and not about the sample only if {games} is enough of them.{}",
        level_note(games)
    );

    // Kills and losses per chassis, which is the table the ballistics arcs
    // kept quoting at each other. Skipped past a handful of variants because
    // it is one column per variant and stops fitting on a screen.
    if digests.len() > 6 {
        return;
    }
    let mut chassis: Vec<String> = digests
        .iter()
        .flat_map(|d| d.kills.keys().chain(d.losses.keys()).cloned())
        .collect();
    chassis.sort();
    chassis.dedup();
    if chassis.is_empty() {
        return;
    }
    heading("per variant: what each chassis killed and lost");
    // Headed by what differs between the variants rather than by the whole
    // label: every column here would otherwise read `partial_pe.`, which is
    // three identical headings over three different columns.
    let heads = distinct(labels);
    let cell = heads.iter().map(|h| h.len()).max().unwrap_or(9).max(9);
    print!("  {:<18}", "vehicle");
    for head in &heads {
        print!(" {head:>cell$}");
    }
    println!();
    for id in &chassis {
        print!("  {:<18}", short(id));
        for d in digests {
            let k = d.kills.get(id).copied().unwrap_or(0);
            let l = d.losses.get(id).copied().unwrap_or(0);
            print!(" {:>cell$}", format!("{k}/{l}"));
        }
        println!();
    }
    println!("  (kills/losses, against the full labels listed at the top of the run)");
}

/// Whatever part of these labels tells them apart.
///
/// Sweep labels share a prefix by construction — every row of a one-axis
/// sweep is `<same field>=<different value>` — so a narrow column headed by
/// the front of the label is the same heading repeated. Dropping the shared
/// front leaves the part that is the point. The full labels are printed once
/// at the top of the run, which is where a reader goes if `55` is not enough.
fn distinct(labels: &[String]) -> Vec<String> {
    let Some(first) = labels.first() else {
        return Vec::new();
    };
    let mut shared = first.chars().count();
    for other in &labels[1..] {
        shared = shared.min(
            first
                .chars()
                .zip(other.chars())
                .take_while(|(a, b)| a == b)
                .count(),
        );
    }
    let trimmed: Vec<String> = labels
        .iter()
        .map(|l| l.chars().skip(shared).collect::<String>())
        .collect();
    if trimmed.iter().any(|t| t.is_empty()) {
        return labels.to_vec();
    }
    trimmed
}

/// How far apart the variants got, per column, under one row.
///
/// The single most useful line in a swept table and the reason it is printed
/// unasked: a difference between two variants means nothing until it is held
/// against how far the same configuration wanders on its own, and the way to
/// get that is a `--sweep seed=` whose spread row *is* the noise floor. Two
/// variants have no spread worth the name, so it starts at three.
fn spread_line(base: &Grid, grids: &[Grid], name: &str, w: usize, vw: usize) {
    if grids.len() < 3 {
        return;
    }
    print!("  {:<w$} {:<vw$}", "", "spread");
    for (j, c) in base.columns.iter().enumerate() {
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for g in grids {
            if let Some(cells) = g.row(name) {
                let v = cells.get(j).copied().unwrap_or(0.0);
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
        let text = if lo.is_finite() && hi > lo {
            c.cell(hi - lo)
        } else {
            "·".to_string()
        };
        print!(" {:>width$}", text, width = c.width());
    }
    println!();
}

/// A difference, printed so that "no change" is visibly not a small number.
fn signed(v: f64, dp: usize) -> String {
    if v.is_nan() {
        return "—".to_string();
    }
    if v.abs() < 0.5 / 10f64.powi(dp as i32) {
        return "·".to_string();
    }
    format!("{}{:.*}", if v > 0.0 { "+" } else { "" }, dp, v)
}

/// Every combination of the axes, first axis varying slowest.
///
/// An odometer rather than anything cleverer, so that reading the table top to
/// bottom groups the rows the way somebody sweeping two numbers expects: all
/// the values of the last axis, then the next value of the one before it.
fn variants(root: &std::path::Path, axes: &[Axis], base: &[Override]) -> Vec<Variant> {
    let mut out = vec![Variant {
        label: String::new(),
        mods: root.to_path_buf(),
        seed: None,
        points: None,
        overrides: base.to_vec(),
    }];
    for axis in axes {
        let mut next = Vec::with_capacity(out.len() * axis.values.len());
        for prefix in &out {
            for value in &axis.values {
                let mut v = Variant {
                    label: prefix.label.clone(),
                    mods: prefix.mods.clone(),
                    seed: prefix.seed,
                    points: prefix.points,
                    overrides: prefix.overrides.clone(),
                };
                // Three axis names are not fields of anything. `mods` selects
                // which tree to load, which is how two *versions* of the
                // content get compared rather than two numbers within one;
                // `seed` selects which battles get fought, which is how the
                // table is made to show its own noise floor; `points` selects
                // how big an army each doctrine buys itself.
                let leaf = if axis.path == "mods" {
                    v.mods = std::path::PathBuf::from(value);
                    std::path::Path::new(value)
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| value.clone())
                } else if axis.path == "seed" {
                    v.seed = Some(value.parse().unwrap_or_else(|_| {
                        eprintln!("error: --sweep seed=...: `{value}` is not a seed");
                        std::process::exit(1);
                    }));
                    format!("seed={value}")
                } else if axis.path == "points" {
                    v.points = Some(value.parse().unwrap_or_else(|_| {
                        eprintln!("error: --sweep points=...: `{value}` is not a budget");
                        std::process::exit(1);
                    }));
                    format!("points={value}")
                } else {
                    let ov = Override {
                        path: axis.path.clone(),
                        value: value.clone(),
                    };
                    let leaf = format!("{}={}", ov.leaf(), ov.value);
                    v.overrides.push(ov);
                    leaf
                };
                if !v.label.is_empty() {
                    v.label.push(' ');
                }
                v.label.push_str(&leaf);
                next.push(v);
            }
        }
        out = next;
    }
    out
}

/// Fight every variant, and put the results beside each other.
///
/// The battles of *all* the variants go into one flat list of jobs before any
/// of them runs, which matters: three variants at twelve battles each is
/// thirty-six independent fights, and scheduling them as three batches of
/// twelve would leave most of a machine idle at the end of each batch while
/// its slowest battle finished. The fold is still per variant and still in
/// seed order, so the table says the same thing at any `--jobs`.
fn sweep(axes: &[Axis], base: &[Override], root: &std::path::Path, cfg: &Run) {
    let plan = variants(root, axes, base);
    println!(
        "\nsweeping {} variant(s) of {} battle(s) each across {} thread(s)",
        plan.len(),
        cfg.games,
        thread_budget().min(plan.len() * cfg.games.max(1)),
    );
    for v in &plan {
        println!(
            "  {}",
            if v.label.is_empty() {
                "baseline"
            } else {
                &v.label
            }
        );
    }

    let registries: Vec<DataRegistry> = plan
        .iter()
        .map(|v| {
            println!("\nloading `{}`:", label_of(v));
            configure(&v.mods, &v.overrides)
        })
        .collect();

    if cfg.verbose {
        for (v, reg) in plan.iter().zip(&registries) {
            heading(&format!("=== {} ===", label_of(v)));
            analytic(reg, cfg);
        }
    }

    if !cfg.sim {
        println!(
            "\n(no --sim, so nothing was fought. The analytic tables above are per\n\
             variant with --verbose; the comparison tables below need battles.)"
        );
        return;
    }

    let labels: Vec<String> = plan.iter().map(|v| label_of(v).to_string()).collect();

    if cfg.only.wants("sim") {
        // One job per (variant, battle). The map roster can differ between
        // variants — a swept mod tree may ship different maps — so each
        // variant resolves its own.
        let maps: Vec<Vec<&str>> = registries.iter().map(battle_maps).collect();
        let jobs: Vec<(usize, u64)> = plan
            .iter()
            .enumerate()
            .flat_map(|(v, variant)| {
                let base = variant.seed.unwrap_or(cfg.seed);
                (0..cfg.games).map(move |g| (v, FOUGHT_SEED + base + g as u64))
            })
            .collect();
        let fought = run_all(&jobs, |&(v, seed)| {
            fight_one(&registries[v], &maps[v], seed)
        });

        let mut tallies: Vec<Tally> = (0..plan.len()).map(|_| Tally::default()).collect();
        for ((v, _), t) in jobs.iter().zip(&fought) {
            tallies[*v].merge(t);
        }

        if cfg.verbose {
            for (v, t) in plan.iter().zip(&tallies) {
                heading(&format!("=== {} ===", label_of(v)));
                report(t, cfg.games);
            }
        }

        let digests: Vec<Digest> = tallies.iter().map(|t| Digest::of(t, cfg.games)).collect();
        compare(&labels, &digests, cfg.games);
        if cfg.csv {
            csv(&labels, &digests);
        }
    }

    if cfg.games < 36 {
        println!(
            "\n  NOTE {} battles is a small sample to read a difference out of: a\n  \
             pairing that is genuinely level lands as far out as {:.0}–{:.0} one time in\n  \
             twenty, before any of these numbers change anything. The doctrine table\n  \
             needs 36 battles before it discriminates at all (CLAUDE.md says so, with\n  \
             the numbers); a sweep comparing two rows of it needs at least as many.",
            cfg.games,
            cfg.games as f64 / 2.0 + coin_band(cfg.games),
            cfg.games as f64 / 2.0 - coin_band(cfg.games),
        );
    }
    // The three that fight their own battles, laid out the same way. They run
    // after the digest rather than beside it because each is already parallel
    // inside itself, and because the digest is the table somebody watching the
    // run is waiting for.
    // The variant's own seed, not the run's: a `--sweep seed=` axis has to move
    // these tables too, or the row it adds is a sample of the fought-out pass
    // alone sitting under three tables that never moved — which reads as three
    // tables nothing can move.
    let grids: Vec<Vec<Grid>> = plan
        .iter()
        .zip(&registries)
        .map(|(v, reg)| {
            fought_grids(
                reg,
                cfg,
                v.seed.unwrap_or(cfg.seed),
                v.points.unwrap_or(cfg.budget),
            )
        })
        .collect();
    for i in 0..grids.first().map(Vec::len).unwrap_or(0) {
        let column: Vec<Grid> = grids.iter().map(|g| g[i].clone()).collect();
        compare_grids(&labels, &column, cfg.absolute);
    }
    if cfg.csv {
        csv_grids(&labels, &grids);
    }
}

/// What one battle said about the ground it was fought on.
#[derive(Default, Clone, Copy)]
struct Ground {
    /// Wins by whoever deployed on side 0's ground, and on side 1's. With the
    /// orders of battle exchanged half the time, the difference between these
    /// two is the *ground* and nothing else.
    west: usize,
    east: usize,
    draws: usize,
    /// Wins by the order of battle the map ships as side 0's, and as side 1's,
    /// counted across both ends. The difference between *these* two is the
    /// force, with the ground cancelled the same way.
    ob0: usize,
    ob1: usize,
    rounds: u32,
    /// Set when the two sides field different numbers of vehicles, so the
    /// exchange could not be made and the row is measuring both effects at
    /// once. Reported rather than hidden.
    unswappable: bool,
    /// Set on an arena, whose two orders of battle are one force and its own
    /// reflection. There is nothing to exchange, so the `OB` columns carry no
    /// information at all and are printed empty rather than as a tie that
    /// looks like a measurement. `west`/`east` is the whole of such a row —
    /// which is exactly what an arena is asked: does either end of this
    /// battlefield pay?
    mirrored: bool,
}

/// One of a shipped map's battles, optionally with the two armies exchanged.
///
/// The exchange keeps every deployment hex exactly where the map put it and
/// swaps only *which vehicles stand on them*, pairing the two sides' placements
/// by index. That is the whole trick and it is why this table can separate two
/// things a single battle cannot: fight a map both ways round and the ground
/// stays put while the force moves.
///
/// Crews are stamped fresh rather than taken from the map, because a crew named
/// for a medium tank has no business in a light one once the armies trade
/// places. So these battles are anonymously crewed and their casualties are
/// not comparable with the fought-out pass — which is fine, the question here
/// is only who won.
fn map_battle(reg: &DataRegistry, id: &str, seed: u64, swap: bool) -> Option<BattleState> {
    let file = reg.map(id)?;
    let map = HexMap::from_map_file(file).ok()?;
    let sides: Vec<SideState> = file
        .sides
        .iter()
        .map(|s| SideState {
            name: s.name.clone(),
            ai: s.ai.clone(),
        })
        .collect();
    let mut placements = file.units.clone();
    if swap {
        let zero: Vec<usize> = (0..placements.len())
            .filter(|i| placements[*i].side == 0)
            .collect();
        let one: Vec<usize> = (0..placements.len())
            .filter(|i| placements[*i].side == 1)
            .collect();
        for (a, b) in zero.iter().zip(&one) {
            let there = placements[*b].vehicle.clone();
            let here = std::mem::replace(&mut placements[*a].vehicle, there);
            placements[*b].vehicle = here;
        }
    }
    let (roster, crews) = Roster::stamp_for(reg, &placements);
    Some(
        BattleState::from_placements(
            reg,
            map,
            sides,
            &placements,
            &crews,
            std::sync::Arc::new(roster),
            seed,
        )
        .expect("the staged placements are content the base mod ships"),
    )
}

fn ground_battle(reg: &DataRegistry, id: &str, seed: u64, swap: bool) -> Ground {
    let mut g = Ground::default();
    let counts = reg.map(id).map(|f| {
        (
            f.units.iter().filter(|u| u.side == 0).count(),
            f.units.iter().filter(|u| u.side == 1).count(),
        )
    });
    g.unswappable = counts.is_some_and(|(a, b)| a != b);
    if swap && g.unswappable {
        return g;
    }
    let Some(mut state) = map_battle(reg, id, seed, swap) else {
        return g;
    };
    let mut ai = AiDriver::new();
    // The same doctrine and the same difficulty on both sides, which is the
    // point: two doctrines here would put a third variable in a table that
    // exists to separate two.
    ai.insert(0, planner(reg, seed, "massed_armor"));
    ai.insert(1, planner(reg, seed + 1, "massed_armor"));
    let mut rounds = 0;
    while !state.is_over() && rounds < 60 {
        ai.plan_round(reg, &mut state);
        state.resolve_round(reg);
        rounds += 1;
    }
    g.rounds = rounds;
    // Side 0 holds the map's own order of battle unless they were exchanged.
    let (side0_ob, side1_ob) = if swap { (1, 0) } else { (0, 1) };
    match state.over.and_then(|r| r.winner) {
        Some(0) => {
            g.west += 1;
            if side0_ob == 0 {
                g.ob0 += 1
            } else {
                g.ob1 += 1
            }
        }
        Some(_) => {
            g.east += 1;
            if side1_ob == 0 {
                g.ob0 += 1
            } else {
                g.ob1 += 1
            }
        }
        None => g.draws += 1,
    }
    g
}

/// One arena battle. Both ends carry the same order of battle by construction,
/// so there is nothing to exchange: every battle is fought the map's own way
/// round and the seeds simply run on, which keeps the row's sample honest
/// rather than counting each battle twice under two labels.
fn arena_ground_battle(reg: &DataRegistry, arena: &Arena, seed: u64) -> Ground {
    let mut g = Ground {
        mirrored: true,
        ..Ground::default()
    };
    let cfg = AiConfig {
        planner: "utility".into(),
        // The same difficulty on both sides, and the one the shipped scenarios
        // use, so this row asks about the ground and nothing else.
        difficulty: 3,
        doctrine: None,
    };
    let Some(outcome) = arena_duel(reg, arena, seed, &cfg, &cfg) else {
        return g;
    };
    g.rounds = outcome.rounds as u32;
    match outcome.winner {
        Some(0) => g.west += 1,
        Some(_) => g.east += 1,
        None => g.draws += 1,
    }
    g
}

/// What each battlefield is worth to the side that deploys on it.
///
/// A map favouring one end is not a bug — ground *is* strategy, and a scenario
/// where one side holds the ridge is a scenario about holding a ridge. What is
/// a bug is not knowing which of your maps do it or by how much, because then
/// every other number measured on them carries an unlabelled term. This table
/// is that label.
///
/// It separates the ground from the order of battle by fighting each map twice
/// per seed with the two armies exchanged between the ends. Summed that way,
/// `west`/`east` differ only by the ground and `OB-0`/`OB-1` differ only by the
/// force — the other effect cancels in each. On `battle_plains` and
/// `battle_forest` the two orders of battle are already identical, so their
/// `OB` columns are a control that should read level whatever the ground does.
fn ground_bias(reg: &DataRegistry, arena: &Arena, games: usize, seed: u64) -> Grid {
    let maps = battle_maps(reg);
    let mut rows: Vec<(String, Vec<f64>)> = maps
        .iter()
        .map(|id| {
            let jobs: Vec<(u64, bool)> = (0..games as u64)
                .flat_map(|g| [(g, false), (g, true)])
                .collect();
            let fought = run_all(&jobs, |&(g, swap)| {
                ground_battle(reg, id, GROUND_SEED + seed + g, swap)
            });
            let mut t = Ground::default();
            for one in &fought {
                t.west += one.west;
                t.east += one.east;
                t.draws += one.draws;
                t.ob0 += one.ob0;
                t.ob1 += one.ob1;
                t.rounds += one.rounds;
                t.unswappable |= one.unswappable;
            }
            let fights = (t.west + t.east + t.draws).max(1) as f64;
            (
                format!("{id}{}", if t.unswappable { " *" } else { "" }),
                vec![
                    t.west as f64,
                    t.east as f64,
                    t.draws as f64,
                    t.ob0 as f64,
                    t.ob1 as f64,
                    t.rounds as f64 / fights,
                ],
            )
        })
        .collect();

    // ...and the arena the skill table is fought on, because it is a
    // battlefield this project measures on and the question this table asks —
    // does an end of it pay? — is exactly the question the skill table's `the
    // ends` row has been standing in for. Two games per seed so that its
    // sample matches the shipped maps' rows above.
    {
        let jobs: Vec<u64> = (0..2 * games as u64).collect();
        let fought = run_all(&jobs, |g| {
            arena_ground_battle(reg, arena, GROUND_SEED + seed + g)
        });
        let mut t = Ground::default();
        for one in &fought {
            t.west += one.west;
            t.east += one.east;
            t.draws += one.draws;
            t.rounds += one.rounds;
            t.mirrored |= one.mirrored;
            t.ob0 += one.ob0;
            t.ob1 += one.ob1;
        }
        let fights = (t.west + t.east + t.draws).max(1) as f64;
        // A mirrored battlefield has no order-of-battle question to answer, so
        // those two columns are `NaN` — printed as a dash — rather than a tie
        // that would read as a measurement of something never measured.
        let (ob0, ob1) = if t.mirrored {
            (f64::NAN, f64::NAN)
        } else {
            (t.ob0 as f64, t.ob1 as f64)
        };
        rows.push((
            format!("{}{}", arena.id, if t.mirrored { " \u{2020}" } else { "" }),
            vec![
                t.west as f64,
                t.east as f64,
                t.draws as f64,
                ob0,
                ob1,
                t.rounds as f64 / fights,
            ],
        ));
    }

    Grid {
        title: format!(
            "ground: {} battles per map, each fought both ways round with the two armies exchanged",
            games * 2
        ),
        preamble: Vec::new(),
        row_head: "map",
        columns: vec![
            col("west won", 0),
            col("east won", 0),
            col("draws", 0),
            col("OB-0 won", 0),
            col("OB-1 won", 0),
            col("rounds", 1),
        ],
        rows,
        note: format!(
            "\n  `west`/`east` is which END of the map won, summed over both arrangements,\n  \
             so the armies cancel and what is left is the ground. `OB-0`/`OB-1` is which\n  \
             ORDER OF BATTLE won, summed over both ends, so the ground cancels and what is\n  \
             left is the force. A map is allowed to favour an end — ground is strategy —\n  \
             and this says which of them do and by how much.\n\n  \
             a row marked `*` fields different numbers of vehicles per side, so the armies\n  \
             could not be exchanged and its columns still carry both effects together. a row\n  \
             marked `\u{2020}` is an arena, whose two orders of battle are one force and its own\n  \
             reflection: there is nothing to exchange, so its `OB` columns are empty and\n  \
             `west`/`east` is the whole of it.\n  \
             crews are anonymous here (a crew named for a medium tank cannot follow it\n  \
             into a light one), so the casualties are not comparable with the fought-out\n  \
             pass; the winner is.{}",
            level_note(games * 2)
        ),
    }
}

/// The three tables that fight battles but are not the fought-out pass.
///
/// One list, called by both the single run and the sweep, because the failure
/// mode of two lists is a table that quietly stops being swept the day it is
/// added to one of them.
fn fought_grids(reg: &DataRegistry, cfg: &Run, seed: u64, budget: i32) -> Vec<Grid> {
    let mut out = Vec::new();
    if cfg.only.wants("delegation") {
        out.push(delegation_tax(reg, cfg.games, seed));
    }
    if cfg.only.wants("mustered") {
        out.push(mustered_forces(reg, cfg.arena, cfg.games, budget, seed));
    }
    if cfg.only.wants("skill") {
        out.push(skill_gap(reg, cfg.arena, cfg.games, seed));
    }
    if cfg.only.wants("ground") {
        out.push(ground_bias(reg, cfg.arena, cfg.games, seed));
    }
    out
}

fn label_of(v: &Variant) -> &str {
    if v.label.is_empty() {
        "baseline"
    } else {
        &v.label
    }
}

/// The same digests again, for something other than a person to read.
///
/// A sweep big enough to be worth plotting is bigger than a terminal, and the
/// alternative to this is somebody re-parsing the aligned table with `awk`.
fn csv(labels: &[String], digests: &[Digest]) {
    let sides = side_names(digests);
    heading("csv");
    print!("variant");
    for name in &sides {
        // Quoted because a side name is content: nothing stops a mod from
        // putting a comma in one, and a csv that only works for the base mod
        // is a csv that breaks on the day somebody uses this for its purpose.
        print!(",{}", quoted(&format!("wins {name}")));
    }
    for col in COLUMNS {
        print!(",{}", col.head.trim_end_matches('%'));
    }
    println!();
    for (label, d) in labels.iter().zip(digests) {
        print!("{}", quoted(label));
        for name in &sides {
            print!(",{}", d.wins.get(name).copied().unwrap_or(0));
        }
        for col in COLUMNS {
            print!(",{:.3}", (col.of)(d));
        }
        println!();
    }
}

/// Every swept grid as csv, one row per (table, row, variant).
///
/// Long rather than wide, because the tables do not share columns and a
/// sheet built by pasting them side by side would have to be un-pasted before
/// anything could be plotted. This shape goes straight into a pivot.
fn csv_grids(labels: &[String], grids: &[Vec<Grid>]) {
    let Some(first) = grids.first() else {
        return;
    };
    heading("csv (tables)");
    println!("table,row,variant,column,value");
    for (i, base) in first.iter().enumerate() {
        // The title carries the battle count and the map list, which change
        // between runs; the part before the colon is the table's name.
        let table = base.title.split(':').next().unwrap_or(&base.title);
        for (name, _) in &base.rows {
            for (label, variant) in labels.iter().zip(grids) {
                let Some(cells) = variant.get(i).and_then(|g| g.row(name)) else {
                    continue;
                };
                for (c, v) in base.columns.iter().zip(cells) {
                    println!(
                        "{},{},{},{},{:.4}",
                        quoted(table),
                        quoted(name),
                        quoted(label),
                        quoted(c.head),
                        v
                    );
                }
            }
        }
    }
}

/// One csv field, escaped the way every reader of csv agrees on.
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

/// How far from level a genuinely even pairing wanders, at this many battles.
///
/// Half the 95% interval of a fair coin over `battles` trials, which for 36 is
/// about six wins either side of 18–18. Printed under every table that has a
/// win column, because this project has repeatedly read a two- or three-win
/// move as a result and then built on it: `28–8` and `26–10` were quoted at
/// each other for months as evidence about a change, and they are the same
/// number to anyone who knows this band. It is deliberately arithmetic rather
/// than a remembered rule of thumb — the rule of thumb is what failed.
///
/// It is a floor and not the whole story. Two runs of *this* game differ by
/// more than a coin does, because the battles are not independent draws from
/// one distribution: they share maps, forces and doctrines. A seed sweep
/// measures the real spread; this says what it can never be smaller than.
fn coin_band(battles: usize) -> f64 {
    1.96 * 0.5 * (battles as f64).sqrt()
}

/// The sentence that goes under a win table.
fn level_note(battles: usize) -> String {
    let band = coin_band(battles);
    let half = battles as f64 / 2.0;
    let (lo, hi) = (half - band, half + band);
    format!(
        "\n  at {battles} battles a genuinely level pairing still lands anywhere from\n\
         \x20 {lo:.0}\u{2013}{hi:.0} to {hi:.0}\u{2013}{lo:.0}, nineteen times in twenty. That band is the\n\
         \x20 floor under every win column here \u{2014} and it is a floor, not the answer:\n\
         \x20 these battles share maps, forces and doctrines, so they scatter wider than\n\
         \x20 a coin does. Sweep the seed for the real spread."
    )
}

/// A table reduced to what a sweep can lay beside another copy of itself:
/// named rows, named columns, numbers.
///
/// The three tables under `--sim` that are not the fought-out pass each ask a
/// question about the *shape* of the game rather than about one shot, and each
/// used to be a `println!` per row with its own format string. That is fine
/// for reading once and useless for comparing, which is the whole of why they
/// build one of these now: a table that knows what its columns are can be laid
/// beside another copy of itself, and one that only knows how to print itself
/// cannot. Anything added here gets sweeping for free; anything that goes on
/// printing itself does not.
#[derive(Clone)]
struct Grid {
    title: String,
    /// Lines printed above the table — a shopping list, a caveat. Compared
    /// across variants too, because a mod change that alters what a doctrine
    /// *buys* has already answered the question before a shot is fired.
    preamble: Vec<String>,
    /// What a row is.
    row_head: &'static str,
    columns: Vec<GridColumn>,
    rows: Vec<(String, Vec<f64>)>,
    /// Prose printed under it: what the reader is looking at, and what would
    /// make it wrong. A `String` rather than a `&'static str` because the most
    /// useful sentence a win table can carry is the one that depends on how
    /// many battles it fought — see [`coin_band`].
    note: String,
}

#[derive(Clone)]
struct GridColumn {
    head: &'static str,
    dp: usize,
}

fn col(head: &'static str, dp: usize) -> GridColumn {
    GridColumn { head, dp }
}

impl GridColumn {
    fn width(&self) -> usize {
        self.head.len().max(6)
    }

    /// One cell. A `NaN` is "this row has no answer in this column" and prints
    /// as a dash rather than as the word NaN — the arena's `OB` columns are
    /// the case, and a tie printed there would read as a measurement of
    /// something that was never measured.
    fn cell(&self, v: f64) -> String {
        if v.is_nan() {
            return "—".to_string();
        }
        format!("{:.*}", self.dp, v)
    }
}

impl Grid {
    fn label_width(&self) -> usize {
        self.rows
            .iter()
            .map(|(n, _)| n.len())
            .chain(std::iter::once(self.row_head.len()))
            .max()
            .unwrap_or(0)
    }

    /// The row of this grid that goes by `name`, if it has one. Matched by
    /// name rather than by position because a swept mod tree may not produce
    /// the same rows at all, and lining up row three with row three when one
    /// of them is missing would compare two different pairings silently.
    fn row(&self, name: &str) -> Option<&[f64]> {
        self.rows
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, cells)| cells.as_slice())
    }
}

fn print_grid(g: &Grid) {
    heading(&g.title);
    for line in &g.preamble {
        println!("{line}");
    }
    if !g.preamble.is_empty() {
        println!();
    }
    let w = g.label_width();
    print!("  {:<w$}", g.row_head);
    for c in &g.columns {
        print!(" {:>width$}", c.head, width = c.width());
    }
    println!();
    for (name, cells) in &g.rows {
        print!("  {name:<w$}");
        for (c, v) in g.columns.iter().zip(cells) {
            print!(" {:>width$}", c.cell(*v), width = c.width());
        }
        println!();
    }
    println!("{}", g.note);
}

/// The same grid from every variant, stacked so the differences are adjacent.
///
/// The baseline's numbers are printed and everybody else's are printed as
/// deltas from them, rather than four rows of absolute figures with the
/// subtraction left to the reader. A row of `·` therefore means what it means
/// everywhere else in this harness: nothing these battles could see moved.
fn compare_grids(labels: &[String], grids: &[Grid], absolute: bool) {
    let (Some(base), Some(base_label)) = (grids.first(), labels.first()) else {
        return;
    };
    heading(&format!("{} — per variant", base.title));

    // A preamble is part of the answer when it differs: a swept mod tree can
    // change what a doctrine buys, and that is a result about the content, not
    // a caption on one.
    if grids.iter().any(|g| g.preamble != base.preamble) {
        for (label, g) in labels.iter().zip(grids) {
            println!("  [{label}]");
            for line in &g.preamble {
                println!("  {line}");
            }
        }
        println!();
    } else {
        for line in &base.preamble {
            println!("{line}");
        }
        if !base.preamble.is_empty() {
            println!();
        }
    }

    let w = base.label_width();
    let vw = labels.iter().map(|l| l.len()).max().unwrap_or(7).max(7);
    print!("  {:<w$} {:<vw$}", base.row_head, "variant");
    for c in &base.columns {
        print!(" {:>width$}", c.head, width = c.width());
    }
    println!();

    for (name, cells) in &base.rows {
        for (i, (label, g)) in labels.iter().zip(grids).enumerate() {
            let head = if i == 0 { name.as_str() } else { "" };
            print!("  {head:<w$} {label:<vw$}");
            match g.row(name) {
                None => print!("  (this variant has no such row)"),
                Some(theirs) => {
                    let mut moved = false;
                    for (j, c) in base.columns.iter().enumerate() {
                        let v = theirs.get(j).copied().unwrap_or(0.0);
                        let text = if i == 0 || absolute {
                            c.cell(v)
                        } else {
                            let delta = v - cells.get(j).copied().unwrap_or(0.0);
                            moved |= delta.abs() >= 0.5 / 10f64.powi(c.dp as i32);
                            signed(delta, c.dp)
                        };
                        print!(" {:>width$}", text, width = c.width());
                    }
                    if i > 0 && !absolute && !moved {
                        print!("   <- the same");
                    }
                }
            }
            println!();
        }
        spread_line(base, grids, name, w, vw);
    }
    if absolute {
        println!(
            "\n  every line is that variant's own number. `spread` is the widest gap\n  \
             between variants in that column."
        );
    } else {
        println!(
            "\n  the first line of each {} is `{base_label}`; the rest are differences\n  \
             from it, and a line of `·` is a variant these battles could not tell\n  \
             apart from the baseline. `spread` is the widest gap between variants.",
            base.row_head
        );
    }
    println!("{}", base.note);
}

/// Which tables to print, by name; empty means all of them.
///
/// Iterating on one number means reading one table, and `--sim` runs four that
/// fight battles. Paying for the other three every time is the kind of friction
/// that ends with somebody not running the instrument at all — which is the
/// failure this whole file is built against.
struct Only(Vec<String>);

/// Every name `--only` accepts, in the order the tables print.
const TABLES: &[&str] = &[
    "roster",
    "detect",
    "hit",
    "pen",
    "kills",
    "flight",
    "flags",
    "sim",
    "delegation",
    "mustered",
    "skill",
    "ground",
];

impl Only {
    fn parse(text: Option<&str>) -> Self {
        let Some(text) = text else {
            return Self(Vec::new());
        };
        let names: Vec<String> = text.split(',').map(str::to_string).collect();
        for name in &names {
            if !TABLES.contains(&name.as_str()) {
                eprintln!(
                    "error: --only {name}: no such table. There is: {}",
                    TABLES.join(", ")
                );
                std::process::exit(1);
            }
        }
        Self(names)
    }

    fn wants(&self, name: &str) -> bool {
        debug_assert!(TABLES.contains(&name), "{name} is not in TABLES");
        self.0.is_empty() || self.0.iter().any(|n| n == name)
    }
}

/// What one invocation of the fought-out pass was asked for.
struct Run {
    games: usize,
    seed: u64,
    budget: i32,
    sim: bool,
    verbose: bool,
    csv: bool,
    /// Print the variants' own numbers rather than their differences from the
    /// baseline. The differences are what a tuning question wants; the values
    /// are what a *range* wants, and a seed sweep is asking for a range.
    absolute: bool,
    only: Only,
    /// Which battlefield the arena tables fight on. A `&'static` rather than a
    /// name so that the map, the deployment and the symmetry checks are one
    /// choice: there is no way to fight one arena's ground with another's
    /// order of battle.
    arena: &'static Arena,
}
