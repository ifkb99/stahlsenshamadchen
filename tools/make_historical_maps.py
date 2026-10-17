#!/usr/bin/env python3
"""Generate the battlefields of real battles, to compare the game against.

    python3 tools/make_historical_maps.py            # write assets/mods/base/maps
    python3 tools/make_historical_maps.py --check    # fail if the files would move
    python3 tools/make_historical_maps.py --render   # print the maps as ASCII

Three maps (ASSAULT.md, *Real battles*), each an assault on defended ground
the way `harness::sector` lays one out, so what the game does on them can be
read against what happened:

- `history_noville`: Team Desobry holding Noville against 2. Panzer-
  Division on the morning of 19 December 1944, in fog that lifted at 10:30.
  A standard battle map, radius 20.
- `history_valley_of_tears`: 7th Armored Brigade's ramps north of Kuneitra
  against a Syrian assault wave, October 1973. Radius 30.
- `history_arracourt`: Combat Command A of 4th Armored Division against
  Panzer-Brigade 113 in the fog of 19 September 1944. Radius 32.

# What is real and what is drawn

**The relief is measured.** `tools/history/<id>_elevation_250m.csv` is the
ground sampled every 250 m (EU-DEM 25 m for France and Belgium, produced
using Copernicus data; SRTM 30 m for the Golan, public domain), and every
hex takes the bilinear reading at its centre, in 10 m levels above the
lowest ground on the map (`0`-`9`, then `a`-`z`). A surface model reads
woods and roofs too, five to twenty metres high; it agrees with the
historical spot heights to within about five.

**The rest is drawn from the histories**, not copied from a modern map:
villages, roads, streams and woods are placed by their coordinates in the
accounts (Cole's *The Ardennes* and *The Lorraine Campaign*, Marshall's
*Bastogne: The First Eight Days*; Asher and Rabinovich, through the
secondary sources the research memo names), because the ground of 1944 or
1973 is not today's: Normandy's orchards are gone and the Golan's were
planted after the war.

**The forces are scaled, and our chassis are not theirs.** A Sherman and a
Panther are both the mod's medium tank; an M18 is its tank destroyer. What
can be compared with history is the shape of the fight and how it ended,
not the exchange ratio of two particular guns.

# The geometry

Rows are odd-r offset coordinates over pointy-top hexes 100 m across
(`tactics_core::offset_to_hex`): a column is 100 m east, a row 86.6 m
south. The map is the hexagon of radius `R` round row and column `R`.
"""

import argparse
import csv
import json
import math
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE.parent / "assets" / "mods" / "base" / "maps"
DATA = HERE / "history"

ROW_KM = 0.0866
COL_KM = 0.1
LEVELS = "0123456789abcdefghijklmnopqrstuvwxyz"


# --- the ground ---------------------------------------------------------------


class Elevation:
    """A 250 m grid of readings, read bilinearly."""

    def __init__(self, battle):
        self.at = {}
        with open(DATA / f"{battle}_elevation_250m.csv") as f:
            for row in csv.DictReader(f):
                key = (round(float(row["x_km_east"]) * 4), round(float(row["y_km_north"]) * 4))
                self.at[key] = float(row["elev_m"])

    def __call__(self, x, y):
        gx, gy = x * 4, y * 4
        x0, y0 = math.floor(gx), math.floor(gy)
        fx, fy = gx - x0, gy - y0
        corners = [self.at.get((x0 + dx, y0 + dy)) for dx, dy in ((0, 0), (1, 0), (0, 1), (1, 1))]
        if any(c is None for c in corners):
            raise ValueError(f"no elevation reading near ({x:.2f}, {y:.2f}) km")
        a, b, c, d = corners
        return (a * (1 - fx) + b * fx) * (1 - fy) + (c * (1 - fx) + d * fx) * fy


