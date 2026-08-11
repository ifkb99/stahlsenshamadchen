#!/usr/bin/env python3
"""Draw the vehicle sprites, isometrically, as three-frame sheets.

    python3 tools/make_vehicle_sprites.py

A vehicle is described as a handful of boxes in its own coordinates — x
forward, y to its left, z up — and this projects them into the same view the
map is drawn in. That is worth the machinery for two reasons: the roster stays
consistent in palette, light direction and proportion while it is still
placeholder-grade art, and a box model can be re-rendered at any yaw, which is
exactly what a six-direction sheet needs.

# Why three frames

The board is isometric, so a vehicle has to be *seen* from the side and above
rather than straight down. Six hex directions would mean six drawings, except
that three of them are mirror images of the other three: west is east flipped,
north-west is north-east flipped, south-west is south-east flipped. So each
sheet is three frames — **east, north-east, south-east**, left to right — and
the renderer sets `flip_x` for the other three.

That is exact rather than approximate, because pointy-top hex directions are
60 degrees apart and the projection is symmetric about the vertical axis.

# The projection

Matches `iso.rs`: ground distances squash to `SQUASH` vertically, and height
goes straight up the screen. Height is exaggerated relative to the map's
`ELEV_PX` — a 3 m tank rendered honestly against 100 m hexes would be about
four pixels tall — which is the same licence the terrain prisms take.

# The team key

Magenta is replaced at load with the academy's colour. It is not in the
palette, so it cannot collide with real art, and painting it into the model
rather than tinting the whole sprite is what keeps the world drab while the
sides stay legible.
"""

import math
import os
import struct
import zlib

FRAME_W, FRAME_H = 56, 40
FRAMES = 3  # east, north-east, south-east
YAWS = [0.0, 60.0, -60.0]

SQUASH = 40.0 / 73.9  # ground Y foreshortening, from iso.rs
ZS = 1.0  # screen pixels per unit of height
ORIGIN = (FRAME_W / 2.0, FRAME_H / 2.0 + 7.0)

# The camera sits below the board in world terms and looks down at it: the
# tile prisms show their walls on the *lower* screen edge, so the faces turned
# towards the viewer are the ones with a negative y normal. Light comes from
# above, in front and to the left, matching those prisms.
VIEW = (0.0, -0.55, 0.83)
LIGHT_RAW = (-0.35, -0.45, 0.82)

HULL = (84, 90, 68)
TURRET = (94, 100, 76)
TRACK = (46, 44, 40)
WHEEL = (40, 38, 35)
GUN = (58, 60, 50)
TEAM = (255, 0, 255)
OUTLINE = (24, 26, 20)


def norm(v):
    m = math.sqrt(sum(c * c for c in v)) or 1.0
    return tuple(c / m for c in v)


LIGHT = norm(LIGHT_RAW)


class Box:
    """An axis-aligned box in vehicle space, painted one flat colour per face
    with a lambert term so the visible faces separate."""

    def __init__(self, x0, x1, y0, y1, z0, z1, color, team_faces=()):
        self.b = (x0, x1, y0, y1, z0, z1)
        self.color = color
        # Faces are named by their outward normal: "+x" is the nose, "+z" the
        # roof, "-x" the rear plate.
        self.team_faces = set(team_faces)

    def faces(self):
        x0, x1, y0, y1, z0, z1 = self.b
        c = [
            (x0, y0, z0), (x1, y0, z0), (x1, y1, z0), (x0, y1, z0),
            (x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1),
        ]
        return [
            ("+z", (c[4], c[5], c[6], c[7]), (0, 0, 1)),
            ("+x", (c[1], c[2], c[6], c[5]), (1, 0, 0)),
            ("-x", (c[3], c[0], c[4], c[7]), (-1, 0, 0)),
            ("+y", (c[2], c[3], c[7], c[6]), (0, 1, 0)),
            ("-y", (c[0], c[1], c[5], c[4]), (0, -1, 0)),
        ]


