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


def fill_poly(px, pts, color):
    xs = [p[0] for p in pts]
    ys = [p[1] for p in pts]
    for y in range(max(0, int(min(ys))), min(FRAME_H, int(max(ys)) + 2)):
        for x in range(max(0, int(min(xs))), min(FRAME_W, int(max(xs)) + 2)):
            if inside(pts, x + 0.5, y + 0.5):
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
            depth = sum(p[1] for p in world) / 4.0
            height = sum(p[2] for p in world) / 4.0
            faces.append((depth, height, [project(p) for p in world], color))
    # Painter's algorithm: far (large y) first, and lower before higher, so a
    # turret lands on top of the hull it sits on.
    faces.sort(key=lambda f: (-f[0], f[1]))
    for _, _, quad, color in faces:
        fill_poly(px, quad, color)
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
    for name, model in (("medium_tank", medium_tank()), ("recon_car", recon_car())):
        frames = [render(model, yaw) for yaw in YAWS]
        write_png(os.path.join(root, f"{name}.png"), frames)
