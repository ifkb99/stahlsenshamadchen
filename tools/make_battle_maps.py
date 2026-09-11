#!/usr/bin/env python3
"""Generate the battlefields the campaign's terrains are fought on.

    python3 tools/make_battle_maps.py            # write assets/mods/base/maps
    python3 tools/make_battle_maps.py --check    # fail if the files would move
    python3 tools/make_battle_maps.py --render   # print the maps as ASCII

Two maps come out of this: `battle_hills`, the ground a fight in the
mountains lands on, and `battle_town`, the ground a fight in a city or on a
factory lands on. They are generated rather than drawn by hand because the one
property that matters most about a battlefield is the one a hand-drawn map
cannot promise — that **neither end of it is better than the other**.

# Why a generator at all

`balance --only ground` fights every map twice with the two armies exchanged
between the ends, so a map that quietly favours the west shows up as a side
edge in a table that every doctrine conclusion is then read against. The
cheapest way not to have that argument is to make a west/east tilt
*inexpressible*: every terrain and elevation decision here is a function of

    a = |U|   where U = 2*col - (40 - row % 2)

which is a doubled lateral coordinate, zero on the map's vertical axis and
negated by the mirror. A feature described by `a` cannot come out asymmetric,
the same trick `Arena::band` plays for the ridge arena. North/south is left
free, because deployment runs west and east and only that axis is a fairness
question.

# The geometry

A battle map is one overworld hex: the hexagon of radius 20 in the scale
block, 41 rows and 1261 tiles, checked by `MapFile::validate_shape`. Rows are
odd-r offset coordinates over pointy-top hexes (`tactics_core::offset_to_hex`),
so row `r` holds `41 - |r - 20|` tiles starting at column `|r - 20| // 2`, and
the mirror of column `c` is `(40 - r % 2) - c`.

# The order of battle

Both maps ship the scenario force `battle_forest` ships — the same nine
placements a side, mirrored end for end, with the same named cadets — because
the `ground` table's control is that a map's two sides field *identical*
orders of battle, and because 23 seats a side is what the academy roll has
room for once. The campaign ignores all of it: a field battle stages armies
through `from_placements`. It matters when the map is played as a scenario
with `STAHL_BATTLE=<id>`.
"""

import argparse
import json
import pathlib
import sys

# --- the hexagon ----------------------------------------------------------

RADIUS = 20
ROWS = 2 * RADIUS + 1
CENTER = (RADIUS, RADIUS)


def row_span(row):
    """`(first_column, tile_count)` for a row of the regulation hexagon."""
    depth = abs(row - RADIUS)
    return depth // 2, ROWS - depth


def cells():
    """Every `(col, row)` of the map, in row-major order."""
    for row in range(ROWS):
        first, count = row_span(row)
        for col in range(first, first + count):
            yield col, row


def mirror(col, row):
    """The column `col` reflects onto, about the map's vertical axis.

    Odd rows are shoved right by half a hex in odd-r offset coordinates, so
    the axis falls between two columns there and the reflection is one short
    of the even-row one. Getting this wrong is not a cosmetic mistake: it
    would put a "symmetric" feature half a hex east on every other row.
    """
    return (2 * RADIUS - row % 2) - col


def lateral(col, row):
    """Doubled distance east of the vertical axis; negated by `mirror`."""
    return 2 * col - (2 * RADIUS - row % 2)


def axial(col, row):
    """Offset -> axial, matching `hexx` in `Odd`/`Pointy` mode."""
    return col - row // 2, row


def hex_distance(a, b):
    ax, ay = axial(*a)
    bx, by = axial(*b)
    dx, dy = ax - bx, ay - by
    return max(abs(dx), abs(dy), abs(dx + dy))


# --- the canvas -----------------------------------------------------------


