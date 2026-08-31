//! A battlefield with nothing to argue about but skill.
//!
//! The skill-gap table needs ground where the only difference between the two
//! ends is who is commanding, so every feature is declared once and reflected
//! through the centre. That was not always true: it used to be written as rows
//! of ASCII with forest at fixed columns — symmetric *as text*, and sheared
//! into asymmetry by the odd-r offset conversion, which gave side A nine
//! forest hexes near its deployment against side B's four and cost about four
//! points of measured win rate.
//!
//! [`assert_arena_is_mirrored`] is why that cannot come back, and it is called
//! by [`arena_map`] itself rather than being left for a caller to remember.
//!
//! # Why the ground is varied, and why that is not a contradiction
//!
//! The first version of this arena was grass with one belt of trees per side,
//! on the reading that ground worth *nothing* is the cleanest way to isolate
//! execution. That reading is wrong, and three separate measurements said so
//! before anybody worked out why:
//!
//! - `5 over 3` pays about 52% here, which is barely more than a coin, so the
//!   table cannot tell a middling commander from a good one at all.
//! - `--sweep planner.horizon_rounds=2,4,6` came back null: a commander who
//!   plans one round ahead and one who plans eight play the same game.
//! - The road-reading terms — `route_caution`, `contest_aversion` and
//!   `difficulty_foresight` — moved the skill table by less than its own noise.
//!
//! Ground with nothing on it does not isolate skill, it **removes the thing
//! skill is for**. A commander cannot be better at reading a road on a map
//! with no roads, and she cannot be better at picking cover on a map that is
//! all one terrain; blur her judgment as hard as you like and she still ends
//! up on grass, next to grass. What the arena has to be is *symmetric*, which
//! is a completely different requirement from *featureless* — and symmetry is
//! exactly what the point reflection gives for free, however complicated the
//! ground gets.
//!
//! So the arena is now a battlefield: a road down the axis of advance, a
//! village on the ground worth holding, graded hills overlooking both, woods
//! to move through unseen, soft ground that makes leaving the road cost
//! something, and water that has to be gone around.
//!
//! # Two reflections, not one
//!
//! Point symmetry alone is **not enough** once the ground is varied, and
//! finding that out cost a measurement. A first draft of this arena was
//! point-symmetric, passed [`assert_arena_is_mirrored`], and then reported
//! side B winning the equal-skill pairing 25-11 on three seeds of four
//! (52-92 over four seeds, against 73-71 on the old bare arena).
//!
//! The cause is a rule this project has already decided about. A tiebreak may
//! only read quantities a reflection preserves, and where a total order is
//! still needed for determinism the coordinate key goes last, "so the compass
//! decides only a left-or-right choice off the line of advance". Four such
//! tiebreaks exist in `ai/`. They are correct as written — *provided that a
//! left-or-right choice is worth nothing*. The moment the arena had a hill on
//! one flank and water on the other, absolute-left stopped being worth what
//! absolute-right was worth, every crew leaned the same absolute way, and the
//! two ends were no longer facing congruent ground.
//!
//! So the arena is symmetric under **both** reflections: the point reflection
//! through the centre ([`arena_mirror`]), which swaps the two ends, and the
//! reflection across the axis of advance ([`arena_flip`]), which swaps left
//! and right. Every feature is declared once and gets up to four images.
//! Under the second symmetry a compass tiebreak still sends everybody the
//! same absolute way and it no longer pays anybody anything, because the
//! ground it sends them to is congruent to the ground it sends them from.

use crate::Hex;
use crate::map::{HexMap, MapFile};
use std::collections::HashMap;

/// Radius of the arena, in hexes.
pub const ARENA_RADIUS: u32 = 10;

/// Where the arena's centre lands in map-file offset coordinates. The map is
/// built around it, so it is the reflection pivot as well as the middle.
pub const ARENA_CENTRE: [i32; 2] = [15, 10];

pub fn arena_centre() -> Hex {
    crate::offset_to_hex(ARENA_CENTRE[0], ARENA_CENTRE[1])
}