def rot(p, yaw):
    """Rotate a point about the vertical axis."""
    a = math.radians(yaw)
    x, y, z = p
    return (x * math.cos(a) - y * math.sin(a), x * math.sin(a) + y * math.cos(a), z)


def project(p):
    """Vehicle space to frame pixels. +y is up-screen and so is +z, which is
    what makes this the same view the hexes are drawn in."""
    x, y, z = p
    return (ORIGIN[0] + x, ORIGIN[1] - y * SQUASH - z * ZS)


def shade(color, n):
    lam = max(0.0, sum(a * b for a, b in zip(n, LIGHT)))
    f = 0.38 + 0.66 * lam
    return tuple(min(255, int(c * f)) for c in color)


def inside(pts, x, y):
    hit = False
    n = len(pts)
    for i in range(n):
        x0, y0 = pts[i]
        x1, y1 = pts[(i + 1) % n]
        if (y0 > y) != (y1 > y):
            t = (y - y0) / (y1 - y0)
            if x < x0 + t * (x1 - x0):
                hit = not hit
    return hit


def depth_plane(pts, depths):
    """Fit `depth = a*sx + b*sy + c` over a projected face.

    The projection is orthographic and therefore affine, so a flat quad's
    view depth is an exact affine function of its screen position — three
    corners determine it and the fourth is guaranteed to agree. Returns None
    for a face that projects to a line, which is a face seen edge-on and has
    no interior to fill anyway."""
    (x0, y0), (x1, y1), (x2, y2) = pts[0], pts[1], pts[2]
    ax, ay = x1 - x0, y1 - y0
    bx, by = x2 - x0, y2 - y0
    det = ax * by - ay * bx
    if abs(det) < 1e-6:
        return None
    d0, d1, d2 = depths[0], depths[1], depths[2]
    u, v = d1 - d0, d2 - d0
    a = (u * by - v * ay) / det
    b = (v * ax - u * bx) / det
    return a, b, d0 - a * x0 - b * y0


def fill_poly(px, zbuf, pts, depths, color):
    """Rasterise one face, keeping whichever surface is nearest per pixel.

    A per-face painter's sort is not enough here and the failure is not
    subtle: a long sloped plate — the tank destroyer's casemate roof, the
    hull deck under a turret — has one mean depth but spans a wide range of
    real ones, so it is drawn either wholly in front of or wholly behind
    something it actually straddles. The symptom was a turret sunk into its
    own deck. A depth buffer costs nothing at 56x40 and removes the whole
    class of artefact."""
    plane = depth_plane(pts, depths)
    flat = sum(depths) / 4.0
    xs = [p[0] for p in pts]
    ys = [p[1] for p in pts]
    for y in range(max(0, int(min(ys))), min(FRAME_H, int(max(ys)) + 2)):
        for x in range(max(0, int(min(xs))), min(FRAME_W, int(max(xs)) + 2)):
            if not inside(pts, x + 0.5, y + 0.5):
                continue
            if plane is None:
                d = flat
            else:
                d = plane[0] * (x + 0.5) + plane[1] * (y + 0.5) + plane[2]
            # Ties go to the face drawn later, which is the sorted order's
            # answer, so coplanar decals like the academy plate still land on
            # top of the roof they are painted on.
            if zbuf[y][x] is None or d >= zbuf[y][x] - 1e-6:
                zbuf[y][x] = d
                px[y][x] = color


def outline(px, color=OUTLINE):
    """A dark border, so a vehicle reads against grass, road and mud alike."""
    src = [row[:] for row in px]
    for y in range(FRAME_H):
        for x in range(FRAME_W):
            if src[y][x] is not None:
                continue
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                nx, ny = x + dx, y + dy
                if 0 <= nx < FRAME_W and 0 <= ny < FRAME_H and src[ny][nx] is not None:
                    px[y][x] = color
                    break


