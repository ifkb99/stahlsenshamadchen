//! What the numbers in `assets/mods` actually do.
//!
//! ```sh
//! cargo run --release -p tactics_core --example balance          # instant
//! cargo run --release -p tactics_core --example balance -- --sim # + fought out
//! cargo run --release -p tactics_core --example balance -- --sim --games 40
//! ```
//!
//! Two passes, deliberately, because balance iteration has two speeds.
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

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use tactics_core::ai::{AiConfig, AiDriver, AiPlanner, make_battle_planner};
use tactics_core::battle::{
    AttackPreview, BattleState, EndReason, Event, Order, SideState, UnitId, blast_overmatches,
    flight_ticks, preview_attack,
};
use tactics_core::data::{ArmorFacing, DataRegistry, ModuleEffect, WeaponDef};
use tactics_core::force;
use tactics_core::map::{Facing, HexMap, MapFile, MapKind, UnitPlacement};
use tactics_core::roster::{CadetId, Roster};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sim = args.iter().any(|a| a == "--sim");
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

    if cfg!(debug_assertions) {
        eprintln!("note: debug build. Fine for the analytic pass, slow for --sim.");
    }

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
    let (registry, _) = DataRegistry::load_dir(&root).expect("mods load");

    let mut duels = Duels::new(&registry);
    roster_table(&registry);
    penetration_table(&registry, &mut duels);
    kill_chain_table(&registry, &mut duels);
    flight_table(&registry);
    flags(&registry, &mut duels);
    if sim {
        simulate(&registry, games);
        delegation_tax(&registry, games);
        mustered_forces(&registry, games, budget);
        skill_gap(&registry, games);
    } else {
        println!("\n(pass --sim to fight {games} battles and see what these numbers do)");
    }
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
    Some(BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        1,
    ))
}

fn roster_table(reg: &DataRegistry) {
    heading("vehicles");
    println!(
        "{:<16} {:>12} {:>9} {:>9} {:>8} {:>6} {:>5} {:>9}",
        "id", "armour F/S/R", "substance", "speed", "sight", "safety", "cost", "rounds"
    );
    for id in vehicle_ids(reg) {
        let v = &reg.vehicles[&id];
        // Substance is the hit-point pool's successor and is derived, not
        // declared: two points per seat plus every module's toughness.
        let substance: u32 = 2 * v.crew_slots.len() as u32
            + reg.modules_for(v).iter().map(|m| m.toughness).sum::<u32>();
        let rounds: u32 = v.stowage.values().sum();
        println!(
            "{:<16} {:>12} {:>9} {:>9} {:>8} {:>6} {:>5} {:>9}",
            id,
            format!("{}/{}/{}", v.armor.front, v.armor.side, v.armor.rear),
            substance,
            reg.scale.format_speed(v.movement.points),
            reg.scale.format_distance(v.vision_range as i32),
            v.safety,
            v.cost,
            rounds,
        );
    }
    println!(
        "\n  `substance` is what she is made of — two points per cadet plus every\n  \
         module's toughness — which is what a penetration spends itself against\n  \
         now that there are no hit points. `rounds` is everything in the racks."
    );
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
                    Some(shot) => print!("{:>12}", shot.pen_chance),
                    None => print!("{:>12}", "-"),
                }
            }
            println!();
        }
    }
    println!(
        "\n  every cell is `preview_attack`'s own pen chance with that round forced\n  \
         into the racks, so it is the number the AI plans on and the die the\n  \
         resolver throws. `-` means no vehicle in the roster carries that gun."
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
        "{:<26} {:>10} {:>8} {:>8} {:>14}",
        "gun / round", "range", "ticks", "seconds", "target moves"
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
            println!(
                "{:<26} {:>10} {:>8} {:>8} {:>14}",
                format!("{weapon_id} / {ammo_id}"),
                format!("{label} {dist}h"),
                ticks,
                format!("{seconds:.0}s"),
                format!("{hexes:.1} hex"),
            );
        }
    }
    println!(
        "\n  a shell is aimed at ground, so `target moves` is how far the quickest\n  \
         vehicle in the roster ({}) travels before it lands — the lead an\n  \
         artillery order has to guess, and the reason standing still is the\n  \
         mistake it historically was.",
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

#[derive(Default)]
struct Tally {
    wins: HashMap<String, usize>,
    draws: usize,
    stalemates: usize,
    rounds: Vec<u32>,
    kills: HashMap<String, usize>,
    deaths: HashMap<String, usize>,
    shots: u32,
    hits: u32,
    bounces: u32,
    misses: u32,
    hits_by_arc: HashMap<String, u32>,
    /// What actually ended each vehicle, by the flag she died carrying.
    causes: BTreeMap<&'static str, usize>,
    girls_wounded: usize,
    girls_out: usize,
    /// Rounds that drove out, and rounds still aboard at the end, by ammo
    /// id. The difference is what the battle cost in ammunition.
    ammo_aboard: BTreeMap<String, u32>,
    ammo_left: BTreeMap<String, u32>,
    racks_destroyed: usize,
    shells: u32,
    shells_on_target: u32,
    shells_bounced: u32,
    /// Infantry, which the kills/losses table already reports per chassis —
    /// the rows appear on their own the moment a map fields them. What that
    /// table cannot say is the thing that makes a platoon different from a
    /// tank: she is worn down rather than killed, so a run in which no foot
    /// unit dies can still be one in which every platoon was shot to pieces.
    /// These four numbers are that story: how many took the field, what
    /// fraction of their rifles the survivors still had at the bell, and how
    /// much of the riding actually happened.
    foot_fielded: usize,
    troops_left: Vec<f32>,
    mounts: usize,
    dismounts: usize,
}

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

/// Which ground this battle is fought on. Keyed on the seed rather than the
/// game index so the pairing of map to battle survives changing `--games`.
fn map_for<'a>(maps: &[&'a str], seed: u64) -> &'a str {
    maps[seed as usize % maps.len()]
}