/// The point reflection that swaps the arena's two ends.
///
/// One function, used by the terrain, the elevation, the objectives, the
/// deployment and the test, so there is no second opinion about what "the
/// other side" means.
pub fn arena_mirror(h: Hex) -> Hex {
    arena_centre() * 2 - h
}

/// Declare something on one side and get it on both.
pub fn mirrored(seeds: impl IntoIterator<Item = Hex>) -> Vec<Hex> {
    let mut out = Vec::new();
    for h in seeds {
        out.push(h);
        out.push(arena_mirror(h));
    }
    out.sort_by_key(|h| (h.x, h.y));
    out.dedup();
    out
}

/// Reflection across the axis of advance, which swaps left and right.
///
/// The axis is the line of constant `y` through the centre — the direction a
/// side advances — and in cube coordinates the reflection fixing it is
/// `(x, y, z) -> (-z, -y, -x)`, which in axial terms is `(x, y) -> (x + y,
/// -y)`. It is an isometry, so a feature and its image are the same shape at
/// the same distance from the middle, and it commutes with [`arena_mirror`]:
/// the two generate a four-element group and every feature has one, two or
/// four images under it.
pub fn arena_flip(h: Hex) -> Hex {
    let c = arena_centre();
    let rel = h - c;
    c + Hex::new(rel.x + rel.y, -rel.y)
}

/// Declare something once and get every image of it under both reflections.
///
/// The stronger twin of [`mirrored`], and the one every feature of the arena
/// goes through. See the module doc for why point symmetry alone was not
/// enough.
pub fn symmetric(seeds: impl IntoIterator<Item = Hex>) -> Vec<Hex> {
    let mut out = Vec::new();
    for h in seeds {
        let flipped = arena_flip(h);
        out.extend([h, flipped, arena_mirror(h), arena_mirror(flipped)]);
    }
    out.sort_by_key(|h| (h.x, h.y));
    out.dedup();
    out
}

/// Every hex within `radius` of `at`, `at` included.
///
/// The shape every feature here is built from, because a disc is closed under
/// the point reflection about its own centre — so a mirrored disc is a disc,
/// and there is no orientation to get wrong the way a text rectangle has.
fn disc(at: Hex, radius: u32) -> Vec<Hex> {
    at.range(radius).collect()
}

/// `n` hexes starting at `from` and stepping by `step`.
fn line(from: Hex, step: Hex, n: i32) -> Vec<Hex> {
    (0..n).map(|k| from + step * k).collect()
}

/// The axis of advance, in arena-relative axial coordinates: west is `-x`,
/// and a line of constant `y` is a straight hex line down the middle.
fn rel(x: i32, y: i32) -> Hex {
    arena_centre() + Hex::new(x, y)
}

/// The road, the whole length of the axis of advance.
///
/// Fixed by both reflections, being the axis itself. It is the fast lane —
/// every movement class pays 1 — and it is also the most exposed ground on
/// the map, which is the entire point: a commander who can read a road has
/// something to decide, and one who cannot drives down it.
pub fn arena_road() -> Vec<Hex> {
    line(rel(-9, 0), Hex::new(1, 0), 19)
}

/// The village: the centre hex and its six neighbours.
///
/// Fixed by both reflections, a disc about the centre being fixed by anything
/// that fixes the centre. Town is cover 40 against the grass's nothing, so
/// holding the objective is also *fighting from* the objective — which is
/// what stops the middle of the map from being a place both sides drive past.
pub fn arena_village() -> Vec<Hex> {
    disc(arena_centre(), 1)
}

