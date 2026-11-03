//! SVG drawing: the ground once, then panels of it with something laid
//! over each. No dependencies; any browser opens the file. (ImageMagick's
//! own SVG reader ignores `fill-opacity`; render with a browser, e.g.
//! `google-chrome --headless --screenshot`.)

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use tactics_core::battle::BattleState;
use tactics_core::hex_to_offset;

use crate::tally::{Cell, HURT, Heat, LAYERS, SEEN};

const R: f64 = 10.0;
pub const SIDE_COLOUR: [&str; 2] = ["#1f5fbf", "#c0392b"];
/// More in the second run than the first, and less: a diverging pair that
/// is neither side's colour.
const MORE: &str = "#e66101";
const LESS: &str = "#5e3c99";
const NEUTRAL: &str = "#222222";

pub fn centre(c: i32, r: i32) -> (f64, f64) {
    let w = 3f64.sqrt() * R;
    (
        w * (f64::from(c) + 0.5 * f64::from(r & 1)) + w / 2.0,
        1.5 * R * f64::from(r) + R,
    )
}

pub fn hexagon(c: i32, r: i32) -> String {
    let (x, y) = centre(c, r);
    let pts: Vec<String> = (0..6)
        .map(|i| {
            let a = std::f64::consts::PI / 180.0 * (60.0 * f64::from(i) - 30.0);
            format!("{:.1},{:.1}", x + R * a.cos(), y + R * a.sin())
        })
        .collect();
    pts.join(" ")
}

fn terrain_colour(t: &str) -> &'static str {
    match t {
        "plains" => "#ece6cf",
        "grass" => "#dbe6c0",
        "road" => "#c9bca0",
        "town" => "#a9a2a0",
        "forest" => "#9fbf8f",
        "deep_forest" => "#86a878",
        "hedgerow" => "#c4d4a4",
        "stream" | "water" => "#a9cbe3",
        "mud" => "#cbbfa6",
        _ => "#ffffff",
    }
}

/// A hex filled with `colour` at `alpha`.
pub fn fill([c, r]: Cell, colour: &str, alpha: f64) -> String {
    format!(
        r##"<polygon points="{}" fill="{colour}" fill-opacity="{alpha:.2}"/>"##,
        hexagon(c, r)
    )
}

fn cross([c, r]: Cell, k: f64, colour: &str) -> String {
    let (x, y) = centre(c, r);
    format!(
        r##"<path d="M{:.1},{:.1}L{:.1},{:.1}M{:.1},{:.1}L{:.1},{:.1}" stroke="{colour}" stroke-width="1.8"/>"##,
        x - k,
        y - k,
        x + k,
        y + k,
        x - k,
        y + k,
        x + k,
        y - k
    )
}

fn line(a: Cell, b: Cell, colour: &str, opacity: f64, width: f64, dash: bool) -> String {
    let (x1, y1) = centre(a[0], a[1]);
    let (x2, y2) = centre(b[0], b[1]);
    format!(
        r##"<line x1="{x1:.1}" y1="{y1:.1}" x2="{x2:.1}" y2="{y2:.1}" stroke="{colour}" stroke-opacity="{opacity:.2}" stroke-width="{width:.1}"{}/>"##,
        if dash {
            r##" stroke-dasharray="3,2""##
        } else {
            ""
        }
    )
}

fn dot([c, r]: Cell, radius: f64, colour: &str) -> String {
    let (x, y) = centre(c, r);
    format!(
        r##"<circle cx="{x:.1}" cy="{y:.1}" r="{radius:.1}" fill="{colour}" fill-opacity="0.7"/>"##
    )
}

/// A page of map panels in two columns.
pub struct Canvas {
    base: String,
    marks: String,
    pw: f64,
    ph: f64,
    top: f64,
    svg: String,
}

const HEAD: f64 = 26.0;
const GAP: f64 = 12.0;

