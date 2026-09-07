//! Turning battle state into the sentences the screen shows.
//!
//! Every function here is pure over a [`BattleState`] and returns a `String`
//! or a `Vec<String>`. **Nothing in this file touches Bevy**, which is the
//! whole reason it is a file: these are the parts of the battle screen that
//! can be read, reviewed and reasoned about without running the game, and
//! they were 580 lines buried in the middle of a 4,100-line module of ECS
//! systems, marker components and query bundles.
//!
//! That is the same move `roster_page` made on the campaign side, for the
//! reason recorded there: a page nobody can see in a diff is a page that
//! rots. A panel whose text is written between a `Query` and a `Commands` is
//! read by nobody who is not already debugging the renderer.
//!
//! What deliberately stayed behind in `battle.rs` is anything holding a Bevy
//! type — `set_portrait` takes a `Query`, `shown_to` answers a question about
//! drawing, and `aim_at` is closer to input handling than to prose.
//!
//! The vocabulary these follow is in `battle/command.rs` in the engine:
//! `Mission::promise`, `Mission::verb` and `Latitude::promise` are the
//! sentences the *rules* claim, and this module only arranges them.

use tactics_core::Hex;
use tactics_core::battle::{
    BattleState, Contact, CrewCondition, Formation, Latitude, Mission, UnitId,
};

/// What to call a formation in the log: the name its map gave it, falling back
/// to the bare id so a formation from a mod this build does not know about is
/// still named rather than silently anonymous.
pub(super) fn formation_name(state: &BattleState, id: &str) -> String {
    state
        .map
        .formations()
        .iter()
        .find(|f| f.id == id)
        .map(|f| f.display_name().to_string())
        .unwrap_or_else(|| id.to_string())
}

/// What to call a unit in a log line, by id. The panel and the event pump
/// each have their own closure for this; the input handler needed one too and
/// this is it rather than a third copy inside a key branch.
pub(super) fn unit_name(state: &BattleState, unit: UnitId) -> String {
    state
        .units
        .get(unit.index())
        .map(|u| u.name.clone())
        .unwrap_or_else(|| "???".into())
}

/// How old a report is, in the words the log uses elsewhere: rounds, because
/// that is the clock the player is reading off the banner.
pub(super) fn report_age(state: &BattleState, contact: &Contact) -> String {
    match state.round.saturating_sub(contact.round) {
        0 => "this round".into(),
        1 => "a round ago".into(),
        n => format!("{n} rounds ago"),
    }
}

