import math
import os
import sys

import numpy as np
from PIL import Image

S = 3  # image pixels per CSS pixel
OUT = sys.argv[1]

GRAD = np.array(
    [[1, 1, 0], [-1, 1, 0], [1, -1, 0], [-1, -1, 0], [1, 0, 1], [-1, 0, 1],
     [1, 0, -1], [-1, 0, -1], [0, 1, 1], [0, -1, 1], [0, 1, -1], [0, -1, -1]], float)


def perlin(x, y, z, seed):
    p = np.random.RandomState(seed).permutation(256)
    p = np.concatenate([p, p, p])
    xi = np.floor(x).astype(int)
    yi = np.floor(y).astype(int)
    zi = np.floor(z).astype(int)
    xf, yf, zf = x - xi, y - yi, z - zi
    xi &= 255
    yi &= 255
    zi &= 255
    fade = lambda t: t * t * t * (t * (t * 6 - 15) + 10)
    u, v, w = fade(xf), fade(yf), fade(zf)

    def g(ix, iy, iz, dx, dy, dz):
        gr = GRAD[p[p[p[ix] + iy] + iz] % 12]
        return gr[..., 0] * dx + gr[..., 1] * dy + gr[..., 2] * dz

    lerp = lambda a, b, t: a + (b - a) * t
    x1 = lerp(g(xi, yi, zi, xf, yf, zf), g(xi + 1, yi, zi, xf - 1, yf, zf), u)
    x2 = lerp(g(xi, yi + 1, zi, xf, yf - 1, zf), g(xi + 1, yi + 1, zi, xf - 1, yf - 1, zf), u)
    y1 = lerp(x1, x2, v)
    x3 = lerp(g(xi, yi, zi + 1, xf, yf, zf - 1), g(xi + 1, yi, zi + 1, xf - 1, yf, zf - 1), u)
    x4 = lerp(g(xi, yi + 1, zi + 1, xf, yf - 1, zf - 1), g(xi + 1, yi + 1, zi + 1, xf - 1, yf - 1, zf - 1), u)
    return lerp(y1, lerp(x3, x4, v), w)


def fbm(x, y, z, octaves, seed, gain=0.5):
    total = np.zeros_like(x)
    amp, freq = 1.0, 1.0
    for o in range(octaves):
        total += amp * perlin(x * freq + 31.7 * o, y * freq + 17.3 * o, z * freq + 5.1 * o, seed + o)
        amp *= gain
        freq *= 2.0
    return total


def sstep(a, b, x):
    t = np.clip((x - a) / (b - a), 0, 1)
    return t * t * (3 - 2 * t)


def mix(a, b, t):
    return np.asarray(a)[None, None, :] * (1 - t[..., None]) + np.asarray(b)[None, None, :] * t[..., None]


def mixa(a, b, t):
    return a * (1 - t[..., None]) + np.asarray(b)[None, None, :] * t[..., None]


def norm(v):
    v = np.asarray(v, float)
    return v / np.linalg.norm(v)


