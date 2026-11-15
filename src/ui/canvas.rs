//! A sub-cell drawing surface.
//!
//! Coordinates are "dots": 2 across and 4 down per terminal cell, the grid a
//! braille character provides. Because a cell is about twice as tall as it
//! is wide, dots come out square, so a circle drawn here looks like a circle.
//!
//! Every dot carries a priority. When a higher-priority layer touches a cell
//! it clears what lower layers drew there, so a flight arc cuts a clean line
//! through the coastline instead of merging into a blob.
//!
//! The dots are always recorded at full resolution. How they are shown is
//! decided at the end, by the `Marker`: as braille where the font has it,
//! otherwise as the ordinary character whose shape best matches the dots.

use std::sync::OnceLock;

use ratatui::{buffer::Buffer, layout::Rect, style::Color, symbols::braille::BRAILLE};

/// How dots become characters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Marker {
    /// 2x4 dots per cell: the sharpest, where the font can draw it.
    Braille,
    /// Line art from punctuation and box-drawing characters that every
    /// console font has. Each cell shows the glyph nearest in shape to its
    /// dots, so a coastline stays a thin line rather than a row of bricks.
    Ascii,
    /// 1x2 half-blocks per cell: chunky, for filled shapes only.
    Half,
}

impl Marker {
    pub fn next(self) -> Self {
        match self {
            Marker::Braille => Marker::Ascii,
            Marker::Ascii => Marker::Half,
            Marker::Half => Marker::Braille,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Marker::Braille => "BRAILLE",
            Marker::Ascii => "LINE ART",
            Marker::Half => "HALF-BLOCK",
        }
    }

    /// Fine enough for coastlines, borders and single-dot cities.
    pub fn draws_lines(self) -> bool {
        self != Marker::Half
    }
}

#[derive(Clone, Copy)]
struct Cell {
    /// Lit dots, in ratatui's row-major braille order: bit `row * 2 + col`.
    bits: u8,
    prio: u8,
    color: Color,
    /// Background texture for markers too coarse to stipple.
    shade: Option<Color>,
}

#[derive(Clone)]
pub struct DotCanvas {
    cols: usize,
    rows: usize,
    marker: Marker,
    cells: Vec<Cell>,
}

impl DotCanvas {
    pub fn new(cols: u16, rows: u16, marker: Marker) -> Self {
        let (cols, rows) = (cols as usize, rows as usize);
        let blank = Cell {
            bits: 0,
            prio: 0,
            color: Color::Reset,
            shade: None,
        };
        Self {
            cols,
            rows,
            marker,
            cells: vec![blank; cols * rows],
        }
    }

