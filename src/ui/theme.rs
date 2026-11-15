//! Colours and glyphs.
//!
//! Colours are xterm-256 indices rather than 24-bit RGB: every terminal that
//! matters understands them, including macOS Terminal.app and the Windows 10
//! console, neither of which can be trusted with true colour.

use ratatui::style::Color;

use super::canvas::Marker;
use crate::continents::Continent;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub bg: Color,
    /// Body text.
    pub fg: Color,
    pub dim: Color,
    pub faint: Color,
    /// Headings and focus.
    pub bright: Color,
    /// The selected flight path and anything demanding attention.
    pub accent: Color,
    pub accent_dim: Color,
    pub alert: Color,
    pub info: Color,
    /// "Cheap", "live", "go".
    pub good: Color,
    pub sel_bg: Color,
    pub bar_bg: Color,
    pub coast: Color,
    pub land: Color,
    pub grid: Color,
    pub frontier: Color,
    pub city: Color,
}

const fn c(index: u8) -> Color {
    Color::Indexed(index)
}

pub const PHOSPHOR: Theme = Theme {
    name: "PHOSPHOR",
    bg: c(16),
    fg: c(40),
    dim: c(28),
    faint: c(22),
    bright: c(46),
    accent: c(214),
    accent_dim: c(130),
    alert: c(196),
    info: c(45),
    good: c(83),
    sel_bg: c(22),
    bar_bg: c(22),
    coast: c(40),
    land: c(28),
    grid: c(237),
    frontier: c(240),
    city: c(136),
};

pub const AMBER: Theme = Theme {
    name: "AMBER",
    bg: c(16),
    fg: c(214),
    dim: c(172),
    faint: c(94),
    bright: c(220),
    accent: c(51),
    accent_dim: c(30),
    alert: c(196),
    info: c(117),
    good: c(83),
    sel_bg: c(58),
    bar_bg: c(58),
    coast: c(214),
    land: c(130),
    grid: c(237),
    frontier: c(240),
    city: c(250),
};

pub const THEMES: [Theme; 2] = [PHOSPHOR, AMBER];

impl Theme {
    pub fn by_name(name: &str) -> Theme {
        THEMES
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
            .unwrap_or(PHOSPHOR)
    }

    pub fn next(self) -> Theme {
        let i = THEMES.iter().position(|t| t.name == self.name).unwrap_or(0);
        THEMES[(i + 1) % THEMES.len()]
    }

    /// Fare heat: 0.0 (cheapest, green) through yellow to 1.0 (dearest, red).
    pub fn heat(self, t: f64) -> Color {
        const RAMP: [u8; 9] = [46, 118, 154, 190, 226, 220, 214, 208, 196];
        let i = (t.clamp(0.0, 1.0) * (RAMP.len() - 1) as f64).round() as usize;
        c(RAMP[i.min(RAMP.len() - 1)])
    }

    /// A journey's colour: 0.0 at the origin (cyan) to 1.0 at the
    /// destination (pink), stepping through blue and violet on the way.
    ///
    /// These hues are kept for the route alone. Fares own green-to-red and
    /// the land owns greens and earth tones, so nothing else on screen can
    /// be mistaken for a flight path, and the two ends of a trip can never
    /// be mistaken for each other.
    pub fn route(self, t: f64) -> Color {
        c(ROUTE_RAMP[route_step(t)])
    }

    /// The same ramp turned down, for the part of an arc beyond the horizon
    /// and for a route that is not the one in hand.
    pub fn route_dim(self, t: f64) -> Color {
        c(dimmed(ROUTE_RAMP[route_step(t)]))
    }

    /// Legs of the itineraries not selected.
    pub fn route_other(self) -> Color {
        c(60)
    }

    /// Tag colour for the trip type: a price for one way and a price for
    /// both ways must never be mistaken for each other.
    pub fn trip(self, round_trip: bool) -> Color {
        if round_trip { c(231) } else { self.accent }
    }

    /// Backdrop for "the usual price range" on charts and bars.
    pub fn band(self) -> Color {
        c(237)
    }

    /// `(land, coastline)` colours for a continent.
    pub fn continent(self, continent: Continent) -> (Color, Color) {
        let (land, coast) = CONTINENT_TINTS[continent.index()];
        (c(land), c(coast))
    }
}

const ROUTE_RAMP: [u8; 7] = [51, 81, 111, 141, 177, 213, 205];

fn route_step(t: f64) -> usize {
    let last = ROUTE_RAMP.len() - 1;
    ((t.clamp(0.0, 1.0) * last as f64).round() as usize).min(last)
}

/// Halve the brightness of a colour from the 6x6x6 cube, keeping its hue.
fn dimmed(index: u8) -> u8 {
    if !(16..232).contains(&index) {
        return index;
    }
    let i = index - 16;
    let half = |level: u8| level.div_ceil(2);
    16 + 36 * half(i / 36) + 6 * half(i / 6 % 6) + half(i % 6)
}

