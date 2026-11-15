//! The globe pane: geography, flight arcs, airport markers and HUD.
//!
//! Layers are drawn in priority order onto a `DotCanvas`; text labels are
//! written over the top afterwards at whole-cell positions.

use std::f64::consts::TAU;

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use super::{
    canvas::{DotCanvas, Marker},
    pane,
    theme::Theme,
};
use crate::{
    airports::{self, Airport},
    app::{App, Leg, MarkKind, Pane},
    continents::{self, Continent},
    fmt,
    geo::{self, LatLon, Projection, Side, V3, View},
    geodata::{self, Polyline},
};

// Layer priorities, lowest first. A higher layer wipes lower layers out of
// any cell it touches.
const P_GRID: u8 = 1;
const P_LAND: u8 = 2;
const P_LIMB: u8 = 3;
const P_COAST: u8 = 4;
const P_CITY: u8 = 5;
const P_FAINT: u8 = 6;
const P_SECONDARY: u8 = 7;
const P_PRIMARY: u8 = 8;
const P_TRAIL: u8 = 9;
const P_MARK: u8 = 10;

/// Seconds for the aircraft marker to fly the whole selected itinerary.
const FLIGHT_LOOP_SECS: f64 = 7.0;

/// Airports and their labels: a neutral tone that belongs to no continent.
const PLACE: Color = Color::Indexed(252);

/// The land mask holds a quarter of a degree per cell. Past this zoom one
/// screen dot is finer than that, and the coastline data takes over.
const COAST_FROM_ZOOM: f64 = 4.0;

