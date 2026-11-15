"""Build the compact data files that icarus-search embeds at compile time.

usage: python -I prep_data.py <airports.csv> <coast110.geojson> <coast50.geojson> <borders110.geojson> <outdir>

Inputs are untrusted downloads: they are only parsed as CSV / JSON text.
"""
import csv
import json
import os
import struct
import sys
import unicodedata

FOLD = {
    "ø": "o", "Ø": "O", "ł": "l", "Ł": "L", "ß": "ss", "đ": "d", "Đ": "D",
    "ı": "i", "æ": "ae", "Æ": "AE", "œ": "oe", "Œ": "OE", "þ": "th", "Þ": "Th",
    "ð": "d", "Ð": "D", "ħ": "h", "’": "'", "‘": "'", "–": "-", "—": "-",
    "ə": "a", "Ə": "A",
}


def fold(s):
    s = "".join(FOLD.get(c, c) for c in s)
    s = unicodedata.normalize("NFKD", s)
    s = "".join(c for c in s if not unicodedata.combining(c))
    s = "".join(c if 32 <= ord(c) < 127 else "" for c in s)
    return " ".join(s.replace("\t", " ").split())


def tidy_name(name):
    for a, b in (
        (" International Airport", " Intl"),
        (" Regional Airport", " Regional"),
        (" Airport", ""),
        ("International", "Intl"),
    ):
        name = name.replace(a, b)
    return name.strip(" /-")


def airports(src, out):
    rank = {"large_airport": 0, "medium_airport": 1, "small_airport": 2}
    best = {}
    with open(src, encoding="utf-8", newline="") as f:
        for r in csv.DictReader(f):
            t = r.get("type", "")
            if t not in rank or r.get("scheduled_service") != "yes":
                continue
            iata = (r.get("iata_code") or "").strip().upper()
            if len(iata) != 3 or not iata.isascii() or not iata.isalpha():
                continue
            try:
                lat, lon = float(r["latitude_deg"]), float(r["longitude_deg"])
            except (TypeError, ValueError):
                continue
            if not (-90 <= lat <= 90 and -180 <= lon <= 180):
                continue
            name = tidy_name(fold(r.get("name") or ""))
            city = fold(r.get("municipality") or "").split(" (")[0].strip() or name
            country = fold(r.get("iso_country") or "")[:2].upper() or "??"
            # search-only aliases ("TYO, Tokyo Narita ...") so "tokyo" finds NRT
            kw = fold(r.get("keywords") or "")[:90]
            row = (rank[t], iata, lat, lon, country, city, name, kw)
            if iata not in best or row[0] < best[iata][0]:
                best[iata] = row
    rows = sorted(best.values(), key=lambda x: (x[0], x[1]))
    with open(out, "w", encoding="ascii", newline="\n") as f:
        for rk, iata, lat, lon, country, city, name, kw in rows:
            f.write(f"{iata}\t{rk}\t{lat:.4f}\t{lon:.4f}\t{country}\t{city}\t{name}\t{kw}\n")
    counts = [sum(1 for r in rows if r[0] == k) for k in (0, 1, 2)]
    return len(rows), counts


def load_lines(path):
    with open(path, encoding="utf-8") as f:
        gj = json.load(f)
    lines = []
    for feat in gj.get("features", []):
        g = feat.get("geometry")
        if not g:
            continue
        t, c = g["type"], g["coordinates"]
        if t == "LineString":
            lines.append(c)
        elif t in ("MultiLineString", "Polygon"):
            lines.extend(c)
        elif t == "MultiPolygon":
            for poly in c:
                lines.extend(poly)
    return lines


def simplify(line, tol):
    if tol <= 0 or len(line) < 3:
        return line
    out = [line[0]]
    for p in line[1:-1]:
        q = out[-1]
        if abs(p[0] - q[0]) >= tol or abs(p[1] - q[1]) >= tol:
            out.append(p)
    out.append(line[-1])
    return out