class Canvas:
    """A hexagon of glyphs and elevations, written through symmetric brushes.

    Nothing here takes a single `(col, row)`: every brush is a predicate over
    `(a, v)` — doubled distance from the axis, and rows north or south of the
    middle — so the west and east halves are painted by the same statement and
    cannot drift apart.
    """

    def __init__(self, terrain, elevation=0):
        self.terrain = {cell: terrain for cell in cells()}
        self.elevation = {cell: elevation for cell in cells()}

    def paint(self, predicate, terrain=None, elevation=None):
        for cell in cells():
            col, row = cell
            if not predicate(abs(lateral(col, row)), row - RADIUS):
                continue
            if terrain is not None:
                self.terrain[cell] = terrain
            if elevation is not None:
                self.elevation[cell] = elevation

    def blob(self, at, radius, terrain=None, elevation=None, keep=()):
        """Paint a disc and its mirror image. `at` is a `(col, row)` seed.

        The seed has to be on the map. A seed off the edge paints whatever
        part of its disc happens to fall inside, which is still symmetric and
        therefore still passes `check_symmetric` — so a mistyped column would
        show up as a clump that quietly is not there rather than as a
        failure.
        """
        first, count = row_span(at[1])
        assert first <= at[0] < first + count, f"blob seed {at} is off the map"
        seeds = {at, (mirror(*at), at[1])}
        for cell in cells():
            if min(hex_distance(cell, seed) for seed in seeds) > radius:
                continue
            if self.terrain[cell] in keep:
                continue
            if terrain is not None:
                self.terrain[cell] = terrain
            if elevation is not None:
                self.elevation[cell] = elevation

    def rows(self):
        out = []
        for row in range(ROWS):
            first, count = row_span(row)
            line = "".join(self.terrain[(col, row)] for col in range(first, first + count))
            out.append(" " * first + line)
        return out

    def elevation_rows(self):
        out = []
        for row in range(ROWS):
            first, count = row_span(row)
            line = "".join(
                str(self.elevation[(col, row)]) for col in range(first, first + count)
            )
            out.append(" " * first + line)
        return out

    def check_symmetric(self, name):
        """The promise this whole file exists to make, asserted rather than
        trusted. A brush written by hand — `blob`'s seed list, say — is the
        one place a west/east tilt could still creep in."""
        for cell in cells():
            col, row = cell
            twin = (mirror(col, row), row)
            for grid, what in ((self.terrain, "terrain"), (self.elevation, "elevation")):
                if grid[cell] != grid[twin]:
                    raise AssertionError(
                        f"{name}: {what} at {cell} is {grid[cell]!r} and its "
                        f"mirror {twin} is {grid[twin]!r}"
                    )


# --- the shared order of battle -------------------------------------------

# The nine placements a side, as `battle_forest` writes them: an armoured
# line, a section built round the guns and the eyes, and a grenadier section
# whose platoon rides out in the back of the halftrack. West coordinates only;
# the eastern half is this list reflected, which is what makes the `ground`
# table's force column read level by construction.
WEST_ORDER = [
    ("kuhlmann_line", (4, 20), "medium_tank", "Anvil 1", ["anka", "juno", "mina", "rosa"], True, None),
    ("kuhlmann_line", (5, 16), "light_tank", "Anvil 2", ["elsa", "ada", "hanne"], False, None),
    ("kuhlmann_line", (5, 24), "tank_destroyer", "Anvil 3", ["yuki", "ilse", "lore"], False, None),
    ("kuhlmann_scouts", (7, 13), "recon_car", "Fenn 1", ["nele", "ruth"], True, None),
    ("kuhlmann_scouts", (6, 27), "light_tank", "Fenn 2", ["marlen", "katrin", "doro"], False, None),
    ("kuhlmann_scouts", (2, 20), "artillery", "Fenn 3", ["petra", "emmi", "vera", "berit"], False, None),
    ("kuhlmann_grenadiers", (7, 21), "halftrack", "Grenadier 1", ["lotte", "wilma"], True, None),
    ("kuhlmann_grenadiers", (8, 21), "rifle_platoon", "Grenadier 2", ["ines", "traudl"], False, (7, 21)),
    ("kuhlmann_scouts", (4, 18), "scout_section", "Spaeher", ["sigrun"], False, None),
]

