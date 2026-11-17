"""Build the shared orbital icon as PNG and ICO."""

from math import hypot
from pathlib import Path
import struct
import zlib

ROOT = Path(__file__).resolve().parents[1]
SIZE = 64

def sample(x, y):
    radius = hypot(x, y)
    axis = x * .91 - y * .41
    normal = x * .41 + y * .91
    orbit = hypot(axis / 13.5, normal / 5.5)
    if abs(x - 7) + abs(y + 5) < 3.3:
        return (250, 225, 169, 255)
    if .84 <= orbit < 1.05 and (radius > 8.5 or normal > 0):
        return (225, 181, 99, 255)
    if 7.1 < radius < 8.6:
        return (181, 145, 85, 255)
    if max(abs(x), abs(y)) < 14 and abs(x) + abs(y) < 24.5:
        return (23, 26, 21, 255)
    return (0, 0, 0, 0)

def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

pixels = bytearray()
for y in range(SIZE):
    pixels.append(0)
    for x in range(SIZE):
        pixels.extend(sample((x+.5)*32/SIZE-16, (y+.5)*32/SIZE-16))
png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(pixels, 9)) + chunk(b"IEND", b"")
assets = ROOT / "src/assets"
(assets / "orbit.png").write_bytes(png)
header = struct.pack("<HHHBBBBHHII", 0, 1, 1, SIZE, SIZE, 0, 0, 1, 32, len(png), 22)
(assets / "orbit.ico").write_bytes(header + png)