pub fn draw(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let focused = app.focus == Pane::Globe && app.popup.is_none();
    let globe = app.view.proj == Projection::Globe;
    let mut block = pane(if globe { "GLOBE" } else { "WORLD MAP" }, focused, th);
    if area.width > 84 {
        // The hint doubles as the colour key for the two ends of a route.
        let dim = Style::new().fg(th.dim);
        let fullscreen = if app.globe_full {
            "F BACK"
        } else {
            "F FULLSCREEN"
        };
        let hint = Line::from(vec![
            Span::styled(format!(" G MAP  {fullscreen}  > < TEXT SIZE  "), dim),
            Span::styled("R-CLICK=ORIGIN", Style::new().fg(th.route(0.0))),
            Span::styled("  ", dim),
            Span::styled("CLICK=DEST", Style::new().fg(th.route(1.0))),
            Span::styled(" ", dim),
        ]);
        block = block.title_bottom(hint.right_aligned());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.hit.globe = area;
    app.hit.globe_inner = inner;
    if inner.width < 8 || inner.height < 4 {
        return;
    }

    // The planet itself is the costly part of a frame and, unless the view
    // has moved, exactly what was drawn last time. Keep it, and draw only
    // the flight paths afresh.
    let scene = Scene {
        view: app.view,
        size: (inner.width, inner.height),
        marker: app.marker,
        theme: th.name,
        tinted: app.cfg.continents,
        borders: app.cfg.borders,
    };
    let mut cv = match &app.geography {
        Some((drawn, canvas)) if *drawn == scene => canvas.clone(),
        _ => {
            let canvas = geography(&scene, th);
            app.geography = Some((scene, canvas.clone()));
            canvas
        }
    };

    let app = &*app;
    let view = app.view;
    let (w, h) = (cv.width(), cv.height());
    let lines = app.marker.draws_lines();
    let routes = app.routes();
    for (a, b) in &routes.faint {
        plain_arc(&mut cv, &view, *a, *b, th.route_other(), th.faint, P_FAINT);
    }
    for leg in &routes.secondary {
        route_arc(
            &mut cv,
            &view,
            leg,
            th,
            Stroke {
                dim: true,
                dashed: false,
            },
            P_SECONDARY,
        );
    }
    for leg in &routes.primary {
        // A route nobody has searched yet is only pencilled in.
        route_arc(
            &mut cv,
            &view,
            leg,
            th,
            Stroke {
                dim: false,
                dashed: routes.planned,
            },
            P_PRIMARY,
        );
    }

    // The aircraft: a bright trail on the canvas, plus a glyph on top.
    let lofted = globe;
    let mut plane: Option<(f64, f64)> = None;
    // Where the aircraft was a moment ago, to tell which way it is heading.
    let mut wake: Option<(f64, f64)> = None;
    if !routes.planned && !routes.primary.is_empty() {
        let t = (app.clock / FLIGHT_LOOP_SECS).fract();
        for k in (0..=12).rev() {
            let Some(p) = along(&routes.primary, t - k as f64 * 0.004, lofted) else {
                continue;
            };
            let (x, y, side) = view.project(p, w, h);
            if side != Side::Hidden {
                cv.plot(x, y, Color::Indexed(231), P_TRAIL);
                match k {
                    0 => plane = Some((x, y)),
                    6 => wake = Some((x, y)),
                    _ => {}
                }
            }
        }
    }

    for mark in &routes.marks {
        let (x, y, side) = view.project(mark.at, w, h);
        if side == Side::Front {
            // The two ends of the trip get a bigger cross than the stops.
            let reach = if mark.kind == MarkKind::Via { 1 } else { 2 };
            let color = th.route(mark.tone);
            cv.plot(x, y, color, P_MARK);
            for d in 1..=reach {
                let d = f64::from(d);
                for (dx, dy) in [(d, 0.0), (-d, 0.0), (0.0, d), (0.0, -d)] {
                    cv.plot(x + dx, y + dy, color, P_MARK);
                }
            }
        }
    }

    let buf = f.buffer_mut();
    cv.render(inner, buf);

    // ---- text overlays -----------------------------------------------------
    let hud = Style::new().fg(th.dim);
    let projection = if globe { "ORTHO" } else { "FLAT" };
    let mut top_left = format!("{projection} {:.1}x", view.zoom);
    if app.marker != Marker::Braille {
        top_left.push_str(&format!("  [{}]", app.marker.label()));
    }
    put(buf, inner, inner.x + 1, inner.y, &top_left, hud);
    let centre = format!(
        "CENTRE {:.1}{} {:.1}{}",
        view.lat0.abs(),
        if view.lat0 >= 0.0 { 'N' } else { 'S' },
        view.lon0.abs(),
        if view.lon0 >= 0.0 { 'E' } else { 'W' }
    );
    put(buf, inner, inner.x + 1, inner.bottom() - 1, &centre, hud);
    if let (Some(a), Some(b)) = (app.form.origin, app.form.dest) {
        let text = format!(
            "{} KM  HDG {:03.0}°",
            fmt::thousands(geo::distance_km(a.pos, b.pos).round() as u64),
            geo::bearing(a.pos, b.pos)
        );
        put_right(buf, inner, inner.y, &text, hud);
    }
    if let Some(airport) = app.hover {
        // Say so when a click here would have to ask which airport is meant.
        let more = match app.hover_more {
            0 => String::new(),
            n => format!("  +{n} NEARBY"),
        };
        let text = format!("► {}{more}", airport.long());
        put_right(
            buf,
            inner,
            inner.bottom() - 1,
            &text,
            Style::new().fg(th.bright).add_modifier(Modifier::BOLD),
        );
    }

    if let Some((x, y)) = plane {
        let (col, row) = (inner.x + (x / 2.0) as u16, inner.y + (y / 4.0) as u16);
        // Fonts without an aeroplane get an arrowhead flying the right way.
        let glyph = app.glyphs.plane.unwrap_or_else(|| {
            let (dx, dy) = wake.map_or((1.0, 0.0), |(wx, wy)| (x - wx, y - wy));
            match (dx.abs() >= dy.abs(), dx >= 0.0, dy >= 0.0) {
                (true, true, _) => "►",
                (true, false, _) => "◄",
                (false, _, true) => "▼",
                (false, _, false) => "▲",
            }
        });
        put(
            buf,
            inner,
            col,
            row,
            glyph,
            Style::new()
                .fg(Color::Indexed(231))
                .add_modifier(Modifier::BOLD),
        );
    }

    // Labels. The HUD corners are spoken for; then the route's two ends are
    // placed first so that, where labels would collide (Haneda and Narita
    // are 60 km apart), it is the layover's label that gives way; and the
    // places you are merely flying past take whatever room is left.
    let mut labels = Labels {
        inner,
        taken: Vec::new(),
    };
    labels.reserve(inner.y, inner.x, inner.right());
    labels.reserve(inner.bottom() - 1, inner.x, inner.right());
    for mark in routes.marks.iter().rev() {
        let Some((col, row)) = cell_of(&view, mark.at, inner) else {
            continue;
        };
        let color = th.route(mark.tone);
        let style = if mark.kind == MarkKind::Via {
            Style::new().fg(color).add_modifier(Modifier::BOLD)
        } else {
            // Origin and destination are solid tags: unmissable, and
            // unmistakable for each other.
            Style::new()
                .fg(th.bg)
                .bg(color)
                .add_modifier(Modifier::BOLD)
        };
        labels.place(buf, col, row, mark.code, style, 4);
    }
    if lines {
        let on_route = |a: &Airport| routes.marks.iter().any(|m| m.code == a.iata);
        let style = Style::new().fg(PLACE);
        // Enough names to find your way by, not so many the map is buried:
        // about one for every hundred and twenty cells of pane.
        let mut room = usize::from(inner.width) * usize::from(inner.height) / 120;
        for airport in places(view.zoom).into_iter().filter(|a| !on_route(a)) {
            if room == 0 {
                break;
            }
            let Some((col, row)) = cell_of(&view, airport.vec, inner) else {
                continue;
            };
            // Close in, there is room to say which city it is. A name sits
            // right beside its dot or not at all (hence only two tries).
            let named = format!("{} {}", airport.iata, airport.city.to_uppercase());
            let placed = (view.zoom >= 5.0 && labels.place(buf, col, row, &named, style, 2))
                || labels.place(buf, col, row, airport.iata, style, 2);
            room -= usize::from(placed);
        }
    }
}

/// Everything that decides what the planet looks like. Two frames with the
/// same `Scene` show the same geography.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scene {
    view: View,
    /// The pane, in cells.
    size: (u16, u16),
    marker: Marker,
    theme: &'static str,
    tinted: bool,
    borders: bool,
}

