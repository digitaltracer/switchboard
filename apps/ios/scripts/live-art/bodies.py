"""Renders the lock-screen card's separate bodies, at @3x, into the extension's asset catalog.

The card lays them out on orbits in SwiftUI, so any number of agents fits. Needs numpy and Pillow:
    python3 apps/ios/scripts/live-art/bodies.py apps/ios/SwitchboardLive/Assets.xcassets
`render.py` is the design's renderer, copied unchanged.
"""
import json
import os
import sys
import tempfile

CATALOG = sys.argv[1]
TMP = tempfile.mkdtemp()
sys.argv = [sys.argv[0], TMP, "none"]
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import render as R  # noqa: E402
import numpy as np  # noqa: E402

GREEN = (0.30, 0.92, 0.58)
PLANETS = {
    "jupiter": (12, dict(roll=0.12)),
    "saturn": (7, dict(roll=-0.35)),
    "earth": (41, dict(roll=0.4, spin=0.6)),
    "mars": (33, {}),
    "neptune": (21, dict(roll=0.3)),
    "venus": (52, {}),
}


def soft_edges(sc, width):
    edge = np.minimum(np.minimum(sc.px, sc.w - sc.px), np.minimum(sc.py, sc.h - sc.py))
    fade = R.sstep(0.5, width, edge)
    sc.rgb *= fade[..., None]
    sc.a *= fade


def imageset(name, render):
    """Each body at @2x and @3x, so a phone decodes only the size it draws."""
    path = os.path.join(CATALOG, f"{name}.imageset")
    os.makedirs(path, exist_ok=True)
    images = []
    for scale in (2, 3):
        R.S = scale
        file = f"{name}@{scale}x.png"
        render(file)
        os.replace(os.path.join(TMP, file), os.path.join(path, file))
        images.append({"idiom": "universal", "filename": file, "scale": f"{scale}x"})
    for old in os.listdir(path):
        if old.endswith(".png") and old not in [i["filename"] for i in images]:
            os.remove(os.path.join(path, old))
    with open(os.path.join(path, "Contents.json"), "w") as f:
        json.dump({"images": images, "info": {"author": "xcode", "version": 1}}, f, indent=2)


def sun(palette, seed):
    # The disc is a third of the image; the rest is corona.
    def draw(file):
        sc = R.Scene(90, 90, 1, transparent=True)
        sc.add_sun((45, 45), 15, seed, palette)
        soft_edges(sc, 14)
        sc.save(file)
    return draw


def planet(kind):
    seed, kw = PLANETS[kind]
    # Saturn's rings reach 2.5 radii; every planet sprite is the same size, its disc a third of it.
    def draw(file):
        sc = R.Scene(30, 30, 1, transparent=True)
        sc.add_body(kind, (15, 15), 5, seed, light=R.FAR_LEFT, **kw)
        sc.save(file)
    return draw


def finished(file):
    sc = R.Scene(60, 60, 1, transparent=True)
    sc.add_glow((30, 30), 26, GREEN, 0.6)
    sc.add_body("earth", (30, 30), 10, 41, light=R.FAR_LEFT, **PLANETS["earth"][1])
    soft_edges(sc, 10)
    sc.save(file)


def moon(file):
    sc = R.Scene(10, 10, 1, transparent=True)
    sc.add_body("moon", (5, 5), 3.2, 2, light=R.FAR_LEFT, dim=0.8)
    sc.save(file)


def stars(file):
    # Wide enough for any phone; the card crops it on the right, where it is faint anyway.
    sc = R.Scene(420, 150, 3)
    sc.stars(80, 8, 200)
    sc.save(file)


os.makedirs(CATALOG, exist_ok=True)
with open(os.path.join(CATALOG, "Contents.json"), "w") as f:
    json.dump({"info": {"author": "xcode", "version": 1}}, f, indent=2)
imageset("live-sun", sun("gold", 4))
imageset("live-sun-red", sun("red", 19))
imageset("live-finished", finished)
imageset("live-moon", moon)
imageset("live-stars", stars)
for k in PLANETS:
    imageset(f"live-{k}", planet(k))