/// The formation panel: who these cadets are, what they were told to do, what
/// is still on its way to them, and which of them can no longer hear it.
///
/// The mission is spelled out in words rather than as an enum name, because
/// the point of the panel is that a player can read her own last order back
/// and check it against what her platoon is actually doing. An order still in
/// transit is listed separately for the same reason — "she has been told" and
/// "she knows" are different states, and the gap between them is the system.
pub(super) fn format_formation(
    registry: &tactics_core::data::DataRegistry,
    state: &BattleState,
    formation: &Formation,
    hovered: Option<Hex>,
) -> String {
    let name = |id: UnitId| {
        state
            .units
            .get(id.index())
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "???".into())
    };
    let mut lines = vec![
        formation_name(state, &formation.id),
        match formation.leader {
            Some(leader) => format!("Leader: {}", name(leader)),
            None => "Leader: nobody left".into(),
        },
    ];
    if let Some(net) = format_net(registry, state, formation) {
        lines.push(net);
    }
    lines.push(String::new());
    lines.push(format!(
        "Orders: {}",
        mission_sentence(state, formation.mission.as_ref())
    ));
    // ...and what that order actually commits them to. The player is reading
    // her own last decision back here, and the whole complaint that started
    // this work was that she could not tell an advance from an assault after
    // giving one. The promise comes from the engine so this line cannot
    // describe an order the rules stopped implementing.
    if let Some(mission) = formation.mission.as_ref() {
        lines.push(format!("  ({})", mission.promise()));
        // And how hard it was meant, which is a second sentence because it
        // is a second decision. The verb says what she will do about the
        // enemy; the latitude says whether her own doctrine may discount the
        // order at all. A player who can read one back and not the other
        // cannot tell why two formations under the same order are behaving
        // differently.
        lines.push(format!("  ({})", formation.latitude.mission_promise()));
    }
    if let Some((change, ticks)) = &formation.incoming {
        // An amendment reads differently from a countermand, because the
        // player who queued a leg should not fear it will replace her plan.
        let verb = match change {
            tactics_core::battle::MissionChange::Replace { .. } => "In the air",
            tactics_core::battle::MissionChange::Append { .. } => "In the air (and then)",
        };
        lines.push(format!(
            "{verb}: {} ({})",
            mission_sentence(state, Some(change.mission())),
            registry.scale.format_duration(*ticks)
        ));
        // An order in transit is the one the player just gave, and under a
        // signals net it is the *only* place she can read it back until it
        // lands. Leaving the promise off this line would mean the panel
        // explained her decision to her only after it was too late to
        // change it.
        lines.push(format!("  ({})", change.mission().promise()));
        lines.push(format!("  ({})", change.latitude().mission_promise()));
    }
    for leg in &formation.plan {
        lines.push(format!("Then: {}", mission_sentence(state, Some(leg))));
    }
    lines.push(String::new());
    lines.push("Members:".into());
    for id in &formation.members {
        let Some(unit) = state.units.get(id.index()) else {
            continue;
        };
        if !unit.alive {
            // Gone is gone, and the panel says which kind: a crew that drove
            // off by an exit came home, and listing her as lost would be the
            // UI telling the lie the engine is careful not to.
            lines.push(format!(
                "  {} - {}",
                unit.name,
                if unit.exited { "withdrawn" } else { "lost" }
            ));
            continue;
        }
        // Two different silences, and the panel must not conflate them: one
        // says she cannot hear you, the other says you have already spoken and
        // she has not heard it yet. A player who cannot tell them apart cannot
        // tell whether to reissue the order.
        let mut tags: Vec<String> = Vec::new();
        if !formation.in_contact(*id) {
            tags.push("out of contact".into());
        }
        if state.command.waiting_for(*id).is_some() {
            tags.push("orders waiting".into());
        }
        // Who will go where: a standing personal march is a promise about
        // future rounds, and a promise the player cannot read is one she
        // will fight against.
        // ...and under how much insistence, because the whole value of being
        // able to say "I mean it" is that the player can see afterwards which
        // of her crews she said it to.
        if let Some(tasking) = unit.tasking {
            tags.push(match unit.latitude {
                Latitude::Binding => format!("pressing on to {}", hex_label(tasking)),
                Latitude::Delegated => format!("moving to {}", hex_label(tasking)),
            });
        }
        // A passenger is in the formation and not on the map, which reads as
        // a missing cadet unless the roll call says where she went.
        if let Some(carrier) = unit.aboard.and_then(|c| state.units.get(c.index())) {
            tags.push(format!("riding in {}", carrier.name));
        }
        let tag = if tags.is_empty() {
            String::new()
        } else {
            format!(" - {}", tags.join(", "))
        };
        lines.push(format!("  {}{}", unit.name, tag));
    }
    lines.push(String::new());
    // The order menu, with what each verb costs, because a verb whose
    // meaning is only discoverable by pressing it and watching is not a verb
    // the player is really choosing between. The keys are the game's to know
    // and the promises are the engine's, so they are joined here and written
    // down in neither place twice.
    lines.push("Orders, on the hovered hex:".into());
    lines.extend(order_menu());
    lines.push("Shift queues a leg behind the last.".into());
    // The second modifier, and it gets its promise from the engine like the
    // verbs above rather than a sentence written here: what insisting costs
    // is a claim about the rules, and the rules should be the ones making it.
    lines.push(format!(
        "Ctrl means it - {}.",
        Latitude::Binding.mission_promise()
    ));
    lines.push("F next formation, Esc drops it.".into());
    if let Some(hex) = hovered {
        lines.push(String::new());
        lines.push(format!("Hovered {}:", hex_label(hex)));
        lines.push(format_tile(registry, state, hex));
    }
    lines.join("\n")
}

/// Which key gives which order, in the order the panel lists them.
///
/// The keys belong to the game and the promises belong to the engine, so
/// this is the join and the only place either is written down twice.
/// `W` is last because it is the one order that is not aimed at the cursor.
pub(super) const MISSION_KEYS: [(&str, &str); 5] = [
    ("G", "advance"),
    ("X", "assault"),
    ("H", "hold"),
    ("R", "reconnoitre"),
    ("W", "withdraw"),
];