impl Canvas {
    pub fn new(ground: &BattleState, rows: usize, title: &str, lines: &[String]) -> Self {
        let (mut w, mut h) = (0, 0);
        for (hex, _) in ground.world.iter() {
            let [c, r] = hex_to_offset(hex);
            w = w.max(c + 1);
            h = h.max(r + 1);
        }
        let pw = 3f64.sqrt() * R * (f64::from(w) + 0.5) + 4.0;
        let ph = 1.5 * R * f64::from(h) + R + 4.0;

        // Height as shade: each level darkens the ground a step, so the
        // plateau, the slope and the valley read without a legend.
        let mut base = String::new();
        for (hex, tile) in ground.world.iter() {
            let [c, r] = hex_to_offset(hex);
            writeln!(
                base,
                r##"<polygon points="{p}" fill="{}" stroke="#ffffff" stroke-width="0.3"/><polygon points="{p}" fill="#3a2a10" fill-opacity="{:.2}"/>"##,
                terrain_colour(tile.terrain),
                0.07 * f64::from(tile.elevation),
                p = hexagon(c, r),
            )
            .ok();
        }
        let objective: BTreeSet<Cell> = ground
            .objectives()
            .flat_map(|(o, _)| o.hexes.iter().map(|h| hex_to_offset(*h)))
            .collect();
        let mut marks = String::new();
        for &[c, r] in &objective {
            writeln!(
                marks,
                r##"<polygon points="{}" fill="none" stroke="#222" stroke-width="1.6"/>"##,
                hexagon(c, r)
            )
            .ok();
        }

        let top = 30.0 + 16.0 * lines.len() as f64;
        let total_w = 2.0 * pw + 3.0 * GAP;
        let total_h = top + rows as f64 * (ph + HEAD + GAP);
        let mut svg = String::new();
        writeln!(
            svg,
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{total_w:.0}" height="{total_h:.0}" font-family="sans-serif" font-size="12">"##
        )
        .ok();
        writeln!(
            svg,
            r##"<rect width="100%" height="100%" fill="#fbfaf7"/>"##
        )
        .ok();
        writeln!(
            svg,
            r##"<text x="{GAP}" y="18" font-size="15" font-weight="bold">{}</text>"##,
            escape(title)
        )
        .ok();
        for (i, l) in lines.iter().enumerate() {
            writeln!(
                svg,
                r##"<text x="{GAP}" y="{}" fill="#555">{}</text>"##,
                34.0 + 16.0 * i as f64,
                escape(l)
            )
            .ok();
        }
        Canvas {
            base,
            marks,
            pw,
            ph,
            top,
            svg,
        }
    }

    pub fn panel(&mut self, row: usize, col: usize, label: &str, colour: &str, body: &str) {
        let x = GAP + col as f64 * (self.pw + GAP);
        let y = self.top + row as f64 * (self.ph + HEAD + GAP);
        writeln!(
            self.svg,
            r##"<g transform="translate({x:.1},{y:.1})"><text x="0" y="16" font-weight="bold" fill="{colour}">{}</text><g transform="translate(0,{HEAD})">{}{body}{}</g></g>"##,
            escape(label),
            self.base,
            self.marks
        )
        .ok();
    }