def render(boxes, yaw):
    px = [[None] * FRAME_W for _ in range(FRAME_H)]
    zbuf = [[None] * FRAME_W for _ in range(FRAME_H)]
    faces = []
    for box in boxes:
        for name, quad, normal in box.faces():
            world = [rot(p, yaw) for p in quad]
            n = rot(normal, yaw)
            # Only faces turned towards the viewer, who is in front (+y) and
            # above (+z).
            if sum(a * b for a, b in zip(n, VIEW)) <= 0.02:
                continue
            base = TEAM if name in box.team_faces else box.color
            color = base if base == TEAM else shade(base, n)
            depths = [sum(a * b for a, b in zip(p, VIEW)) for p in world]
            centre = sum(depths) / 4.0
            faces.append((centre, [project(p) for p in world], depths, color))
    # Sorted along the line of sight, far first: `VIEW` points from the board
    # towards the camera, so a larger dot product is nearer. The depth buffer
    # decides what is actually visible; this order only settles exact ties,
    # which is what puts a decal on the roof it shares a plane with.
    #
    # The sort used to be by y with height as a tiebreak, which is a different
    # thing and got the south-east frame wrong: the camera looks down as well
    # as forward, so a hull roof is *behind* the turret standing on it even
    # though its centre is nearer in y. A turret sank into its own deck and
    # took the academy plate with it — the south-east frame carried eleven
    # magenta pixels where east carried seventy-four, and the turretless
    # casemate carried none at all.
    faces.sort(key=lambda f: f[0])
    for _, quad, depths, color in faces:
        fill_poly(px, zbuf, quad, depths, color)
    outline(px)
    return px


def medium_tank():
    """Panther: long tracked hull, big turret set back, long gun."""
    return [
        # Tracks. They overlap the hull in y and reach up past its floor, so
        # the near one reads as attached rather than as a bar lying alongside.
        Box(-16, 15, 7, 12, 1, 8, TRACK),
        Box(-16, 15, -12, -7, 1, 8, TRACK),
        # Hull.
        Box(-16, 17, -9, 9, 4, 11, HULL),
        # Turret, set back of centre.
        Box(-7, 5, -7, 7, 11, 16, TURRET),
        # School marking: a thin plate on the turret roof. A whole face turned
        # magenta swamps the vehicle; a plate is a marking.
        Box(-6, 3, -5, 5, 16, 16.9, TEAM),
        # And a panel on the rear plate, for when the roof is turned away.
        Box(-16.9, -16, -6, 6, 5, 10, TEAM),
        # Mantlet and gun.
        Box(5, 8, -3, 3, 12, 15, GUN),
        Box(8, 24, -1, 1, 13, 14.6, GUN),
    ]


def recon_car():
    """Luchs: small wheeled hull, pointed nose, little turret, autocannon."""
    boxes = []
    # Four wheels, overlapping the hull sides so they read as attached. The
    # gap between the pairs is the whole silhouette cue that says "wheeled".
    for wx in (-9, 7):
        boxes.append(Box(wx - 4, wx + 4, 6, 10, 0, 8, WHEEL))
        boxes.append(Box(wx - 4, wx + 4, -10, -6, 0, 8, WHEEL))
    boxes += [
        # Hull, narrower and shorter than the tank's.
        Box(-13, 11, -8, 8, 5, 11, HULL),
        # Nose, a smaller box carried forward and slightly lower.
        Box(11, 17, -5, 5, 5.5, 9.5, HULL),
        # Small open-topped turret, forward of centre.
        Box(-5, 2, -5, 5, 11, 14.5, TURRET),
        Box(-4.5, 1.5, -4, 4, 14.5, 15.3, TEAM),
        Box(-13.9, -13, -5, 5, 6, 10, TEAM),
        # Autocannon, clearly shorter than the tank's gun.
        Box(2, 16, -1, 1, 12, 13.4, GUN),
    ]
    return boxes