class Map:
    """A hexagon of radius `radius` laid on the ground with its centre at
    (`cx`, `cy`) km, every tile grass until something is drawn on it."""

    def __init__(self, battle, radius, cx, cy):
        self.radius, self.cx, self.cy = radius, cx, cy
        self.elevation = Elevation(battle)
        self.terrain = {cell: "grass" for cell in self.cells()}
        self.units = []
        self.formations = []
        self.objectives = []

    def cells(self):
        rows = 2 * self.radius + 1
        for row in range(rows):
            depth = abs(row - self.radius)
            for col in range(depth // 2, depth // 2 + rows - depth):
                yield col, row

    def km(self, cell):
        col, row = cell
        x = self.cx + (col + 0.5 * (row % 2) - self.radius - 0.5 * (self.radius % 2)) * COL_KM
        y = self.cy - (row - self.radius) * ROW_KM
        return x, y

    def cell(self, x, y):
        """The cell nearest (`x`, `y`) km, or `None` off the map."""
        row = round(self.radius - (y - self.cy) / ROW_KM)
        col = round((x - self.cx) / COL_KM + self.radius + 0.5 * (self.radius % 2) - 0.5 * (row % 2))
        return (col, row) if (col, row) in self.terrain else None

    def near(self, x, y, within):
        """Every cell whose centre is within `within` km of (`x`, `y`)."""
        return [c for c in self.terrain if math.dist(self.km(c), (x, y)) <= within]

    def paint(self, terrain, cells, over=None):
        for c in cells:
            if over is None or self.terrain[c] in over:
                self.terrain[c] = terrain

    def blob(self, terrain, x, y, within, over=None):
        self.paint(terrain, self.near(x, y, within), over)

    def line(self, terrain, points, width=0.0, over=None):
        """`terrain` along the polyline through `points`, every cell a
        sample lands in (samples every 30 m, so the line is unbroken)."""
        cells = []
        for (x0, y0), (x1, y1) in zip(points, points[1:]):
            steps = max(1, int(math.dist((x0, y0), (x1, y1)) / 0.03))
            for i in range(steps + 1):
                t = i / steps
                x, y = x0 + (x1 - x0) * t, y0 + (y1 - y0) * t
                if width > 0:
                    cells.extend(self.near(x, y, width))
                else:
                    c = self.cell(x, y)
                    if c:
                        cells.append(c)
        self.paint(terrain, cells, over)

    def below(self, terrain, metres, over=("grass",)):
        """`terrain` wherever the ground lies lower than `metres`."""
        self.paint(terrain, [c for c in self.terrain if self.elevation(*self.km(c)) < metres], over)

    # --- the scenario -------------------------------------------------------

    def objective(self, id, name, x, y, within, held=None, kind=None, side=None, value=1):
        cells = sorted(self.near(x, y, within), key=lambda c: math.dist(self.km(c), (x, y)))
        if not cells:
            raise ValueError(f"objective {id} is off the map")
        spec = {"id": id, "name": name, "at": [list(c) for c in cells], "value": value}
        if kind:
            spec["kind"] = kind
        if side is not None:
            spec["side"] = side
        if held is not None:
            spec["held"] = held
        self.objectives.append(spec)

    def formation(self, id, name, side, doctrine=None):
        spec = {"id": id, "name": name, "side": side}
        if doctrine:
            spec["doctrine"] = doctrine
        self.formations.append(spec)

    def place(self, side, formation, vehicle, count, x, y, name, carries=None, spread=0.25, dismounted=False):
        """`count` of `vehicle` round (`x`, `y`) km, each on the nearest free
        cell it can stand on, the first leading the formation. `dismounted`
        starts what she carries on the ground beside her: a prepared
        defence has had the hours to get its riflemen into their holes."""
        taken = {tuple(u["at"]) for u in self.units if "aboard_at" not in u}
        foot = vehicle in ("rifle_platoon", "rifle_squad", "scout_section")
        stands = {"water"} if foot else {"water", "tank_ditch", "mountains"}
        free = sorted(
            (c for c in self.near(x, y, spread + 0.6) if c not in taken and self.terrain[c] not in stands),
            key=lambda c: (math.dist(self.km(c), (x, y)), c),
        )
        leading = not any(u.get("formation") == formation for u in self.units)
        for i in range(count):
            if i >= len(free):
                raise ValueError(f"no room for {name} {i + 1} near ({x}, {y})")
            # Every other free cell, so a platoon stands in a loose line.
            cell = free[min(i * 2, len(free) - 1 - (count - 1 - i))]
            free.remove(cell)
            unit = {
                "at": list(cell),
                "side": side,
                "vehicle": vehicle,
                "name": f"{name} {i + 1}" if count > 1 else name,
                "formation": formation,
            }
            if leading and i == 0:
                unit["leads"] = True
            self.units.append(unit)
            if carries:
                rider = {
                    "at": list(cell),
                    "side": side,
                    "vehicle": carries,
                    "name": f"{unit['name']}'s section",
                    "formation": formation,
                }
                if not dismounted:
                    rider = {"at": list(cell), "aboard_at": list(cell), **{k: v for k, v in rider.items() if k != "at"}}
                self.units.append(rider)

    def file(self, id, name, sides, scenario):
        rows = 2 * self.radius + 1
        palette, glyphs = {}, {}
        for t in sorted(set(self.terrain.values())):
            g = GLYPHS[t]
            palette[g] = t
            glyphs[t] = g
        heights = {c: self.elevation(*self.km(c)) for c in self.terrain}
        base = math.floor(min(heights.values()) / 10) * 10
        terrain_rows, elevation_rows = [], []
        for row in range(rows):
            depth = abs(row - self.radius)
            first = depth // 2
            t = [" "] * first
            e = [" "] * first
            for col in range(first, first + rows - depth):
                t.append(glyphs[self.terrain[(col, row)]])
                level = round((heights[(col, row)] - base) / 10)
                if level >= len(LEVELS):
                    raise ValueError(f"{id}: {level} levels of relief is more than a map can say")
                e.append(LEVELS[level])
            terrain_rows.append("".join(t))
            elevation_rows.append("".join(e))
        spec = {
            "id": id,
            "name": name,
            "kind": "battle",
        }
        if self.radius != 20:
            spec["shape"] = "free"
        spec.update(
            {
                "palette": palette,
                "rows": terrain_rows,
                "elevation": elevation_rows,
                "sides": sides,
                "formations": self.formations,
                "units": self.units,
                "objectives": self.objectives,
            }
        )
        spec.update(scenario)
        return spec


GLYPHS = {
    "grass": "g",
    "road": "R",
    "forest": "f",
    "hedgerow": "h",
    "water": "w",
    "stream": "s",
    "mud": "m",
    "town": "t",
    "plains": "p",
    "mountains": "M",
    "wire": "x",
    "tank_ditch": "d",
}


# --- Noville, 19 December 1944 -------------------------------------------------


def noville():
    """Team Desobry in Noville, the morning the fog lifted.

    Noville sits in a dish: "relatively high ground… commanded by two ridges
    from about 800 yards, one in the southeast and the other running from
    north to northwest" (Marshall). The panzers came over both. At 10:30 the
    fog lifted on "at least thirty" tanks; fourteen came on in a line short
    of Vaux, nine were hit almost at once and the assault ebbed by 11:30.
    The ground off the roads was too soft for tank manoeuvre (Cole), so the
    low ground is mud. No mines: the engineers could not lay them for the
    stragglers on the roads. The Americans had been in the village since the
    evening before, so they are dug in.

    History: the attack of the 19th failed; Noville was given up by order on
    the 20th, after Foy and the road behind were cut.
    """
    m = Map("nov", 20, 0.1, 0.25)
    m.below("mud", 445)
    # The village along the Houffalize road, and its neighbours.
    m.line("town", [(0.02, 0.78), (0.14, 1.03), (0.28, 1.32)], width=0.17)
    m.blob("town", 0.32, 0.98, 0.12)
    m.blob("town", -0.67, 0.71, 0.12)
    m.blob("town", -0.67, -1.16, 0.18)
    m.blob("town", -1.66, -0.34, 0.14)
    m.blob("town", -1.28, 1.81, 0.1)
    # Woods: the copses along the Bourcy road and the edge of Bois Jacques.
    m.blob("forest", 1.05, 0.75, 0.16)
    m.blob("forest", 1.15, 1.45, 0.12)
    m.line("forest", [(-0.95, -1.5), (-0.35, -1.55)], width=0.12)
    # The draw north-west of the village where four panzers hid.
    m.line("stream", [(-1.7, 0.4), (-1.28, 1.81), (-0.6, 2.25)])
    # Roads: the N30 straight through, the Bourcy and Vaux roads, and the
    # side road to Cobru and Recogne.
    m.line("road", [(-0.95, -2.0), (-0.67, -1.16), (-0.2, -0.1), (0.14, 1.03), (0.45, 2.5)])
    m.line("road", [(0.18, 1.0), (1.2, 0.85), (2.5, 0.6)])
    m.line("road", [(0.1, 1.08), (-0.51, 2.25)])
    m.line("road", [(0.05, 0.92), (-0.67, 0.71), (-1.66, -0.34), (-0.67, -1.16)])

    m.objective("noville", "Noville", 0.14, 1.03, 0.2, held=1)
    m.objective("cobru", "the Cobru crossroads", -0.67, 0.71, 0.12, held=1)
    m.objective("foy", "Foy and the road home", -0.67, -1.16, 0.15, held=1)
    m.objective("south", "the Bastogne road", -0.85, -1.45, 0.12, kind="exit", side=1)
    m.objective("north", "the Houffalize road", 0.36, 1.97, 0.08, kind="exit", side=0)

    # 2. Panzer-Division: a line of tanks on the ridge short of Vaux, more
    # over the south-east ridge on the Bourcy road, grenadiers behind.
    m.formation("pz_north_1", "Panzer company, north (1)", 0)
    m.formation("pz_north_2", "Panzer company, north (2)", 0)
    m.formation("pz_east", "Panzer company, south-east ridge", 0)
    m.formation("grenadiers", "Panzergrenadiers", 0)
    m.formation("pz_artillery", "Divisional artillery", 0)
    m.place(0, "pz_north_1", "medium_tank", 5, -0.45, 1.7, "Panzer N", spread=0.3)
    m.place(0, "pz_north_2", "medium_tank", 5, 0.75, 1.6, "Panzer NE", spread=0.3)
    m.place(0, "pz_east", "medium_tank", 6, 1.35, 0.75, "Panzer SE", spread=0.3)
    m.place(0, "grenadiers", "halftrack", 4, -0.1, 1.7, "Grenadiers", carries="rifle_platoon")
    m.place(0, "pz_artillery", "artillery", 2, 1.6, 1.4, "Battery")

    # Team Desobry: B/3rd Tank Bn's Shermans round the village, B/20th AIB
    # in it, an M18 platoon come up from the south, M8s at Cobru, and the
    # 420th's guns firing from back toward Bastogne.
    m.formation("desobry_north", "B/3rd Tank, north edge", 1)
    m.formation("desobry_east", "B/3rd Tank, Bourcy road", 1)
    m.formation("desobry_west", "B/3rd Tank, Vaux road", 1)
    m.formation("b20_infantry", "B/20th Armored Infantry", 1)
    m.formation("td_609", "609th Tank Destroyers", 1)
    m.formation("cavalry", "90th Cavalry Recon", 1)
    m.formation("us_artillery", "420th Armored Field Artillery", 1)
    m.place(1, "desobry_north", "medium_tank", 4, 0.22, 1.25, "Sherman N", spread=0.15)
    m.place(1, "desobry_east", "medium_tank", 4, 0.38, 0.95, "Sherman E", spread=0.15)
    m.place(1, "desobry_west", "medium_tank", 3, -0.05, 1.05, "Sherman W", spread=0.15)
    m.place(1, "b20_infantry", "halftrack", 3, 0.1, 0.95, "B/20", carries="rifle_platoon", spread=0.15, dismounted=True)
    m.place(1, "td_609", "tank_destroyer", 4, -0.1, 0.55, "M18")
    m.place(1, "cavalry", "recon_car", 2, -0.67, 0.71, "M8")
    m.place(1, "us_artillery", "artillery", 2, -0.6, -1.3, "M7")

    return m.file(
        "history_noville",
        "Noville, 19 December 1944",
        [
            {
                "name": "2. Panzer-Division",
                "ai": {"planner": "command", "difficulty": 4, "doctrine": "massed_armor"},
            },
            {"name": "Team Desobry"},
        ],
        {
            "assault": {"attacker": 0, "needs": 2},
            "breaking": [{"side": 0, "lost_percent": 50}, {"side": 1, "lost_percent": 70}],
            "prepared": [1],
            "standing": [{"side": 1, "mission": {"hold": {"at": None}}}],
            # Fog until it lifts: a few hundred yards, then clear.
            "visibility": [{"from_round": 0, "hexes": 3}, {"from_round": 6, "hexes": None}],
        },
    )


# --- the Valley of Tears, October 1973 ----------------------------------------


def valley_of_tears():
    """7th Armored Brigade on the ramps between Hermonit and Booster.

    Kahalani's battalion fought from earthen ramps along the western ridge,
    3 to 8 levels above a valley floor 1 to 2.5 km deep, with an anti-tank
    ditch behind the Purple Line and mines along it; the Syrians crossed on
    bridgelayers and causeways, and the bridging tanks were shot first. One
    assault wave: about a hundred tanks came on the morning of 9 October.

    History: the Syrians reached the crest of Booster; Kahalani took the
    ramps back with a handful of tanks, Ben-Hanan's dozen struck the flank,
    and the attack was called off.
    """
    m = Map("gol", 30, -0.3, 0.3)
    # The anti-tank ditch behind the Purple Line, cut where the roads cross
    # it (the causeways), and mines on its far side.
    ditch = [(0.95, -1.4), (0.85, 0.5), (0.9, 2.9)]
    m.line("tank_ditch", ditch)
    # Roads: 98 behind the ridge, and the valley roads to the crossings.
    m.line("road", [(-2.6, -2.4), (-2.5, 0.0), (-2.6, 1.6), (-2.9, 3.1)])
    m.line("road", [(2.6, 1.2), (0.85, 1.25), (-0.4, 0.9), (-1.3, 0.6), (-2.5, 0.4)])
    m.line("road", [(2.4, -1.0), (0.9, -0.6), (-0.5, -0.9), (-1.6, -1.2), (-2.5, -1.1)])
    # Basalt boulder fields in the valley are slow going.
    m.blob("mud", 0.2, 2.0, 0.35)
    m.blob("mud", 0.0, -0.1, 0.3)
    # Hermonit's cone is too steep at the top for a hull.
    m.blob("mountains", -2.0, 2.3, 0.12)

    m.objective("hermonit", "Hermonit", -1.85, 2.05, 0.15, held=1)
    m.objective("ramps", "the Oz 77 ramps", -1.45, 1.1, 0.15, held=1)
    m.objective("booster", "Booster", -0.65, -1.7, 0.15, held=1)
    m.objective("junction", "the road behind the ridge", -2.5, 0.4, 0.15, held=1)
    m.objective("west", "the road to El Rom", -2.8, 0.6, 0.12, kind="exit", side=1)
    m.objective("east", "the Purple Line", 2.55, 0.3, 0.12, kind="exit", side=0)

    minefields = []
    for y0, y1 in ((-1.2, -0.3), (0.0, 1.0), (1.5, 2.6)):
        cells = {m.cell(1.15, y0 + (y1 - y0) * i / 8) for i in range(9)}
        cells.discard(None)
        minefields.append(
            {"side": 1, "mine": "anti_tank_mine", "density": 70, "at": [list(c) for c in sorted(cells)]}
        )

    # The Syrian wave, east of the line, a battalion a crossing, with
    # carriers behind.
    for i, (x, y) in enumerate(((1.7, 1.7), (2.0, 0.6), (1.8, -0.6))):
        m.formation(f"syrian_{i + 1}", f"Tank battalion {i + 1}", 0)
        m.place(0, f"syrian_{i + 1}", "medium_tank", 10, x, y, f"T-55 {i + 1}", spread=0.45)
    m.formation("syrian_mech", "Mechanized infantry", 0)
    m.place(0, "syrian_mech", "apc", 6, 2.3, 0.1, "BTR", carries="rifle_platoon", spread=0.4)
    m.formation("syrian_guns", "Artillery", 0)
    m.place(0, "syrian_guns", "artillery", 3, 2.4, -0.3, "Battery")

    # 7th Armored Brigade on its ramps: Battalion 77 on the central ridge,
    # 74 under Hermonit, a company on Booster, and the guns behind.
    m.formation("bn77", "Battalion 77", 1)
    m.formation("bn74", "Battalion 74", 1)
    m.formation("booster", "Booster company", 1)
    m.formation("idf_guns", "Artillery", 1)
    m.place(1, "bn77", "medium_tank", 6, -1.4, 1.1, "Sho't 77", spread=0.35)
    m.place(1, "bn74", "medium_tank", 5, -1.75, 2.05, "Sho't 74", spread=0.3)
    m.place(1, "booster", "medium_tank", 4, -0.6, -1.6, "Sho't B", spread=0.3)
    m.place(1, "idf_guns", "artillery", 2, -2.8, -0.5, "Battery")

    return m.file(
        "history_valley_of_tears",
        "The Valley of Tears, October 1973",
        [
            {
                "name": "Syrian 7th Infantry Division",
                "ai": {"planner": "command", "difficulty": 4, "doctrine": "massed_armor"},
            },
            {"name": "7th Armored Brigade"},
        ],
        {
            "assault": {"attacker": 0, "needs": 3},
            "breaking": [{"side": 0, "lost_percent": 50}, {"side": 1, "lost_percent": 70}],
            "prepared": [1],
            # No standing order: "hold where you are" closes a platoon on its
            # leader, which pulls tanks off a line of ramps, and the order the
            # brigade actually had — every tank on her own ramp — is one the
            # vocabulary cannot give yet. Measured: the attacker's wins went
            # from 62% without one to 86% with one.
            "minefields": minefields,
        },
    )


# --- Arracourt, 19 September 1944 ----------------------------------------------


def arracourt():
    """Combat Command A against Panzer-Brigade 113 in the morning fog.

    Rolling farmland "with tank defilade" and little forest cover (Cole). At
    07:30 in fog Panthers ran into C/37 at Lezey and an M18 platoon west of
    Bezange at a hundred-odd yards; Lamison's four Shermans raced for the
    ridge west of Bezange, killed four of eight Panthers, dropped back over
    the crest and finished the rest from the south. In the afternoon Hunter
    swung two companies through Réchicourt into the German flank. A hasty
    defence: nobody had dug.

    History: the brigade's attack broke up and fell back east; CCA claimed
    43 tanks for 5 Shermans and 3 tank destroyers (an upper bound).
    """
    m = Map("arr", 32, 1.2, 0.6)
    # The Loutre Noire's low ground to the north is wet.
    m.below("mud", 211)
    m.line("stream", [(-1.8, 2.3), (-0.71, 2.45), (1.0, 2.5), (2.78, 2.61), (4.2, 2.9)])
    m.line("stream", [(1.4, -0.6), (1.9, 0.6), (2.4, 1.6)])
    m.line("stream", [(3.2, -2.2), (3.6, -1.0), (4.2, 0.2)])
    for x, y, r in ((2.78, 2.61, 0.16), (1.77, 0.13, 0.17), (-0.57, -0.97, 0.18), (3.48, -1.08, 0.16), (-0.71, 2.45, 0.14)):
        m.blob("town", x, y, r)
    # Small woods: little forest cover, a few copses.
    m.blob("forest", 2.0, -1.45, 0.2)
    m.blob("forest", 3.75, -2.6, 0.25)
    m.blob("forest", -1.5, 1.6, 0.25)
    m.line("road", [(-1.9, -0.75), (-0.57, -0.97), (1.5, -1.05), (3.48, -1.08), (4.3, 0.2)])
    m.line("road", [(-0.57, -0.97), (0.6, -0.35), (1.77, 0.13), (2.3, 1.4), (2.78, 2.61), (3.6, 3.2)])
    m.line("road", [(-1.9, 2.0), (-0.71, 2.45), (2.78, 2.61)])
    m.line("road", [(1.77, 0.13), (3.0, 0.5), (4.3, 0.85)])

    m.objective("lezey", "the Lezey crossroads", 2.78, 2.61, 0.15, held=1)
    m.objective("bezange_ridge", "the ridge west of Bezange", 0.9, -0.1, 0.15, held=1)
    m.objective("rechicourt", "Réchicourt-la-Petite", -0.57, -0.97, 0.15, held=1)
    m.objective("haut_deau", "the Haut d'Eau", 0.5, -1.55, 0.15, held=1)
    m.objective("west", "the road to Arracourt", -1.2, -0.75, 0.12, kind="exit", side=1)
    m.objective("east", "the road to Ley", 4.3, 0.75, 0.12, kind="exit", side=0)

    # Panzer-Brigade 113, coming on in company jabs from the east.
    m.formation("pz113_1", "Panther company (1)", 0)
    m.formation("pz113_2", "Panther company (2)", 0)
    m.formation("pz113_3", "Panther company (3)", 0)
    m.formation("pz113_grenadiers", "Panzergrenadier-Regiment 2113", 0)
    m.place(0, "pz113_1", "medium_tank", 8, 3.2, 2.0, "Panther N", spread=0.35)
    m.place(0, "pz113_2", "medium_tank", 8, 4.0, 0.6, "Panther C", spread=0.35)
    m.place(0, "pz113_3", "medium_tank", 8, 3.2, -1.2, "Panther S", spread=0.35)
    m.place(0, "pz113_grenadiers", "halftrack", 4, 4.0, 1.2, "Grenadiers", carries="rifle_platoon")

    # Combat Command A: C/37 at Lezey, Lamison's platoon near Bezange, the
    # M18s west of it, Hunter's companies back at Réchicourt, the light
    # tanks and the guns in the west.
    m.formation("c37", "C/37th Tank, Lezey", 1)
    m.formation("lamison", "Lamison's platoon", 1)
    m.formation("td_704", "C/704th Tank Destroyers", 1)
    m.formation("hunter", "Hunter's companies", 1)
    m.formation("d37", "D/37th light tanks", 1)
    m.formation("cca_infantry", "53rd Armored Infantry", 1)
    m.formation("cca_guns", "Armored field artillery", 1)
    m.place(1, "c37", "medium_tank", 4, 2.6, 2.5, "Sherman C", spread=0.2)
    m.place(1, "lamison", "medium_tank", 4, 0.3, -0.4, "Sherman L", spread=0.2)
    m.place(1, "td_704", "tank_destroyer", 4, 1.0, 0.2, "M18", spread=0.2)
    m.place(1, "hunter", "medium_tank", 8, -0.7, -1.2, "Sherman H", spread=0.3)
    m.place(1, "d37", "light_tank", 5, -1.2, 0.3, "Stuart", spread=0.3)
    m.place(1, "cca_infantry", "halftrack", 2, 1.7, 0.0, "53rd AIB", carries="rifle_platoon", spread=0.15)
    m.place(1, "cca_guns", "artillery", 2, -1.3, -0.3, "M7")

    return m.file(
        "history_arracourt",
        "Arracourt, 19 September 1944",
        [
            {
                "name": "Panzer-Brigade 113",
                "ai": {"planner": "command", "difficulty": 4, "doctrine": "massed_armor"},
            },
            {"name": "Combat Command A"},
        ],
        {
            "assault": {"attacker": 0, "needs": 3},
            "breaking": [{"side": 0, "lost_percent": 50}, {"side": 1, "lost_percent": 70}],
            # No standing order to hold: CCA won by manoeuvre, racing for the
            # Bezange ridge and swinging companies into the German flank, and
            # nobody had dug in. Measured with one: the attacker's wins went
            # from 17% to 55%.
            # "Seventy-five yards": fog thick enough to hide a Panther at
            # two hexes, lifting by mid-morning.
            "visibility": [{"from_round": 0, "hexes": 2}, {"from_round": 15, "hexes": None}],
        },
    )


MAPS = [noville, valley_of_tears, arracourt]


def render(spec):
    print(f"# {spec['id']} — {spec['name']}")
    for terrain, elevation in zip(spec["rows"], spec["elevation"]):
        print(f"  {terrain:<65}{elevation}")
    print()


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--check", action="store_true", help="fail if a file would change")
    parser.add_argument("--render", action="store_true", help="print the maps as ASCII")
    args = parser.parse_args()
    failed = False
    for build in MAPS:
        spec = build()
        if args.render:
            render(spec)
        text = json.dumps(spec, indent=4, ensure_ascii=False) + "\n"
        path = OUT / f"{spec['id']}.json"
        if args.check:
            if (path.read_text() if path.exists() else None) != text:
                print(f"{path} is not what the generator produces", file=sys.stderr)
                failed = True
            continue
        path.write_text(text)
        print(f"wrote {path}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