def surface(kind, x, y, z, seed):
    """Colour of a point on the unit sphere in the body's own frame. y is the pole axis."""
    atmos, rim = (0.5, 0.6, 0.9), 0.25
    if kind == "jupiter":
        warp = fbm(x * 2, y * 5, z * 2, 5, seed)
        t = y + 0.07 * warp
        a = 0.5 + 0.5 * np.sin(t * 10 + 0.8)
        b = 0.5 + 0.5 * np.sin(t * 23 + 2.0 + 2.0 * warp)
        col = mix((0.93, 0.86, 0.74), (0.70, 0.46, 0.30), a)
        col = mixa(col, (0.86, 0.68, 0.48), 0.45 * b)
        streak = fbm(x * 3, y * 16, z * 3, 4, seed + 9)
        col *= (1 + 0.16 * streak)[..., None]
        spot = np.exp(-(((x - 0.35) / 0.2) ** 2 + ((y - 0.3) / 0.11) ** 2))
        col = mixa(col, (0.74, 0.30, 0.18), 0.8 * spot * (z > 0))
        atmos, rim = (0.95, 0.8, 0.6), 0.18
    elif kind == "saturn":
        warp = fbm(x * 2, y * 4, z * 2, 4, seed)
        t = y + 0.04 * warp
        a = 0.5 + 0.5 * np.sin(t * 9)
        b = 0.5 + 0.5 * np.sin(t * 26 + 1.0)
        col = mix((0.93, 0.84, 0.62), (0.78, 0.64, 0.42), a)
        col = mixa(col, (0.88, 0.76, 0.52), 0.35 * b)
        atmos, rim = (0.95, 0.85, 0.6), 0.15
    elif kind == "neptune":
        warp = fbm(x * 1.6, y * 4, z * 1.6, 5, seed)
        t = y + 0.08 * warp
        a = 0.5 + 0.5 * np.sin(t * 6 + 1.0)
        col = mix((0.10, 0.22, 0.70), (0.30, 0.52, 0.95), a)
        cloud = sstep(0.20, 0.55, fbm(x * 1.4, y * 6, z * 1.4, 4, seed + 4))
        col = mixa(col, (0.80, 0.90, 1.0), 0.32 * cloud)
        atmos, rim = (0.45, 0.65, 1.0), 0.5
    elif kind == "earth":
        h = fbm(x * 1.5, y * 1.5, z * 1.5, 6, seed)
        land = sstep(0.02, 0.07, h)
        col = mix((0.02, 0.10, 0.34), (0.05, 0.30, 0.60), sstep(-0.4, 0.02, h))
        dry = sstep(-0.2, 0.3, fbm(x * 3, y * 3, z * 3, 4, seed + 3))
        ground = mix((0.14, 0.36, 0.13), (0.58, 0.47, 0.28), dry)
        col = col * (1 - land[..., None]) + ground * land[..., None]
        ice = sstep(0.78, 0.88, np.abs(y) + 0.06 * h)
        col = mixa(col, (0.95, 0.97, 1.0), ice)
        cloud = sstep(0.12, 0.42, fbm(x * 2.0 + 7, y * 3.0, z * 2.0, 5, seed + 7))
        col = mixa(col, (1.0, 1.0, 1.0), 0.8 * cloud)
        atmos, rim = (0.35, 0.6, 1.0), 0.7
    elif kind == "mars":
        h = fbm(x * 1.8, y * 1.8, z * 1.8, 6, seed)
        col = mix((0.78, 0.40, 0.20), (0.40, 0.19, 0.11), sstep(-0.15, 0.3, h))
        col *= (1 + 0.2 * fbm(x * 6, y * 6, z * 6, 3, seed + 2))[..., None]
        ice = sstep(0.84, 0.92, -y + 0.05 * h)
        col = mixa(col, (0.96, 0.94, 0.92), ice)
        atmos, rim = (0.95, 0.62, 0.42), 0.3
    elif kind == "venus":
        warp = fbm(x * 1.4 + y * 2.2, y * 3.5, z * 1.4, 5, seed)
        col = mix((0.94, 0.84, 0.58), (0.76, 0.56, 0.28), sstep(-0.35, 0.35, warp))
        atmos, rim = (1.0, 0.9, 0.65), 0.4
    else:  # moon
        h = fbm(x * 2.0, y * 2.0, z * 2.0, 6, seed)
        col = mix((0.60, 0.60, 0.62), (0.30, 0.31, 0.33), sstep(-0.1, 0.25, h))
        rs = np.random.RandomState(seed)
        for _ in range(7):
            c = norm(rs.normal(size=3))
            r = rs.uniform(0.10, 0.24)
            d = np.arccos(np.clip(x * c[0] + y * c[1] + z * c[2], -1, 1)) / r
            col *= (1 - 0.35 * sstep(1.0, 0.6, d) + 0.25 * np.exp(-((d - 1.0) / 0.12) ** 2))[..., None]
        atmos, rim = (0.6, 0.6, 0.65), 0.0
    return np.clip(col, 0, 1.2), atmos, rim