def light_tank():
    """Wiesel: the medium's cheap little sibling. Short hull, narrow tracks, a
    small turret carried forward and a stubby gun that barely clears the nose.

    Everything is scaled *down* from `medium_tank` rather than restyled — the
    two are meant to read as the same factory's work, and the point of the
    silhouette is that a player can tell at a glance which one is worth
    shooting first."""
    return [
        Box(-12, 11, 5, 9.5, 1, 7, TRACK),
        Box(-12, 11, -9.5, -5, 1, 7, TRACK),
        # Hull: two thirds the medium's length and visibly shallower.
        Box(-12, 13, -7, 7, 4, 10, HULL),
        # Turret forward of centre, which is the other cue: the medium's sits
        # back, so even the deck proportions differ.
        Box(-5, 4, -5.5, 5.5, 10, 13.6, TURRET),
        Box(-4, 2.5, -4.5, 4.5, 13.6, 14.5, TEAM),
        Box(-12.9, -12, -5, 5, 5, 9, TEAM),
        # 37 mm: short enough that it stops well short of the medium's muzzle.
        Box(4, 6.5, -2.5, 2.5, 10.8, 13, GUN),
        Box(6.5, 17, -0.8, 0.8, 11.4, 12.6, GUN),
    ]


def heavy_tank():
    """Löwe: bulk read three ways at once — a taller hull, tracks that stand
    proud of it, and a turret big enough to look like it costs something.

    The gun is the longest on the roster and reaches nearly to the frame edge,
    which is deliberate: at this size length is the cheapest legible signal
    that the thing outranges what it is pointed at."""
    return [
        # Wide tracks. They are both thicker in plan and taller than the
        # medium's, so they show under the hull from every frame.
        Box(-17, 15, 8, 14, 0.5, 9.5, TRACK),
        Box(-17, 15, -14, -8, 0.5, 9.5, TRACK),
        # Hull, deeper than the medium's and squarer in plan.
        Box(-17, 17, -10.5, 10.5, 3.5, 12, HULL),
        # Turret: broad, set slightly back, and the tallest box on any vehicle.
        Box(-9, 6, -9, 9, 12, 17.5, TURRET),
        # The plate is a marking, not a paint job: it stays the same physical
        # size across the roster, so on the biggest turret it covers least.
        Box(-6, 2, -5, 5, 17.5, 18.4, TEAM),
        Box(-17.9, -17, -7, 7, 5, 11, TEAM),
        # Mantlet and the 88, thicker in section as well as longer.
        Box(6, 9.5, -4, 4, 13, 16.5, GUN),
        Box(9.5, 25, -1.4, 1.4, 13.9, 15.9, GUN),
    ]


def tank_destroyer():
    """Marder: a casemate, and no turret at all.

    The absence is the characterisation, so nothing above the deck may look
    like it could traverse — and the first attempt at this failed exactly
    there: a superstructure smaller than the hull left bare deck fore and aft
    of it, and bare deck around a raised box is the visual definition of a
    turret. So the casemate is the vehicle. It runs the full width and very
    nearly the full length, and only its upper step draws in, which is how a
    box model spells "sloped"."""
    return [
        Box(-15, 13, 6.5, 11, 0.5, 6.5, TRACK),
        Box(-15, 13, -11, -6.5, 0.5, 6.5, TRACK),
        # Hull, low and long — everything above it is fighting compartment.
        Box(-15, 15, -8, 8, 2.5, 7, HULL),
        # Casemate: the whole footprint, so no deck shows around it. Painted in
        # the superstructure tone for the same reason the artillery's walls
        # are — a box standing on the hull with the same normal shades to the
        # same value, and the two would merge into one slab.
        Box(-14, 14, -8, 8, 7, 9.5, TURRET),
        # The upper step is pulled in at the sides and cut off short of the
        # nose, which reads as a glacis running back to a flat roof.
        Box(-14, 8, -6.5, 6.5, 9.5, 12, TURRET),
        Box(-12, -3, -5, 5, 12, 12.9, TEAM),
        Box(-15.9, -15, -6, 6, 3.5, 6.5, TEAM),
        # The 88 leaves the front plate, fixed forward, and sits low: the
        # muzzle is below the height of the light tank's turret roof, which is
        # the rest of the silhouette argument.
        Box(6, 9, -3, 3, 9.4, 12, GUN),
        Box(9, 25, -1, 1, 10.1, 11.5, GUN),
    ]