/// Draw the planet: grid, land, limb, borders, coast and airports.
fn geography(scene: &Scene, th: Theme) -> DotCanvas {
    let view = scene.view;
    let globe = view.proj == Projection::Globe;
    let mut cv = DotCanvas::new(scene.size.0, scene.size.1, scene.marker);
    let (w, h) = (cv.width(), cv.height());

    // Line work needs a marker that can draw a line. Half-blocks cannot:
    // there every coastline becomes a wall of bricks, so that marker shows
    // shaded land and nothing else beneath the routes.
    let lines = scene.marker.draws_lines();
    let solid = scene.marker == Marker::Braille;
    if solid {
        graticule(&mut cv, &view, th.grid);
    }
    land(&mut cv, &view, th, scene.tinted);
    if globe {
        let steps = ((TAU * view.globe_radius(w, h)) as usize).clamp(64, 6000);
        for i in 0..steps {
            let a = TAU * i as f64 / steps as f64;
            let (x, y) = view.canvas_point([a.cos(), a.sin(), 0.0], w, h);
            cv.plot(x, y, th.dim, P_LIMB);
        }
    }
    if lines {
        // On solid land a border is a hairline cut out of it. That needs no
        // colour of its own, so it never muddies the continent it crosses.
        if scene.borders && solid {
            polylines(&mut cv, &view, geodata::borders(), &Paint::Carve, P_LAND);
        }
        // Solid land already shows the shore to the dot, so a drawn
        // coastline would only thin it to a wire. Close in, the land mask
        // runs out of detail and its edge turns to stair-steps; from there
        // the surveyed coastline is drawn over it. Line art has no solid
        // land, so it needs the coastline at every zoom.
        let detailed = view.zoom >= 2.2;
        if !solid || view.zoom >= COAST_FROM_ZOOM {
            let paint = if scene.tinted {
                let mut coast = [th.coast; Continent::COUNT];
                for continent in CONTINENTS {
                    coast[continent.index()] = th.continent(continent).1;
                }
                Paint::ByContinent {
                    of: geodata::coast_continents(detailed),
                    coast,
                }
            } else {
                Paint::Plain(th.coast)
            };
            polylines(&mut cv, &view, geodata::coast(detailed), &paint, P_COAST);
        }

        // Airports worth clicking: majors always, the rest as you zoom in.
        let max_size = if view.zoom >= 6.0 {
            2
        } else if view.zoom >= 2.5 {
            1
        } else {
            0
        };
        for airport in airports::db().all().iter().filter(|a| a.size <= max_size) {
            let (x, y, side) = view.project(airport.vec, w, h);
            if side == Side::Front {
                cv.plot(x, y, PLACE, P_CITY);
            }
        }
    }
    cv
}