def body(kind, radius, light, seed, roll=0.3, pitch=0.25, spin=0.0, dim=1.0):
    """Returns (rgb, alpha, half) for a lit sphere sprite. radius in CSS px."""
    R = radius * S
    ext = 2.5 if kind == "saturn" else 1.08
    half = int(math.ceil(R * ext)) + 2
    yy, xx = np.mgrid[-half:half, -half:half] + 0.5
    x, y = xx / R, yy / R
    d = np.sqrt(x * x + y * y)
    z = np.sqrt(np.clip(1 - d * d, 0, 1))
    alpha = np.clip((1 - d) * R + 0.5, 0, 1)
    xr = x * math.cos(roll) + y * math.sin(roll)
    yr = -x * math.sin(roll) + y * math.cos(roll)
    yp = yr * math.cos(pitch) - z * math.sin(pitch)
    zp = yr * math.sin(pitch) + z * math.cos(pitch)
    xs = xr * math.cos(spin) + zp * math.sin(spin)
    zs = -xr * math.sin(spin) + zp * math.cos(spin)
    col, atmos, rim = surface(kind, xs, yp, zs, seed)
    L = norm(light)
    ndl = x * L[0] + y * L[1] + z * L[2]
    lit = sstep(-0.12, 0.30, ndl) * (0.30 + 0.70 * np.clip(ndl, 0, 1) ** 0.8)
    limb = 0.62 + 0.38 * z ** 0.7
    rgb = col * (0.018 + lit * limb)[..., None]
    fres = (1 - z) ** 2.5 * sstep(-0.25, 0.35, ndl)
    rgb += np.asarray(atmos)[None, None, :] * (rim * fres)[..., None]
    rgb *= dim
    if kind == "saturn":
        q = 0.34
        rr = np.sqrt(xr * xr + (yr / q) ** 2)
        band = sstep(1.28, 1.34, rr) * sstep(2.28, 2.22, rr)
        gaps = 0.55 + 0.45 * np.sin(rr * 38) * 0.5 + 0.2
        gaps *= 1 - 0.85 * np.exp(-((rr - 1.96) / 0.035) ** 2)
        ring_a = np.clip(band * gaps, 0, 1) * 0.9
        ring_rgb = np.asarray((0.86, 0.78, 0.62))[None, None, :] * (0.55 + 0.35 * sstep(-0.6, 0.6, xr * L[0] + yr * L[1]))[..., None] * dim
        front = yr > 0
        behind = ring_a * (1 - alpha) * (~front)
        rgb = rgb * alpha[..., None] + ring_rgb * behind[..., None]
        a = alpha + behind
        fa = ring_a * front
        rgb = rgb * (1 - fa[..., None]) + ring_rgb * fa[..., None]
        a = a * (1 - fa) + fa
        rgb = rgb / np.maximum(a, 1e-6)[..., None]
        alpha = a
    return np.clip(rgb, 0, 1), alpha, half


SUN_RAMPS = {
    "gold": [(0.0, (0.55, 0.10, 0.01)), (0.35, (0.95, 0.36, 0.04)), (0.62, (1.0, 0.66, 0.16)),
             (0.85, (1.0, 0.88, 0.50)), (1.05, (1.0, 0.98, 0.90))],
    "red": [(0.0, (0.35, 0.03, 0.02)), (0.35, (0.75, 0.10, 0.06)), (0.62, (0.96, 0.26, 0.16)),
            (0.85, (1.0, 0.52, 0.38)), (1.05, (1.0, 0.86, 0.78))],
}
SUN_GLOW = {"gold": (1.0, 0.62, 0.26), "red": (1.0, 0.30, 0.20)}


def ramp(stops, t):
    out = np.zeros(t.shape + (3,))
    for (t0, c0), (t1, c1) in zip(stops[:-1], stops[1:]):
        w = np.clip((t - t0) / (t1 - t0), 0, 1)
        seg = (t >= t0) if t0 > stops[0][0] else np.ones_like(t, bool)
        val = np.asarray(c0)[None, None, :] * (1 - w[..., None]) + np.asarray(c1)[None, None, :] * w[..., None]
        out = np.where(seg[..., None], val, out)
    return out


