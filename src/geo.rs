//! Spherical geometry for the globe: the only "GIS" icarus-search needs.
//!
//! Conventions: a point on the unit sphere is `[x, y, z]` with +y through the
//! north pole and +z through (lat 0, lon 0). The camera sits at +z infinity
//! (orthographic), so once the world is rotated into camera space, `z > 0`
//! means "on the near side of the planet".

use std::f64::consts::PI;

pub const EARTH_RADIUS_KM: f64 = 6371.0;

pub type V3 = [f64; 3];

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct LatLon {
    pub lat: f64,
    pub lon: f64,
}

impl LatLon {
    pub const fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }

    pub fn to_vec(self) -> V3 {
        let (la, lo) = (self.lat.to_radians(), self.lon.to_radians());
        [la.cos() * lo.sin(), la.sin(), la.cos() * lo.cos()]
    }

    /// Direction of `v` as a coordinate; the length of `v` is ignored.
    pub fn from_vec(v: V3) -> Self {
        let len = dot(v, v).sqrt().max(1e-12);
        Self {
            lat: (v[1] / len).clamp(-1.0, 1.0).asin().to_degrees(),
            lon: v[0].atan2(v[2]).to_degrees(),
        }
    }
}

impl LatLon {
    /// The same as [`LatLon::from_vec`] for a vector already of unit length,
    /// several times faster and good to a few thousandths of a degree.
    ///
    /// The map asks "is this dot on land?" of every dot it draws - a hundred
    /// thousand a frame on a full screen - and the land raster it asks holds
    /// a quarter of a degree per cell. Exact trigonometry there is precision
    /// nobody can see, paid for in a globe that drags.
    pub fn from_unit_vec_fast(v: V3) -> Self {
        Self {
            lat: fast_asin(v[1]).to_degrees(),
            lon: fast_atan2(v[0], v[2]).to_degrees(),
        }
    }
}

/// Arcsine by polynomial (Abramowitz & Stegun 4.4.45): within 0.00007 rad.
fn fast_asin(x: f64) -> f64 {
    let a = x.abs().min(1.0);
    let poly = 1.570_728_8 + a * (-0.212_114_4 + a * (0.074_261_0 + a * -0.018_729_3));
    (std::f64::consts::FRAC_PI_2 - (1.0 - a).sqrt() * poly).copysign(x)
}

/// Two-argument arctangent by polynomial: within 0.00002 rad.
fn fast_atan2(y: f64, x: f64) -> f64 {
    let (ax, ay) = (x.abs(), y.abs());
    let (small, large) = if ax < ay { (ax, ay) } else { (ay, ax) };
    if large == 0.0 {
        return 0.0;
    }
    // atan on [0, 1], then unfolded into the right octant.
    let a = small / large;
    let s = a * a;
    let mut r = a
        * (0.999_977_26
            + s * (-0.332_623_47
                + s * (0.193_543_46
                    + s * (-0.116_432_87 + s * (0.052_653_32 + s * -0.011_721_20)))));
    if ay > ax {
        r = std::f64::consts::FRAC_PI_2 - r;
    }
    if x < 0.0 {
        r = PI - r;
    }
    if y < 0.0 { -r } else { r }
}

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn scale(v: V3, k: f64) -> V3 {
    [v[0] * k, v[1] * k, v[2] * k]
}

/// Central angle between two unit vectors, in radians.
pub fn angle(a: V3, b: V3) -> f64 {
    dot(a, b).clamp(-1.0, 1.0).acos()
}

pub fn distance_km(a: LatLon, b: LatLon) -> f64 {
    angle(a.to_vec(), b.to_vec()) * EARTH_RADIUS_KM
}

/// Point a fraction `t` of the way along the great circle from `a` to `b`.
pub fn slerp(a: V3, b: V3, t: f64) -> V3 {
    let w = angle(a, b);
    let s = w.sin();
    if s.abs() < 1e-9 {
        return a; // coincident (or exactly antipodal, where the path is undefined)
    }
    let (ka, kb) = (((1.0 - t) * w).sin() / s, (t * w).sin() / s);
    [
        ka * a[0] + kb * b[0],
        ka * a[1] + kb * b[1],
        ka * a[2] + kb * b[2],
    ]
}