const CONTINENTS: [Continent; Continent::COUNT] = [
    Continent::Africa,
    Continent::Antarctica,
    Continent::Asia,
    Continent::Europe,
    Continent::NorthAmerica,
    Continent::Oceania,
    Continent::SouthAmerica,
];

/// The world's busiest crossroads: the names that orient a map.
const HUBS: [&str; 84] = [
    "LHR", "CDG", "AMS", "FRA", "MAD", "FCO", "IST", "DXB", "DOH", "SIN", "HKG", "NRT", "ICN",
    "PEK", "PVG", "BKK", "DEL", "BOM", "SYD", "MEL", "AKL", "JFK", "LAX", "ORD", "ATL", "DFW",
    "SFO", "MIA", "YYZ", "YVR", "MEX", "GRU", "EZE", "SCL", "BOG", "LIM", "JNB", "CPT", "NBO",
    "ADD", "LOS", "CAI", "CMN", "SVO", "BCN", "MUC", "ZRH", "VIE", "CPH", "ARN", "OSL", "HEL",
    "DUB", "LIS", "ATH", "AUH", "JED", "RUH", "TLV", "BLR", "CMB", "SGN", "HAN", "KUL", "CGK",
    "DPS", "MNL", "CAN", "TPE", "KIX", "BNE", "PER", "ADL", "CHC", "NAN", "PPT", "HNL", "ANC",
    "SEA", "DEN", "KEF", "TAS", "KTM", "DSS",
];

/// Airports to name at this zoom, most important first: the hubs from far
/// out, every major airport closer in, the regional ones closer still.
fn places(zoom: f64) -> Vec<&'static Airport> {
    let db = airports::db();
    let mut list: Vec<&'static Airport> = Vec::new();
    if zoom >= 1.5 {
        list.extend(HUBS.iter().filter_map(|code| db.get(code)));
    }
    for (size, from_zoom) in [(0, 3.0), (1, 6.0)] {
        if zoom >= from_zoom {
            list.extend(
                db.all()
                    .iter()
                    .filter(|a| a.size == size && !HUBS.contains(&a.iata)),
            );
        }
    }
    list
}

/// The terminal cell a point on the planet is drawn in, if it is in view.
fn cell_of(view: &View, at: V3, inner: Rect) -> Option<(u16, u16)> {
    let (w, h) = (f64::from(inner.width) * 2.0, f64::from(inner.height) * 4.0);
    let (x, y, side) = view.project(at, w, h);
    let visible = side == Side::Front && (0.0..w).contains(&x) && (0.0..h).contains(&y);
    visible.then(|| (inner.x + (x / 2.0) as u16, inner.y + (y / 4.0) as u16))
}

/// Keeps text labels from being written over one another.
struct Labels {
    inner: Rect,
    /// Row, first column, last column of everything placed so far.
    taken: Vec<(u16, u16, u16)>,
}

impl Labels {
    fn reserve(&mut self, row: u16, from: u16, to: u16) {
        self.taken.push((row, from, to));
    }

    /// Write `text` beside the marker at (`col`, `row`): to its right, else
    /// its left, else just above or below, giving up after `tries` of those
    /// positions. Returns false if none was free.
    fn place(
        &mut self,
        buf: &mut Buffer,
        col: u16,
        row: u16,
        text: &str,
        style: Style,
        tries: usize,
    ) -> bool {
        let inner = self.inner;
        let width = text.chars().count() as u16;
        let candidates = [
            (col + 2, row),
            (col.saturating_sub(width + 1), row),
            (col + 1, row.saturating_sub(1)),
            (col + 1, row + 1),
        ];
        for (x0, y0) in candidates.into_iter().take(tries) {
            let x1 = x0 + width - 1;
            let inside =
                x0 >= inner.x && x1 < inner.right() && y0 >= inner.y && y0 < inner.bottom();
            let clear = self
                .taken
                .iter()
                .all(|(r, a, b)| *r != y0 || x1 + 1 < *a || x0 > *b + 1);
            if inside && clear {
                put(buf, inner, x0, y0, text, style);
                self.taken.push((y0, x0, x1));
                return true;
            }
        }
        false
    }
}

