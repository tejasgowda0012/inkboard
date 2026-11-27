"""Render the canonical Inkboard mark at launcher sizes (pure Python or Pillow).

Run from the project root. All paths stay within this project.
"""
import math
import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WAVE = [(12.0, 36.0), (17.0, 31.0), (23.0, 36.0), (29.0, 28.0), (36.0, 32.0), (44.0, 18.0)]
BG = (0x27, 0x48, 0xE8)


def _dist_seg(px, py, ax, ay, bx, by):
    vx, vy = bx - ax, by - ay
    t = max(0.0, min(1.0, ((px - ax) * vx + (py - ay) * vy) / (vx * vx + vy * vy)))
    return math.hypot(px - (ax + t * vx), py - (ay + t * vy))


def _render_rgba(size):
    scale = 56.0 / size
    r_box = 16.0
    half_stroke = 2.5
    aa = 0.65 * scale
    out = bytearray(size * size * 4)
    idx = 0
    for y in range(size):
        py = (y + 0.5) * scale
        for x in range(size):
            px = (x + 0.5) * scale
            # Signed distance to 56x56 rounded rect with rx=16
            qx = abs(px - 28.0) - (28.0 - r_box)
            qy = abs(py - 28.0) - (28.0 - r_box)
            sd = math.hypot(max(qx, 0.0), max(qy, 0.0)) + min(max(qx, qy), 0.0) - r_box
            bg_a = max(0.0, min(1.0, (0.5 * scale - sd) / aa))
            if bg_a <= 0.0:
                idx += 4
                continue
            d_wave = 1e9
            for i in range(len(WAVE) - 1):
                d = _dist_seg(px, py, WAVE[i][0], WAVE[i][1], WAVE[i + 1][0], WAVE[i + 1][1])
                if d < d_wave:
                    d_wave = d
            fg_a = max(0.0, min(1.0, (half_stroke - d_wave) / aa))
            out[idx] = round(BG[0] + (255 - BG[0]) * fg_a)
            out[idx + 1] = round(BG[1] + (255 - BG[1]) * fg_a)
            out[idx + 2] = round(BG[2] + (255 - BG[2]) * fg_a)
            out[idx + 3] = round(255 * bg_a)
            idx += 4
    return bytes(out)


def _save_png(path, size, rgba):
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    raw = bytearray()
    stride = size * 4
    for y in range(size):
        raw.append(0)
        raw.extend(rgba[y * stride : (y + 1) * stride])
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", zlib.compress(bytes(raw), 6)) + chunk(b"IEND", b"")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


if __name__ == "__main__":
    for density, size in [("mdpi", 48), ("hdpi", 72), ("xhdpi", 96), ("xxhdpi", 144), ("xxxhdpi", 192)]:
        path = ROOT / f"app/android/app/src/main/res/mipmap-{density}/ic_launcher.png"
        _save_png(path, size, _render_rgba(size))
    _save_png(ROOT / "branding/logo.png", 1024, _render_rgba(1024))
    print("Generated canonical Inkboard PNGs and branding/logo.png")
