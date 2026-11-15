# Embedded data

These files are compiled into the binary with `include_str!` /
`include_bytes!`, so the program needs nothing on disk at run time.

| File             | Contents                                             | Source                                   |
|------------------|------------------------------------------------------|------------------------------------------|
| `airports.tsv`   | 4,007 airports with an IATA code and scheduled service | OurAirports `airports.csv`             |
| `coast110.bin`   | Coastlines, 1:110m                                   | Natural Earth `ne_110m_coastline`        |
| `coast50.bin`    | Coastlines, 1:50m (used when zoomed in)              | Natural Earth `ne_50m_coastline`         |
| `borders110.bin` | Land borders, 1:110m                                 | Natural Earth `ne_110m_admin_0_boundary_lines_land` |
| `landmask.bin`   | Land/sea bitmap, 0.25 degrees per cell               | Natural Earth `ne_50m_land`, rasterised  |

Both sources are public domain:

- OurAirports: https://ourairports.com/data/
- Natural Earth: https://www.naturalearthdata.com/ (GeoJSON from
  https://github.com/nvkelso/natural-earth-vector)

## Formats

`airports.tsv`: tab-separated, one airport per line:
IATA, size (0 large, 1 medium, 2 small), latitude, longitude, country code,
city, name, search aliases. ASCII only; accents are folded.

`*.bin` line work, little-endian: `u32` line count, then per line a `u16`
point count followed by `(i16 lon, i16 lat)` pairs in hundredths of a degree.

`landmask.bin`: 1440 x 720 bits, row 0 at the north pole, column 0 at 180 W,
bit index `row * 1440 + col`, least significant bit first within each byte.
A set bit is land.

## Regenerating

Download the four GeoJSON files and `airports.csv`, then:

```bash
python prep_data.py airports.csv ne_110m_coastline.geojson ne_50m_coastline.geojson ne_110m_admin_0_boundary_lines_land.geojson .
```

```bash
python prep_data.py --land ne_50m_land.geojson landmask.bin
```

The script needs only the Python standard library.
