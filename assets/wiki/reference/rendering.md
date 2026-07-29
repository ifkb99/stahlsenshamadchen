---
id: rendering
title: Rendering
category: Reference
---

# Rendering

Sprites are drawn at native size: one texel is one world unit.

Hex faces are whole-texel sized (64×40, with a 64×30 tiling pitch). Sprite
and camera positions snap to the pixel grid. Zoom is quantised to a whole
number of physical pixels per texel (1×–4×).

Without that last part, a fractional desktop scale factor (1.25, 1.5, …)
feeds straight into the projection and smears the art.