/// The order menu: every mission key, its verb, and what that verb commits
/// the platoon to.
///
/// A verb the engine no longer publishes a promise for is dropped rather
/// than printed bare — a menu entry that explains nothing is worse than one
/// line fewer — and `every_mission_key_has_a_promise` fails the build before
/// a player ever sees the gap.
pub(super) fn order_menu() -> Vec<String> {
    let vocabulary = Mission::vocabulary();
    MISSION_KEYS
        .iter()
        .filter_map(|(key, verb)| {
            vocabulary
                .iter()
                .find(|(v, _)| v == verb)
                .map(|(_, promise)| format!("  {key} {verb} - {promise}"))
        })
        .collect()
}

/// The formation's net in numbers: how far the leader's radio carries, how
/// much of that is her crew rather than her hardware, and how far a flag
/// carries to anybody at all.
///
/// The ring drawn on the map says *where* the radio ends; this says why it
/// ends there, which is the half a player can act on — a poor signaller is a
/// crew problem with a crew answer. Both numbers come from the engine
/// ([`BattleState::radio_reach`] and the command block itself) rather than
/// from arithmetic repeated here, so the sentence cannot come apart from the
/// ring beside it.
///
/// `None` for a mod that prices no chain of command: there is no net, so
/// there is no line, and the panel is the one it was before any of this.
/// The visual clause is dropped the same way when `visual_range` is zero,
/// which is what a mod that declares radios and no flags looks like.
pub(super) fn format_net(
    registry: &tactics_core::data::DataRegistry,
    state: &BattleState,
    formation: &Formation,
) -> Option<String> {
    let rules = registry.command.as_ref()?;
    let leader = formation.leader?;
    let unit = state.unit(leader)?;
    // The set by name, because it is a thing the vehicle carries — and a
    // receive-only set says so instead of showing a range it does not have.
    let set = registry
        .vehicle(&unit.vehicle)
        .and_then(|v| v.radio.as_deref())
        .and_then(|id| registry.radio(id));
    let mut line = match (set, state.radio_reach(registry, leader)) {
        (Some(set), Some(reach)) => {
            let hardware = set.send.unwrap_or(rules.radius);
            let mut line = format!("Net: {} {reach}", set.name);
            if reach != hardware {
                // The difference between hardware and reach is exactly what
                // the crew is worth, credited to the leader by name; the
                // seat actually working the set may be her operator's,
                // which `crew_skill` already knows.
                line.push_str(&format!(
                    " ({:+}, {}'s signals)",
                    reach as i32 - hardware as i32,
                    unit.name
                ));
            }
            line
        }
        (Some(set), None) => format!("Net: {} — receives only", set.name),
        (None, Some(reach)) => format!("Net: radio {reach}"),
        (None, None) => return None,
    };
    if rules.visual_range > 0 {
        line.push_str(&format!(", visual {}", rules.visual_range));
    }
    Some(line)
}

/// A mission as a sentence a person would say, naming ground the way the map
/// file does so a player can find it again.
pub(super) fn mission_sentence(state: &BattleState, mission: Option<&Mission>) -> String {
    match mission {
        None => "none given".into(),
        Some(Mission::Advance { to }) => format!("advance on {}", hex_label(*to)),
        // Named apart from the advance, because the difference is the whole
        // point: this one does not stop when somebody shoots at it, and the
        // player is entitled to see which of the two her platoon is under.
        Some(Mission::Assault { to }) => format!("assault {}", hex_label(*to)),
        Some(Mission::Hold { at: Some(at) }) => format!("hold {}", hex_label(*at)),
        Some(Mission::Hold { at: None }) => "hold where you are".into(),
        Some(Mission::Recon { toward }) => format!("reconnoitre toward {}", hex_label(*toward)),
        // Named the way the order was given — after the people, not the
        // ground — because that is what a base of fire is about, and the
        // display name is what the player calls that platoon everywhere else
        // in this log.
        Some(Mission::Support { formation }) => format!(
            "stand base of fire for the {}",
            formation_name(state, formation)
        ),
        Some(Mission::Withdraw { via }) => format!(
            "withdraw by {}",
            state
                .map
                .objectives()
                .iter()
                .find(|o| &o.id == via)
                .map(|o| o.name.clone())
                .unwrap_or_else(|| via.clone())
        ),
    }
}