def pack(lines, tol, path):
    """u32 line count, then per line: u16 point count + (i16 lon, i16 lat) in 0.01 deg."""
    kept = []
    for line in lines:
        line = simplify(line, tol)
        while len(line) >= 2:
            kept.append(line[:60000])
            line = line[59999:] if len(line) > 60000 else []
    buf = bytearray(struct.pack("<I", len(kept)))
    pts = 0
    for line in kept:
        buf += struct.pack("<H", len(line))
        for p in line:
            lon = max(-180.0, min(180.0, float(p[0])))
            lat = max(-90.0, min(90.0, float(p[1])))
            buf += struct.pack("<hh", round(lon * 100), round(lat * 100))
            pts += 1
    with open(path, "wb") as f:
        f.write(buf)
    return len(kept), pts, len(buf)


def landmask(path, out, width=1440, height=720):
    """Rasterise land polygons to a 1-bit equirectangular mask (even-odd scanline fill).

    Row 0 is the northernmost; bit index = row * width + col, LSB-first in each byte.
    """
    with open(path, encoding="utf-8") as f:
        gj = json.load(f)
    rings = []
    for feat in gj.get("features", []):
        g = feat.get("geometry")
        if not g:
            continue
        if g["type"] == "Polygon":
            rings.extend(g["coordinates"])
        elif g["type"] == "MultiPolygon":
            for poly in g["coordinates"]:
                rings.extend(poly)
    crossings = [[] for _ in range(height)]
    dlat = 180.0 / height
    for ring in rings:
        for (x0, y0), (x1, y1) in zip(ring, ring[1:]):
            x0, y0, x1, y1 = float(x0), float(y0), float(x1), float(y1)
            if y0 == y1:
                continue
            lo, hi = (y0, y1) if y0 < y1 else (y1, y0)
            # rows whose centre latitude lies in [lo, hi)
            r_first = max(0, int((90.0 - hi) / dlat - 0.5) - 1)
            r_last = min(height - 1, int((90.0 - lo) / dlat - 0.5) + 1)
            for r in range(r_first, r_last + 1):
                yc = 90.0 - (r + 0.5) * dlat
                if lo <= yc < hi:
                    crossings[r].append(x0 + (yc - y0) / (y1 - y0) * (x1 - x0))
    bits = bytearray(width * height // 8)
    dlon = 360.0 / width
    land = 0
    for r, xs in enumerate(crossings):
        xs.sort()
        for a, b in zip(xs[0::2], xs[1::2]):
            c0 = max(0, int((a + 180.0) / dlon + 0.5))
            c1 = min(width, int((b + 180.0) / dlon + 0.5))
            for c in range(c0, c1):
                i = r * width + c
                bits[i >> 3] |= 1 << (i & 7)
                land += 1
    with open(out, "wb") as f:
        f.write(bits)
    return width, height, len(bits), f"{100.0 * land / (width * height):.1f}% set"


def preview(path, width=1440, height=720, cols=96, rows=30):
    data = open(path, "rb").read()
    for r in range(rows):
        line = ""
        for c in range(cols):
            i = (r * height // rows) * width + (c * width // cols)
            line += "#" if data[i >> 3] >> (i & 7) & 1 else "."
        print(line)


def main():
    if len(sys.argv) == 4 and sys.argv[1] == "--land":
        print("landmask:", landmask(sys.argv[2], sys.argv[3]))
        preview(sys.argv[3])
        return
    a_csv, c110, c50, b110, outdir = sys.argv[1:6]
    os.makedirs(outdir, exist_ok=True)
    n, counts = airports(a_csv, os.path.join(outdir, "airports.tsv"))
    print(f"airports: {n} (large {counts[0]}, medium {counts[1]}, small {counts[2]})")
    print("coast110:", pack(load_lines(c110), 0.0, os.path.join(outdir, "coast110.bin")))
    print("coast50: ", pack(load_lines(c50), 0.08, os.path.join(outdir, "coast50.bin")))
    print("borders: ", pack(load_lines(b110), 0.0, os.path.join(outdir, "borders110.bin")))


if __name__ == "__main__":
    main()