/// Write `text` at a cell, clipped to `inner`.
fn put(buf: &mut Buffer, inner: Rect, x: u16, y: u16, text: &str, style: Style) {
    if y < inner.y || y >= inner.bottom() || x < inner.x || x >= inner.right() {
        return;
    }
    buf.set_stringn(x, y, text, usize::from(inner.right() - x), style);
}

fn put_right(buf: &mut Buffer, inner: Rect, y: u16, text: &str, style: Style) {
    let width = text.chars().count() as u16;
    if width + 2 <= inner.width {
        put(buf, inner, inner.right() - width - 1, y, text, style);
    }
}

/// Stipple the continents. Rather than rasterising land polygons onto the
/// screen, ask the land mask about two staggered dots in every cell: a
/// sparse, even texture that reads as "land" without drowning the coastline.
/// Each dot takes the colour of the continent it falls on.
fn land(cv: &mut DotCanvas, view: &View, th: Theme, tinted: bool) {
    let (w, h) = (cv.width(), cv.height());
    let camera = view.camera();
    let radius = view.globe_radius(w, h);
    let (cx, cy) = view.globe_centre(w, h);
    let land_at = |x: f64, y: f64| -> Option<LatLon> {
        let p = match view.proj {
            // Same maths as `View::unproject`, with the rotation and the
            // planet's position hoisted out.
            Projection::Globe => {
                let (px, py) = ((x - cx) / radius, (cy - y) / radius);
                let rho2 = px * px + py * py;
                if rho2 > 1.0 {
                    return None;
                }
                // A rotation keeps a unit vector a unit vector.
                LatLon::from_unit_vec_fast(camera.invert([px, py, (1.0 - rho2).sqrt()]))
            }
            Projection::Map => view.unproject(x, y, w, h)?,
        };
        geodata::is_land(p).then_some(p)
    };
    let tint = |p: LatLon| {
        if tinted {
            th.continent(continents::of(p)).0
        } else {
            th.land
        }
    };
    if cv.marker() == Marker::Braille {
        // Every dot is asked. Land comes out solid, with a shoreline as
        // fine as the dots themselves - eight times the detail of a cell.
        for yi in 0..cv.rows() * 4 {
            for xi in 0..cv.cols() * 2 {
                let (x, y) = (xi as f64 + 0.5, yi as f64 + 0.5);
                if let Some(p) = land_at(x, y) {
                    cv.plot(x, y, tint(p), P_LAND);
                }
            }
        }
    } else {
        // Coarser markers get one answer per cell, as a background texture.
        for row in 0..cv.rows() {
            for col in 0..cv.cols() {
                if let Some(p) = land_at(col as f64 * 2.0 + 1.0, row as f64 * 4.0 + 2.0) {
                    cv.shade(col, row, tint(p));
                }
            }
        }
    }
}

/// Dotted parallels and meridians every 30 degrees.
fn graticule(cv: &mut DotCanvas, view: &View, color: Color) {
    let (w, h) = (cv.width(), cv.height());
    let step = (3.0 / view.zoom).clamp(0.5, 3.0);
    let mut dot = |lat: f64, lon: f64| {
        let (x, y, side) = view.project(LatLon::new(lat, lon).to_vec(), w, h);
        if side == Side::Front {
            cv.plot(x, y, color, P_GRID);
        }
    };
    for lat in (-60..=60).step_by(30) {
        let mut lon = -180.0;
        while lon < 180.0 {
            dot(f64::from(lat), lon);
            lon += step;
        }
    }
    for lon in (-180..180).step_by(30) {
        let mut lat = -84.0;
        while lat <= 84.0 {
            dot(lat, f64::from(lon));
            lat += step;
        }
    }
}

/// How a set of polylines is put on the canvas.
enum Paint<'a> {
    Plain(Color),
    /// Each point in its continent's colour: `of` gives the continent of
    /// every point, `coast` the colour for each continent.
    ByContinent {
        of: &'a [Vec<u8>],
        coast: [Color; Continent::COUNT],
    },
    /// Not drawn at all, but rubbed out of whatever is already there.
    Carve,
}