/// The high ground, and it is the ground worth holding.
///
/// A graded knoll centred on the village: crest at level 2 under the objective
/// itself, a level-1 skirt around it, level 0 beyond. Fixed by both
/// reflections, a disc about the centre being fixed by anything that fixes the
/// centre.
///
/// **Where the hills go is not decoration, and the first draft got it
/// backwards.** Four hills flanking the village put the objective in a hollow
/// overlooked from both ends, which prices holding the ground *below* sitting
/// on the ground above it — so both sides settle on their own ridge and shoot,
/// which is the stalemate objectives were introduced to end. Measured: it took
/// `5 over 1` from 61.5% on the bare arena to 53.9%, because a commander who
/// drives to the objective is punished for it and skill mostly consists of
/// driving to the objective.
///
/// Putting the crest *under* the objective says the opposite and the right
/// thing: the best firing position on the battlefield and the ground that
/// scores are the same hexes, so taking it is worth the drive and holding it
/// is worth the fight. The grading is not decoration either — every vehicle in
/// the base mod has `max_climb: 1`, so a level-2 crest with level-0 ground
/// against it would be an objective nothing can drive onto.
pub fn arena_hill() -> HashMap<Hex, i32> {
    let mut out = HashMap::new();
    let centre = arena_centre();
    for hex in disc(centre, 2) {
        out.insert(hex, if hex.distance_to(centre) <= 1 { 2 } else { 1 });
    }
    out
}

/// The timber: a wood on each flank of each end.
///
/// Cover 30 and concealment 25, at double the movement cost — the covered
/// approach, slower than the road and worth taking anyway. Deliberately clear
/// of every deployment hex, so that a battle does not begin with one crew in
/// the trees and three in the open; [`assert_arena_is_mirrored`] checks it.
pub fn arena_woods() -> Vec<Hex> {
    symmetric(disc(rel(-4, 4), 2))
}

/// Soft going short of the village: no cover, twice the cost to cross.
///
/// This is what makes the road mean something. Without it the open ground
/// beside the road is as quick as the road, so there is no route decision to
/// read and `route_caution` has nothing to trade against.
pub fn arena_mud() -> Vec<Hex> {
    symmetric(disc(rel(-4, 1), 2))
}

/// Water: short reaches that nothing on this battlefield can cross.
///
/// Small on purpose. It exists so that the crow flight and the road disagree
/// — which is the whole of what `difficulty_foresight` blends between, and
/// therefore the only thing on the map that can tell a commander who has read
/// the ground from one who has merely looked at it. A barrier large enough to
/// split the arena would stop the battle from happening at all.
pub fn arena_water() -> Vec<Hex> {
    symmetric(line(rel(-5, 2), Hex::new(0, 1), 3))
}

/// Where each side forms up, and its mirror. Side 0 takes the first of each
/// pair.
///
/// Six hexes on the radius-7 ring, in three [`arena_flip`] pairs, and the two
/// halves of a pair are **adjacent in this list**. That ordering is a contract
/// with the caller: a battle stages the i-th vehicle of each side on the i-th
/// pair, so entries `2k` and `2k + 1` getting the same chassis is what makes
/// the *force* symmetric across the axis of advance as well as the ground.
///
/// It is not a nicety. A laterally symmetric battlefield with a laterally
/// asymmetric force puts the lateral asymmetry straight back: two mediums on
/// one flank and a tank destroyer on the other means absolute-left is worth
/// something different to the two ends again, which is the confound the whole
/// second reflection exists to remove. `arena_flip` is an isometry, so a seed
/// on the ring stays on the ring and all six sit the same distance out.
pub fn arena_deployment() -> Vec<(Hex, Hex)> {
    [rel(-7, 1), rel(-7, 3), rel(-7, 5)]
        .into_iter()
        .flat_map(|h| [h, arena_flip(h)])
        .map(|w| (w, arena_mirror(w)))
        .collect()
}

