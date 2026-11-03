#!/usr/bin/env python3
"""Draw the slice's battlefields (SLICE.md, *The three battles*).

    python3 tools/make_slice_maps.py            # write assets/mods/base/maps
    python3 tools/make_slice_maps.py --check    # fail if the files would move
    python3 tools/make_slice_maps.py --render   # print the maps as ASCII

The maps are content, and this script is how they are edited: the ground is
described in shapes (a road from here to there, a wood round this point), so
moving a wood is one number rather than a dozen retyped rows. The JSON it
writes is the ordinary map format and the game reads nothing else.

# The geometry

Rows are odd-r offset coordinates over pointy-top hexes 100 m across
(`tactics_core::offset_to_hex`): a column is 100 m east, a row 86.6 m south.
In odd-r a hex and the one directly below it in the same column are always
neighbours, so a road drawn down one column is a connected road.

The slice maps are `"shape": "free"` rectangles about the size Combat Mission
gives a platoon fight, not the regulation 4 km hexagon: a platoon on a field
that size spends the battle driving.
"""

import argparse
import json
import math
import pathlib
import sys

HERE = pathlib.Path(__file__).resolve().parent
OUT = HERE.parent / "assets" / "mods" / "base" / "maps"

PALETTE = {
    "p": "plains",  # ploughed fields: open, and firm enough for wheels
    "g": "grass",  # meadow: open, soft
    "R": "road",
    "t": "town",
    "f": "forest",
    "h": "hedgerow",  # orchards, hedges and the sunken lane
    "s": "stream",
    "m": "mud",
}


class Ground:
    """A rectangle of glyphs and levels, drawn on in shapes."""

    def __init__(self, width, height, fill, level):
        self.w, self.h = width, height
        self.glyph = [[fill] * width for _ in range(height)]
        self.level = [[level(c, r) for c in range(width)] for r in range(height)]

    @staticmethod
    def xy(col, row):
        """Cartesian centre of a hex, in hex widths, so shapes are round."""
        return col + 0.5 * (row & 1), row * 0.866

    def cells(self):
        for r in range(self.h):
            for c in range(self.w):
                yield c, r

    def put(self, col, row, glyph):
        if 0 <= col < self.w and 0 <= row < self.h:
            self.glyph[row][col] = glyph

    def raise_(self, col, row, by):
        if 0 <= col < self.w and 0 <= row < self.h:
            self.level[row][col] += by

    def ellipse(self, glyph, centre, rx, ry, only=None):
        cx, cy = self.xy(*centre)
        for c, r in self.cells():
            x, y = self.xy(c, r)
            if ((x - cx) / rx) ** 2 + ((y - cy) / (ry * 0.866)) ** 2 <= 1.0:
                if only is None or self.glyph[r][c] in only:
                    self.put(c, r, glyph)

    def path(self, glyph, points, only=None):
        """A connected line of hexes through each point in turn."""
        for (c0, r0), (c1, r1) in zip(points, points[1:]):
            steps = max(abs(c1 - c0), abs(r1 - r0)) * 2 + 1
            for i in range(steps + 1):
                t = i / steps
                c = round(c0 + (c1 - c0) * t)
                r = round(r0 + (r1 - r0) * t)
                if only is None or self.glyph[r][c] in only:
                    self.put(c, r, glyph)

    def rows(self):
        return ["".join(row) for row in self.glyph]

    def levels(self):
        digits = "0123456789abcdefghijklmnopqrstuvwxyz"
        return ["".join(digits[max(0, v)] for v in row) for row in self.level]


# ---------------------------------------------------------------------------
# Day 1: Hollerbach. A meeting engagement at a crossroads village.
# ---------------------------------------------------------------------------