# The same nine, crewed from the other academy. Kept as a parallel list rather
# than generated so that the eastern names read as an order of battle somebody
# wrote, and so a cadet appears at most once per map.
#
# Both `scout_section`s used to be crewed by nobody, which is not the same as
# being crewed badly: an empty crew answers `Roster::unspecified`, which is
# AVERAGE for every skill, so the two units whose whole job is to go and look
# were exactly average at looking and at everything else. They carry the
# roster's athletic cadets now — `sigrun` at 12 west and `ludmila` at 12 east
# against an average of 10 — which is what a section is for and what makes
# `battle_hills`'s tors reachable by the units that should want them.
#
# Each section rides with **one** cadet, and the arithmetic is why: nine units
# a side at the sizes above is 25 seats, and the mod has 49 characters, so two
# fully crewed orders of battle want 50. One side would always be a cadet
# shorter than the other. The first attempt put two in each section and left
# the eastern grenadiers' halftrack riding with one — and `battle_town`, a map
# that is exactly mirror-symmetric and whose row is supposed to read nothing
# but the engine's residual compass bias, went from 32-40 to **54-18**. That
# is what one seat is worth on a map where the taxi matters, and it is the
# `ground` control telling the truth. So both sections ride one up, which is
# symmetric, fills the seat that answers for `athletics`, and leaves `erika`
# on the roll without a vehicle.
EAST_CREWS = [
    (
        "valkyrie_line",
        "Wotan 1",
        ["irma", "astrid", "beatrix", "franka"],
    ),
    ("valkyrie_line", "Wotan 2", ["sofia", "kira", "rea"]),
    ("valkyrie_line", "Wotan 3", ["greta", "runa", "britta"]),
    ("valkyrie_scouts", "Raven 1", ["malin", "zofia"]),
    ("valkyrie_scouts", "Raven 2", ["marta", "ingrid", "selin"]),
    ("valkyrie_scouts", "Raven 3", ["nadja", "hedda", "tamara", "carmen"]),
    ("valkyrie_grenadiers", "Sturm 1", ["odette", "sanna"]),
    ("valkyrie_grenadiers", "Sturm 2", ["alma", "cecile"]),
    ("valkyrie_scouts", "Vixen", ["ludmila"]),
]

FORMATIONS = [
    {"id": "kuhlmann_line", "name": "1st Armored Platoon", "side": 0},
    {"id": "kuhlmann_grenadiers", "name": "1st Grenadier Section", "side": 0},
    {"id": "kuhlmann_scouts", "name": "Reconnaissance Section", "side": 0},
    {"id": "valkyrie_line", "name": "Valkyrie Armor", "side": 1},
    {"id": "valkyrie_grenadiers", "name": "Valkyrie Grenadiers", "side": 1},
    {"id": "valkyrie_scouts", "name": "Valkyrie Screen", "side": 1},
]


def order_of_battle():
    units = []
    for formation, at, vehicle, name, crew, leads, aboard in WEST_ORDER:
        unit = {"at": list(at)}
        if aboard is not None:
            unit["aboard_at"] = list(aboard)
        unit.update(
            {"side": 0, "vehicle": vehicle, "name": name, "crew": crew, "formation": formation}
        )
        if leads:
            unit["leads"] = True
        units.append(unit)
    for (_, at, vehicle, _, _, leads, aboard), (formation, name, crew) in zip(
        WEST_ORDER, EAST_CREWS
    ):
        unit = {"at": [mirror(*at), at[1]]}
        if aboard is not None:
            unit["aboard_at"] = [mirror(*aboard), aboard[1]]
        unit.update(
            {"side": 1, "vehicle": vehicle, "name": name, "crew": crew, "formation": formation}
        )
        if leads:
            unit["leads"] = True
        units.append(unit)
    return units