def sun(radius, seed, palette="gold", extent=3.4):
    """Returns (disc_rgb, disc_alpha, glow_rgb, half). Glow is additive."""
    R = radius * S
    half = int(math.ceil(R * extent))
    yy, xx = np.mgrid[-half:half, -half:half] + 0.5
    x, y = xx / R, yy / R
    d = np.sqrt(x * x + y * y)
    z = np.sqrt(np.clip(1 - d * d, 0, 1))
    cells = fbm(x * 9, y * 9, z * 9, 4, seed)
    swirl = fbm(x * 2.6 + 0.4 * cells, y * 2.6, z * 2.6, 5, seed + 11)
    spots = sstep(0.50, 0.66, fbm(x * 4.2, y * 4.2, z * 4.2, 3, seed + 23))
    veins = 1 - sstep(0.0, 0.10, np.abs(fbm(x * 3.4 + 0.3 * swirl, y * 3.4, z * 3.4, 4, seed + 37)))
    t = (0.52 + 0.48 * z ** 0.55) * (1 + 0.16 * cells + 0.20 * swirl) - 0.22 * spots - 0.10 * veins
    disc = ramp(SUN_RAMPS[palette], np.clip(t, 0, 1.05))
    alpha = np.clip((1 - d) * R + 0.5, 0, 1)
    theta = np.arctan2(y, x)
    rays = fbm(np.cos(theta) * 2.2, np.sin(theta) * 2.2, d * 0.9, 4, seed + 31)
    rays = np.clip(0.75 + 0.9 * rays, 0.15, 1.6)
    out = np.clip(d - 1, 0, None)
    corona = 0.95 * np.exp(-out * 5.5) + 0.42 * np.exp(-out * 1.9) * rays + 0.10 * np.exp(-out * 0.8)
    corona *= sstep(extent, extent * 0.72, d)
    corona = np.where(d < 1, 0.9 * (1 - z) ** 1.5, corona)
    hot = np.exp(-out * 9)[..., None]
    g = np.asarray(SUN_GLOW[palette])[None, None, :]
    glow = (g * (1 - 0.6 * hot) + np.asarray((1.0, 0.95, 0.82))[None, None, :] * 0.6 * hot) * corona[..., None]
    return np.clip(disc, 0, 1), alpha, glow, half