impl Paint<'_> {
    /// Put one segment down, ending at point `point` of line `line`.
    fn stroke(
        &self,
        cv: &mut DotCanvas,
        a: (f64, f64),
        b: (f64, f64),
        line: usize,
        point: usize,
        prio: u8,
    ) {
        let color = match self {
            Paint::Carve => return cv.carve(a.0, a.1, b.0, b.1, prio),
            Paint::Plain(color) => *color,
            Paint::ByContinent { of, coast } => {
                let continent = of
                    .get(line)
                    .and_then(|l| l.get(point))
                    .copied()
                    .unwrap_or(0);
                coast[usize::from(continent).min(Continent::COUNT - 1)]
            }
        };
        cv.line(a.0, a.1, b.0, b.1, color, prio);
    }
}

/// Coastlines and borders. On the globe, a segment that crosses the horizon
/// is cut at the limb rather than dropped, so outlines meet the edge of the
/// disc cleanly instead of fraying.
fn polylines(cv: &mut DotCanvas, view: &View, lines: &[Polyline], paint: &Paint, prio: u8) {
    let (w, h) = (cv.width(), cv.height());
    match view.proj {
        Projection::Globe => {
            let at = |c: V3| view.canvas_point(c, w, h);
            let camera = view.camera();
            for (li, line) in lines.iter().enumerate() {
                let mut prev: Option<V3> = None;
                for (pi, &v) in line.iter().enumerate() {
                    let cur = camera.apply(v);
                    if let Some(p) = prev {
                        let (p_front, cur_front) = (p[2] >= 0.0, cur[2] >= 0.0);
                        if p_front && cur_front {
                            paint.stroke(cv, at(p), at(cur), li, pi, prio);
                        } else if p_front != cur_front {
                            let t = p[2] / (p[2] - cur[2]);
                            let (ex, ey) = (p[0] + (cur[0] - p[0]) * t, p[1] + (cur[1] - p[1]) * t);
                            let len = (ex * ex + ey * ey).sqrt().max(1e-9);
                            let (a, b) = (
                                at(if p_front { p } else { cur }),
                                at([ex / len, ey / len, 0.0]),
                            );
                            paint.stroke(cv, a, b, li, pi, prio);
                        }
                    }
                    prev = Some(cur);
                }
            }
        }
        Projection::Map => {
            // A jump of half the world's width means the line crossed the
            // antimeridian; drawing it would streak across the whole map.
            let wrap = w * view.zoom * 0.5;
            for (li, line) in lines.iter().enumerate() {
                let mut prev: Option<(f64, f64)> = None;
                for (pi, &v) in line.iter().enumerate() {
                    let (x, y, _) = view.project(v, w, h);
                    if let Some((px, py)) = prev {
                        if (x - px).abs() < wrap {
                            paint.stroke(cv, (px, py), (x, y), li, pi, prio);
                        }
                    }
                    prev = Some((x, y));
                }
            }
        }
    }
}

/// One short straight piece of a flight arc, in canvas dots.
struct Piece {
    /// How far along the leg it is, 0.0 to 1.0.
    at: f64,
    from: (f64, f64),
    to: (f64, f64),
    /// Both ends are on the near side of the planet.
    in_front: bool,
}

/// One great-circle leg, cut into short pieces on screen. On the globe the
/// leg is lofted above the surface, so it can be seen rising over the
/// horizon; the part hidden behind the planet is left out.
fn arc_pieces(view: &View, a: V3, b: V3, w: f64, h: f64) -> Vec<Piece> {
    let lofted = view.proj == Projection::Globe;
    let samples =
        ((geo::angle(a, b).to_degrees() * view.zoom.max(1.0)).ceil() as usize).clamp(16, 480);
    let wrap = w * view.zoom * 0.5;
    let mut pieces = Vec::with_capacity(samples);
    let mut prev: Option<(f64, f64, Side)> = None;
    for i in 0..=samples {
        let cur = view.project(
            geo::arc_point(a, b, i as f64 / samples as f64, lofted),
            w,
            h,
        );
        if let Some(p) = prev {
            let visible = p.2 != Side::Hidden && cur.2 != Side::Hidden;
            let wrapped = !lofted && (cur.0 - p.0).abs() >= wrap;
            if visible && !wrapped {
                pieces.push(Piece {
                    at: (i as f64 - 0.5) / samples as f64,
                    from: (p.0, p.1),
                    to: (cur.0, cur.1),
                    in_front: p.2 == Side::Front && cur.2 == Side::Front,
                });
            }
        }
        prev = Some(cur);
    }
    pieces
}