    pub fn marker(&self) -> Marker {
        self.marker
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Width in dots.
    pub fn width(&self) -> f64 {
        self.cols as f64 * 2.0
    }

    /// Height in dots.
    pub fn height(&self) -> f64 {
        self.rows as f64 * 4.0
    }

    pub fn plot(&mut self, x: f64, y: f64, color: Color, prio: u8) {
        if !(x >= 0.0 && y >= 0.0 && x < self.width() && y < self.height()) {
            return; // also rejects NaN
        }
        let (xi, yi) = (x as usize, y as usize);
        let bit: u8 = 1 << ((yi % 4) * 2 + xi % 2);
        let cell = &mut self.cells[(yi / 4) * self.cols + xi / 2];
        if prio > cell.prio {
            *cell = Cell {
                bits: bit,
                prio,
                color,
                shade: cell.shade,
            };
        } else if prio == cell.prio {
            cell.bits |= bit;
            cell.color = color;
        }
    }

    pub fn line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, color: Color, prio: u8) {
        let Some((x0, y0, x1, y1)) = clip(x0, y0, x1, y1, self.width(), self.height()) else {
            return;
        };
        let steps = ((x1 - x0).abs().max((y1 - y0).abs()).ceil() as usize).max(1);
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            self.plot(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t, color, prio);
        }
    }

    /// Rub out the dots along a line, wherever they belong to a layer of
    /// `prio` or lower. Drawn through solid land this leaves a dark hairline:
    /// a border, with no second colour fighting the land for the cell.
    pub fn carve(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, prio: u8) {
        let Some((x0, y0, x1, y1)) = clip(x0, y0, x1, y1, self.width(), self.height()) else {
            return;
        };
        let steps = ((x1 - x0).abs().max((y1 - y0).abs()).ceil() as usize).max(1);
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            let (xi, yi) = ((x0 + (x1 - x0) * t) as usize, (y0 + (y1 - y0) * t) as usize);
            if let Some(cell) = self.cells.get_mut((yi / 4) * self.cols + xi / 2) {
                if cell.prio <= prio {
                    cell.bits &= !(1 << ((yi % 4) * 2 + xi % 2));
                }
            }
        }
    }

    /// Tint a whole cell (by cell coordinates); shown only where no dot lands.
    pub fn shade(&mut self, col: usize, row: usize, color: Color) {
        if col < self.cols && row < self.rows {
            self.cells[row * self.cols + col].shade = Some(color);
        }
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        for row in 0..self.rows.min(area.height as usize) {
            for col in 0..self.cols.min(area.width as usize) {
                let cell = self.cells[row * self.cols + col];
                let (symbol, color) = if cell.bits != 0 {
                    let symbol = match self.marker {
                        Marker::Braille => BRAILLE[cell.bits as usize],
                        Marker::Ascii => shape_table()[cell.bits as usize],
                        Marker::Half => match (cell.bits & 0x0F != 0, cell.bits & 0xF0 != 0) {
                            (true, false) => '▀',
                            (false, true) => '▄',
                            _ => '█',
                        },
                    };
                    (symbol, cell.color)
                } else if let Some(color) = cell.shade {
                    (
                        if self.marker == Marker::Half {
                            '░'
                        } else {
                            '.'
                        },
                        color,
                    )
                } else {
                    continue;
                };
                if let Some(target) = buf.cell_mut((area.x + col as u16, area.y + row as u16)) {
                    target.set_char(symbol).set_fg(color);
                }
            }
        }
    }
}