def artillery():
    """Hummel: an open-topped fighting compartment at the back, and a stubby
    howitzer pointing up out of it.

    The compartment is four walls with nothing on top rather than a solid box,
    because the open top is the whole reason a self-propelled gun looks
    fragile, and the painter's algorithm draws the far walls before the near
    one so the box reads as hollow. The barrel is short and steeply elevated —
    the one gun on the roster that does not lie along the hull."""
    boxes = [
        Box(-16, 13, 7, 12, 1, 8, TRACK),
        Box(-16, 13, -12, -7, 1, 8, TRACK),
        # Hull, with its deck left clear forward of the compartment: the engine
        # and driver live up front on this one, which is why the gun is at the
        # back.
        Box(-16, 16, -9, 9, 4, 10, HULL),
        # Compartment floor, so the hollow has a bottom rather than a hole.
        Box(-15, 4, -7.5, 7.5, 10, 10.6, TRACK),
        # Four walls. The rear one is tallest; the front one is cut down to a
        # shield so the interior is not simply boxed in from every angle.
        #
        # They are painted in the superstructure tone rather than the hull's.
        # A wall standing on the hull shares its outward normal and therefore
        # its lambert term exactly, so in the east frame the two flat faces
        # were the same value and the whole vehicle read as one dark slab with
        # a barrel stuck to it. The lighter tone is the same separation a
        # turret already gets, and it costs nothing.
        Box(-15, 4, 7.5, 9, 10, 16.5, TURRET),
        Box(-15, 4, -9, -7.5, 10, 16.5, TURRET),
        Box(-15.5, -13, -9, 9, 10, 16.5, TURRET),
        Box(2.5, 4, -9, 9, 10, 14, TURRET),
        Box(-15.9, -15.5, -6, 6, 11, 15.5, TEAM),
        # With no roof there is no roof plate, so the academy's marking goes on
        # the compartment's outer walls instead. Both sides carry it because
        # the frames show the near wall in one and the far in another, and a
        # unit that loses its colour when it turns is a unit you shoot by
        # mistake.
        Box(-12, 1, 9, 9.4, 11.5, 15, TEAM),
        Box(-12, 1, -9.4, -9, 11.5, 15, TEAM),
        # Short howitzer at high elevation. The breech sits inside the
        # compartment and the muzzle clears the front shield well above it.
        Box(-6, -3, -3, 3, 11.5, 14.5, GUN),
    ]
    # The barrel is the one part no axis-aligned box can express, so it is
    # built as a short staircase climbing forward — coarse, but at 56x40 a
    # two-pixel step is exactly what a drawn diagonal would look like anyway.
    x, z = -3.0, 12.3
    for _ in range(7):
        boxes.append(Box(x, x + 2.6, -1.1, 1.1, z, z + 1.6, GUN))
        x += 2.2
        z += 0.85
    return boxes


def write_png(path, frames):
    w, h = FRAME_W * FRAMES, FRAME_H
    raw = b""
    for y in range(h):
        raw += b"\x00"
        for x in range(w):
            c = frames[x // FRAME_W][y][x % FRAME_W]
            raw += bytes(c) + b"\xff" if c else b"\x00\x00\x00\x00"

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(png)
    print(f"wrote {path} ({w}x{h}, {FRAMES} frames)")


if __name__ == "__main__":
    root = os.path.join(os.path.dirname(__file__), "..", "assets", "mods", "base", "sprites")
    roster = (
        ("medium_tank", medium_tank()),
        ("recon_car", recon_car()),
        ("light_tank", light_tank()),
        ("heavy_tank", heavy_tank()),
        ("tank_destroyer", tank_destroyer()),
        ("artillery", artillery()),
    )
    for name, model in roster:
        frames = [render(model, yaw) for yaw in YAWS]
        write_png(os.path.join(root, f"{name}.png"), frames)