class Scene:
    def __init__(self, w, h, seed, transparent=False):
        self.w, self.h = w, h
        self.W, self.H = w * S, h * S
        yy, xx = np.mgrid[0:self.H, 0:self.W] + 0.5
        self.px, self.py = xx / S, yy / S
        self.transparent = transparent
        self.rgb = np.zeros((self.H, self.W, 3))
        self.a = np.zeros((self.H, self.W))
        if not transparent:
            self.rgb += np.asarray((0.018, 0.024, 0.036))[None, None, :]
            haze = sstep(-0.1, 0.6, fbm(self.px / 90, self.py / 90, np.zeros_like(self.px), 4, seed))
            self.rgb += np.asarray((0.020, 0.024, 0.050))[None, None, :] * haze[..., None]
            self.a += 1

    def stars(self, n, seed, text_x):
        rs = np.random.RandomState(seed)
        for _ in range(n):
            sx, sy = rs.uniform(0, self.w), rs.uniform(0, self.h)
            b = rs.uniform(0.15, 1.0) ** 2.2 * 0.9
            b *= 1 - 0.75 * float(sstep(text_x - 30, text_x + 20, np.asarray(sx)))
            r = rs.uniform(0.35, 0.8)
            tint = np.asarray([(1, 1, 1), (0.8, 0.88, 1.0), (1.0, 0.92, 0.8)][rs.randint(3)])
            x0, x1 = int(max(0, (sx - 4) * S)), int(min(self.W, (sx + 4) * S))
            y0, y1 = int(max(0, (sy - 4) * S)), int(min(self.H, (sy + 4) * S))
            dx = self.px[y0:y1, x0:x1] - sx
            dy = self.py[y0:y1, x0:x1] - sy
            self.rgb[y0:y1, x0:x1] += tint[None, None, :] * (b * np.exp(-(dx * dx + dy * dy) / (r * r)))[..., None]

    def orbits(self, centre, axes, q, rot, fade_x, alpha=0.19, quiet=()):
        cx, cy = centre
        dx, dy = self.px - cx, self.py - cy
        xr = dx * math.cos(rot) + dy * math.sin(rot)
        yr = -dx * math.sin(rot) + dy * math.cos(rot)
        fade = 1 - sstep(fade_x - 28, fade_x + 14, self.px)
        for x0, y0, x1, y1 in quiet:
            inside = sstep(x0 - 8, x0, self.px) * sstep(x1 + 8, x1, self.px) * sstep(y0 - 6, y0, self.py) * sstep(y1 + 6, y1, self.py)
            fade = fade * (1 - 0.92 * inside)
        for a in axes:
            b = a * q
            e = np.sqrt((xr / a) ** 2 + (yr / b) ** 2)
            grad = np.sqrt((xr / (a * a)) ** 2 + (yr / (b * b)) ** 2) / np.maximum(e, 1e-6)
            dist = np.abs(e - 1) / np.maximum(grad, 1e-6)
            line = np.clip(0.75 - dist, 0, 1) * alpha * fade
            self.rgb += np.asarray((0.72, 0.80, 1.0))[None, None, :] * line[..., None]
            self.a = np.maximum(self.a, np.clip(line * 3, 0, 1))

    def on_orbit(self, centre, a, q, rot, deg):
        t = math.radians(deg)
        x, y = a * math.cos(t), a * q * math.sin(t)
        return (centre[0] + x * math.cos(rot) - y * math.sin(rot), centre[1] + x * math.sin(rot) + y * math.cos(rot))

    def _box(self, pos, half):
        cx, cy = int(round(pos[0] * S)), int(round(pos[1] * S))
        x0, y0 = cx - half, cy - half
        sx0, sy0 = max(0, -x0), max(0, -y0)
        x1, y1 = min(self.W, x0 + 2 * half), min(self.H, y0 + 2 * half)
        x0, y0 = max(0, x0), max(0, y0)
        return (slice(y0, y1), slice(x0, x1)), (slice(sy0, sy0 + (y1 - y0)), slice(sx0, sx0 + (x1 - x0)))

    def add_glow(self, pos, radius, colour, strength):
        d = np.sqrt((self.px - pos[0]) ** 2 + (self.py - pos[1]) ** 2) / radius
        g = strength * np.exp(-d * d * 2.2)
        self.rgb += np.asarray(colour)[None, None, :] * g[..., None]
        self.a = np.maximum(self.a, np.clip(g * 1.6, 0, 1))

    def add_sun(self, pos, radius, seed, palette="gold"):
        disc, alpha, glow, half = sun(radius, seed, palette)
        dst, src = self._box(pos, half)
        self.rgb[dst] += glow[src]
        self.a[dst] = np.maximum(self.a[dst], np.clip(glow[src].max(axis=2) * 1.4, 0, 1))
        self.rgb[dst] = self.rgb[dst] * (1 - alpha[src][..., None]) + disc[src] * alpha[src][..., None]
        self.a[dst] = np.maximum(self.a[dst], alpha[src])

    def add_body(self, kind, pos, radius, seed, light=None, toward=None, **kw):
        if toward is not None:
            v = np.asarray(toward, float) - np.asarray(pos, float)
            v /= np.linalg.norm(v)
            light = (v[0], v[1], 0.62)
        rgb, alpha, half = body(kind, radius, light, seed, **kw)
        dst, src = self._box(pos, half)
        self.rgb[dst] = self.rgb[dst] * (1 - alpha[src][..., None]) + rgb[src] * alpha[src][..., None]
        self.a[dst] = np.maximum(self.a[dst], alpha[src])

    def save(self, name):
        rgb = np.clip(self.rgb, 0, 1)
        if self.transparent:
            a = np.clip(self.a, 0, 1)
            rgb = np.where(a[..., None] > 0, rgb / np.maximum(a, 1e-6)[..., None], 0)
            arr = np.dstack([np.clip(rgb, 0, 1), a])
            Image.fromarray((arr * 255 + 0.5).astype(np.uint8), "RGBA").save(os.path.join(OUT, name))
        else:
            Image.fromarray((rgb * 255 + 0.5).astype(np.uint8), "RGB").save(os.path.join(OUT, name))


FAR_LEFT = (-0.72, -0.30, 0.62)
Q, ROT = 0.42, math.radians(-10)