def hollerbach():
    """The crossroads village where the platoon meets the Valkyrie recon.

    The brief (SLICE.md, *Day 1*): a village of five or six hexes at a
    crossroads, orchards and hedges to the west, open fields to the south,
    and a sunken lane the scouts can use unseen. Kuhlmann comes down from the
    north; the Valkyries come up from the south and must cross the Hollerbach
    brook to reach the village, by the road bridge or by fording.

    The ground leans the fight on purpose. Kuhlmann is nearer the village and
    has the orchards; the Valkyries have the better kit, the Buchwald to
    form up in, and the only gun on the field. Whether that is even is a
    question for the instruments, not for this file.
    """
    W, H = 26, 22

    def level(c, r):
        # The valley floor is the brook (row ~14). Ground rises gently to
        # the north (the platoon's side, two levels up at the edge) and less
        # to the south. Ten metres a level: a fold, not a ridge.
        if r <= 14:
            return round((14 - r) / 6)
        return round((r - 14) / 6)

    g = Ground(W, H, "p", level)

    # Meadow along the brook: open, soft, and wet in places.
    for c, r in g.cells():
        if 12 <= r <= 16:
            g.put(c, r, "g")
    g.ellipse("m", (5, 15), 3.0, 1.2, only="g")
    g.ellipse("m", (22, 13), 2.0, 1.0, only="g")

    # The Hollerbach brook, wandering east to west across the valley floor.
    # Drawn column by column, with both rows filled where it steps, so there
    # is no diagonal gap to cross it dry-shod.
    brook = [15] * 6 + [14] * 6 + [15] * 6 + [14] * 8
    for c, r in enumerate(brook):
        g.put(c, r, "s")
        if c and brook[c - 1] != r:
            g.put(c, brook[c - 1], "s")

    # Woods. The Buchwald south-east is where the Valkyries can form up out
    # of sight; the Kreuzholz north-east is Kuhlmann's flank; a copse
    # south-west gives the far side of the brook somewhere to hide.
    g.ellipse("f", (20, 18), 4.0, 2.8)
    g.ellipse("f", (21, 3), 3.0, 2.4)
    g.ellipse("f", (4, 19), 2.2, 1.6)

    # Orchards and field hedges west of the village: cover in strips, with
    # gaps, so crossing them is a choice of where.
    for r in (6, 8, 10, 12):
        for c in range(2, 10):
            if (c + r) % 5 != 0:
                g.put(c, r, "h")

    # The sunken lane: from the platoon's start line down to the church, a
    # line of hedgerow a section can walk unseen.
    g.path("h", [(6, 0), (7, 4), (9, 7)])

    # Roads first, so the village can be built along them: the north-south
    # road and its bridge, and the east-west road through the crossroads.
    g.path("R", [(13, 0), (13, 21)])
    g.path("R", [(0, 10), (25, 10)])

    # Hollerbach itself, built along both roads (a town hex moves like a
    # road, so the houses do not cut it), with the crossroads and the road
    # north and south of it left open. Hof Lenz is a farm east along the road.
    for c, r in [
        (12, 9), (14, 9), (15, 9),
        (11, 10), (12, 10), (14, 10), (15, 10),
        (11, 11), (12, 11), (14, 11),
    ]:  # fmt: skip
        g.put(c, r, "t")
    for c, r in [(18, 10), (19, 10), (18, 9)]:
        g.put(c, r, "t")

    # St. Georg on its knoll, the highest point in the village.
    for c, r in [(10, 8), (11, 9)]:
        g.put(c, r, "t")
        g.raise_(c, r, 1)
    for c, r in [(9, 8), (10, 9), (11, 8), (10, 7)]:
        g.raise_(c, r, 1)

    return {
        "id": "slice_hollerbach",
        "name": "Hollerbach, the first day",
        "kind": "battle",
        "shape": "free",
        "palette": PALETTE,
        "rows": g.rows(),
        "elevation": g.levels(),
        "sides": [
            {"name": "Kuhlmann Academy"},
            {
                "name": "Iron Valkyries",
                "ai": {"planner": "command", "difficulty": 4, "doctrine": "recon_pull"},
            },
        ],
        "formations": [
            # One formation per section (a squad and her carrier), so the
            # player's job-and-ground orders reach each element on its own:
            # a mission goes to a formation, and one formation for the whole
            # platoon would mean one job for everybody.
            {"id": "sec1", "name": "1st Section", "side": 0},
            {"id": "sec2", "name": "2nd Section", "side": 0},
            {"id": "sec3", "name": "3rd Section", "side": 0},
            {"id": "kuhlmann_scouts", "name": "Brandt's scouts", "side": 0},
            {"id": "valk_recon", "name": "Valkyrie recon", "side": 1},
            {"id": "valk_armour", "name": "Ravenna's Wiesel", "side": 1},
            {"id": "valk_scouts", "name": "Müller's scouts", "side": 1},
        ],
        "units": [
            # Kuhlmann, coming down the north road in column, mounted.
            # Falk leads from 1st Squad (SLICE.md, *The force*). The squads
            # are led by the three Kuhlmann cadets the data says are
            # infantry (small arms and fieldcraft); casting Sandoval and
            # Sorge there from their bios lost every battle, because under
            # the old stats both are tank commanders. Sandoval commands
            # Falk's carrier, so the pair still fight side by side.
            {"at": [13, 1], "side": 0, "vehicle": "halftrack", "crew": ["vera", "nele"],
             "name": "1st Carrier", "formation": "sec1"},
            {"at": [13, 1], "aboard_at": [13, 1], "side": 0, "vehicle": "rifle_squad",
             "crew": ["sigrun"], "name": "1st Squad", "formation": "sec1", "leads": True},
            {"at": [12, 0], "side": 0, "vehicle": "halftrack", "crew": ["wilma", "yuki"],
             "name": "2nd Carrier", "formation": "sec2"},
            {"at": [12, 0], "aboard_at": [12, 0], "side": 0, "vehicle": "rifle_squad",
             "crew": ["traudl"], "name": "2nd Squad", "formation": "sec2", "leads": True},
            {"at": [14, 0], "side": 0, "vehicle": "halftrack", "crew": ["ruth", "lotte"],
             "name": "3rd Carrier", "formation": "sec3"},
            {"at": [14, 0], "aboard_at": [14, 0], "side": 0, "vehicle": "rifle_squad",
             "crew": ["marlen"], "name": "3rd Squad", "formation": "sec3", "leads": True},
            # The scouts went ahead on foot and are already at the lane.
            {"at": [7, 3], "side": 0, "vehicle": "scout_section", "crew": ["elsa", "ilse"],
             "name": "Scouts", "formation": "kuhlmann_scouts", "leads": True},
            # The Wiesel, the only gun on the field, is not on it when the
            # battle opens: Sofia Ravenna brings her up the south road
            # around round 4 (`arrivals` below; SLICE.md, *Reinforcements*).
            # On the map from the start she drove straight into the
            # village and fought there at one hex (playtest 1b); arriving,
            # she has to cross the open valley the recon has already
            # crossed, to a village Kuhlmann may already hold.
            {"at": [13, 21], "side": 1, "vehicle": "light_tank", "crew": ["sofia", "kira", "rea"],
             "name": "Wiesel", "formation": "valk_armour", "leads": True},
            # Halftracks, like Kuhlmann's: the Wiesel is the Valkyries' edge
            # on day 1, and the measurements say it is edge enough.
            # Playthroughs, 16 seeds, Kuhlmann wins (doctrines as shipped /
            # swapped): a Schütze IFV + a Kasten + the Wiesel, 0-1; two
            # Kastens (armour 2, which nothing Kuhlmann carries but six
            # rockets goes through) + the Wiesel, 2 / 1; two halftracks +
            # the Wiesel, 3 / 7; two halftracks and no Wiesel, 15 / 15.
            # The ground favours Kuhlmann; one light tank takes it back.
            {"at": [12, 21], "side": 1, "vehicle": "halftrack", "crew": ["franka", "malin"],
             "name": "Kette 1", "formation": "valk_recon"},
            {"at": [12, 21], "aboard_at": [12, 21], "side": 1, "vehicle": "rifle_squad",
             "crew": ["sanna"], "name": "Lindholm's Squad", "formation": "valk_recon", "leads": True},
            {"at": [14, 21], "side": 1, "vehicle": "halftrack", "crew": ["cecile", "selin"],
             "name": "Kette 2", "formation": "valk_recon"},
            {"at": [14, 21], "aboard_at": [14, 21], "side": 1, "vehicle": "rifle_squad",
             "crew": ["marta"], "name": "Kovač's Squad", "formation": "valk_recon"},
            {"at": [17, 19], "side": 1, "vehicle": "scout_section", "crew": ["greta", "runa"],
             "name": "Valkyrie Scouts", "formation": "valk_scouts", "leads": True},
        ],  # fmt: skip
        "objectives": [
            {"id": "crossroads", "name": "the Hollerbach crossroads",
             "at": [[13, 10], [12, 10], [14, 10], [13, 9], [13, 11]], "value": 2},
            {"id": "church", "name": "St. Georg's", "at": [[10, 8], [11, 9]], "value": 1},
            # The ways home: Kuhlmann up the sunken lane's head on the north
            # edge, the Valkyries out by the copse on the south edge. Off the
            # roads both columns arrive on, so nobody deploys standing on
            # her own way out (tests/field.rs). An exit is only run for
            # when a side is losing (`withdraw_threshold`).
            {"id": "north_lane", "name": "the lane north", "kind": "exit", "side": 0,
             "value": 1, "at": [[5, 0], [6, 0], [7, 0]]},
            {"id": "south_track", "name": "the track south", "kind": "exit", "side": 1,
             "value": 1, "at": [[5, 21], [6, 21], [7, 21]]},
        ],  # fmt: skip
        "arrivals": [{"formation": "valk_armour", "round": 4, "spread": 1}],
        "victory_score": 24,
    }


MAPS = [hollerbach]


def render(m):
    print(f"{m['id']}: {m['name']}")
    for r, (row, lev) in enumerate(zip(m["rows"], m["elevation"])):
        pad = " " if r & 1 else ""
        print(f"{r:3} {pad}{' '.join(row)}    {pad}{' '.join(lev)}")
    print()


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--check", action="store_true")
    ap.add_argument("--render", action="store_true")
    args = ap.parse_args()

    stale = []
    for make in MAPS:
        m = make()
        if args.render:
            render(m)
            continue
        text = json.dumps(m, ensure_ascii=False, indent=1) + "\n"
        path = OUT / f"{m['id']}.json"
        if args.check:
            if not path.exists() or path.read_text() != text:
                stale.append(path.name)
        else:
            path.write_text(text)
            print(f"wrote {path.relative_to(HERE.parent)}")
    if stale:
        sys.exit(f"stale: {', '.join(stale)} (run tools/make_slice_maps.py)")


if __name__ == "__main__":
    main()
