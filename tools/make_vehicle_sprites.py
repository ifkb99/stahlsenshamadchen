#!/usr/bin/env python3
"""Draw the vehicle sprites.

Kept as a script rather than hand-painted files because these are placeholder-
grade art meant to be replaced by a real artist, and because a script makes the
whole roster consistent: one palette, one light direction, one set of
proportions. Re-run it and every vehicle moves together.

    python3 tools/make_vehicle_sprites.py

Constraints the art has to satisfy, all of them from the renderer:

* **Top-down, pointing east.** `battle::sync_units` rotates the sprite with
  `Quat::from_rotation_z(facing_angle(..))`, so one image is turned to face any
  of six directions under any of six view rotations. A 3/4 view would only be
  correct at one of them.
* **Squashed vertically.** The map is isometric with `SQUASH = 40/64`, so a
  vehicle that should read as twice as long as it is wide is drawn nearer three
  times as long. This is the same compromise the placeholder blob makes.
* **Pixel art at native size.** `ImagePlugin::default_nearest`, so no
  antialiasing: every edge is a deliberate pixel. Shapes are drawn by predicate
  rather than by PIL's smoothing primitives.
* **One key colour for the side.** `TEAM` pixels are replaced at load with the
  academy's colour (`iso::SIDE_COLORS`), which is what keeps the world drab and
  the sides legible — the split the tone notes call the one part of the current
  look that reads as design.
"""

import os
import struct
import zlib

W, H = 48, 32
CX, CY = W / 2.0, H / 2.0

# Muted military palette. The vehicle is deliberately drab; the only saturated
# thing on it is the team flash.
HULL = (86, 92, 70)
HULL_LIT = (108, 114, 88)
HULL_DARK = (58, 63, 47)
TRACK = (48, 46, 42)
TRACK_LIT = (66, 63, 57)
TURRET = (96, 102, 78)
TURRET_LIT = (120, 126, 98)
GUN = (52, 54, 46)
OUTLINE = (28, 30, 24)
TEAM = (255, 0, 255)  # replaced per side at load time


def blank():
    return [[None] * W for _ in range(H)]


def put(px, x, y, color):
    if 0 <= x < W and 0 <= y < H:
        px[int(y)][int(x)] = color


def rect(px, x0, y0, x1, y1, color):
    for y in range(int(y0), int(y1) + 1):
        for x in range(int(x0), int(x1) + 1):
            put(px, x, y, color)


def ellipse(px, cx, cy, rx, ry, color):
    for y in range(H):
        for x in range(W):
            dx, dy = (x + 0.5 - cx) / rx, (y + 0.5 - cy) / ry
            if dx * dx + dy * dy <= 1.0:
                put(px, x, y, color)


def outline(px, color=OUTLINE):
    """One-pixel dark border around everything opaque, for readability against
    grass, road and mud alike. Done last, and only into empty pixels."""
    src = [row[:] for row in px]
    for y in range(H):
        for x in range(W):
            if src[y][x] is not None:
                continue
            for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                nx, ny = x + dx, y + dy
                if 0 <= nx < W and 0 <= ny < H and src[ny][nx] is not None:
                    px[y][x] = color
                    break


def rounded(px, x0, y0, x1, y1, color, corner=2):
    """Axis-aligned box with the corners knocked off, which is what stops a
    hull from reading as a brick at this size."""
    for y in range(int(y0), int(y1) + 1):
        for x in range(int(x0), int(x1) + 1):
            dx = min(x - x0, x1 - x)
            dy = min(y - y0, y1 - y)
            if dx + dy >= corner:
                put(px, x, y, color)