/// Wrap a longitude (or longitude difference) into [-180, 180).
pub fn wrap_lon(lon: f64) -> f64 {
    (lon + 180.0).rem_euclid(360.0) - 180.0
}

/// Compass heading when leaving `a` for `b`, degrees clockwise from north.
pub fn bearing(a: LatLon, b: LatLon) -> f64 {
    let (la, lb) = (a.lat.to_radians(), b.lat.to_radians());
    let dl = (b.lon - a.lon).to_radians();
    let y = dl.sin() * lb.cos();
    let x = la.cos() * lb.sin() - la.sin() * lb.cos() * dl.cos();
    y.atan2(x).to_degrees().rem_euclid(360.0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Projection {
    /// Orthographic: the planet seen from far away.
    Globe,
    /// Equirectangular, stretched to fill the pane.
    Map,
}

/// Where a projected point sits relative to the planet's disc.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Facing the camera.
    Front,
    /// Behind the planet's centre but outside its silhouette (a lofted arc
    /// peeking around the limb).
    Beyond,
    /// Occluded by the planet.
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub lat0: f64,
    pub lon0: f64,
    pub zoom: f64,
    pub proj: Projection,
    /// Globe only: how far the planet's centre sits below the middle of the
    /// canvas, in planet radii. Sinking a large globe leaves its upper limb
    /// across the pane as a horizon for flight paths to arc over.
    pub drop: f64,
    /// Globe only: the same, sideways (positive moves the planet right).
    /// Wheel-zooming toward the pointer slides the planet as it grows.
    pub shift: f64,
}

/// A camera position that frames a route: see [`View::fit`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Framing {
    pub lat0: f64,
    pub lon0: f64,
    pub zoom: f64,
    pub drop: f64,
}

/// A view's rotation with its trigonometry already done. Drawing a frame
/// rotates tens of thousands of coastline points; computing four sines and
/// cosines once instead of per point roughly halves the cost of a frame.
#[derive(Clone, Copy, Debug)]
pub struct Camera {
    sin_lon: f64,
    cos_lon: f64,
    sin_lat: f64,
    cos_lat: f64,
}

impl Camera {
    /// Rotate a world-space vector into camera space.
    pub fn apply(&self, v: V3) -> V3 {
        let x = v[0] * self.cos_lon - v[2] * self.sin_lon;
        let z = v[0] * self.sin_lon + v[2] * self.cos_lon;
        [
            x,
            v[1] * self.cos_lat - z * self.sin_lat,
            v[1] * self.sin_lat + z * self.cos_lat,
        ]
    }

    /// Inverse of [`Camera::apply`].
    pub fn invert(&self, c: V3) -> V3 {
        let y = c[1] * self.cos_lat + c[2] * self.sin_lat;
        let z = c[2] * self.cos_lat - c[1] * self.sin_lat;
        [
            c[0] * self.cos_lon + z * self.sin_lon,
            y,
            z * self.cos_lon - c[0] * self.sin_lon,
        ]
    }
}

impl View {
    pub const MAX_ZOOM: f64 = 14.0;

    pub fn camera(&self) -> Camera {
        let (sin_lon, cos_lon) = self.lon0.to_radians().sin_cos();
        let (sin_lat, cos_lat) = self.lat0.to_radians().sin_cos();
        Camera {
            sin_lon,
            cos_lon,
            sin_lat,
            cos_lat,
        }
    }

    pub fn min_zoom(&self) -> f64 {
        match self.proj {
            Projection::Globe => 0.6,
            Projection::Map => 1.0,
        }
    }

    /// Globe radius in canvas units for a `w` x `h` canvas.
    pub fn globe_radius(&self, w: f64, h: f64) -> f64 {
        0.46 * w.min(h) * self.zoom
    }

    /// Camera-space point to canvas coordinates (globe projection).
    pub fn canvas_point(&self, c: V3, w: f64, h: f64) -> (f64, f64) {
        let r = self.globe_radius(w, h);
        (
            w / 2.0 + (self.shift + c[0]) * r,
            h / 2.0 + (self.drop - c[1]) * r,
        )
    }

    /// Canvas units per degree (x, y) in map mode.
    pub fn map_scale(&self, w: f64, h: f64) -> (f64, f64) {
        (w / 360.0 * self.zoom, h / 180.0 * self.zoom)
    }

