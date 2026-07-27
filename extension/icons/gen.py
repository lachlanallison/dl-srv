import struct
import zlib
from pathlib import Path

def png(w, h, rgb=(91, 141, 239)):
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    raw = b"".join(b"\x00" + bytes(rgb) * w for _ in range(h))
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )

out = Path(__file__).parent
for size in (16, 48, 128):
    (out / f"icon{size}.png").write_bytes(png(size, size))
print("ok")