/// A leg in one flat colour, dimmer where it runs beyond the horizon.
fn plain_arc(cv: &mut DotCanvas, view: &View, a: V3, b: V3, front: Color, back: Color, prio: u8) {
    for piece in arc_pieces(view, a, b, cv.width(), cv.height()) {
        let color = if piece.in_front { front } else { back };
        cv.line(
            piece.from.0,
            piece.from.1,
            piece.to.0,
            piece.to.1,
            color,
            prio,
        );
    }
}

#[derive(Clone, Copy)]
struct Stroke {
    /// Turned down: a route that is not the one in hand.
    dim: bool,
    /// Pencilled in: a route that has not been searched yet.
    dashed: bool,
}

/// A leg of the journey, shading from its starting colour to its ending
/// colour so the whole trip reads origin-to-destination at a glance.
fn route_arc(cv: &mut DotCanvas, view: &View, leg: &Leg, th: Theme, stroke: Stroke, prio: u8) {
    let mut travelled = 0.0;
    for piece in arc_pieces(view, leg.a, leg.b, cv.width(), cv.height()) {
        let tone = leg.t0 + (leg.t1 - leg.t0) * piece.at;
        let color = if piece.in_front && !stroke.dim {
            th.route(tone)
        } else {
            th.route_dim(tone)
        };
        let (from, to) = (piece.from, piece.to);
        travelled += (to.0 - from.0).hypot(to.1 - from.1);
        // Dashes are measured in dots on screen, so they look the same
        // however long the leg is and however far in the view is zoomed.
        if !stroke.dashed || (travelled / 5.0) as u64 % 2 == 0 {
            cv.line(from.0, from.1, to.0, to.1, color, prio);
        }
    }
}

/// The point a fraction `t` (0..1, by distance) along a multi-leg route.
fn along(legs: &[Leg], t: f64, lofted: bool) -> Option<V3> {
    if !(0.0..=1.0).contains(&t) {
        return None;
    }
    let total: f64 = legs.iter().map(|leg| geo::angle(leg.a, leg.b)).sum();
    if total <= 0.0 {
        return None;
    }
    let mut remaining = t * total;
    for (i, leg) in legs.iter().enumerate() {
        let len = geo::angle(leg.a, leg.b);
        if remaining <= len || i == legs.len() - 1 {
            let local = if len > 0.0 {
                (remaining / len).clamp(0.0, 1.0)
            } else {
                0.0
            };
            return Some(geo::arc_point(leg.a, leg.b, local, lofted));
        }
        remaining -= len;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_position_walks_leg_by_leg() {
        let a = LatLon::new(0.0, 0.0).to_vec();
        let b = LatLon::new(0.0, 30.0).to_vec();
        let c = LatLon::new(0.0, 90.0).to_vec();
        let legs = [
            Leg {
                a,
                b,
                t0: 0.0,
                t1: 0.5,
            },
            Leg {
                a: b,
                b: c,
                t0: 0.5,
                t1: 1.0,
            },
        ];
        let lon = |t: f64| LatLon::from_vec(along(&legs, t, false).unwrap()).lon;
        assert!((lon(0.0) - 0.0).abs() < 1e-6);
        assert!(
            (lon(1.0 / 3.0) - 30.0).abs() < 1e-6,
            "one third of the distance is the first stop"
        );
        assert!((lon(0.5) - 45.0).abs() < 1e-6);
        assert!((lon(1.0) - 90.0).abs() < 1e-6);
        assert!(along(&legs, -0.1, false).is_none());
        assert!(along(&[], 0.5, false).is_none());
    }

    #[test]
    fn hubs_are_real_airports_listed_once() {
        let db = airports::db();
        for (i, code) in HUBS.iter().enumerate() {
            assert!(
                db.get(code).is_some(),
                "{code} is not in the airport database"
            );
            assert!(!HUBS[i + 1..].contains(code), "{code} is listed twice");
        }
        // Further out shows fewer names, never more.
        assert!(places(1.0).is_empty());
        assert_eq!(places(2.0).len(), HUBS.len());
        assert!(places(4.0).len() > places(2.0).len());
        assert!(places(8.0).len() > places(4.0).len());
    }
}