def hero(with_sun):
    sc = Scene(366, 150, 3)
    c = (60, 76)
    axes = [74, 104, 136, 170, 206]
    sc.stars(70, 5, 205)
    if with_sun:
        sc.add_glow(c, 150, (1.0, 0.55, 0.22), 0.10)
    sc.orbits(c, axes, Q, ROT, 205)
    spots = [
        ("earth", sc.on_orbit(c, axes[0], Q, ROT, 52), 8.5, 41, dict(roll=0.4, spin=0.6)),
        ("jupiter", sc.on_orbit(c, axes[1], Q, ROT, -52), 13, 12, dict(roll=0.12, pitch=0.2)),
        ("saturn", sc.on_orbit(c, axes[2], Q, ROT, 38), 9.5, 7, dict(roll=-0.35, pitch=0.2)),
        ("moon", sc.on_orbit(c, axes[3], Q, ROT, -44), 4.5, 2, dict(dim=0.75)),
        ("moon", sc.on_orbit(c, axes[4], Q, ROT, 62), 4.0, 9, dict(dim=0.7)),
    ]
    if with_sun:
        sc.add_sun(c, 27, 4)
        for kind, pos, r, seed, kw in spots:
            sc.add_body(kind, pos, r, seed, toward=c, **kw)
    else:
        sc.add_body("neptune", c, 12, 21, light=FAR_LEFT, roll=0.3)
        for kind, pos, r, seed, kw in spots:
            sc.add_body(kind, pos, r, seed, light=FAR_LEFT, **kw)
    sc.save("hero-sun.png" if with_sun else "hero-before.png")


def state(name):
    sc = Scene(366, 100, 8 + len(name))
    c = (62, 51)
    axes = [48, 66, 86, 106, 128]
    sc.stars(46, 13 + len(name), 150)
    if name == "calm":
        sc.orbits(c, axes, Q, ROT, 150)
        for kind, i, deg, r, seed, kw in [
            ("neptune", 0, 160, 9, 21, dict(roll=0.3)),
            ("earth", 0, -20, 7, 41, dict(roll=0.4, spin=0.6)),
            ("jupiter", 1, -90, 10.5, 12, dict(roll=0.12)),
            ("mars", 1, 100, 6.5, 33, dict()),
            ("saturn", 2, 50, 7.5, 7, dict(roll=-0.35)),
            ("moon", 3, -100, 4, 2, dict(dim=0.75)),
        ]:
            sc.add_body(kind, sc.on_orbit(c, axes[i], Q, ROT, deg), r, seed, light=FAR_LEFT, **kw)
    elif name == "two":
        axes = [60, 82, 104, 126]
        a, b = (44, 56), (78, 42)
        sc.add_glow(c, 110, (1.0, 0.55, 0.22), 0.10)
        sc.orbits(c, axes, Q, ROT, 150)
        sc.add_sun(b, 12.5, 14)
        sc.add_sun(a, 16.5, 4)
        for kind, i, deg, r, seed, kw in [
            ("jupiter", 1, -60, 9, 12, dict(roll=0.12)),
            ("mars", 0, 50, 6, 33, dict()),
            ("moon", 1, 110, 4, 2, dict(dim=0.75)),
            ("moon", 2, 55, 3.6, 9, dict(dim=0.7)),
        ]:
            pos = sc.on_orbit(c, axes[i], Q, ROT, deg)
            sc.add_body(kind, pos, r, seed, toward=c, **kw)
    elif name == "done":
        sc.orbits(c, axes, Q, ROT, 150)
        done = sc.on_orbit(c, axes[0], Q, ROT, -150)
        sc.add_glow(done, 24, (0.30, 0.92, 0.58), 0.6)
        sc.add_body("earth", done, 10, 41, light=FAR_LEFT, roll=0.4, spin=0.6)
        for kind, i, deg, r, seed, kw in [
            ("jupiter", 1, -80, 10.5, 12, dict(roll=0.12)),
            ("venus", 0, 20, 6.5, 52, dict()),
            ("saturn", 2, 60, 7.5, 7, dict(roll=-0.35)),
            ("mars", 1, 110, 6.5, 33, dict()),
            ("moon", 3, -100, 4, 2, dict(dim=0.75)),
        ]:
            sc.add_body(kind, sc.on_orbit(c, axes[i], Q, ROT, deg), r, seed, light=FAR_LEFT, **kw)
    else:  # error
        sc.add_glow(c, 110, (1.0, 0.25, 0.16), 0.10)
        sc.orbits(c, axes, Q, ROT, 150)
        sc.add_sun(c, 17, 19, "red")
        for kind, i, deg, r, seed, kw in [
            ("earth", 0, 40, 7, 41, dict(roll=0.4, spin=0.6)),
            ("jupiter", 1, -70, 10, 12, dict(roll=0.12)),
            ("saturn", 2, 60, 7.5, 7, dict(roll=-0.35)),
            ("moon", 1, 130, 4, 2, dict(dim=0.75)),
            ("moon", 3, -100, 3.6, 9, dict(dim=0.7)),
        ]:
            sc.add_body(kind, sc.on_orbit(c, axes[i], Q, ROT, deg), r, seed, toward=c, **kw)
    sc.save(f"state-{name}.png")


