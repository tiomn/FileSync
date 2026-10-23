"""生成 FileSync 图标（纯标准库，无第三方依赖）。
输出: src-tauri/icons/{32x32.png,128x128.png,icon.ico}
"""
import os
import struct
import zlib

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "src-tauri", "icons")

# ---------- 绘制 ----------
BLUE_TOP = (47, 107, 255)
BLUE_BOT = (111, 155, 255)
WHITE = (255, 255, 255)


def _rounded(x, y, n, r):
    """圆角矩形内部判定"""
    if r <= x < n - r or r <= y < n - r:
        return True
    cx = r if x < r else n - 1 - r
    cy = r if y < r else n - 1 - r
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r


def _arrow_right(x, y, n):
    """→ 箭头"""
    if 0.20 * n <= x <= 0.60 * n and 0.30 * n <= y <= 0.42 * n:
        return True
    if 0.55 * n <= x <= 0.80 * n:
        t = (x - 0.55 * n) / (0.80 * n - 0.55 * n)
        half = (1 - t) * 0.11 * n
        if abs(y - 0.36 * n) <= half:
            return True
    return False


def _arrow_left(x, y, n):
    """← 箭头"""
    if 0.40 * n <= x <= 0.80 * n and 0.58 * n <= y <= 0.70 * n:
        return True
    if 0.20 * n <= x <= 0.45 * n:
        t = (0.45 * n - x) / (0.45 * n - 0.20 * n)
        half = (1 - t) * 0.11 * n
        if abs(y - 0.64 * n) <= half:
            return True
    return False


def render(n):
    r = max(2, int(n * 0.22))
    rows = []
    for y in range(n):
        row = []
        t = y / max(1, n - 1)
        bg = tuple(int(BLUE_TOP[i] + (BLUE_BOT[i] - BLUE_TOP[i]) * t) for i in range(3))
        for x in range(n):
            if not _rounded(x, y, n, r):
                row.append((0, 0, 0, 0))
                continue
            if _arrow_right(x, y, n) or _arrow_left(x, y, n):
                row.append((*WHITE, 255))
            else:
                row.append((*bg, 255))
        rows.append(row)
    return rows


# ---------- PNG ----------
def png(w, h, px):
    raw = b"".join(b"\x00" + b"".join(bytes(p) for p in row) for row in px)

    def chunk(tag, data):
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


# ---------- ICO ----------
def bmp_entry(px, w, h):
    hdr = struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    body = b""
    for y in range(h - 1, -1, -1):
        for x in range(w):
            r, g, b, a = px[y][x]
            body += bytes((b, g, r, a))
    mask = ((w + 31) // 32) * 4
    body += b"\x00" * (mask * h)
    return hdr + body


def ico(entries):
    n = len(entries)
    head = struct.pack("<HHH", 0, 1, n)
    offset = 6 + 16 * n
    dirs, datas = b"", b""
    for w, h, px in entries:
        d = bmp_entry(px, w, h)
        dirs += struct.pack(
            "<BBBBHHII",
            w if w < 256 else 0,
            h if h < 256 else 0,
            0,
            0,
            1,
            32,
            len(d),
            offset,
        )
        offset += len(d)
        datas += d
    return head + dirs + datas


def main():
    os.makedirs(OUT, exist_ok=True)
    for n in (32, 128):
        px = render(n)
        with open(os.path.join(OUT, f"{n}x{n}.png"), "wb") as f:
            f.write(png(n, n, px))
        print("wrote", f"{n}x{n}.png")
    with open(os.path.join(OUT, "icon.ico"), "wb") as f:
        f.write(ico([(s, s, render(s)) for s in (16, 32, 48, 256)]))
    print("wrote icon.ico")


if __name__ == "__main__":
    main()