/// One line of the command picture: who was seen, by whom, and how stale the
/// report is.
pub(super) fn format_contact(state: &BattleState, contact: &Contact) -> String {
    let name = |id: UnitId| {
        state
            .units
            .get(id.index())
            .map(|u| u.name.clone())
            .unwrap_or_else(|| "???".into())
    };
    format!(
        "Last reported here\n{}\nby {}, {}",
        name(contact.unit),
        name(contact.reporter),
        report_age(state, contact)
    )
}

/// A hex in the coordinates a map file writes down, which is what a player
/// can count on the board and an author can find in JSON. The axial pair is
/// an implementation detail nobody outside the engine reads.
pub(super) fn hex_label(hex: Hex) -> String {
    let [col, row] = tactics_core::hex_to_offset(hex);
    format!("({col},{row})")
}

/// The shot the player is contemplating, with the arithmetic spelled out.
///
/// Distances lead with the real-world figure and keep the hex count in
/// parentheses: the metre value is what tells the player whether this is a
/// long shot, the hex count is what they need to count tiles on the board.
pub(super) fn format_attack(
    registry: &tactics_core::data::DataRegistry,
    preview: &tactics_core::battle::AttackPreview,
) -> String {
    let scale = &registry.scale;
    let mut lines = vec![
        format!("Attack: {}", preview.target_name),
        format!("{} - {}", preview.target_vehicle, preview.target_side),
        format!(
            "Condition {}%  at {} ({} hexes)",
            preview.target_condition,
            scale.format_distance(preview.distance),
            preview.distance
        ),
        String::new(),
        preview.weapon_name.clone(),
        // On its own line: the panel is 240 px wide and wraps at roughly
        // thirty characters, so a metric range and a hex range do not fit
        // beside a weapon name.
        format!(
            "  rng {} ({}-{})",
            scale.format_range(preview.weapon_range),
            preview.weapon_range[0],
            preview.weapon_range[1]
        ),
    ];
    if !preview.in_range {
        lines.push("OUT OF RANGE".into());
    }
    lines.push(format!("Hit {}%", preview.hit.total));
    lines.push(format!("  base {}", preview.hit.base));
    for modifier in &preview.hit.modifiers {
        lines.push(format!("  {:+} {}", modifier.delta, modifier.label));
    }
    if preview.hit.clamped {
        lines.push(format!(
            "  (capped at {}-{}%)",
            registry.balance.min_hit, registry.balance.max_hit
        ));
    }
    lines.push(String::new());
    match &preview.ammo {
        Some(round) => lines.push(format!(
            "{round}: {}% through {:?} armor {}",
            preview.pen_chance, preview.facing, preview.effective_armor
        )),
        None => lines.push("NO AMMUNITION".into()),
    }
    lines.push(format!(
        "Effect {}  expected {:.1}",
        preview.damage, preview.expected_damage
    ));
    if preview.lethal {
        lines.push("A penetration could finish her.".into());
    }
    match &preview.counter {
        Some(counter) => lines.push(format!(
            "Return fire: {} {}% for {}",
            counter.weapon_name, counter.hit_chance, counter.damage
        )),
        None => lines.push("No return fire.".into()),
    }
    lines.push(String::new());
    if preview.in_range {
        lines.push("A or click to fire".into());
    }
    lines.join("\n")
}

/// The bands the danger overlay paints in, worst last, and the words the
/// panel puts beside them.
///
/// The thresholds are shares of what the crew has *left to lose*, not of her
/// paper complement: a half-wrecked tank is in far more trouble from the same
/// gun than a fresh one, and a legend quoted against the datasheet would tell
/// her otherwise. Index 0 is "nothing spotted bears on it", which is a
/// different statement from "a little" and gets the ordinary move-range blue
/// rather than a colour of its own.
///
/// The words live here and the colours live in `battle.rs`, joined by index,
/// because this file holds no Bevy types — the array the renderer keeps is
/// sized off this one so the two cannot come apart in length.
pub(super) const DANGER_LEGEND: [&str; 4] = [
    "blue nothing bears on it",
    "yellow under a tenth of her",
    "orange under a third",
    "red a third or more",
];