/// `(land, coastline)` per continent, in `Continent` order. Neighbours
/// get contrasting hues; none borrows from the route's blues and pinks.
const CONTINENT_TINTS: [(u8, u8); Continent::COUNT] = [
    (136, 178), // Africa: ochre
    (250, 255), // Antarctica: ice
    (106, 148), // Asia: moss
    (109, 152), // Europe: pale slate
    (36, 43),   // North America: sea green
    (130, 172), // Oceania: rust
    (34, 40),   // South America: green
];

/// Symbols that not every terminal font can draw. The "safe" set stays inside
/// WGL4, the repertoire every Windows console font ships (box drawing, block
/// shades, arrows, a few shapes) - which notably lacks braille.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glyphs {
    pub rich: bool,
    /// The aircraft on the map. `None` where the font may not have one: an
    /// arrowhead pointing along the flight path is drawn instead.
    pub plane: Option<&'static str>,
    /// Vertical connector for a layover in the itinerary timeline.
    pub wait: &'static str,
    pub ellipsis: &'static str,
    /// Low-to-high levels for one-line price trends.
    pub spark: &'static [char],
    pub spinner: &'static [&'static str],
}

pub const RICH: Glyphs = Glyphs {
    rich: true,
    plane: Some("✈"),
    wait: "┊",
    ellipsis: "…",
    spark: &['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'],
    spinner: &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
};

/// For a console font that has braille but is on its own: the classic
/// Windows console never borrows a missing glyph from another font, and
/// even the best of its fonts has no aeroplane.
pub const BRAILLE: Glyphs = Glyphs {
    plane: None,
    wait: ":",
    ..RICH
};

pub const SAFE: Glyphs = Glyphs {
    rich: false,
    plane: None,
    wait: ":",
    ellipsis: "…",
    spark: &[' ', '░', '▒', '▓', '█'],
    spinner: &["|", "/", "-", "\\"],
};

impl Glyphs {
    /// `charset` is "rich", "braille" or "safe". Anything else is settled
    /// by a guess from the environment; the interactive start-up asks the
    /// console itself first (`console::prepare`), so it never gets here
    /// without a definite answer.
    pub fn detect(charset: &str) -> (Glyphs, Marker) {
        match charset.to_ascii_lowercase().as_str() {
            "rich" => (RICH, Marker::Braille),
            "braille" => (BRAILLE, Marker::Braille),
            "safe" => (SAFE, Marker::Ascii),
            _ if crate::console::looks_modern() => (RICH, Marker::Braille),
            _ => (SAFE, Marker::Ascii),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heat_ramp_runs_green_to_red() {
        assert_eq!(PHOSPHOR.heat(0.0), c(46));
        assert_eq!(PHOSPHOR.heat(1.0), c(196));
        assert_eq!(PHOSPHOR.heat(-3.0), c(46));
        assert_eq!(PHOSPHOR.heat(9.0), c(196));
    }

    #[test]
    fn themes_cycle_and_resolve_by_name() {
        assert_eq!(Theme::by_name("amber").name, "AMBER");
        assert_eq!(Theme::by_name("nonsense").name, "PHOSPHOR");
        assert_eq!(PHOSPHOR.next().name, "AMBER");
        assert_eq!(AMBER.next().name, "PHOSPHOR");
    }

    #[test]
    fn explicit_charset_overrides_detection() {
        assert_eq!(Glyphs::detect("safe"), (SAFE, Marker::Ascii));
        assert_eq!(Glyphs::detect("RICH"), (RICH, Marker::Braille));
        // Braille without the rarer symbols: dots yes, aeroplane no.
        let (glyphs, marker) = Glyphs::detect("braille");
        assert_eq!((marker, glyphs.plane), (Marker::Braille, None));
        assert_eq!(glyphs.spinner, RICH.spinner);
    }

    #[test]
    fn a_route_runs_cyan_to_pink_and_dims_without_changing_hue() {
        assert_eq!(PHOSPHOR.route(0.0), c(51));
        assert_eq!(PHOSPHOR.route(1.0), c(205));
        assert_eq!(PHOSPHOR.route(-1.0), PHOSPHOR.route(0.0));
        assert_ne!(PHOSPHOR.route(0.5), PHOSPHOR.route(0.0));
        // Cyan (0,5,5) halves to (0,3,3); greys and the base 16 are left be.
        assert_eq!(dimmed(51), 37);
        assert_eq!(dimmed(244), 244);
        assert_eq!(dimmed(16), 16);
    }

    #[test]
    fn neighbouring_continents_never_share_a_colour() {
        use Continent::*;
        let tint = |c| PHOSPHOR.continent(c);
        for (a, b) in [
            (Europe, Asia),
            (Europe, Africa),
            (Africa, Asia),
            (Asia, Oceania),
            (NorthAmerica, SouthAmerica),
        ] {
            assert_ne!(tint(a).0, tint(b).0, "{a:?} / {b:?}");
            assert_ne!(tint(a).1, tint(b).1, "{a:?} / {b:?}");
        }
        // Nothing on land may be confused with a flight path.
        for continent in [
            Africa,
            Antarctica,
            Asia,
            Europe,
            NorthAmerica,
            Oceania,
            SouthAmerica,
        ] {
            let (land, coast) = tint(continent);
            assert!(
                ROUTE_RAMP.iter().all(|r| c(*r) != land && c(*r) != coast),
                "{continent:?}"
            );
        }
    }
}