    /// Project a world-space vector (which may be lofted above the surface,
    /// i.e. longer than 1) to canvas coordinates.
    pub fn project(&self, v: V3, w: f64, h: f64) -> (f64, f64, Side) {
        match self.proj {
            Projection::Globe => {
                let c = self.camera().apply(v);
                let side = if c[2] >= 0.0 {
                    Side::Front
                } else if c[0] * c[0] + c[1] * c[1] >= 1.0 {
                    Side::Beyond
                } else {
                    Side::Hidden
                };
                let (x, y) = self.canvas_point(c, w, h);
                (x, y, side)
            }
            Projection::Map => {
                let p = LatLon::from_vec(v);
                let (sx, sy) = self.map_scale(w, h);
                (
                    w / 2.0 + wrap_lon(p.lon - self.lon0) * sx,
                    h / 2.0 - (p.lat - self.lat0) * sy,
                    Side::Front,
                )
            }
        }
    }

    /// Canvas coordinates back to a point on the surface (mouse picking).
    pub fn unproject(&self, x: f64, y: f64, w: f64, h: f64) -> Option<LatLon> {
        match self.proj {
            Projection::Globe => {
                let r = self.globe_radius(w, h);
                let (cx, cy) = (
                    (x - w / 2.0) / r - self.shift,
                    (h / 2.0 - y) / r + self.drop,
                );
                let rho2 = cx * cx + cy * cy;
                if rho2 > 1.0 {
                    return None; // clicked on space, not the planet
                }
                let world = self.camera().invert([cx, cy, (1.0 - rho2).sqrt()]);
                Some(LatLon::from_vec(world))
            }
            Projection::Map => {
                let (sx, sy) = self.map_scale(w, h);
                let lat = self.lat0 + (h / 2.0 - y) / sy;
                let dlon = (x - w / 2.0) / sx;
                (lat.abs() <= 90.0 && dlon.abs() <= 180.0)
                    .then(|| LatLon::new(lat, wrap_lon(self.lon0 + dlon)))
            }
        }
    }

    /// Drag the surface by a canvas-space delta.
    pub fn pan(&mut self, dx: f64, dy: f64, w: f64, h: f64) {
        match self.proj {
            Projection::Globe => {
                let k = 1.0 / self.globe_radius(w, h);
                let stretch = self.lat0.to_radians().cos().max(0.3);
                self.lon0 = wrap_lon(self.lon0 - (dx * k).to_degrees() / stretch);
                self.lat0 = (self.lat0 + (dy * k).to_degrees()).clamp(-89.0, 89.0);
            }
            Projection::Map => {
                let (sx, sy) = self.map_scale(w, h);
                self.lon0 = wrap_lon(self.lon0 - dx / sx);
                self.lat0 += dy / sy;
            }
        }
        self.clamp();
    }

    pub fn zoom_by(&mut self, factor: f64) {
        self.zoom = (self.zoom * factor).clamp(self.min_zoom(), Self::MAX_ZOOM);
        self.clamp();
    }

    /// Zoom while keeping whatever is at canvas point `(x, y)` exactly
    /// where it is, like magnifying a picture under the cursor.
    pub fn zoom_about(&mut self, factor: f64, x: f64, y: f64, w: f64, h: f64) {
        match self.proj {
            Projection::Globe => {
                // Position of the anchor in planet radii from the planet's
                // centre. Scaling leaves that unchanged; solve for the
                // offsets that put it back under (x, y) at the new radius.
                let r = self.globe_radius(w, h);
                let (ax, ay) = (
                    (x - w / 2.0) / r - self.shift,
                    self.drop - (y - h / 2.0) / r,
                );
                self.zoom_by(factor);
                let r = self.globe_radius(w, h);
                self.shift = (x - w / 2.0) / r - ax;
                self.drop = ay + (y - h / 2.0) / r;
            }
            Projection::Map => {
                // The flat map is linear, so a pan corrects exactly.
                let anchor = self.unproject(x, y, w, h);
                self.zoom_by(factor);
                if let Some(geo) = anchor {
                    let (nx, ny, _) = self.project(geo.to_vec(), w, h);
                    self.pan(x - nx, y - ny, w, h);
                }
            }
        }
        self.clamp();
    }