/// Which band a tile falls in, given the expected fire on it as a share of
/// what the crew standing there has left.
///
/// Pure and separately testable on purpose: it is the one piece of judgment
/// in the overlay — every other number in it comes from the resolver — and a
/// band boundary that moved without anybody noticing would recolour the whole
/// map while every test still passed.
pub(super) fn danger_band(share: f32) -> usize {
    if share <= 0.0 {
        0
    } else if share < 0.10 {
        1
    } else if share < 0.33 {
        2
    } else {
        3
    }
}

/// What can be put on the selected crew if she stands on a given hex: every
/// enemy her own side has *found* who could bring a gun to bear, with the
/// resolver's own chance of hitting and what the round is expected to be
/// worth, and the total she would be standing in for a round.
///
/// This is the player's half of `battle::danger::fire_on`, and it is the
/// same call the evaluator makes — that identity is the whole point. The
/// governing principle in DIRECTION.md is that friction the player can
/// predict and price is drama and friction she cannot see is a bug report;
/// until this, the single most expensive decision in the game — where to put
/// a tank — was priced by the AI in numbers no player could read.
///
/// Fog-honest by construction rather than by care taken here: `fire_on` lists
/// only enemies the side has already spotted, so a hex that reads clear may
/// still hold an ambush and the panel is not lying when it says so. It is
/// reporting the *picture*, which is the only thing anybody in this game gets
/// to act on.
///
/// Empty for a crew who is not there. A hex nothing bears on says so out
/// loud, because a section that simply vanished would be indistinguishable
/// from one that had not been written yet.
pub(super) fn format_danger(
    registry: &tactics_core::data::DataRegistry,
    state: &BattleState,
    unit: UnitId,
    at: Hex,
) -> String {
    let Some(me) = state.unit(unit) else {
        return String::new();
    };
    let bearings = tactics_core::battle::fire_on(registry, state, unit, at);
    // Named as a place when it is one and as her own ground when it is not.
    // "Danger at (10,20)" for the hex she is already parked on reads as a
    // question about somewhere else, and the player would go looking for it.
    let mut lines = vec![if at == me.pos {
        "Danger where she stands:".to_string()
    } else {
        format!("Danger at {}:", hex_label(at))
    }];
    if bearings.is_empty() {
        lines.push("  nothing spotted can reach her".into());
        return lines.join("\n");
    }
    let mut total = 0.0;
    for bearing in &bearings {
        total += bearing.expected;
        // Two lines per gun rather than one: the panel is 300 px wide and a
        // line that wraps to three is a line nobody reads. The name and the
        // arithmetic are what a player scans down, so they lead, and the
        // gun that will do it is the detail underneath.
        lines.push(format!(
            "  {}  {}% for {:.1}",
            unit_name(state, bearing.enemy),
            bearing.hit_percent,
            bearing.expected
        ));
        if let Some(gun) = weapon_name(registry, state, bearing) {
            lines.push(format!("    {gun}"));
        }
    }
    lines.push(format!("  {total:.1} expected, one shot each"));
    // ...and what that is worth against her, which is the number that
    // actually decides anything. Two points is a scratch to a heavy tank and
    // the end of a scout car, and a bare figure cannot say which.
    let (have, full) = state.substance(registry, me);
    let left = if have > 0 { have } else { full }.max(1);
    lines.push(format!(
        "  {}% of what she has left",
        ((total / left as f32) * 100.0).round() as i32
    ));
    lines.join("\n")
}

/// The gun behind a bearing, by name. `Bearing::weapon` is an index into the
/// firing chassis' own weapon list, which is the cheapest thing for the
/// engine to carry and the least useful thing to show somebody, so the two
/// lookups happen here rather than in core.
fn weapon_name(
    registry: &tactics_core::data::DataRegistry,
    state: &BattleState,
    bearing: &tactics_core::battle::Bearing,
) -> Option<String> {
    let enemy = state.unit(bearing.enemy)?;
    let vehicle = registry.vehicle(&enemy.vehicle)?;
    let id = vehicle.weapons.get(bearing.weapon)?;
    Some(registry.weapon(id)?.name.clone())
}