    pub fn finish(mut self) -> String {
        self.svg.push_str("</svg>\n");
        self.svg
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn max_of<'a>(lists: impl IntoIterator<Item = &'a Vec<(Cell, f64)>>) -> f64 {
    lists
        .into_iter()
        .flat_map(|l| l.iter().map(|(_, v)| *v))
        .fold(0.0f64, f64::max)
}

/// Heat over the ground: every layer for both sides, then lines of fire.
pub fn heat(ground: &BattleState, heat: &Heat) -> String {
    let per = f64::from(heat.battles.max(1));
    let names = &heat.names;
    let rows = LAYERS.len() + 1;
    let mut page = Canvas::new(
        ground,
        rows,
        &heat.title,
        &[
            format!(
                "{} won {}, {} won {}, of {}.",
                names[0], heat.wins[0], names[1], heat.wins[1], heat.battles
            ),
            "Values per battle; both sides of a row share one scale. Black outlines: objectives. Darker ground is higher.".into(),
        ],
    );
    for (l, layer) in LAYERS.iter().enumerate() {
        let max = max_of(&heat.layers[l]);
        for s in 0..2 {
            let mut body = String::new();
            for &(cell, n) in &heat.layers[l][s] {
                // Under 2% of the row's busiest hex is a stray, not a
                // pattern.
                if n < 0.02 * max {
                    continue;
                }
                body.push_str(&fill(
                    cell,
                    SIDE_COLOUR[s],
                    0.08 + 0.8 * (n / max).powf(0.7),
                ));
            }
            if l == HURT {
                for &(cell, n) in &heat.lost[s] {
                    body.push_str(&cross(cell, 2.5 + 2.5 * (n / per).sqrt().min(2.0), "#000"));
                }
            }
            // Where this side was first seen, and from how near: the other
            // side's first contacts, dashed from the nearest spotter.
            if l == SEEN {
                for c in &heat.first[1 - s] {
                    body.push_str(&line(c.spotter, c.spotted, NEUTRAL, 0.7, 1.2, true));
                    body.push_str(&dot(c.spotted, 2.2, NEUTRAL));
                }
            }
            let label = format!(
                "{} — {layer}{} (row max {:.1})",
                names[s],
                match l {
                    HURT => ", × = lost",
                    SEEN => ", dashed = first contact",
                    _ => "",
                },
                max / per
            );
            page.panel(l, s, &label, SIDE_COLOUR[s], &body);
        }
    }

    let lmax = heat
        .lines
        .iter()
        .flat_map(|m| m.iter().map(|(_, v)| *v))
        .fold(0.0f64, f64::max);
    for s in 0..2 {
        let mut body = String::new();
        let mut v: Vec<_> = heat.lines[s].iter().collect();
        v.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        for &&((f, t), n) in v.iter().rev().take(150).rev() {
            let k = (n / lmax).sqrt();
            body.push_str(&line(
                f,
                t,
                SIDE_COLOUR[s],
                0.25 + 0.6 * k,
                0.6 + 3.5 * k,
                false,
            ));
            body.push_str(&dot(t, 0.8 + 1.5 * k, SIDE_COLOUR[s]));
        }
        let label = format!(
            "{} — lines of fire, dot at the target (busiest 150 pairs, max {:.1})",
            names[s],
            lmax / per
        );
        page.panel(LAYERS.len(), s, &label, SIDE_COLOUR[s], &body);
    }
    page.finish()
}

/// The difference between two runs, layer by layer: orange where the second
/// has more per battle, purple where it has less.
pub fn diff(ground: &BattleState, a: &Heat, b: &Heat) -> String {
    let (pa, pb) = (f64::from(a.battles.max(1)), f64::from(b.battles.max(1)));
    let names = &b.names;
    let mut page = Canvas::new(
        ground,
        LAYERS.len(),
        &format!("B minus A, per battle: {}", b.map),
        &[
            format!(
                "A: {} ({}–{} of {})",
                a.title, a.wins[0], a.wins[1], a.battles
            ),
            format!(
                "B: {} ({}–{} of {})",
                b.title, b.wins[0], b.wins[1], b.battles
            ),
            "Orange: more in B. Purple: less in B. Both sides of a row share one scale.".into(),
        ],
    );
    for (l, layer) in LAYERS.iter().enumerate() {
        let mut deltas: [BTreeMap<Cell, f64>; 2] = Default::default();
        for (s, delta) in deltas.iter_mut().enumerate() {
            for &(c, n) in &a.layers[l][s] {
                *delta.entry(c).or_default() -= n / pa;
            }
            for &(c, n) in &b.layers[l][s] {
                *delta.entry(c).or_default() += n / pb;
            }
        }
        let max = deltas
            .iter()
            .flat_map(|m| m.values().map(|v| v.abs()))
            .fold(0.0f64, f64::max)
            .max(1e-9);
        for (s, d) in deltas.iter().enumerate() {
            let mut body = String::new();
            for (&cell, &v) in d {
                if v.abs() < 0.05 * max {
                    continue;
                }
                let colour = if v > 0.0 { MORE } else { LESS };
                body.push_str(&fill(cell, colour, 0.1 + 0.8 * (v.abs() / max).powf(0.7)));
            }
            let total: f64 = d.values().sum();
            page.panel(
                l,
                s,
                &format!(
                    "{} — {layer} (net {total:+.1}, row max ±{max:.1})",
                    names[s]
                ),
                SIDE_COLOUR[s],
                &body,
            );
        }
    }
    page.finish()
}

/// Sight lines: how much ground each hex overlooks, and what chosen hexes
/// see.
pub fn sight(
    ground: &BattleState,
    map: &str,
    range: u32,
    overlook: &BTreeMap<Cell, f64>,
    views: &[(String, Cell, BTreeSet<Cell>)],
) -> String {
    let rows = 1 + views.len().div_ceil(2);
    let mut page = Canvas::new(
        ground,
        rows,
        &format!("{map}: sight lines within {range} hexes"),
        &[
            "Geometry only (terrain, height, the eye heights in balance): no concealment, no detection roll, no smoke.".into(),
            "Darker green: this hex sees more of the map. A view panel shows what one hex sees (and, by symmetry, what sees it).".into(),
        ],
    );
    let max = overlook.values().copied().fold(0.0f64, f64::max).max(1.0);
    let mut body = String::new();
    for (&cell, &n) in overlook {
        body.push_str(&fill(cell, "#1b7837", 0.05 + 0.8 * (n / max)));
    }
    page.panel(
        0,
        0,
        &format!("hexes in sight within {range} (max {max:.0})"),
        "#1b7837",
        &body,
    );
    // The same, inverted: where a crew is hard to see from anywhere.
    let mut body = String::new();
    for (&cell, &n) in overlook {
        body.push_str(&fill(cell, "#762a83", 0.05 + 0.8 * (1.0 - n / max)));
    }
    page.panel(0, 1, "hidden ground (the inverse)", "#762a83", &body);
    for (i, (label, from, seen)) in views.iter().enumerate() {
        let mut body = String::new();
        for &cell in seen {
            body.push_str(&fill(cell, "#f1a340", 0.55));
        }
        body.push_str(&cross(*from, 5.0, "#000"));
        page.panel(
            1 + i / 2,
            i % 2,
            &format!(
                "seen from {label} ({},{}): {} hexes",
                from[0],
                from[1],
                seen.len()
            ),
            "#333",
            &body,
        );
    }
    page.finish()
}