/// The ground worth holding, as two objectives rather than one.
///
/// The village — the whole seven hexes of it — and a pair of copses out on
/// the flanks, equidistant from both ends. Both sets are mapped onto
/// themselves by both reflections, which is what lets them be scored at all.
///
/// **Two, because one is a funnel, and the funnel showed up in the control
/// row.** With a single seven-hex objective every vehicle on the field
/// converges on the same ground, and at difficulty 5 — where the blur is zero
/// and both sides therefore make identical decisions in mirrored situations —
/// the first crew to reach it is decided by movement resolution order, which
/// walks units in id order and hands side A every race. That is not a coin
/// flip that averages out: it is the same tiebreak in the same direction every
/// battle, so the equal-skill pairing read 34-110 rather than anything near
/// level, and an instrument whose control is degenerate cannot be read.
///
/// Somewhere else worth being is the fix. It costs nothing that the arena is
/// for — the copses are as far from one end as the other — and it means a
/// force has a decision to make about where to weight itself, which is a thing
/// a better commander can be better at.
pub fn arena_objectives() -> Vec<(&'static str, &'static str, u32, Vec<Hex>)> {
    vec![
        ("village", "The Village", 2, arena_village()),
        // `symmetric` folds these two into each other: a hex whose axis
        // projection is zero has its point reflection and its lateral
        // reflection at the same place, so the pair is the whole set.
        ("copses", "The Copses", 2, symmetric([rel(-2, 4)])),
    ]
}

/// Terrain, as layers applied in order — later wins.
///
/// The order is a set of small claims about the ground: soft going is
/// underneath everything because it is what the open ground *is* rather than
/// something on it; trees stand on it; water cuts through trees; the road is
/// cut through all of it, because a road through a wood is a road; and the
/// village is built where the roads meet.
fn arena_layers() -> Vec<(&'static str, Vec<Hex>)> {
    vec![
        ("mud", arena_mud()),
        ("forest", arena_woods()),
        ("water", arena_water()),
        ("road", arena_road()),
        ("town", arena_village()),
    ]
}

pub fn arena_map() -> Option<HexMap> {
    let centre = arena_centre();
    let radius = ARENA_RADIUS as i32;

    let mut terrain: HashMap<Hex, char> = HashMap::new();
    for (glyph, hexes) in arena_layers() {
        let ch = match glyph {
            "mud" => 'm',
            "forest" => 'f',
            "water" => 'w',
            "road" => 'R',
            "town" => 't',
            _ => unreachable!("unknown arena layer `{glyph}`"),
        };
        for hex in hexes {
            if hex.distance_to(centre) <= radius {
                terrain.insert(hex, ch);
            }
        }
    }
    let elevation = arena_hill();

    // A bounding box on the offset grid wide enough for the hexagon. The
    // shear costs half a column per row, so the columns have to stretch by
    // the radius as well as by itself.
    let cols = 0..=ARENA_CENTRE[0] + radius + radius / 2 + 1;
    let mut rows: Vec<String> = Vec::new();
    let mut levels: Vec<String> = Vec::new();
    for row in 0..=ARENA_CENTRE[1] + radius {
        let mut terrain_row = String::new();
        let mut level_row = String::new();
        for col in cols.clone() {
            let hex = crate::offset_to_hex(col, row);
            if hex.distance_to(centre) > radius {
                // A space is "no tile here", which is how a sparse `HexMap`
                // spells the outside of a shape.
                terrain_row.push(' ');
                level_row.push(' ');
            } else {
                terrain_row.push(*terrain.get(&hex).unwrap_or(&'g'));
                level_row.push(match elevation.get(&hex).copied().unwrap_or(0) {
                    0 => '0',
                    1 => '1',
                    n => char::from_digit(n as u32, 10).unwrap_or('9'),
                });
            }
        }
        rows.push(terrain_row);
        levels.push(level_row);
    }

    let objectives: Vec<serde_json::Value> = arena_objectives()
        .into_iter()
        .map(|(id, name, value, hexes)| {
            let at: Vec<[i32; 2]> = hexes.into_iter().map(crate::hex_to_offset).collect();
            serde_json::json!({ "id": id, "name": name, "at": at, "value": value })
        })
        .collect();
    let file: MapFile = serde_json::from_value(serde_json::json!({
        "id": "skill_arena",
        "kind": "battle",
        "shape": "free",
        "palette": {
            "g": "grass", "f": "forest", "R": "road",
            "m": "mud", "w": "water", "t": "town",
        },
        "rows": rows,
        "elevation": levels,
        "objectives": objectives,
        "victory_score": 30,
    }))
    .ok()?;
    let map = HexMap::from_map_file(&file).ok()?;
    assert_arena_is_mirrored(&map);
    Some(map)
}