def medium_tank():
    """Panther: long hull, wide tracks, big turret set back, long gun."""
    px = blank()
    # Tracks: the full length of the vehicle, and the widest thing on it.
    for (ty0, ty1) in ((5, 9), (22, 26)):
        rounded(px, 5, ty0, 40, ty1, TRACK, corner=1)
        # Tread ticks. Lit on the top run, dark on the bottom, so the two
        # sides do not read as identical stripes.
        for x in range(7, 39, 3):
            put(px, x, ty0 + 1 if ty0 == 5 else ty1 - 1, TRACK_LIT)

    # Hull, running past the tracks at the nose so the glacis is visible.
    rounded(px, 6, 10, 41, 21, HULL, corner=2)
    for i in range(5):
        rect(px, 41 + i, 12 + i, 41 + i, 19 - i, HULL)
    # Top-lit: a strong band along the upper flank, a dark one below.
    rect(px, 7, 10, 40, 12, HULL_LIT)
    rect(px, 7, 20, 40, 21, HULL_DARK)
    # Engine deck grille at the rear, to break up the flat plate.
    for x in range(9, 17, 2):
        rect(px, x, 14, x, 18, HULL_DARK)
    # Team flash. Sized for legibility rather than realism: with the hull this
    # drab, a small marking makes the two sides indistinguishable at a glance,
    # and knowing whose tank that is at a glance is the whole job. A band
    # across the rear deck plus the turret roof reads from any facing.
    rect(px, 7, 10, 13, 21, TEAM)
    rect(px, 6, 12, 6, 19, TEAM)

    # Turret: pulled back off the nose and ringed in shadow, so it sits *on*
    # the hull instead of merging with it.
    ellipse(px, 24, 16, 9.5, 6.5, HULL_DARK)
    ellipse(px, 24, 16, 8.0, 5.2, TURRET)
    # Highlight is a crescent on the upper-left, not a second full ellipse —
    # a filled lighter oval just reads as a smaller turret.
    for y in range(H):
        for x in range(W):
            dx, dy = (x + 0.5 - 24) / 7.0, (y + 0.5 - 15.4) / 4.2
            d = dx * dx + dy * dy
            if 0.30 < d <= 1.0 and (y < 16 or x < 24):
                put(px, x, y, TURRET_LIT)
    # Commander's cupola, off to the left rear of the turret roof.
    ellipse(px, 20, 13, 2.2, 1.7, HULL_DARK)
    ellipse(px, 20, 13, 1.4, 1.0, TEAM)

    # Mantlet: narrow, and darker than the turret so the gun has a root.
    rect(px, 31, 14, 33, 18, HULL_DARK)
    rect(px, 33, 15, 46, 16, GUN)
    rect(px, 44, 13, 46, 18, GUN)
    outline(px)
    return px


def recon_car():
    """Luchs: small, four wheels, little turret, stubby autocannon."""
    px = blank()
    # Wheels stand proud of the hull, which is the whole silhouette cue that
    # says "wheeled" rather than "tracked" at this size.
    for wx in (14, 32):
        for wy in (8, 24):
            ellipse(px, wx, wy, 4.0, 3.4, TRACK)
            ellipse(px, wx, wy - 1, 2.8, 2.1, TRACK_LIT)
            ellipse(px, wx, wy, 1.4, 1.1, HULL_DARK)

    # Hull: shorter than the tank, and pointed at the nose.
    rounded(px, 9, 11, 36, 20, HULL, corner=2)
    for i in range(6):
        rect(px, 36 + i, 12 + i, 36 + i, 19 - i, HULL)
    rect(px, 10, 11, 35, 12, HULL_LIT)
    rect(px, 10, 19, 35, 20, HULL_DARK)
    # Same reasoning as the tank: a band, not a dot.
    rect(px, 10, 11, 16, 20, TEAM)

    # Small open-topped turret, set back so the pointed nose stays readable.
    ellipse(px, 22, 15.5, 6.0, 4.8, HULL_DARK)
    ellipse(px, 22, 15.5, 4.8, 3.6, TURRET)
    ellipse(px, 22, 15.5, 1.6, 1.2, TEAM)
    for y in range(H):
        for x in range(W):
            dx, dy = (x + 0.5 - 22) / 4.2, (y + 0.5 - 15.0) / 3.0
            d = dx * dx + dy * dy
            if 0.25 < d <= 1.0 and (y < 16 or x < 22):
                put(px, x, y, TURRET_LIT)

    # Autocannon: thin, and it has to clear the nose or it reads as nothing.
    rect(px, 26, 15, 45, 16, GUN)
    rect(px, 43, 14, 45, 17, GUN)
    outline(px)
    return px


def write_png(path, px):
    raw = b""
    for y in range(H):
        raw += b"\x00"
        for x in range(W):
            c = px[y][x]
            raw += bytes(c) + b"\xff" if c else b"\x00\x00\x00\x00"

    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(png)
    print(f"wrote {path} ({W}x{H})")


if __name__ == "__main__":
    root = os.path.join(os.path.dirname(__file__), "..", "assets", "mods", "base", "sprites")
    write_png(os.path.join(root, "medium_tank.png"), medium_tank())
    write_png(os.path.join(root, "recon_car.png"), recon_car())