def island():
    sc = Scene(62, 24, 1, transparent=True)
    sc.add_sun((12, 12), 6.5, 4)
    sc.add_body("jupiter", (30, 12), 4.6, 12, toward=(12, 12), roll=0.12)
    sc.add_body("earth", (43, 12), 3.6, 41, toward=(12, 12), roll=0.4, spin=0.6)
    sc.add_body("moon", (53, 12), 2.4, 2, toward=(12, 12), dim=0.8)
    sc.save("island-system.png")
    sc = Scene(24, 24, 1, transparent=True)
    sc.add_sun((12, 12), 6.5, 4)
    sc.save("island-sun.png")



Q2, ROT2 = 0.9, math.radians(-6)


def axis_through(c, pos, q=Q2, rot=ROT2):
    dx, dy = pos[0] - c[0], pos[1] - c[1]
    xr = dx * math.cos(rot) + dy * math.sin(rot)
    yr = -dx * math.sin(rot) + dy * math.cos(rot)
    return math.sqrt(xr * xr + (yr / q) ** 2)


def label_zone(x, top, w=62, h=15):
    return (x - w / 2, top, x + w / 2, top + h)


def labelled(name, w, h, c, bodies, quiet, seed, lead=None, star_n=60):
    """bodies: (kind, pos, r, seed, kw). lead: None | ('sun', r, seed, palette) | ('suns', [(pos, r, seed)]) | ('glow', colour)."""
    sc = Scene(w, h, seed)
    sc.stars(star_n, seed + 5, 10000)
    lit_by_sun = lead is not None and lead[0] in ("sun", "suns")
    if lit_by_sun:
        sc.add_glow(c, h, (1.0, 0.25, 0.16) if lead[0] == "sun" and lead[3] == "red" else (1.0, 0.55, 0.22), 0.10)
    sc.orbits(c, sorted({round(axis_through(c, b[1]), 1) for b in bodies} - {0.0}), Q2, ROT2, 10000, alpha=0.17, quiet=quiet)
    if lead and lead[0] == "sun":
        sc.add_sun(c, lead[1], lead[2], lead[3])
    elif lead and lead[0] == "suns":
        for pos, r, sd in lead[1]:
            sc.add_sun(pos, r, sd)
    for kind, pos, r, sd, kw in bodies:
        kw = dict(kw)
        glow = kw.pop("glow", None)
        if glow:
            sc.add_glow(pos, r * 2.4, glow, 0.6)
        if lit_by_sun:
            sc.add_body(kind, pos, r, sd, toward=c, **kw)
        else:
            sc.add_body(kind, pos, r, sd, light=FAR_LEFT, **kw)
    sc.save(name)