fn simulate(reg: &DataRegistry, games: usize) {
    let maps = battle_maps(reg);
    heading(&format!(
        "fought out: {games} battles across {}",
        maps.join(", ")
    ));
    let mut t = Tally::default();

    for game in 0..games {
        let seed = 1000 + game as u64;
        let mut state = BattleState::from_map(reg, map_for(&maps, seed), seed).expect("battle");
        let mut ai = AiDriver::new();
        ai.insert(0, planner(reg, seed, "massed_armor"));
        ai.insert(1, planner(reg, seed + 1, "elastic_defense"));
        let mut rounds = 0;
        let mut last_hit: HashMap<UnitId, String> = HashMap::new();
        // Final state per cadet, so a cadet wounded and then killed is counted
        // once, as killed.
        let mut cadets: BTreeMap<CadetId, bool> = BTreeMap::new();
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
            for event in state.resolve_round(reg) {
                let landed = std::mem::take(&mut shell_from);
                match event {
                    Event::ShotFired { .. } => t.shots += 1,
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
                            // The flags are still on her: reap clears `alive`
                            // and nothing else, so the cause of death is
                            // readable exactly here.
                            let cause = if u.brewed {
                                "brewed"
                            } else if u.wrecked {
                                "wrecked by blast"
                            } else if u.abandoned {
                                "abandoned"
                            } else {
                                "crew out"
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
    }

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
fn delegation_tax(reg: &DataRegistry, games: usize) {
    let maps = battle_maps(reg);
    heading(&format!(
        "delegation tax: {games} battles per pairing across {}, opponent held constant",
        maps.join(", ")
    ));
    // Read off the chassis rather than named: a taxi is anything that lifts
    // somebody and a foot unit is anything that walks, so a mod's own
    // transports and its own infantry land in these columns unasked.
    let taxi = |unit: &tactics_core::battle::Unit| {
        reg.vehicle(&unit.vehicle).is_some_and(|v| v.capacity > 0)
    };
    let afoot = |unit: &tactics_core::battle::Unit| {
        reg.vehicle(&unit.vehicle)
            .is_some_and(|v| v.movement.class == tactics_core::data::MovementClass::Foot)
    };
    let run = |p0: &str, p1: &str| -> Delegation {
        let (mut wins_massed, mut wins_elastic, mut draws) = (0, 0, 0);
        let mut rounds_total = 0u32;
        let (mut taxis_lost, mut foot_lost, mut foot_shots) = (0usize, 0usize, 0usize);
        for game in 0..games {
            let seed = 1000 + game as u64;
            let mut state = BattleState::from_map(reg, map_for(&maps, seed), seed).expect("battle");
            let mut ai = AiDriver::new();
            ai.insert(0, planner_with(reg, seed, p0, "massed_armor"));
            ai.insert(1, planner_with(reg, seed + 1, p1, "elastic_defense"));
            let mut rounds = 0;
            while !state.is_over() && rounds < 60 {
                ai.plan_round(reg, &mut state);
                for event in &state.resolve_round(reg) {
                    if let Event::ShotFired { attacker, .. } = event
                        && state
                            .units
                            .get(attacker.index())
                            .is_some_and(|u| u.side == 0 && afoot(u))
                    {
                        foot_shots += 1;
                    }
                }
                rounds += 1;
            }
            rounds_total += rounds;
            // Side 0 only, always — the side whose planner the middle row
            // swaps. Counting both sides would average the commanded force
            // together with its flat opponent and hide exactly the
            // difference the table is asking about.
            taxis_lost += state
                .lost_units()
                .filter(|u| u.side == 0 && taxi(u))
                .count();
            foot_lost += state
                .lost_units()
                .filter(|u| u.side == 0 && afoot(u))
                .count();
            match state.over.and_then(|r| r.winner) {
                Some(0) => wins_massed += 1,
                Some(1) => wins_elastic += 1,
                _ => draws += 1,
            }
        }
        Delegation {
            wins_massed,
            wins_elastic,
            draws,
            rounds: rounds_total as f32 / games.max(1) as f32,
            taxis_lost,
            foot_lost,
            foot_shots,
        }
    };
    let flat = run("utility", "utility");
    let massed_cmd = run("command", "utility");
    let elastic_cmd = run("utility", "command");

    println!(
        "  {:<24} {:>6} {:>8} {:>6} {:>7} {:>7} {:>7} {:>7}",
        "pairing", "massed", "elastic", "draws", "rounds", "taxis", "platoons", "shots"
    );
    for (name, t) in [
        ("both flat", &flat),
        ("massed under command", &massed_cmd),
        ("elastic under command", &elastic_cmd),
    ] {
        println!(
            "  {:<24} {:>6} {:>8} {:>6} {:>7.1} {:>7} {:>7} {:>7}",
            name,
            t.wins_massed,
            t.wins_elastic,
            t.draws,
            t.rounds,
            t.taxis_lost,
            t.foot_lost,
            t.foot_shots
        );
    }
    println!(
        "\n  a side's tax is its win drop against the same flat opponent when it\n  \
         fights through missions instead; zero is the target.\n\n  \
         the last three columns are always side 0's — carriers lost, foot units\n  \
         lost, and rounds fired by anybody on their feet — so the middle row is\n  \
         the commanded force and the two rows around it are the same force flat."
    );
}

/// One row of [`delegation_tax`]: who won, how long it took, and what the
/// infantry made of it.
struct Delegation {
    wins_massed: usize,
    wins_elastic: usize,
    draws: usize,
    rounds: f32,
    taxis_lost: usize,
    foot_lost: usize,
    foot_shots: usize,
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
fn mustered_forces(reg: &DataRegistry, games: usize, budget: i32) {
    heading(&format!(
        "mustered forces: {games} battles per pairing, {budget} points a side, each doctrine buying its own army"
    ));

    let doctrines = ["massed_armor", "elastic_defense", "recon_pull"];
    let forces: Vec<(&str, Vec<String>)> = doctrines
        .iter()
        .filter_map(|id| {
            let doctrine = reg.doctrine(id)?;
            Some((*id, force::muster(reg, doctrine, budget)))
        })
        .collect();

    println!("  the shopping lists:");
    for (id, army) in &forces {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for vehicle in army {
            *counts.entry(vehicle.as_str()).or_default() += 1;
        }
        println!(
            "    {:<16} {:>3} pts, {} units — {}",
            id,
            force::cost_of(reg, army),
            army.len(),
            counts
                .iter()
                .map(|(v, n)| format!("{n}x {v}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    println!(
        "\n  {:<34} {:>6} {:>6} {:>6} {:>7} {:>9}",
        "pairing (A vs B)", "A won", "B won", "draws", "rounds", "A pts lost"
    );
    for (i, (a_id, a_force)) in forces.iter().enumerate() {
        for (b_id, b_force) in forces.iter().skip(i + 1) {
            // Both orientations, summed: the arena is mirror-symmetric but
            // resolution order is not, and side B has held a measured edge on
            // this ground since the plateau rule went in.
            let (mut a_wins, mut b_wins, mut draws) = (0, 0, 0);
            let (mut rounds_total, mut a_points_lost) = (0u32, 0i32);
            for game in 0..games {
                let seed = 4000 + game as u64;
                let flip = game % 2 == 1;
                let (west, east) = if flip {
                    (b_force, a_force)
                } else {
                    (a_force, b_force)
                };
                let (west_doctrine, east_doctrine) =
                    if flip { (*b_id, *a_id) } else { (*a_id, *b_id) };
                let Some(mut state) = muster_arena(reg, seed, west, east) else {
                    continue;
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
                rounds_total += rounds;
                // A is whichever end she deployed on this game, so the
                // tallies follow the flip rather than the side number.
                let a_side = if flip { 1u8 } else { 0u8 };
                a_points_lost += state
                    .lost_units()
                    .filter(|u| u.side == a_side)
                    .filter_map(|u| reg.vehicle(&u.vehicle))
                    .map(|v| v.cost)
                    .sum::<i32>();
                match state.over.and_then(|r| r.winner) {
                    Some(s) if s == a_side => a_wins += 1,
                    Some(_) => b_wins += 1,
                    None => draws += 1,
                }
            }
            println!(
                "  {:<34} {:>6} {:>6} {:>6} {:>7.1} {:>9.1}",
                format!("{a_id} vs {b_id}"),
                a_wins,
                b_wins,
                draws,
                rounds_total as f32 / games.max(1) as f32,
                a_points_lost as f32 / games.max(1) as f32,
            );
        }
    }
    println!(
        "\n  each pairing alternates ends game by game, so neither column is a\n  \
         statement about deployment. `A pts lost` is the requisition value of\n  \
         A's dead per battle — what the win cost, in the same currency the\n  \
         army was bought with."
    );
}

/// The skill-gap arena, filled with two bought armies instead of the mirrored
/// four. Same ground, different orders of battle — which is the whole point,
/// and why the placement is separate rather than a parameter: the skill-gap
/// table's four-unit spacing is baked into numbers already recorded in the
/// log, and must not move because this section wanted a ninth slot.
fn muster_arena(
    reg: &DataRegistry,
    seed: u64,
    west: &[String],
    east: &[String],
) -> Option<BattleState> {
    let map = arena_map()?;
    let mut placements = Vec::new();
    for (side, army) in [(0u8, west), (1u8, east)] {
        for (i, vehicle) in army.iter().enumerate() {
            // A column down the deployment edge, spilling into the next
            // column inward once eleven rows are used. Deliberately dense:
            // these forces differ in size — that is the interesting part —
            // and a spacing rule that scaled with the count would hand the
            // smaller army more room as a hidden bonus.
            let column = (i / 11) as i32;
            let y = 1 + (i % 11) as i32;
            let x = if side == 0 { 2 - column } else { 22 + column };
            placements.push(UnitPlacement {
                aboard_at: None,
                at: [x, y],
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
    Some(BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    ))
}

fn arena_map() -> Option<HexMap> {
    let width = 25usize;
    let mut rows = Vec::new();
    for _ in 0..13 {
        let mut row: Vec<char> = "g".repeat(width).chars().collect();
        row[8] = 'f';
        row[16] = 'f';
        rows.push(row.into_iter().collect::<String>());
    }
    let file: MapFile = serde_json::from_value(serde_json::json!({
        "id": "skill_arena",
        "kind": "battle",
        "shape": "free",
        "palette": { "g": "grass", "f": "forest" },
        "rows": rows,
        "objectives": [
            {
                "id": "center",
                "name": "The Crossroads",
                "at": [[12, 5], [12, 6], [12, 7]],
                "value": 2
            }
        ],
        "victory_score": 30,
    }))
    .ok()?;
    HexMap::from_map_file(&file).ok()
}

fn symmetric_arena(reg: &DataRegistry, seed: u64) -> Option<BattleState> {
    let map = arena_map()?;

    let roster_of = ["medium_tank", "medium_tank", "tank_destroyer", "light_tank"];
    let mut placements = Vec::new();
    for (i, vehicle) in roster_of.iter().enumerate() {
        let y = (3 + i * 2) as i32;
        for (side, x) in [(0u8, 2i32), (1u8, 22i32)] {
            placements.push(UnitPlacement {
                aboard_at: None,
                at: [x, y],
                side,
                vehicle: vehicle.to_string(),
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
    Some(BattleState::from_placements(
        reg,
        map,
        sides,
        &placements,
        &crews,
        std::sync::Arc::new(roster),
        seed,
    ))
}

fn skill_gap(reg: &DataRegistry, games: usize) {
    heading(&format!(
        "skill gap: {games} battles per pairing on a mirrored arena — same forces, same doctrine, only execution differs"
    ));
    println!(
        "  {:<12} {:>5} {:>5} {:>6} {:>12} {:>12} {:>8}",
        "pairing", "A won", "B won", "draws", "A lost/game", "B lost/game", "ratio"
    );
    for (a, b) in [(5, 5), (1, 1), (5, 3), (3, 5), (5, 1), (1, 5)] {
        let (mut a_wins, mut b_wins, mut draws) = (0, 0, 0);
        let (mut a_losses, mut b_losses) = (0usize, 0usize);
        for seed in 0..games as u64 {
            let Some(mut state) = symmetric_arena(reg, 9000 + seed) else {
                continue;
            };
            let mut ai = AiDriver::new();
            for (side, diff) in [(0u8, a), (1u8, b)] {
                ai.insert(
                    side,
                    make_battle_planner(
                        &AiConfig {
                            planner: "utility".into(),
                            difficulty: diff,
                            doctrine: None,
                        },
                        seed * 2 + side as u64,
                        reg,
                    ),
                );
            }
            let mut rounds = 0;
            while !state.is_over() && rounds < 60 {
                ai.plan_round(reg, &mut state);
                state.resolve_round(reg);
                rounds += 1;
            }
            match state.over.and_then(|r| r.winner) {
                Some(0) => a_wins += 1,
                Some(_) => b_wins += 1,
                None => draws += 1,
            }
            a_losses += state.lost_units().filter(|u| u.side == 0).count();
            b_losses += state.lost_units().filter(|u| u.side == 1).count();
        }
        let per = |l: usize| l as f32 / games.max(1) as f32;
        println!(
            "  {:<12} {:>5} {:>5} {:>6} {:>12.2} {:>12.2} {:>8}",
            format!("{a} vs {b}"),
            a_wins,
            b_wins,
            draws,
            per(a_losses),
            per(b_losses),
            format!(
                "1:{:.1}",
                if a_losses > 0 {
                    b_losses as f32 / a_losses as f32
                } else {
                    f32::INFINITY
                }
            ),
        );
        // Each pairing is a few hundred battles' worth of planning; print it
        // as it finishes rather than making the reader wait for the block.
        let _ = std::io::stdout().flush();
    }
    println!(
        "\n  ratio is B's losses per A's loss: a side that wins by outfighting\n  \
         rather than by outlasting shows it here, not in the win column."
    );
}