/// Refuse to hand back an arena that is not the mirror it claims to be.
///
/// A runtime check rather than a test, because the thing being defended is a
/// *claim in a table heading*: `skill gap: ... on a mirrored arena`. The first
/// version of this arena was asymmetric for months — nine forest hexes within
/// six of one deployment against four of the other — and no test failed,
/// because there was no test, because the symmetry was believed rather than
/// asserted. It costs about a microsecond per run of a table that fights
/// hundreds of battles, and every number the skill-gap and brains tables print
/// depends on it being true.
///
/// It compares elevation as well as terrain. That is not hypothetical
/// tidiness: the ground carries a hill now, and a hill whose mirror is half a
/// level lower is worth a shot from one end of the map and not the other,
/// which is the same confound as the sheared woods in a form nobody would see
/// by reading the map.
pub fn assert_arena_is_mirrored(map: &HexMap) {
    let tiles: HashMap<Hex, (&str, i32)> = map
        .iter()
        .map(|(h, t)| (h, (t.terrain.as_str(), t.elevation)))
        .collect();
    for (hex, ground) in &tiles {
        let mirror = arena_mirror(*hex);
        match tiles.get(&mirror) {
            None => panic!("the arena has {hex:?} but not its mirror {mirror:?}"),
            Some(other) => assert_eq!(
                ground, other,
                "the arena is {ground:?} at {hex:?} and {other:?} at its mirror {mirror:?}"
            ),
        }
    }
    for objective in map.objectives() {
        for hex in &objective.hexes {
            assert!(
                objective.hexes.contains(&arena_mirror(*hex)),
                "objective `{}` holds {hex:?} but not its mirror",
                objective.id
            );
        }
    }
    // ...and across the axis of advance, which is the symmetry that keeps a
    // left-or-right tiebreak from being worth anything. See the module doc.
    for (hex, ground) in &tiles {
        let flipped = arena_flip(*hex);
        match tiles.get(&flipped) {
            None => panic!("the arena has {hex:?} but not its flip {flipped:?}"),
            Some(other) => assert_eq!(
                ground, other,
                "the arena is {ground:?} at {hex:?} and {other:?} across the axis at {flipped:?}"
            ),
        }
    }
    for objective in map.objectives() {
        for hex in &objective.hexes {
            assert!(
                objective.hexes.contains(&arena_flip(*hex)),
                "objective `{}` holds {hex:?} but not its flip",
                objective.id
            );
        }
    }
    let deployment: std::collections::HashSet<Hex> = arena_deployment()
        .into_iter()
        .flat_map(|(w, e)| [w, e])
        .collect();
    let pairs = arena_deployment();
    assert!(
        pairs.len().is_multiple_of(2),
        "the deployment must come in flip pairs, got {}",
        pairs.len()
    );
    for k in (0..pairs.len()).step_by(2) {
        assert_eq!(
            arena_flip(pairs[k].0),
            pairs[k + 1].0,
            "deployment entries {k} and {} are not a flip pair, so the i-th \
             vehicle of each side cannot be placed symmetrically about the axis",
            k + 1
        );
    }
    for (west, east) in arena_deployment() {
        assert_eq!(
            arena_mirror(west),
            east,
            "the deployment pair {west:?}/{east:?} are not mirrors"
        );
        assert!(
            deployment.contains(&arena_flip(west)),
            "the deployment hex {west:?} has no counterpart across the axis"
        );
        for hex in [west, east] {
            match tiles.get(&hex) {
                None => panic!("the deployment hex {hex:?} is not on the map"),
                // Every crew starts on the same kind of ground. A battle that
                // begins with one vehicle in the trees and three in the open
                // is measuring the deployment and not the commanders, and the
                // woods here are a disc that can drift onto the ring.
                Some((terrain, _)) => assert_eq!(
                    *terrain, "grass",
                    "the deployment hex {hex:?} is `{terrain}`, not open ground"
                ),
            }
        }
    }
}
