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
    /// canvas, as a fraction of the canvas's shorter side. Zero is centred.
    ///
    /// Where the planet sits is the user's to choose and nothing else's:
    /// only [`View::slide`] changes it. Zooming and framing a route turn
    /// the planet and resize it, and leave it where it is. The unit is a
    /// share of the pane, not of the planet, precisely so that the zoom can
    /// change without the planet moving.
    pub drop: f64,
    /// Globe only: the same, sideways (positive moves the planet right).
    pub shift: f64,
}

/// A camera position that frames a route: see [`View::fit`]. It says which
/// way the planet faces and how large it is, never where it sits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Framing {
    pub lat0: f64,
    pub lon0: f64,
    pub zoom: f64,
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

    /// How much of the planet may leave the pane, in radii past the edge.
    /// The rest stays in sight, so a planet moved aside can always be
    /// found and dragged back, whatever the zoom or the window size.
    const OUT_OF_SIGHT: f64 = 0.6;

    /// Where the planet's centre is on a `w` x `h` canvas (globe only).
    pub fn globe_centre(&self, w: f64, h: f64) -> (f64, f64) {
        let side = w.min(h);
        let reach = Self::OUT_OF_SIGHT * self.globe_radius(w, h);
        (
            (w / 2.0 + self.shift * side).clamp(-reach, w + reach),
            (h / 2.0 + self.drop * side).clamp(-reach, h + reach),
        )
    }

    /// Camera-space point to canvas coordinates (globe projection).
    pub fn canvas_point(&self, c: V3, w: f64, h: f64) -> (f64, f64) {
        let r = self.globe_radius(w, h);
        let (cx, cy) = self.globe_centre(w, h);
        (cx + c[0] * r, cy - c[1] * r)
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
                let (cx, cy) = self.globe_centre(w, h);
                let (px, py) = ((x - cx) / r, (cy - y) / r);
                let rho2 = px * px + py * py;
                if rho2 > 1.0 {
                    return None; // clicked on space, not the planet
                }
                let world = self.camera().invert([px, py, (1.0 - rho2).sqrt()]);
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

    /// Move the planet itself across the pane by a canvas-space delta,
    /// without turning it. On the flat map, which fills its pane, this is
    /// the same as dragging the surface.
    pub fn slide(&mut self, dx: f64, dy: f64, w: f64, h: f64) {
        if self.proj != Projection::Globe {
            return self.pan(dx, dy, w, h);
        }
        let side = w.min(h).max(1.0);
        let place = |view: &mut Self, cx: f64, cy: f64| {
            view.shift = (cx - w / 2.0) / side;
            view.drop = (cy - h / 2.0) / side;
        };
        // Start from where the planet is drawn, and store where it ends up
        // being drawn, so that a planet stopped at the edge of the pane
        // answers the very next movement back the other way.
        let (cx, cy) = self.globe_centre(w, h);
        place(self, cx + dx, cy + dy);
        let (cx, cy) = self.globe_centre(w, h);
        place(self, cx, cy);
    }

    /// How close to the planet's edge [`View::aim`] will put a place, in
    /// radii. At the very edge the surface is seen side-on, and a nudge of
    /// the pointer would spin the planet a long way.
    const AIM_LIMIT: f64 = 0.92;

    /// Globe only: turn the planet so that the place `p` (a unit vector)
    /// is drawn at canvas point `(x, y)`. The planet stays where it is and
    /// keeps its size; only the way it faces changes.
    ///
    /// The camera turns two ways, round the poles and over them, and never
    /// rolls. That is enough to put any place at any point of the disc
    /// except where a place near a pole would have to be carried far out
    /// sideways; then it goes as far as it can.
    pub fn aim(&mut self, p: V3, x: f64, y: f64, w: f64, h: f64) {
        let r = self.globe_radius(w, h);
        let (cx, cy) = self.globe_centre(w, h);
        // Where `p` is wanted, in planet radii from the planet's centre.
        let (mut tx, mut ty) = ((x - cx) / r, (cy - y) / r);
        let rho = tx.hypot(ty);
        if rho > Self::AIM_LIMIT {
            (tx, ty) = (tx * Self::AIM_LIMIT / rho, ty * Self::AIM_LIMIT / rho);
        }
        // Turning round the poles carries `p` along its circle of latitude,
        // whose radius seen from above is `ring`. Choose the turn that
        // leaves it `tx` to the side, on the near half of the circle.
        let ring = p[0].hypot(p[2]);
        let tx = tx.clamp(-ring, ring);
        let depth = (ring * ring - tx * tx).max(0.0).sqrt();
        let lon = depth.atan2(tx) - p[2].atan2(p[0]);
        // Tipping the planet then swings `p` up or down a circle of radius
        // `swing` until it is `ty` high, again on the near side.
        let swing = p[1].hypot(depth);
        let ty = ty.clamp(-swing, swing);
        let near = (swing * swing - ty * ty).max(0.0).sqrt();
        let lat = near.atan2(ty) - depth.atan2(p[1]);
        self.lon0 = wrap_lon(lon.to_degrees());
        self.lat0 = lat.to_degrees().clamp(-89.0, 89.0);
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
                // The planet grows where it stands, then turns to bring the
                // place that was under (x, y) back under it. Over empty
                // space there is nothing to hold on to: it only grows.
                let anchor = self.unproject(x, y, w, h);
                self.zoom_by(factor);
                if let Some(place) = anchor {
                    self.aim(place.to_vec(), x, y, w, h);
                }
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
            // Only a bound on what is stored. What keeps the planet in
            // sight is `globe_centre`, which knows the size of the pane.
            self.drop = self.drop.clamp(-8.0, 8.0);
            self.shift = self.shift.clamp(-8.0, 8.0);
        }
    }

    /// The most a framed route may sit off the middle of the planet, in
    /// degrees. A planet in the middle of its pane is simply turned to
    /// face the route. One the user has moved aside shows its route
    /// toward the middle of the pane instead, seen at a slant, and past
    /// this angle the far end starts to slip round the edge.
    const TILT: f64 = 34.0;

    /// Clear space kept between a framed route and the sides of the pane,
    /// and its top and bottom (where the readouts are), as shares of it.
    const MARGIN: (f64, f64) = (0.07, 0.12);

    /// Whether `p` is drawn on the near side and inside the pane's margins.
    fn shows(&self, p: V3, w: f64, h: f64) -> bool {
        let (x, y, side) = self.project(p, w, h);
        let (mx, my) = (Self::MARGIN.0 * w, Self::MARGIN.1 * h);
        side != Side::Hidden && (mx..=w - mx).contains(&x) && (my..=h - my).contains(&y)
    }

    /// Globe only: the canvas point a framed route's middle is put at. It
    /// is the middle of the pane, unless the planet has been moved so far
    /// that this would be more than `slant` (a sine) from its own middle.
    fn focus(&self, slant: f64, w: f64, h: f64) -> (f64, f64) {
        let r = self.globe_radius(w, h);
        let (cx, cy) = self.globe_centre(w, h);
        let (dx, dy) = (w / 2.0 - cx, h / 2.0 - cy);
        let far = dx.hypot(dy);
        let pull = if far > slant * r {
            slant * r / far
        } else {
            1.0
        };
        (cx + dx * pull, cy + dy * pull)
    }

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
                })
            }
            Projection::Globe if targets.len() == 1 => {
                // A lone airport: just turn the globe to face it.
                Some(Framing {
                    lat0: centre.lat.clamp(-60.0, 60.0),
                    lon0: centre.lon,
                    zoom: 1.0,
                })
            }
            Projection::Globe => {
                // The planet stays where it is. Turn it so the middle of
                // the route is at the middle of the pane, then find the
                // largest size at which every part of the route is still
                // in the pane and on the near side.
                //
                // Very long routes wrap so far round the planet that a
                // slanted view would push their ends over the horizon;
                // keep every point within 80 degrees of the camera.
                let spread = unit.iter().map(|p| angle(middle, *p)).fold(0.0, f64::max);
                let slant = Self::TILT
                    .min(80.0 - spread.to_degrees())
                    .max(0.0)
                    .to_radians()
                    .sin();
                let framed = |zoom: f64| {
                    let mut trial = View { zoom, ..*self };
                    let (x, y) = trial.focus(slant, w, h);
                    trial.aim(middle, x, y, w, h);
                    trial
                };
                // Sizes from 9x down to 1x in even steps of about 5%. Trying
                // each in turn is a few thousand multiplications, once per
                // route, and unlike a formula it stays right when the
                // planet has been moved off-centre.
                const STEPS: i32 = 44;
                let chosen = (0..=STEPS)
                    .map(|i| framed(9f64.powf(1.0 - f64::from(i) / f64::from(STEPS))))
                    .find(|trial| targets.iter().all(|p| trial.shows(*p, w, h)))
                    // Nothing fits (a route right round the world, or a
                    // planet moved mostly out of the pane): show it whole.
                    .unwrap_or_else(|| framed(1.0));
                Some(Framing {
                    lat0: chosen.lat0,
                    lon0: chosen.lon0,
                    zoom: chosen.zoom,
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
                    // Framing turns and sizes the planet; it stays put.
                    let view = View {
                        lat0: f.lat0,
                        lon0: f.lon0,
                        zoom: f.zoom,
                        ..base
                    };
                    if proj == Projection::Globe {
                        assert_eq!(view.globe_centre(w, h), (w / 2.0, h / 2.0));
                    }
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
            // Anchor well away from the centre, where an approximate
            // correction used to drift by several cells.
            let (x, y, _) = view.project(LHR.to_vec(), w, h);
            let planet = view.globe_centre(w, h);
            for factor in [1.2, 1.2, 1.2, 0.7] {
                view.zoom_about(factor, x, y, w, h);
                let (nx, ny, _) = view.project(LHR.to_vec(), w, h);
                assert!(
                    (nx - x).abs() < 1e-6 && (ny - y).abs() < 1e-6,
                    "{proj:?}: moved to ({nx:.2}, {ny:.2})"
                );
                // On the globe the place is held by turning the planet,
                // never by moving it.
                assert_eq!(view.globe_centre(w, h), planet, "{proj:?}");
            }
        }
    }

    #[test]
    fn aiming_puts_a_place_at_a_point_without_moving_the_planet() {
        let (w, h) = (164.0, 52.0);
        for (drop, shift) in [(0.0, 0.0), (0.4, -0.5)] {
            for zoom in [0.8, 1.0, 3.5, 9.0] {
                let mut view = View {
                    lat0: -10.0,
                    lon0: 30.0,
                    zoom,
                    proj: Projection::Globe,
                    drop,
                    shift,
                };
                let planet = view.globe_centre(w, h);
                let r = view.globe_radius(w, h);
                // Points across the disc, by direction and by distance out.
                for place in [LHR, NRT, JFK, LatLon::new(-33.9, 151.2)] {
                    for (ux, uy) in [(0.0, 0.0), (0.5, 0.2), (-0.3, -0.4), (0.1, 0.7)] {
                        let (x, y) = (planet.0 + ux * r, planet.1 - uy * r);
                        view.aim(place.to_vec(), x, y, w, h);
                        let (nx, ny, side) = view.project(place.to_vec(), w, h);
                        assert_eq!(side, Side::Front);
                        assert!(
                            (nx - x).abs() < 1e-6 && (ny - y).abs() < 1e-6,
                            "{place:?} wanted at ({x:.2}, {y:.2}), drawn at ({nx:.2}, {ny:.2})"
                        );
                        assert_eq!(view.globe_centre(w, h), planet);
                    }
                }
            }
        }
    }

    #[test]
    fn aiming_past_the_edge_of_the_planet_stops_just_inside_it() {
        let (w, h) = (164.0, 52.0);
        let mut view = View {
            lat0: 0.0,
            lon0: 0.0,
            zoom: 1.0,
            proj: Projection::Globe,
            drop: 0.0,
            shift: 0.0,
        };
        // Far out in space to the right of the planet.
        view.aim(LatLon::new(0.0, 100.0).to_vec(), w, h / 2.0, w, h);
        let (x, y, side) = view.project(LatLon::new(0.0, 100.0).to_vec(), w, h);
        let r = view.globe_radius(w, h);
        assert_eq!(side, Side::Front);
        assert!((y - h / 2.0).abs() < 1e-6);
        assert!(
            ((x - w / 2.0) / r - 0.92).abs() < 1e-6,
            "at {:.3} radii",
            (x - w / 2.0) / r
        );
    }

    #[test]
    fn sliding_moves_the_planet_without_turning_it_and_never_loses_it() {
        let (w, h) = (164.0, 52.0);
        let mut view = View {
            lat0: 12.0,
            lon0: -40.0,
            zoom: 2.0,
            proj: Projection::Globe,
            drop: 0.0,
            shift: 0.0,
        };
        assert_eq!(view.globe_centre(w, h), (82.0, 26.0), "centred to begin");
        view.slide(10.0, -6.0, w, h);
        let (cx, cy) = view.globe_centre(w, h);
        assert!((cx - 92.0).abs() < 1e-9 && (cy - 20.0).abs() < 1e-9);
        assert_eq!((view.lat0, view.lon0, view.zoom), (12.0, -40.0, 2.0));

        // Zooming does not move it either.
        view.zoom_by(3.0);
        let (zx, zy) = view.globe_centre(w, h);
        assert!((zx - 92.0).abs() < 1e-9 && (zy - 20.0).abs() < 1e-9);

        // Flung far away it stops with part of itself still in the pane...
        view.slide(1e6, 1e6, w, h);
        let visible = |view: &View| {
            let (cx, cy) = view.globe_centre(w, h);
            let r = view.globe_radius(w, h);
            cx - r < w && cx + r > 0.0 && cy - r < h && cy + r > 0.0
        };
        assert!(visible(&view));
        // ...and the first movement back is not swallowed by the overshoot.
        let (before, _) = view.globe_centre(w, h);
        view.slide(-5.0, 0.0, w, h);
        let (after, _) = view.globe_centre(w, h);
        assert!((before - after - 5.0).abs() < 1e-9, "{before} -> {after}");
        // A planet at the edge is still in sight after shrinking.
        view.slide(1e6, 1e6, w, h);
        view.zoom_by(0.01);
        assert!(visible(&view));
    }

    #[test]
    fn framing_finds_the_route_wherever_the_planet_has_been_put() {
        let (w, h) = (164.0, 52.0);
        for (drop, shift) in [(0.0, 0.0), (0.45, 0.0), (0.3, -0.6), (-0.2, 0.5)] {
            for (a, b) in [
                (LHR, NRT),
                (LHR, LatLon::new(49.0, 2.55)),
                (LatLon::new(-33.9, 151.2), LatLon::new(-31.9, 115.9)),
            ] {
                let base = View {
                    lat0: 0.0,
                    lon0: 0.0,
                    zoom: 1.0,
                    proj: Projection::Globe,
                    drop,
                    shift,
                };
                let legs = [(a.to_vec(), b.to_vec())];
                let f = base.fit(&legs, &[], w, h).expect("framed");
                let view = View {
                    lat0: f.lat0,
                    lon0: f.lon0,
                    zoom: f.zoom,
                    ..base
                };
                assert_eq!(
                    view.globe_centre(w, h),
                    base.globe_centre(w, h),
                    "framing left the planet where it was"
                );
                for i in 0..=20 {
                    let p = arc_point(legs[0].0, legs[0].1, f64::from(i) / 20.0, true);
                    let (x, y, side) = view.project(p, w, h);
                    assert!(
                        side != Side::Hidden && (0.0..w).contains(&x) && (0.0..h).contains(&y),
                        "planet at ({drop}, {shift}), {a:?}>{b:?}, t={i}: ({x:.1}, {y:.1}) {side:?}"
                    );
                }
                // A short hop is still magnified, not left as a speck.
                if angle(a.to_vec(), b.to_vec()) < 0.2 {
                    assert!(f.zoom > 4.0, "zoom {} for a short hop", f.zoom);
                }
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