def v2():
    J = dict(roll=0.12)
    E = dict(roll=0.4, spin=0.6)
    SA = dict(roll=-0.35)
    M1, M2 = dict(dim=0.8), dict(dim=0.75)
    # hero, 366 x 150
    c = (62, 58)
    bodies = [("jupiter", (150, 86), 12.5, 12, J), ("earth", (212, 44), 9, 41, E), ("saturn", (278, 84), 10, 7, SA),
              ("moon", (322, 38), 4.4, 2, M1), ("moon", (343, 60), 4.0, 9, M2)]
    quiet = [(10, 92, 122, 136), label_zone(150, 103, 44), label_zone(212, 57, 68), label_zone(278, 99, 56), label_zone(333, 70, 40)]
    labelled("v2-hero-sun.png", 366, 150, c, bodies, quiet, 3, lead=("sun", 25, 4, "gold"), star_n=70)
    labelled("v2-hero-before.png", 366, 150, c, bodies + [("neptune", c, 12, 21, dict(roll=0.3))], quiet, 3, star_n=70)
    # two need you, 366 x 100
    c = (64, 35)
    bodies = [("jupiter", (176, 44), 10, 12, J), ("mars", (238, 30), 6.5, 33, {}), ("moon", (296, 50), 4, 2, M1), ("moon", (318, 36), 3.6, 9, M2)]
    quiet = [(6, 56, 70, 90), (72, 48, 122, 80), label_zone(176, 58, 68), label_zone(238, 40, 56), label_zone(307, 59, 40)]
    labelled("v2-two.png", 366, 100, c, bodies, quiet, 15, lead=("suns", [((96, 32), 12, 14), ((36, 36), 15.5, 4)]), star_n=44)
    # error
    c = (40, 36)
    bodies = [("earth", (150, 50), 7.5, 41, E), ("jupiter", (208, 34), 10, 12, J), ("saturn", (270, 54), 8, 7, SA),
              ("moon", (318, 34), 4, 2, M1), ("moon", (338, 50), 3.6, 9, M2)]
    quiet = [(6, 58, 132, 94), label_zone(150, 61, 56), label_zone(208, 48, 44), label_zone(270, 66, 68), label_zone(328, 59, 40)]
    labelled("v2-error.png", 366, 100, c, bodies, quiet, 17, lead=("sun", 16, 19, "red"), star_n=44)
    # one finished: the finished planet takes the lead slot with a green glow
    bodies = [("earth", (40, 36), 12, 41, dict(E, glow=(0.30, 0.92, 0.58))), ("jupiter", (146, 48), 10, 12, J), ("venus", (196, 30), 6.5, 52, {}),
              ("saturn", (250, 52), 8, 7, SA), ("mars", (300, 32), 6.5, 33, {}), ("moon", (340, 52), 4, 2, M1)]
    quiet = [(6, 58, 132, 94), label_zone(146, 62, 44), label_zone(196, 40, 56), label_zone(250, 64, 56), label_zone(300, 42, 26), label_zone(340, 60, 38)]
    sc_bodies = bodies
    sc = Scene(366, 100, 19)
    sc.stars(44, 24, 10000)
    sc.orbits((40, 36), sorted({round(axis_through((40, 36), b[1]), 1) for b in sc_bodies[1:]}), Q2, ROT2, 10000, alpha=0.17, quiet=quiet)
    sc.add_glow((40, 36), 30, (0.30, 0.92, 0.58), 0.6)
    for kind, pos, r, sd, kw in sc_bodies:
        kw = {k: v for k, v in kw.items() if k != "glow"}
        sc.add_body(kind, pos, r, sd, light=FAR_LEFT, **kw)
    sc.save("v2-done.png")
    # calm: nothing at the centre
    c = (48, 44)
    bodies = [("neptune", (146, 52), 9, 21, dict(roll=0.3)), ("jupiter", (184, 30), 10.5, 12, J), ("earth", (232, 54), 7.5, 41, E),
              ("saturn", (282, 32), 8, 7, SA), ("mars", (322, 58), 6.5, 33, {}), ("moon", (346, 28), 4, 2, M1)]
    quiet = [(6, 22, 118, 70), label_zone(146, 65, 56), label_zone(184, 44, 44), label_zone(232, 65, 68), label_zone(282, 44, 56), label_zone(322, 68, 26), label_zone(340, 36, 38)]
    labelled("v2-calm.png", 366, 100, c, bodies, quiet, 21, star_n=44)


which = sys.argv[2:] or ["hero", "states", "island"]
if "v2" in which:
    v2()
if "hero" in which:
    hero(True)
    hero(False)
if "states" in which:
    for n in ["calm", "two", "done", "error"]:
        state(n)
if "island" in which:
    island()