def sides(doctrine):
    return [
        {"name": "Kuhlmann Academy"},
        {
            "name": "Iron Valkyries",
            "ai": {"planner": "utility", "difficulty": 4, "doctrine": doctrine},
        },
    ]


def exits():
    """The two retreat lanes, one per side, at the road's ends.

    Three hexes each and mirror images of one another, exactly as
    `battle_plains` and `river_crossing` write them. `deploy` refuses to form
    anybody up on an exit hex, so these have to be at the very lip of the map
    or a side loses its shallowest deployment ground to its own way home.
    """
    return [
        {
            "id": "west_road",
            "name": "The Western Road",
            "at": [[0, 20], [0, 19], [0, 21]],
            "value": 1,
            "kind": "exit",
            "side": 0,
        },
        {
            "id": "east_road",
            "name": "The Eastern Road",
            "at": [[40, 20], [39, 19], [39, 21]],
            "value": 1,
            "kind": "exit",
            "side": 1,
        },
    ]


# --- battle_hills ---------------------------------------------------------


def hills():
    """Rolling high ground: one ridge across the middle of the fight.

    The ridge runs north to south, which puts it *between* the two ends
    rather than under either of them — the only arrangement in which a hill
    is a thing to be taken instead of a thing one side was given. It carries
    two humps at elevation 2 with a saddle between them where the road
    crosses, spurs at elevation 1 reaching out to both flanks, and a pair of
    crag walls of `mountains` — foot-only, so genuinely impassable to
    everything with tracks — that keep the northern crest from being driven
    round at speed.
    """
    c = Canvas("g")

    # The ridge itself, written as one profile so the humps, the saddle and
    # the way the ground falls away are a single statement rather than three
    # brushes overwriting each other.
    #
    # The shape it replaced was two wide *bands* — everything within two and a
    # half hexes of the axis at one height, everything within five and a half
    # at another — and it read 186–102 to the western end over four seeds,
    # +4.9 sd, where the two hand-drawn maps sit inside the band. Wide bands
    # of identical ground are wide plateaus of identical *score*, and this
    # engine's remaining tie-breaks put the coordinate last rather than
    # nowhere: every tie is a coin that falls the same compass way, so the
    # more ground ties, the harder the map leans west. So the ridge is
    # written as a slope now — height falls with lateral distance *and* with
    # distance from a hump top, every step of it three hexes wide — which
    # gives the same shape with far less ground at exactly one value.
    def ridge(a, v):
        # How high the spine itself stands, row by row: two hump tops nine
        # rows either side of the middle, a saddle between them that dips to
        # one step rather than to nothing, and both ends running out.
        spine = 0
        for reach, height in ((16, 1), (13, 2)):
            if abs(v) <= reach:
                spine = height
        if abs(v) <= 3:
            spine = 1
        # `a` is doubled, so `(a + 2) // 6` drops a step every three hexes out
        # from the axis. Height therefore changes three hexes at a time in
        # both directions instead of standing still across a whole band.
        return max(0, spine - (a + 2) // 6)

    for cell in cells():
        col, row = cell
        c.elevation[cell] = ridge(abs(lateral(col, row)), row - RADIUS)

    # Spurs: two arms of high ground running out to either flank, so a
    # commander who does not want to go over the top has somewhere to go
    # instead. They run on from the elevation-1 skirt of each hump rather
    # than starting somewhere of their own, so the high ground is one
    # connected piece.
    c.paint(lambda a, v: abs(abs(v) - 8) <= 1 and a <= 21, elevation=1)
    c.paint(lambda a, v: abs(abs(v) - 8) <= 1 and a <= 9, elevation=2)

    # Anything above the flood plain is open upland rather than meadow.
    for cell in cells():
        if c.elevation[cell] >= 1:
            c.terrain[cell] = "p"

    # The tors: a rock knob far out on each spur, standing three levels above
    # the shoulder it sits on.
    #
    # Tracked vehicles climb one level and foot climbs two, so until
    # `balance.athletics_per_climb_level` existed there was no ground in this
    # game that any crew could reach and another could not for want of a
    # skill — no shipped battle map had an adjacent step steeper than two,
    # which is exactly the two infantry already had. This is the first. A
    # platoon whose leader is athletic goes up it; one whose leader is not
    # walks round; nothing with tracks goes near it. It is out on the spur
    # rather than on a hump top on purpose — the crest objective must stay a
    # thing two armies can fight over, not a thing one platoon is handed —
    # and what it is worth is the sight line, which is the terrain prior's
    # business rather than an objective's.
    #
    # Everything off the spur band falls to the flood plain here, so the tor
    # is a step of three along the spur and a cliff everywhere else: there is
    # one way up and it is the way the ridge already runs.
    def is_tor(a, v):
        return abs(abs(v) - 8) <= 1 and 15 <= a <= 19

    c.paint(is_tor, elevation=4)

    # The crags: rock the humps break through in, foot-only and therefore a
    # genuine wall to everything with tracks. Broken into knots on the humps'
    # outer rims rather than laid as one straight bar — a wall with no way
    # through is a smaller map, and a bar across the ridge was also the
    # widest single run of identical ground left on it.
    c.paint(lambda a, v: abs(abs(v) - 9) == 4 and 9 <= a <= 13, terrain="M")

    # Timber, scattered rather than sparse: broken ground is ground the two
    # commanders can tell apart, and the woods on the shoulders are what stop
    # the approach to each hump being one flat plateau of equal tiles.
    for seed in [
        (11, 3),
        (11, 34),
        (14, 15),
        (16, 30),
        (12, 37),
        (8, 8),
        (15, 24),
        (9, 18),
    ]:
        c.blob(seed, 1, terrain="f", keep=("M",))

    # Hedgerows along the foot of the ridge: cover a crew can see out of.
    #
    # Until this terrain existed, `forest` (30) and `town` (40) were the only
    # covering ground in the game and both block sight at 2, so everything
    # commanding was bare and everything covered was blind — "cover" and
    # "dead ground" were the same word to the evaluator. A bank at 22 that
    # blocks nothing is the first ground on which a crew can be hard to hit
    # and still lay her gun, which is the whole tactical idea of a hull-down
    # position written as terrain.
    #
    # Along the *foot* of the slope rather than on the crest: the approach to
    # the ridge was the widest run of bare ground on the map, and this is the
    # ground a commander wants to be on while she is looking at the crest.
    # Broken into banks with gaps between them, and the reason is the same
    # one the ridge is written as a slope for: laid as two continuous bands
    # this read 26-46 to the eastern end against 36-36 without it, because a
    # wide run of identical ground is a wide plateau of identical *score* and
    # every tie left in this engine is a coin that falls the same compass
    # way. `(a // 4) % 2` cuts it into four-hex banks with four-hex gaps —
    # which is also what a field boundary looks like.
    c.paint(
        lambda a, v: abs(abs(v) - 5) <= 1 and 7 <= a <= 25 and (a // 4) % 2 == 0,
        terrain="h",
    )

    # The road across, through the saddle: the one lane that crosses the
    # ridge without climbing it.
    c.paint(lambda a, v: v == 0, terrain="R")

    # The crown of each tor, named so that somebody wants it. High ground
    # with nothing on it is scenery: the terrain prior pays for elevation
    # only where no found gun reaches, `impatience` charges the walk, and a
    # section at one movement point takes most of a battle to get out there
    # — so measured with no objective on them the tors changed nothing at
    # all. One point each, against three for the crest and two for the
    # saddle: a reason to climb rather than a prize worth the battle. Taken
    # from the same predicate the rock is painted from, and split by which
    # side of the axis it falls on, so the two cannot drift apart.
    # No objective sits on a tor, and that is the map contract rather than an
    # oversight. Every objective here has to be symmetric about the axis —
    # `check_symmetric` refuses one that is not, which is what makes a
    # west/east tilt inexpressible on a generated map — and a tor stands on a
    # flank. An objective covering both flanks' crowns at once would be one
    # prize in two places neither side can hold whole, which contest rules
    # would cancel to nothing. So what a tor is worth is the sight line, and
    # the measured consequence is that nobody walks out to one: with the
    # terrain prior paying for elevation only where no found gun reaches, and
    # `impatience` charging a section at one movement point for most of a
    # battle's walk, `athletics_per_climb_level` is still bit-identical across
    # 180 battles. The rule works and the ground is real; what neither has
    # yet is a reason, which is the same gap `concealment` has carried since
    # detection shipped.
    c.check_symmetric("battle_hills")
    return {
        "id": "battle_hills",
        "name": "The Rauhkamm",
        "kind": "battle",
        "palette": {
            "g": "grass",
            "p": "plains",
            "f": "forest",
            "h": "hedgerow",
            "M": "mountains",
            "R": "road",
        },
        "rows": c.rows(),
        "elevation": c.elevation_rows(),
        "sides": sides("bounding_overwatch"),
        "formations": FORMATIONS,
        "units": order_of_battle(),
        "objectives": [
            {
                "id": "crest",
                "name": "The Rauhkamm Crest",
                "at": [[19, 12], [20, 12], [21, 12]],
                "value": 3,
            },
            {
                "id": "saddle",
                "name": "The Saddle Road",
                "at": [[19, 20], [20, 20], [21, 20]],
                "value": 2,
            },
        ]
        + exits(),
        "victory_score": 60,
    }


# --- battle_town ----------------------------------------------------------


def town():
    """A market town on a crossroads, with a hamlet up the northern road.

    The two roads meet in the middle and stay roads through the built-up
    hexes, which is the whole reason the objective sits on the crossroads
    rather than in a house: `town` blocks sight at two levels, so a solid
    block of it would be an objective nobody inside can see out of. The lanes
    are the sight lines, and holding the junction means holding them.

    The north road is two hexes wide on odd rows and one on even rows. That
    is not decoration — the vertical axis falls between two columns on an odd
    row, so a one-hex lane there would sit half a hex east of centre and hand
    the eastern end a marginally shorter drive.
    """
    c = Canvas("g")

    # Cultivated ground round the town, meadow beyond it.
    c.blob(CENTER, 9, terrain="p")

    # The two roads. Row 20 runs the width of the map; the north-south lane
    # is written as a symmetric pair of columns so it cannot lean.
    c.paint(lambda a, v: v == 0, terrain="R")
    c.paint(lambda a, v: a <= 1, terrain="R")

    # The town: buildings crowding the junction, roads left as roads.
    for cell in cells():
        if c.terrain[cell] == "R":
            continue
        if hex_distance(cell, CENTER) <= 4:
            c.terrain[cell] = "t"

    # The hamlet up the north road, on a slight rise so it is worth the
    # walk and worth the shells.
    hamlet = (20, 10)
    c.blob(hamlet, 3, elevation=1)
    for cell in cells():
        if c.terrain[cell] == "R":
            continue
        if hex_distance(cell, hamlet) <= 2:
            c.terrain[cell] = "t"

    # Woodlots on the field boundaries, out where they screen an approach
    # without smothering the town.
    for seed in [(8, 6), (5, 26), (13, 33), (10, 16), (16, 3), (10, 36)]:
        c.blob(seed, 1, terrain="f", keep=("t", "R"))

    c.check_symmetric("battle_town")
    return {
        "id": "battle_town",
        "name": "Immenrode",
        "kind": "battle",
        "palette": {"g": "grass", "p": "plains", "f": "forest", "t": "town", "R": "road"},
        "rows": c.rows(),
        "elevation": c.elevation_rows(),
        "sides": sides("elastic_defense"),
        "formations": FORMATIONS,
        "units": order_of_battle(),
        "objectives": [
            {
                "id": "market",
                "name": "The Immenrode Crossroads",
                "at": [[19, 20], [20, 20], [21, 20]],
                "value": 3,
            },
            {
                "id": "hamlet",
                "name": "Oberdorf",
                "at": [[19, 10], [20, 10], [21, 10]],
                "value": 2,
            },
        ]
        + exits(),
        "victory_score": 60,
    }


# --- emit -----------------------------------------------------------------

MAPS = [hills, town]
OUT = pathlib.Path("assets/mods/base/maps")


def sanity(spec):
    """The checks a mistake here would otherwise reach `validate-mods` as.

    Cheap, and they name the mistake in the generator's own terms: a map that
    is not the regulation hexagon, an objective off the map or on an odd row
    (where no three columns can be symmetric), a unit formed up somewhere no
    vehicle can drive, or a unit standing on its own way off the map.
    """
    tiles = {(col, row) for col, row in cells()}
    assert len(tiles) == 3 * RADIUS * RADIUS + 3 * RADIUS + 1, "not the regulation hexagon"
    impassable = {"M"}
    grid = {}
    for row, line in enumerate(spec["rows"]):
        for col, glyph in enumerate(line):
            if glyph != " ":
                grid[(col, row)] = glyph
    assert set(grid) == tiles, f"{spec['id']}: rows do not fill the hexagon"

    exit_hexes = {
        (at[0], at[1])
        for objective in spec["objectives"]
        if objective.get("kind") == "exit"
        for at in objective["at"]
    }
    for objective in spec["objectives"]:
        for at in objective["at"]:
            cell = (at[0], at[1])
            assert cell in tiles, f"{spec['id']}: objective `{objective['id']}` at {at} is off the map"
        if objective.get("kind") != "exit":
            rows = {at[1] for at in objective["at"]}
            assert len(rows) == 1 and rows.pop() % 2 == 0, (
                f"{spec['id']}: objective `{objective['id']}` must sit on one even row, "
                "or its hexes cannot be each other's mirror images"
            )
            columns = sorted(at[0] for at in objective["at"])
            assert [mirror(col, objective["at"][0][1]) for col in columns][::-1] == columns, (
                f"{spec['id']}: objective `{objective['id']}` is not symmetric about the axis"
            )
    for unit in spec["units"]:
        cell = (unit["at"][0], unit["at"][1])
        assert cell in tiles, f"{spec['id']}: {unit['name']} is off the map"
        assert grid[cell] not in impassable, (
            f"{spec['id']}: {unit['name']} forms up on `{grid[cell]}`, which no vehicle can enter"
        )
        assert cell not in exit_hexes, f"{spec['id']}: {unit['name']} forms up on an exit"


def render(spec):
    print(f"# {spec['id']} — {spec['name']}")
    for terrain, elevation in zip(spec["rows"], spec["elevation"]):
        print(f"  {terrain:<45}{elevation}")
    counts = {}
    for line in spec["rows"]:
        for glyph in line.replace(" ", ""):
            counts[glyph] = counts.get(glyph, 0) + 1
    legend = ", ".join(
        f"{spec['palette'][glyph]} {count}" for glyph, count in sorted(counts.items())
    )
    print(f"  {sum(counts.values())} tiles: {legend}\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail if a file would change")
    parser.add_argument("--render", action="store_true", help="print the maps as ASCII")
    args = parser.parse_args()

    failed = False
    for build in MAPS:
        spec = build()
        sanity(spec)
        if args.render:
            render(spec)
        text = json.dumps(spec, indent=4) + "\n"
        path = OUT / f"{spec['id']}.json"
        if args.check:
            current = path.read_text() if path.exists() else None
            if current != text:
                print(f"{path} is not what the generator produces", file=sys.stderr)
                failed = True
            continue
        path.write_text(text)
        print(f"wrote {path}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