    /// Keep the flat map from scrolling past the poles.
    pub fn clamp(&mut self) {
        self.zoom = self.zoom.clamp(self.min_zoom(), Self::MAX_ZOOM);
        if self.proj == Projection::Map {
            let limit = 90.0 * (1.0 - 1.0 / self.zoom);
            self.lat0 = self.lat0.clamp(-limit, limit);
        } else {
            self.lat0 = self.lat0.clamp(-89.0, 89.0);
            // Never let the planet slide entirely out of the pane.
            self.drop = self.drop.clamp(-1.5, 1.5);
            self.shift = self.shift.clamp(-1.5, 1.5);
        }
    }

    /// Degrees the camera sits south of a route it frames. Looking straight
    /// down on a route hides its loft (the arc rises toward the viewer);
    /// looking from the south lets it arc visibly over the planet's curve.
    const TILT: f64 = 34.0;

    /// Camera position that frames `legs` (flight paths) and any lone
    /// `points`, on a canvas of `w` x `h` dots.
    pub fn fit(&self, legs: &[(V3, V3)], points: &[V3], w: f64, h: f64) -> Option<Framing> {
        // Everything that must end up on screen, including the lofted arcs.
        let lofted = self.proj == Projection::Globe;
        let mut targets: Vec<V3> = points.to_vec();
        for (a, b) in legs {
            targets.extend((0..=8).map(|i| arc_point(*a, *b, f64::from(i) / 8.0, lofted)));
        }
        // Centre on the midpoint of the two points farthest apart. (The
        // average of all points will not do: it is dragged toward wherever
        // the samples cluster, and on a long multi-leg route that can leave
        // the far end behind the horizon.)
        let unit: Vec<V3> = targets
            .iter()
            .map(|p| scale(*p, 1.0 / dot(*p, *p).sqrt().max(1e-12)))
            .collect();
        let first = *unit.first()?;
        let (mut a, mut b, mut widest) = (first, first, 0.0);
        for (i, p) in unit.iter().enumerate() {
            for q in &unit[i + 1..] {
                let apart = angle(*p, *q);
                if apart > widest {
                    (a, b, widest) = (*p, *q, apart);
                }
            }
        }
        let mid = [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
        // Exactly antipodal ends have no midpoint; face the first of them.
        let middle = if dot(mid, mid) < 1e-6 {
            a
        } else {
            scale(mid, 1.0 / dot(mid, mid).sqrt())
        };
        let centre = LatLon::from_vec(middle);

        match self.proj {
            Projection::Map => {
                let (mut dlon, mut dlat) = (4.0f64, 4.0f64);
                for p in &targets {
                    let q = LatLon::from_vec(*p);
                    dlon = dlon.max(wrap_lon(q.lon - centre.lon).abs());
                    dlat = dlat.max((q.lat - centre.lat).abs());
                }
                let zoom = (180.0 / (1.4 * dlon))
                    .min(90.0 / (1.4 * dlat))
                    .clamp(1.0, 9.0);
                Some(Framing {
                    lat0: centre.lat,
                    lon0: centre.lon,
                    zoom,
                    drop: 0.0,
                })
            }
            Projection::Globe if targets.len() == 1 => {
                // A lone airport: just turn the globe to face it.
                Some(Framing {
                    lat0: centre.lat.clamp(-60.0, 60.0),
                    lon0: centre.lon,
                    zoom: 1.0,
                    drop: 0.0,
                })
            }
            Projection::Globe => {
                // Very long routes wrap so far round the planet that a full
                // tilt would push their ends over the horizon; ease it off so
                // every point stays within 80 degrees of the camera.
                let spread = unit.iter().map(|p| angle(middle, *p)).fold(0.0, f64::max);
                let tilt = Self::TILT.min(80.0 - spread.to_degrees()).max(0.0);
                let lat0 = (centre.lat - tilt).clamp(-89.0, 89.0);
                let camera = View {
                    lat0,
                    lon0: centre.lon,
                    zoom: 1.0,
                    proj: Projection::Globe,
                    drop: 0.0,
                    shift: 0.0,
                }
                .camera();
                // Where the route lands on screen, in planet radii.
                let (mut reach, mut low, mut high) = (0.05f64, f64::INFINITY, f64::NEG_INFINITY);
                for p in &targets {
                    let c = camera.apply(*p);
                    reach = reach.max(c[0].abs());
                    low = low.min(c[1]);
                    high = high.max(c[1]);
                }
                // Grow the planet until the route spans the pane.
                let radius = (0.43 * w / reach).min(0.60 * h / (high - low).max(0.25));
                let zoom = (radius / (0.46 * w.min(h))).clamp(1.0, 9.0);
                let radius = 0.46 * w.min(h) * zoom;
                // Then sink it so the top of the route sits just under the
                // top edge - but never lift the planet above centre.
                let drop = ((0.2 * h - h / 2.0) / radius + high).max(0.0);
                Some(Framing {
                    lat0,
                    lon0: centre.lon,
                    zoom,
                    drop,
                })
            }
        }
    }
}

/// How far a flight arc is lofted above the surface at its midpoint, as a
/// fraction of the planet radius. Longer hops fly higher, missile-style.
pub fn loft(central_angle: f64) -> f64 {
    0.03 + 0.2 * (central_angle / PI)
}

/// A point `t` along the lofted arc from `a` to `b`.
pub fn arc_point(a: V3, b: V3, t: f64, lofted: bool) -> V3 {
    let p = slerp(a, b, t);
    if lofted {
        scale(p, 1.0 + loft(angle(a, b)) * (PI * t).sin())
    } else {
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LHR: LatLon = LatLon::new(51.4707, -0.4599);
    const JFK: LatLon = LatLon::new(40.6394, -73.7793);
    const NRT: LatLon = LatLon::new(35.7686, 140.3887);

    #[test]
    fn great_circle_distances_match_published_figures() {
        assert!((distance_km(LHR, JFK) - 5540.0).abs() < 25.0);
        assert!((distance_km(LHR, NRT) - 9590.0).abs() < 40.0);
    }

    #[test]
    fn fast_trigonometry_agrees_with_the_real_thing() {
        // Every point of a fine grid over the sphere, poles and seams included.
        let mut worst: f64 = 0.0;
        for lat in (-900..=900).step_by(7) {
            for lon in (-1800..1800).step_by(11) {
                let p = LatLon::new(f64::from(lat) / 10.0, f64::from(lon) / 10.0);
                let q = LatLon::from_unit_vec_fast(p.to_vec());
                // Longitude means nothing at a pole, and less and less near one.
                let east_west = wrap_lon(q.lon - p.lon).abs() * p.lat.to_radians().cos();
                worst = worst.max((q.lat - p.lat).abs()).max(east_west);
            }
        }
        assert!(
            worst < 0.005,
            "off by {worst} degrees; the land raster's cells are 0.25"
        );
        assert_eq!(fast_atan2(0.0, 0.0), 0.0);
        assert!((fast_asin(1.0) - std::f64::consts::FRAC_PI_2).abs() < 1e-4);
        assert!((fast_asin(-1.0) + std::f64::consts::FRAC_PI_2).abs() < 1e-4);
    }

    #[test]
    fn vector_round_trip() {
        for p in [
            LHR,
            JFK,
            NRT,
            LatLon::new(-33.9, 151.2),
            LatLon::new(0.0, 179.5),
        ] {
            let q = LatLon::from_vec(p.to_vec());
            assert!((p.lat - q.lat).abs() < 1e-9 && (p.lon - q.lon).abs() < 1e-9);
        }
    }

    #[test]
    fn camera_rotation_is_invertible_and_centres_the_view() {
        let view = View {
            lat0: 35.0,
            lon0: 139.0,
            zoom: 1.0,
            proj: Projection::Globe,
            drop: 0.0,
            shift: 0.0,
        };
        let centre = view.camera().apply(LatLon::new(35.0, 139.0).to_vec());
        assert!(centre[0].abs() < 1e-9 && centre[1].abs() < 1e-9 && (centre[2] - 1.0).abs() < 1e-9);
        let v = JFK.to_vec();
        let back = view.camera().invert(view.camera().apply(v));
        assert!((0..3).all(|i| (v[i] - back[i]).abs() < 1e-9));
    }

    #[test]
    fn project_then_unproject_recovers_the_point() {
        for proj in [Projection::Globe, Projection::Map] {
            let view = View {
                lat0: 20.0,
                lon0: 100.0,
                zoom: 1.7,
                proj,
                drop: 0.4,
                shift: -0.3,
            };
            let (x, y, side) = view.project(NRT.to_vec(), 200.0, 120.0);
            assert_eq!(side, Side::Front);
            let p = view.unproject(x, y, 200.0, 120.0).expect("on the planet");
            assert!((p.lat - NRT.lat).abs() < 1e-6 && wrap_lon(p.lon - NRT.lon).abs() < 1e-6);
        }
    }

    #[test]
    fn far_side_is_hidden_and_space_is_not_pickable() {
        let view = View {
            lat0: 0.0,
            lon0: 0.0,
            zoom: 1.0,
            proj: Projection::Globe,
            drop: 0.0,
            shift: 0.0,
        };
        let (_, _, side) = view.project(LatLon::new(0.0, 180.0).to_vec(), 100.0, 100.0);
        assert_eq!(side, Side::Hidden);
        assert!(view.unproject(1.0, 1.0, 100.0, 100.0).is_none());
    }

    #[test]
    fn framing_keeps_the_whole_route_on_screen() {
        let (w, h) = (164.0, 52.0);
        for proj in [Projection::Globe, Projection::Map] {
            for (a, b) in [
                (LHR, NRT),
                (LHR, JFK),
                (JFK, LatLon::new(-33.9, 151.2)),
                (LHR, LatLon::new(49.0, 2.55)),
            ] {
                let base = View {
                    lat0: 0.0,
                    lon0: 0.0,
                    zoom: 1.0,
                    proj,
                    drop: 0.0,
                    shift: 0.0,
                };
                // Fly it direct, and also with two short hops first: uneven
                // legs once pulled the camera away from the far end.
                let hop1 = LatLon::new(a.lat - 3.0, a.lon - 4.0);
                let hop2 = LatLon::new(a.lat - 5.0, a.lon - 9.0);
                let direct = vec![(a.to_vec(), b.to_vec())];
                let hopped = vec![
                    (a.to_vec(), hop1.to_vec()),
                    (hop1.to_vec(), hop2.to_vec()),
                    (hop2.to_vec(), b.to_vec()),
                ];
                for legs in [direct, hopped] {
                    let f = base
                        .fit(&legs, &[], w, h)
                        .expect("a route can always be framed");
                    let view = View {
                        lat0: f.lat0,
                        lon0: f.lon0,
                        zoom: f.zoom,
                        proj,
                        drop: f.drop,
                        shift: 0.0,
                    };
                    for (from, to) in &legs {
                        for i in 0..=20 {
                            let p = arc_point(
                                *from,
                                *to,
                                f64::from(i) / 20.0,
                                proj == Projection::Globe,
                            );
                            let (x, y, side) = view.project(p, w, h);
                            assert_ne!(
                                side,
                                Side::Hidden,
                                "{proj:?} {a:?}>{b:?} hides part of the route"
                            );
                            assert!(
                                (0.0..w).contains(&x) && (0.0..h).contains(&y),
                                "{proj:?} {a:?}>{b:?} ({} legs) at t={i}: ({x:.1}, {y:.1})",
                                legs.len()
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn zooming_about_a_point_keeps_that_point_still() {
        for proj in [Projection::Globe, Projection::Map] {
            let mut view = View {
                lat0: 40.0,
                lon0: 90.0,
                zoom: 2.0,
                proj,
                drop: 0.3,
                shift: 0.0,
            };
            let (w, h) = (164.0, 52.0);
            // Anchor well away from the centre, where a rotation-based
            // correction used to drift by several cells.
            let (x, y, _) = view.project(LHR.to_vec(), w, h);
            for factor in [1.2, 1.2, 1.2, 0.7] {
                view.zoom_about(factor, x, y, w, h);
                let (nx, ny, _) = view.project(LHR.to_vec(), w, h);
                assert!(
                    (nx - x).abs() < 1e-6 && (ny - y).abs() < 1e-6,
                    "{proj:?}: moved to ({nx:.2}, {ny:.2})"
                );
            }
        }
    }

    #[test]
    fn slerp_midpoint_is_equidistant() {
        let (a, b) = (LHR.to_vec(), NRT.to_vec());
        let m = slerp(a, b, 0.5);
        assert!((angle(a, m) - angle(m, b)).abs() < 1e-9);
        assert!((dot(m, m) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn bearing_lhr_to_jfk_heads_west_north_west() {
        let b = bearing(LHR, JFK);
        assert!((280.0..300.0).contains(&b), "bearing was {b}");
    }
}