/// One crew, as the panel describes her. `own` says whether she is the
/// viewer's to order, which is the only thing that decides whether the
/// order hints belong on the page: telling the player which key would press
/// an *enemy* crew on through fire is nonsense, and worse, it reads as an
/// offer.
pub(super) fn format_unit(
    registry: &tactics_core::data::DataRegistry,
    state: &BattleState,
    unit: &tactics_core::battle::Unit,
    tile: Hex,
    own: bool,
) -> String {
    let vehicle = registry.vehicle(&unit.vehicle);
    let mut lines = vec![
        unit.name.clone(),
        vehicle
            .map(|v| v.name.clone())
            .unwrap_or_else(|| "?".into()),
        format!(
            "Condition {}%",
            (state.condition(registry, unit) * 100.0).round() as i32
        ),
        format!("Side: {}", state.sides[unit.side as usize].name),
    ];
    // Where she is, when "where" is not a tile. A passenger's position
    // mirrors her carrier's, so without this line the panel shows two units
    // apparently standing on one hex and no reason for it. The other half of
    // the same sentence goes on the carrier, because "is my taxi loaded" is a
    // question the map itself stops being able to answer once the ramp is up.
    if let Some(carrier) = unit.aboard.and_then(|c| state.unit(c)) {
        lines.push(format!("Aboard: {}", carrier.name));
    }
    let passengers: Vec<String> = state
        .passengers(unit.id)
        .iter()
        .filter_map(|id| state.unit(*id))
        .map(|u| u.name.clone())
        .collect();
    if !passengers.is_empty() {
        lines.push(format!("Carrying: {}", passengers.join(", ")));
    }
    let scale = &registry.scale;
    if let Some(v) = vehicle {
        lines.push(format!(
            "Armor F{}/S{}/R{}",
            v.armor.front, v.armor.side, v.armor.rear
        ));
        // The crewed figures, not the vehicle's paper ones: what this unit
        // actually does with these cadets aboard is the interesting number,
        // and it is the only place the player can see the crew bonus land.
        let speed = tactics_core::battle::move_points(
            registry,
            &state.roster,
            unit,
            state.terrain_at(unit.pos),
        );
        let vision = tactics_core::battle::stats::vision_range(
            registry,
            &state.roster,
            unit,
            state.terrain_at(unit.pos),
        );
        lines.push(format!("Move {} ({})", scale.format_speed(speed), speed));
        lines.push(format!(
            "Sight {} ({})",
            scale.format_distance(vision as i32),
            vision
        ));
        for weapon in v.weapons.iter().filter_map(|w| registry.weapon(w)) {
            lines.push(format!("  {} dmg {}", weapon.name, weapon.damage));
            lines.push(format!(
                "    {} ({}-{})",
                scale.format_range(weapon.range),
                weapon.range[0],
                weapon.range[1]
            ));
            lines.push(format!(
                "    a shot every {}",
                scale.format_duration(weapon.reload(scale))
            ));
        }
    }
    lines.push("Crew:".into());
    for (seat, c) in unit.crew.iter().enumerate() {
        if let Some(cadet) = state.roster.get(*c) {
            // Her strongest training, named. Words rather than a stat block:
            // cadets read as people when described and as units when
            // tabulated, and the exact numbers belong behind a toggle.
            let mut best: Vec<(&String, &i32)> = cadet.skills.iter().collect();
            best.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            let summary = best
                .iter()
                .take(2)
                .map(|(id, level)| {
                    let name = registry
                        .skill(id)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| (*id).clone());
                    format!("{name} {level}")
                })
                .collect::<Vec<_>>()
                .join(", ");
            // What she is doing, when it is not "her job". A seat nobody is
            // sitting in has to be visible or the crew bonuses simply look
            // wrong: the panel would show a full crew and the tank would
            // drive like an empty one.
            let state_tag = match unit.crew_state.get(seat) {
                Some(CrewCondition::Absent) => " - in the infirmary, not aboard",
                Some(CrewCondition::Wounded) => " - hurt, still at her station",
                Some(CrewCondition::Out) => " - out of the fight",
                _ => "",
            };
            if summary.is_empty() {
                lines.push(format!("  {}{state_tag}", cadet.name));
            } else {
                lines.push(format!("  {} ({summary}){state_tag}", cadet.name));
            }
        }
    }
    // What she is under, and the two ways to change it. The formation panel
    // explains its verbs; this one had nothing at all to say about the key
    // that presses a single crew on through fire, which meant the whole
    // point of being able to insist was discoverable only by reading the
    // changelog. Both promises come from the engine, so the two scales of
    // the same decision are described in the same words.
    if own {
        lines.push(String::new());
        match unit.tasking {
            Some(to) => {
                lines.push(format!(
                    "Marching on {} - {}",
                    hex_label(to),
                    unit.latitude.promise()
                ));
            }
            None => lines.push("No standing destination.".into()),
        }
        lines.push(format!("  click - {}", Latitude::Delegated.promise()));
        lines.push(format!("  X - {}", Latitude::Binding.promise()));
    }
    lines.push(String::new());
    lines.push(format_tile(registry, state, tile));
    lines.join("\n")
}