/// Where each character puts its ink, on the same 2x4 grid as the dots:
/// `[row0 left, row0 right, row1 left, ...]`, 0.0 (bare) to 1.0 (solid).
/// Only characters from WGL4 - the set every Windows console font carries -
/// so this works in a font that has never heard of braille.
const SHAPES: [(char, [f32; 8]); 23] = [
    // Single marks, by where they sit in the cell. The middle dot and the
    // middle rule sit on the line between the two middle rows of dots, so
    // each is listed once for either row.
    ('`', [0.9, 0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ('´', [0.1, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ('\'', [0.5, 0.5, 0.3, 0.3, 0.0, 0.0, 0.0, 0.0]),
    ('·', [0.0, 0.0, 0.6, 0.6, 0.0, 0.0, 0.0, 0.0]),
    ('·', [0.0, 0.0, 0.0, 0.0, 0.6, 0.6, 0.0, 0.0]),
    ('.', [0.0, 0.0, 0.0, 0.0, 0.1, 0.1, 0.6, 0.6]),
    (',', [0.0, 0.0, 0.0, 0.0, 0.2, 0.0, 0.9, 0.3]),
    // Horizontal strokes at three heights, and a heavy one.
    ('¯', [1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ('─', [0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0]),
    ('─', [0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0]),
    ('═', [0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 0.0, 0.0]),
    ('_', [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0]),
    ('=', [0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 1.0]),
    // Vertical strokes.
    ('│', [0.6, 0.6, 0.6, 0.6, 0.6, 0.6, 0.6, 0.6]),
    (':', [0.0, 0.0, 0.7, 0.7, 0.0, 0.0, 0.7, 0.7]),
    // Diagonals and curves. The brackets are thin glyphs, so they are
    // given less ink than the slashes: a run of dots that could be either
    // is a straight diagonal far more often than it is a curve.
    ('/', [0.0, 1.0, 0.2, 0.8, 0.8, 0.2, 1.0, 0.0]),
    ('\\', [1.0, 0.0, 0.8, 0.2, 0.2, 0.8, 0.0, 1.0]),
    ('(', [0.1, 0.5, 0.8, 0.0, 0.8, 0.0, 0.1, 0.5]),
    (')', [0.5, 0.1, 0.0, 0.8, 0.0, 0.8, 0.5, 0.1]),
    ('^', [0.6, 0.6, 0.8, 0.8, 0.0, 0.0, 0.0, 0.0]),
    ('v', [0.0, 0.0, 0.0, 0.0, 0.8, 0.8, 0.6, 0.6]),
    // Clusters.
    ('+', [0.0, 0.0, 0.8, 0.8, 0.8, 0.8, 0.3, 0.3]),
    ('#', [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]),
];

/// For each of the 256 dot patterns, the character closest to it in shape.
fn shape_table() -> &'static [char; 256] {
    static TABLE: OnceLock<[char; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [' '; 256];
        for (bits, slot) in table.iter_mut().enumerate().skip(1) {
            let distance = |shape: &[f32; 8]| -> f32 {
                (0..8)
                    .map(|i| {
                        let lit = if bits >> i & 1 == 1 { 1.0 } else { 0.0 };
                        (lit - shape[i]) * (lit - shape[i])
                    })
                    .sum()
            };
            // A lit dot must never vanish, so the blank is not a candidate.
            *slot = SHAPES
                .iter()
                .min_by(|a, b| distance(&a.1).total_cmp(&distance(&b.1)))
                .map_or('#', |(ch, _)| *ch);
        }
        table
    })
}

/// Liang-Barsky clip of a segment to the canvas. Without this, a zoomed-in
/// coastline segment that extends far off-screen would cost thousands of
/// wasted plot calls.
fn clip(x0: f64, y0: f64, x1: f64, y1: f64, w: f64, h: f64) -> Option<(f64, f64, f64, f64)> {
    if ![x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
        return None;
    }
    let (dx, dy) = (x1 - x0, y1 - y0);
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [
        (-dx, x0),
        (dx, w - 0.001 - x0),
        (-dy, y0),
        (dy, h - 0.001 - y0),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None; // parallel to this edge and outside it
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                if r > t1 {
                    return None;
                }
                t0 = t0.max(r);
            } else {
                if r < t0 {
                    return None;
                }
                t1 = t1.min(r);
            }
        }
    }
    Some((x0 + t0 * dx, y0 + t0 * dy, x0 + t1 * dx, y0 + t1 * dy))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(canvas: &DotCanvas, col: u16, row: u16) -> String {
        let area = Rect::new(0, 0, canvas.cols as u16, canvas.rows as u16);
        let mut buf = Buffer::empty(area);
        canvas.render(area, &mut buf);
        buf[(col, row)].symbol().to_string()
    }

    #[test]
    fn braille_dots_land_in_the_right_corner() {
        let mut c = DotCanvas::new(2, 2, Marker::Braille);
        c.plot(0.0, 0.0, Color::Green, 1); // top-left dot of cell (0,0)
        c.plot(3.0, 7.0, Color::Green, 1); // bottom-right dot of cell (1,1)
        assert_eq!(symbol(&c, 0, 0), "⠁");
        assert_eq!(symbol(&c, 1, 1), "⢀");
        assert_eq!(symbol(&c, 1, 0), " ");
    }

    #[test]
    fn higher_priority_replaces_lower_in_a_cell() {
        let mut c = DotCanvas::new(1, 1, Marker::Braille);
        c.plot(0.0, 0.0, Color::Green, 1);
        c.plot(1.0, 0.0, Color::Yellow, 5);
        assert_eq!(symbol(&c, 0, 0), "⠈", "only the high-priority dot survives");
        c.plot(0.0, 3.0, Color::Green, 1);
        assert_eq!(
            symbol(&c, 0, 0),
            "⠈",
            "a later low-priority dot cannot intrude"
        );
    }

    #[test]
    fn lines_are_clipped_not_dropped() {
        let mut c = DotCanvas::new(4, 1, Marker::Braille);
        c.line(-1000.0, 1.0, 1000.0, 1.0, Color::Green, 1);
        assert!((0..4).all(|col| symbol(&c, col, 0) == "⠒"));
        // Entirely off-canvas: nothing drawn, and no panic on NaN.
        let mut d = DotCanvas::new(4, 1, Marker::Braille);
        d.line(-50.0, -50.0, -10.0, -10.0, Color::Green, 1);
        d.line(f64::NAN, 0.0, 1.0, 1.0, Color::Green, 1);
        assert!((0..4).all(|col| symbol(&d, col, 0) == " "));
    }

    #[test]
    fn carving_rubs_a_line_out_of_what_lies_beneath_but_not_above() {
        let mut c = DotCanvas::new(2, 1, Marker::Braille);
        for y in 0..4 {
            c.line(0.0, f64::from(y), 3.0, f64::from(y), Color::Green, 2);
        }
        assert_eq!(symbol(&c, 0, 0), "⣿", "solid to start with");
        c.carve(0.0, 1.0, 3.0, 1.0, 3);
        assert_eq!(symbol(&c, 0, 0), "⣭", "the second row of dots is gone");
        assert_eq!(symbol(&c, 1, 0), "⣭");
        // A flight path drawn above the land is not cut by a border.
        c.line(0.0, 3.0, 3.0, 3.0, Color::Yellow, 8);
        c.carve(0.0, 3.0, 3.0, 3.0, 3);
        assert_eq!(symbol(&c, 0, 0), "⣀");
    }

    #[test]
    fn half_blocks_show_which_half_is_lit() {
        let mut c = DotCanvas::new(2, 1, Marker::Half);
        c.plot(0.0, 0.0, Color::Green, 1);
        assert_eq!(symbol(&c, 0, 0), "▀");
        c.plot(0.0, 3.0, Color::Green, 1);
        assert_eq!(symbol(&c, 0, 0), "█");
        c.plot(3.0, 2.0, Color::Green, 1);
        assert_eq!(symbol(&c, 1, 0), "▄");
        let mut shaded = DotCanvas::new(1, 1, Marker::Half);
        shaded.shade(0, 0, Color::Green);
        assert_eq!(symbol(&shaded, 0, 0), "░");
    }

    /// Draw a stroke through one cell and say which character it became.
    fn stroke(from: (f64, f64), to: (f64, f64)) -> String {
        let mut c = DotCanvas::new(1, 1, Marker::Ascii);
        c.line(from.0, from.1, to.0, to.1, Color::Green, 1);
        symbol(&c, 0, 0)
    }

    #[test]
    fn line_art_picks_the_character_shaped_like_the_stroke() {
        assert_eq!(stroke((0.0, 0.0), (1.0, 0.0)), "¯", "along the top");
        assert_eq!(stroke((0.0, 1.0), (1.0, 1.0)), "─", "through the middle");
        assert_eq!(stroke((0.0, 2.0), (1.0, 2.0)), "─");
        assert_eq!(stroke((0.0, 3.0), (1.0, 3.0)), "_", "along the bottom");
        assert_eq!(stroke((1.0, 0.0), (0.0, 3.0)), "/");
        assert_eq!(stroke((0.0, 0.0), (1.0, 3.0)), "\\");
        // A single dot keeps its corner of the cell.
        assert_eq!(stroke((0.0, 0.0), (0.0, 0.0)), "`");
        assert_eq!(stroke((1.0, 0.0), (1.0, 0.0)), "´");
        assert_eq!(stroke((1.0, 3.0), (1.0, 3.0)), ".");
    }

    #[test]
    fn line_art_never_loses_a_dot_and_stays_inside_wgl4() {
        let table = shape_table();
        assert_eq!(table[0], ' ');
        assert!(
            table[1..].iter().all(|ch| *ch != ' '),
            "every lit pattern shows something"
        );
        assert_eq!(table[255], '#');
        // Nothing here may need a font beyond the basic Windows repertoire.
        assert!(
            SHAPES
                .iter()
                .all(|(ch, _)| (*ch as u32) < 0x2600 && !('\u{2800}'..='\u{28FF}').contains(ch))
        );
    }
}
