"""Renders the Aura orb (the logo above the task box) as the 1024 px app icon source.

    python tools/make-icon.py src-tauri/icons/source.png
    ui/node_modules/.bin/tauri icon src-tauri/icons/source.png   # every size, .ico and .icns

The same look as `.orb` in ui/src/styles/app.css: a blurred violet/cyan/pink conic halo behind a
glassy sphere lit from the upper left.
"""
import math
import sys

import numpy as np
from PIL import Image, ImageFilter

SIZE = 1024
c = (SIZE - 1) / 2
y, x = np.mgrid[0:SIZE, 0:SIZE].astype(np.float32)
dx, dy = x - c, y - c
r = np.hypot(dx, dy)


def lerp_stops(t, stops):
    """Piecewise-linear colour/alpha ramp; t in 0..1, stops [(pos, (r,g,b,a)), ...]."""
    out = np.zeros(t.shape + (4,), np.float32)
    for (p0, c0), (p1, c1) in zip(stops, stops[1:]):
        m = (t >= p0) & (t <= p1)
        k = ((t - p0) / max(p1 - p0, 1e-6))[m][:, None]
        out[m] = np.array(c0, np.float32) * (1 - k) + np.array(c1, np.float32) * k
    out[t > stops[-1][0]] = stops[-1][1]
    return out


# Halo: a conic gradient disc, blurred and at 80 % opacity.
violet, cyan, pink = (139, 92, 246), (34, 211, 238), (244, 114, 182)
angle = (np.arctan2(dx, -dy) / (2 * math.pi)) % 1.0
conic = lerp_stops(angle, [(0, violet + (255,)), (1 / 3, cyan + (255,)), (2 / 3, pink + (255,)), (1, violet + (255,))])
halo_r = 0.40 * SIZE
conic[..., 3] = np.where(r <= halo_r, 255 * 0.8, 0)
halo = Image.fromarray(conic.astype(np.uint8), "RGBA").filter(ImageFilter.GaussianBlur(0.055 * SIZE))

# Sphere: radial gradient from a highlight at 35 % / 30 % of its box.
sphere_r = 0.30 * SIZE
box_left, box_top, diameter = c - sphere_r, c - sphere_r, 2 * sphere_r
hx, hy = box_left + 0.35 * diameter, box_top + 0.30 * diameter
reach = math.hypot(0.65, 0.70) * diameter
t = np.hypot(x - hx, y - hy) / reach
sphere = lerp_stops(t, [
    (0.00, (255, 255, 255, 0.95 * 255)),
    (0.28, (196, 181, 253, 0.80 * 255)),
    (0.62, (139, 92, 246, 0.62 * 255)),
    (1.00, (34, 211, 238, 0.55 * 255)),
])
edge = np.clip(sphere_r - r + 0.5, 0, 1)  # anti-aliased rim
# A soft inner rim light, like the inset glow in the CSS.
rim = np.clip(1 - (sphere_r - r) / (0.12 * sphere_r), 0, 1) * edge
sphere[..., :3] = sphere[..., :3] * (1 - 0.25 * rim[..., None]) + 255 * 0.25 * rim[..., None]
sphere[..., 3] = np.maximum(sphere[..., 3], 0.5 * 255) * edge
sphere_img = Image.fromarray(sphere.clip(0, 255).astype(np.uint8), "RGBA")

icon = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
icon = Image.alpha_composite(icon, halo)
icon = Image.alpha_composite(icon, sphere_img)
icon.save(sys.argv[1] if len(sys.argv) > 1 else "icon.png")
print("saved")