/// What a tile does to whoever stands on it.
pub(super) fn format_tile(
    registry: &tactics_core::data::DataRegistry,
    state: &BattleState,
    hex: Hex,
) -> String {
    let Some(tile) = state.map.get(hex) else {
        return String::new();
    };
    let Some(terrain) = registry.terrain(&tile.terrain) else {
        return tile.terrain.clone();
    };
    let scale = &registry.scale;
    let mut lines = vec![format!(
        "{} (elev {})",
        terrain.name,
        scale.format_elevation(tile.elevation)
    )];
    if terrain.cover > 0 {
        lines.push(format!(
            "Cover {}%: -{}% to be hit",
            terrain.cover,
            terrain.cover / 2
        ));
    } else {
        lines.push("No cover".into());
    }
    if terrain.vision_block > 0 {
        // Sight blocking is priced in elevation steps, so it is a height:
        // forest stands 20 m over the ground it grows on.
        lines.push(format!(
            "Blocks sight (+{})",
            scale.format_elevation(terrain.vision_block)
        ));
    }
    let costs: Vec<String> = [
        (tactics_core::data::MovementClass::Tracked, "trk"),
        (tactics_core::data::MovementClass::Wheeled, "whl"),
        (tactics_core::data::MovementClass::Foot, "ft"),
    ]
    .iter()
    .map(|(class, label)| match terrain.cost_for(*class) {
        Some(cost) => format!("{label}{cost}"),
        None => format!("{label}-"),
    })
    .collect();
    // Costs are multipliers on a vehicle's speed rather than speeds
    // themselves, so they stay in movement points: "trk2" is half pace.
    lines.push(format!("Move cost {}", costs.join(" ")));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tactics_core::battle::SideState;
    use tactics_core::map::{HexMap, UnitPlacement};

    fn registry() -> tactics_core::data::DataRegistry {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/mods");
        let mut reg = tactics_core::data::DataRegistry::load_dir(&root)
            .expect("mods load")
            .0;
        // The whole reach becomes the near band, so nobody has to be *found*
        // before this stage means anything. The twin of `seen()` in the
        // engine's own tests and here for the same reason: a test about what
        // the panel says must not also be a test of whether anybody happened
        // to roll a spot on the tick it was set up.
        reg.balance.detection_certain_percent = 100;
        reg
    }

    /// Two crews facing each other across three hexes of open grass — close
    /// enough that every gun on the field bears, and in plain sight.
    fn face_to_face(reg: &tactics_core::data::DataRegistry) -> BattleState {
        let file: tactics_core::map::MapFile = serde_json::from_value(serde_json::json!({
            "id": "panel_test_map",
            "palette": { "g": "grass" },
            "rows": ["gggggggg", "gggggggg", "gggggggg"],
        }))
        .expect("the stage parses");
        let map = HexMap::from_map_file(&file).expect("the stage builds");
        let placements = vec![
            placement([0, 1], 0, "medium_tank", "Ours"),
            placement([3, 1], 1, "medium_tank", "Theirs"),
        ];
        let (roster, crews) = tactics_core::roster::Roster::stamp_for(reg, &placements);
        BattleState::from_placements(
            reg,
            map,
            vec![
                SideState {
                    name: "West".into(),
                    ai: None,
                },
                SideState {
                    name: "East".into(),
                    ai: None,
                },
            ],
            &placements,
            &crews,
            std::sync::Arc::new(roster),
            7,
        )
        .expect("the staged placements are content the base mod ships")
    }

    fn placement(at: [i32; 2], side: u8, vehicle: &str, name: &str) -> UnitPlacement {
        UnitPlacement {
            aboard_at: None,
            at,
            side,
            vehicle: vehicle.into(),
            crew: Vec::new(),
            name: Some(name.into()),
            facing: None,
            formation: None,
            leads: false,
        }
    }

    /// The danger section prices a piece of ground in the resolver's own
    /// numbers, names every gun that bears on it, and says so plainly when
    /// none does.
    ///
    /// Three promises, and each is a way this section would quietly stop
    /// being worth reading. A total that is not the sum of the lines above it
    /// is a number a player learns to distrust. A gun that bears and is not
    /// named is exactly the silence the whole feature exists to end. And a
    /// hex nothing can reach that prints a bare heading reads as a panel that
    /// has broken rather than as ground that is safe.
    #[test]
    fn the_danger_line_prices_the_ground_the_way_the_resolver_would() {
        let reg = registry();
        let state = face_to_face(&reg);
        let me = state.units.iter().find(|u| u.side == 0).expect("ours");
        let bearings = tactics_core::battle::fire_on(&reg, &state, me.id, me.pos);
        assert!(
            !bearings.is_empty(),
            "the stage is meaningless if nothing bears on her"
        );

        let here = format_danger(&reg, &state, me.id, me.pos);
        // Her own ground is named as hers: "Danger at (0,1)" for the hex she
        // is parked on sends the player looking for somewhere else.
        assert!(
            here.starts_with("Danger where she stands:"),
            "the heading should say it is her own ground:\n{here}"
        );
        let mut total = 0.0;
        for bearing in &bearings {
            let who = unit_name(&state, bearing.enemy);
            assert!(
                here.contains(&who),
                "{who} bears on her and the panel does not say so:\n{here}"
            );
            assert!(
                here.contains(&format!(
                    "{}% for {:.1}",
                    bearing.hit_percent, bearing.expected
                )),
                "{who}'s shot is not priced the way the resolver prices it:\n{here}"
            );
            total += bearing.expected;
        }
        assert!(
            here.contains(&format!("{total:.1} expected, one shot each")),
            "the total should be the sum of the guns above it ({total:.1}):\n{here}"
        );

        // Ground nothing on the field can reach says so, and names itself,
        // because a heading with nothing under it is indistinguishable from a
        // panel that has stopped working.
        let far = tactics_core::offset_to_hex(0, 0);
        let away = format_danger(&reg, &state, me.id, far);
        assert!(
            away.starts_with(&format!("Danger at {}:", hex_label(far))),
            "ground she is not standing on is named:\n{away}"
        );
    }

    /// The overlay's bands and the words beside them are one table read two
    /// ways, and the boundaries are the only judgment in the whole feature.
    #[test]
    fn every_danger_band_has_a_colour_and_a_sentence() {
        assert_eq!(danger_band(0.0), 0, "nothing bearing is its own band");
        assert_eq!(danger_band(0.05), 1);
        assert_eq!(danger_band(0.2), 2);
        assert_eq!(danger_band(0.9), 3);
        // Monotone, and every band reachable: a boundary typed backwards
        // would recolour the whole map with every other test still green.
        let mut last = 0;
        for step in 0..100 {
            let band = danger_band(step as f32 / 100.0);
            assert!(band >= last, "the bands must not go backwards");
            last = band;
        }
        assert_eq!(last, DANGER_LEGEND.len() - 1);
    }

    /// Every key the order menu offers explains itself.
    ///
    /// The menu joins two tables that live in different crates — the keys
    /// here, the promises in the engine beside the missions they describe —
    /// and the failure mode of a join like that is silent: rename a verb in
    /// core and the panel simply lists one order fewer, which nobody notices
    /// until a player cannot find the key for it.
    #[test]
    fn every_mission_key_has_a_promise() {
        let menu = order_menu();
        assert_eq!(
            menu.len(),
            MISSION_KEYS.len(),
            "an order key dropped out of the menu: {menu:#?}"
        );
        for (key, verb) in MISSION_KEYS {
            assert!(
                menu.iter().any(|line| line.contains(verb)),
                "{key} ({verb}) is offered by the keyboard and explained nowhere"
            );
        }
        // ...and the two that the whole step exists to tell apart are both
        // there, saying different things.
        let advance = menu.iter().find(|l| l.contains("advance")).unwrap();
        let assault = menu.iter().find(|l| l.contains("assault")).unwrap();
        assert_ne!(
            advance.split(" - ").nth(1),
            assault.split(" - ").nth(1),
            "the menu must not describe G and X the same way"
        );
    }
}
