//! The fixed panes: header, footer, mission form, price radar, tracks table
//! and the itinerary dossier.

use chrono::{NaiveDate, Utc};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

use super::{chip, pane, pane_with, theme::Glyphs, title_style, trip_chip};
use crate::{
    airports::{self, Airport},
    app::{App, Field, KeyState, Level, Pane, Popup, Slot, TRACK_COLUMNS},
    fmt,
    model::{Cabin, Itinerary, LayoverFlag, Stops, TimeWindow},
    score::deal_score,
};

const BOLD: Modifier = Modifier::BOLD;

// ------------------------------------------------------------------ header

pub fn header(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let bar = Style::new().fg(th.bright).bg(th.bar_bg);
    let buf = f.buffer_mut();
    buf.set_style(area, bar);
    let name = Span::styled(
        " ICARUS-SEARCH ",
        Style::new().fg(th.bg).bg(th.bright).add_modifier(BOLD),
    );
    let tagline = Span::styled(" FLIGHT INTERCEPT TERMINAL", bar);

    // The right-hand side, in pieces, each with how much it matters (0 is
    // never dropped). A narrow window loses the least important first, so
    // the data source and the two buttons are the last things to go: live
    // mode has more to say than fits in 120 columns, and it must not be
    // the source of the fares that falls off the end.
    #[derive(Clone, Copy, PartialEq)]
    enum Role {
        Text,
        Setup,
        Currency,
    }
    let button = Style::new().fg(th.bg).bg(th.accent).add_modifier(BOLD);
    let mut pieces: Vec<(u8, Span, Role)> = Vec::new();
    let source = if !app.live {
        // Simulated prices must never be mistaken for real ones: this blinks.
        let color = if app.frame(1.5, 2) == 0 {
            th.accent
        } else {
            th.accent_dim
        };
        Span::styled(
            "● SIMULATED DATA",
            Style::new().fg(color).add_modifier(BOLD),
        )
    } else if matches!(app.key_state, KeyState::Rejected(_)) {
        Span::styled(
            "● KEY REJECTED",
            Style::new().fg(th.alert).add_modifier(BOLD),
        )
    } else {
        Span::styled(
            format!("● LIVE {}", app.source),
            Style::new().fg(th.good).add_modifier(BOLD),
        )
    };
    pieces.push((0, source, Role::Text));
    if let Some(quota) = app.quota {
        let color = if quota.left < 20 {
            th.alert
        } else if quota.left < 60 {
            th.accent
        } else {
            th.bright
        };
        pieces.push((
            2,
            Span::styled(
                format!("FUEL {}/{}", quota.left, quota.per_month),
                Style::new().fg(color),
            ),
            Role::Text,
        ));
    }
    if app.live && app.calls > 0 {
        pieces.push((
            4,
            Span::styled(format!("CALLS {}", app.calls), bar),
            Role::Text,
        ));
    }
    // Two buttons: click one, or press its key (F2, $).
    pieces.push((1, Span::styled(" F2 SETTINGS ", button), Role::Setup));
    pieces.push((
        1,
        Span::styled(format!(" {} ▼ ", app.cfg.currency), button),
        Role::Currency,
    ));
    pieces.push((
        5,
        Span::styled(Utc::now().format("%Y-%m-%d %H%MZ ").to_string(), bar),
        Role::Text,
    ));

    const SEP: &str = "  │  ";
    let sep_width = SEP.chars().count();
    let mut with_tagline = true;
    loop {
        let right: usize = pieces
            .iter()
            .map(|(_, span, _)| span.width())
            .sum::<usize>()
            + sep_width * pieces.len().saturating_sub(1);
        let left = name.width() + if with_tagline { tagline.width() } else { 0 };
        if left + right + 2 <= usize::from(area.width) {
            break;
        }
        let worst = pieces.iter().map(|(rank, _, _)| *rank).max().unwrap_or(0);
        if worst > 3 || (!with_tagline && worst > 0) {
            if let Some(at) = pieces.iter().rposition(|(rank, _, _)| *rank == worst) {
                pieces.remove(at);
            }
        } else if with_tagline {
            with_tagline = false;
        } else {
            break;
        }
    }

    let left = if with_tagline {
        Line::from(vec![name, tagline])
    } else {
        Line::from(name)
    };
    buf.set_line(area.x, area.y, &left, area.width);
    let width: usize = pieces
        .iter()
        .map(|(_, span, _)| span.width())
        .sum::<usize>()
        + sep_width * pieces.len().saturating_sub(1);
    if width + left.width() <= usize::from(area.width) {
        let mut x = area.right() - width as u16;
        let last = pieces.len().saturating_sub(1);
        for (i, (_, span, role)) in pieces.into_iter().enumerate() {
            let span_width = span.width() as u16;
            let place = Rect::new(x, area.y, span_width, 1);
            match role {
                Role::Setup => app.hit.setup = place,
                Role::Currency => app.hit.currency = place,
                Role::Text => {}
            }
            buf.set_span(x, area.y, &span, span_width);
            x += span_width;
            if i < last {
                buf.set_span(
                    x,
                    area.y,
                    &Span::styled(SEP, Style::new().fg(th.dim).bg(th.bar_bg)),
                    sep_width as u16,
                );
                x += sep_width as u16;
            }
        }
    }
}

// ------------------------------------------------------------------ footer

pub fn footer(f: &mut Frame, area: Rect, app: &App) {
    let th = app.theme;
    let bar = Style::new().fg(th.fg).bg(th.bar_bg);
    let buf = f.buffer_mut();
    buf.set_style(area, bar);

    let mut left = vec![Span::styled(" ", bar)];
    let pending = app.inflight + app.survey_left;
    if pending > 0 {
        let spinner = app.glyphs.spinner[app.frame(12.0, app.glyphs.spinner.len())];
        left.push(Span::styled(
            format!("{spinner} {pending} PENDING  "),
            Style::new().fg(th.accent).add_modifier(BOLD),
        ));
    }
    let (level, text) = &app.status;
    let style = match level {
        Level::Info => Style::new().fg(th.bright),
        Level::Good => Style::new().fg(th.good).add_modifier(BOLD),
        Level::Warn => Style::new().fg(th.accent).add_modifier(BOLD),
        Level::Alert => Style::new().fg(th.alert).add_modifier(BOLD),
    };
    left.push(Span::styled(text.clone(), style));
    let left = Line::from(left);

    let hint = Line::from(Span::styled(
        format!("{} ", hints(app)),
        Style::new().fg(th.fg).bg(th.bar_bg),
    ));
    let hint_width = hint.width() as u16;
    // The status message wins; key hints only appear if both fit.
    let fits = left.width() as u16 + hint_width + 3 <= area.width;
    buf.set_line(
        area.x,
        area.y,
        &left,
        if fits {
            area.width - hint_width - 1
        } else {
            area.width
        },
    );
    if fits {
        buf.set_line(area.right() - hint_width, area.y, &hint, hint_width);
    }
}

fn hints(app: &App) -> &'static str {
    match &app.popup {
        Some(Popup::Airport { .. }) => "TYPE TO SEARCH  ↑↓ CHOOSE  ENTER SELECT  ESC CANCEL",
        Some(Popup::Calendar { .. }) => {
            "←↑↓→ DAY  PGUP/PGDN MONTH  HOME SOONEST  ENTER SET  ESC CANCEL"
        }
        Some(Popup::Text { .. }) => "CTRL+V PASTE  ENTER CONFIRM  ESC CANCEL",
        Some(Popup::Dossier { .. }) => "↑↓ SCROLL  O BOOK IN BROWSER  ESC CLOSE",
        Some(Popup::Survey { .. }) => "←→ HORIZON  ENTER LAUNCH  ESC CANCEL",
        Some(Popup::Currency { .. }) => "TYPE TO FILTER  ↑↓ CHOOSE  ENTER SELECT  ESC CANCEL",
        Some(Popup::Settings { .. }) => {
            "↑↓ CHOOSE  ←→ CHANGE  ENTER EDIT  T TEST KEY  S SAVE  ESC CLOSE"
        }
        Some(Popup::Help | Popup::Guide) => "ANY KEY TO CLOSE",
        None if app.intel.is_some() => {
            "←↑↓→ PICK A DAY  ENTER FLY THEN  V SURVEY  ? HOW TO READ  I/ESC BACK"
        }
        None => match app.focus {
            Pane::Form if app.form.field() == Field::Depart => {
                "←→ DAY  ENTER CALENDAR  BACKSPACE TOMORROW  S SCAN  ? HELP"
            }
            Pane::Form => "↑↓ FIELD  ←→ CHANGE  ENTER EDIT  S SCAN  I INTEL  F2 SETTINGS  ? HELP",
            Pane::Tracks => "↑↓ TRACK  ENTER OPEN  O BOOK  1-7 SORT  [ ] DATE  ? HELP",
            Pane::Radar => "↑↓ DATE  ENTER TRACKS  S SCAN  I INTEL  ? HELP",
            Pane::Globe => "←↑↓→ ROTATE  +/- ZOOM  > < DETAIL  C CENTRE  G MAP  ? HELP",
        },
    }
}

// -------------------------------------------------------------------- form

pub fn form(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let focused = app.focus == Pane::Form && app.popup.is_none();
    let block = pane("MISSION PARAMETERS", focused, th);
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.hit.form = area;
    app.hit.form_rows = inner;

    let app = &*app;
    let lines: Vec<Line> = app
        .form
        .fields()
        .into_iter()
        .enumerate()
        .map(|(i, field)| {
            form_row(
                app,
                field,
                i == app.form.cursor,
                focused,
                usize::from(inner.width),
            )
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn form_row(
    app: &App,
    field: Field,
    at_cursor: bool,
    focused: bool,
    width: usize,
) -> Line<'static> {
    let th = app.theme;
    let form = &app.form;
    // Each end of the trip wears the colour it has on the map.
    let airport = |a: Option<&'static Airport>, tone: f64| match a {
        Some(a) => (a.short(), th.route(tone)),
        None => ("-- NOT SET --".to_string(), th.alert),
    };
    // A filter that is narrowing the search stands out from one left open.
    let filter = |narrowing: bool| if narrowing { th.accent } else { th.bright };
    let window = |w: TimeWindow| match (w.preset_name(), w) {
        (Some(name), TimeWindow::ANY) => name.to_string(),
        (Some(name), _) => format!("{} {name}", w.label()),
        (None, _) => w.label(),
    };
    let plural = |n: i64| if n == 1 { "" } else { "S" };
    let (label, value, tone, steps) = match field {
        Field::Trip => {
            let text = if form.round_trip {
                "ROUND-TRIP"
            } else {
                "ONE-WAY"
            };
            ("TRIP", text.to_string(), th.trip(form.round_trip), true)
        }
        Field::From => {
            let (text, tone) = airport(form.origin, 0.0);
            ("FROM", text, tone, false)
        }
        Field::To => {
            let (text, tone) = airport(form.dest, 1.0);
            ("TO", text, tone, false)
        }
        Field::Depart => ("DEPART", fmt::day(form.depart), th.bright, true),
        Field::Return => {
            let nights = (form.ret - form.depart).num_days();
            (
                "RETURN",
                format!("{} +{nights}D", fmt::day(form.ret)),
                th.bright,
                true,
            )
        }
        Field::Flex => {
            let text = match (form.flex, form.flex_later) {
                (0, _) => "EXACT DATE".to_string(),
                (n, false) => format!("± {n} DAY{}", plural(i64::from(n))),
                // That day or any of the days after it, never before.
                (n, true) => format!("OR UP TO {n} LATER"),
            };
            ("FLEX", text, th.bright, true)
        }
        Field::DepWindow => (
            "DEP TIME",
            window(form.dep_window),
            filter(form.dep_window != TimeWindow::ANY),
            true,
        ),
        Field::ArrWindow => (
            "ARR TIME",
            window(form.arr_window),
            filter(form.arr_window != TimeWindow::ANY),
            true,
        ),
        Field::Stops => (
            "STOPS",
            form.stops.label().to_string(),
            filter(form.stops != Stops::default()),
            true,
        ),
        Field::Cabin => (
            "CABIN",
            form.cabin.label().to_string(),
            filter(form.cabin != Cabin::default()),
            true,
        ),
        Field::Pax => {
            let text = format!("{} ADULT{}", form.adults, plural(i64::from(form.adults)));
            ("PAX", text, filter(form.adults > 1), true)
        }
        Field::Airlines => {
            let text = if form.airlines.is_empty() {
                "ANY".to_string()
            } else {
                form.airlines.join(",")
            };
            ("AIRLINES", text, filter(!form.airlines.is_empty()), false)
        }
        Field::Launch => {
            let n = app.scan_dates().len() as i64;
            let unit = if app.live { "CALL" } else { "DATE" };
            let verb = if app.stale() {
                "RE-SCAN (CHANGED)"
            } else {
                "LAUNCH SCAN"
            };
            let text = format!("[ {verb} ]  {n} {unit}{}", plural(n));
            let attention = app.stale() && app.frame(2.0, 2) == 0;
            (
                "",
                text,
                if attention { th.alert } else { th.accent },
                false,
            )
        }
    };

    let active = at_cursor && focused;
    let marker = if at_cursor { "►" } else { " " };
    let stepping = active && steps;
    // The trip type is a solid tag: every price on screen depends on it.
    let tag = field == Field::Trip;
    // The launch button has no label, so it gets the whole row.
    let label_width = if field == Field::Launch { 0 } else { 9 };
    let room = width
        .saturating_sub(2 + label_width + if stepping { 4 } else { 0 } + if tag { 2 } else { 0 });
    let value = fmt::clip(&value, room, app.glyphs.ellipsis);
    let row_bg = if active { th.sel_bg } else { th.bg };
    let base = Style::new().bg(row_bg);
    let arrow = base.fg(th.bright).add_modifier(BOLD);
    let mut spans = vec![
        Span::styled(
            format!("{marker} "),
            base.fg(if focused { th.bright } else { th.dim }),
        ),
        Span::styled(format!("{label:<label_width$}"), base.fg(th.dim)),
    ];
    if stepping {
        spans.push(Span::styled("◄ ", arrow));
    }
    spans.push(if tag {
        chip(&value, tone, th)
    } else {
        let weight = if active || field == Field::Launch {
            BOLD
        } else {
            Modifier::empty()
        };
        Span::styled(value, base.fg(tone).add_modifier(weight))
    });
    if stepping {
        spans.push(Span::styled(" ►", arrow));
    }
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), base));
    Line::from(spans)
}

// ------------------------------------------------------------------- radar

enum RadarCell {
    Pending,
    Failed,
    Empty,
    Price(f64),
}

pub fn radar(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let focused = app.focus == Pane::Radar && app.popup.is_none();
    let block = pane("PRICE RADAR", focused, th);
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.hit.radar = area;
    if inner.height == 0 || inner.width < 20 {
        return;
    }

    let rows: Vec<(NaiveDate, RadarCell)> = app
        .board
        .slots
        .iter()
        .map(|(date, slot)| {
            let cell = match slot {
                Slot::Pending => RadarCell::Pending,
                Slot::Failed(_) => RadarCell::Failed,
                Slot::Ready(_) => slot.lowest().map_or(RadarCell::Empty, RadarCell::Price),
            };
            (*date, cell)
        })
        .collect();
    if rows.is_empty() {
        let hint = vec![
            Line::from(""),
            Line::from(Span::styled(
                " NO SCAN YET",
                Style::new().fg(th.dim).add_modifier(BOLD),
            )),
            Line::from(Span::styled(
                " SET FROM AND TO, THEN PRESS S.",
                Style::new().fg(th.dim),
            )),
            Line::from(Span::styled(
                " FLEX > 0 COMPARES NEARBY DATES.",
                Style::new().fg(th.dim),
            )),
            Line::from(""),
            Line::from(Span::styled(
                " I  FARE INTEL: WHEN TO FLY,",
                Style::new().fg(th.dim),
            )),
            Line::from(Span::styled("    WHEN TO BUY", Style::new().fg(th.dim))),
        ];
        f.render_widget(Paragraph::new(hint), inner);
        return;
    }

    let insight = insight_lines(app, usize::from(inner.width));
    let list_height = usize::from(inner.height)
        .saturating_sub(insight.len())
        .max(1)
        .min(rows.len());
    let active = rows
        .iter()
        .position(|(d, _)| *d == app.board.active)
        .unwrap_or(0);
    // Keep the active date centred when the list is taller than the pane.
    let offset = active
        .saturating_sub(list_height / 2)
        .min(rows.len() - list_height);
    app.hit.radar_rows = Rect::new(inner.x, inner.y, inner.width, list_height as u16);
    app.hit.radar_offset = offset;

    let app = &*app;
    let prices: Vec<f64> = rows
        .iter()
        .filter_map(|(_, c)| {
            if let RadarCell::Price(p) = c {
                Some(*p)
            } else {
                None
            }
        })
        .collect();
    let lo = prices.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = prices.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let bar_width = usize::from(inner.width).saturating_sub(25).clamp(3, 16);
    // The usual price range for this trip, laid over the bars as a grey
    // band: a bar that stops short of it is a bargain, one that runs out
    // past it is dear.
    let typical = match app.board.slots.get(&app.board.active) {
        Some(Slot::Ready(found)) => found.insights.as_ref().and_then(|i| i.typical),
        _ => None,
    };
    let in_band = |cell: usize| {
        let price = lo + (hi - lo) * cell as f64 / (bar_width - 1) as f64;
        hi - lo > 1e-9 && typical.is_some_and(|(a, b)| (a..=b).contains(&price))
    };

    let mut lines: Vec<Line> = Vec::new();
    for (date, cell) in rows.iter().skip(offset).take(list_height) {
        let is_active = *date == app.board.active;
        let base = Style::new().bg(if is_active { th.sel_bg } else { th.bg });
        let mut spans = vec![
            Span::styled(if is_active { "►" } else { " " }, base.fg(th.bright)),
            Span::styled(
                format!("{} ", fmt::day_short(*date)),
                base.fg(if is_active { th.bright } else { th.fg })
                    .add_modifier(if is_active { BOLD } else { Modifier::empty() }),
            ),
        ];
        match cell {
            RadarCell::Price(price) => {
                let t = if hi - lo > 1e-9 {
                    (price - lo) / (hi - lo)
                } else {
                    0.0
                };
                let filled = 1 + (t * (bar_width - 1) as f64).round() as usize;
                let color = th.heat(t);
                let tag = if prices.len() >= 2 && *price <= lo {
                    "LOW"
                } else if prices.len() >= 3 && *price >= hi {
                    "HIGH"
                } else {
                    ""
                };
                spans.push(Span::styled(
                    format!("{:>8} ", fmt::money(*price, &app.cfg.currency)),
                    base.fg(color).add_modifier(BOLD),
                ));
                for cell in 0..bar_width {
                    spans.push(match (cell < filled, in_band(cell)) {
                        (true, _) => Span::styled("█", base.fg(color)),
                        (false, true) => Span::styled(" ", base.bg(th.band())),
                        (false, false) => Span::styled("░", base.fg(th.faint)),
                    });
                }
                spans.push(Span::styled(
                    format!(" {tag:<4}"),
                    base.fg(color).add_modifier(BOLD),
                ));
            }
            RadarCell::Pending => {
                let spinner = app.glyphs.spinner[app.frame(12.0, app.glyphs.spinner.len())];
                spans.push(Span::styled(
                    format!("{:>8} {spinner}", "SCANNING"),
                    base.fg(th.accent),
                ));
            }
            RadarCell::Failed => spans.push(Span::styled(
                format!("{:>8}", "ERROR"),
                base.fg(th.alert).add_modifier(BOLD),
            )),
            RadarCell::Empty => spans.push(Span::styled(
                format!("{:>8} NO FLIGHTS", "-"),
                base.fg(th.dim),
            )),
        }
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        spans.push(Span::styled(
            " ".repeat(usize::from(inner.width).saturating_sub(used)),
            base,
        ));
        lines.push(Line::from(spans));
    }
    lines.extend(insight);
    f.render_widget(Paragraph::new(lines), inner);
}

/// Price context for the date on show: Google's "is this cheap?" verdict,
/// the usual range, and a 60-day trend.
fn insight_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let th = app.theme;
    let Some(Slot::Ready(found)) = app.board.slots.get(&app.board.active) else {
        return Vec::new();
    };
    let mut lines = vec![Line::from(Span::styled(
        "─".repeat(width),
        Style::new().fg(th.faint),
    ))];
    let label = |text: &str| Span::styled(format!(" {text:<9}"), Style::new().fg(th.dim));
    if let Some(insights) = &found.insights {
        if let Some(level) = &insights.level {
            let color = match level.as_str() {
                "low" => th.good,
                "high" => th.alert,
                _ => th.accent,
            };
            lines.push(Line::from(vec![
                label("LEVEL"),
                Span::styled(
                    level.to_uppercase(),
                    Style::new().fg(color).add_modifier(BOLD),
                ),
                Span::styled(" FOR THIS TRIP", Style::new().fg(th.dim)),
            ]));
        }
        if let Some((lo, hi)) = insights.typical {
            let cur = &app.cfg.currency;
            lines.push(Line::from(vec![
                label("TYPICAL"),
                Span::styled(
                    format!("{} - {}", fmt::money(lo, cur), fmt::money(hi, cur)),
                    Style::new().fg(th.fg),
                ),
                Span::raw(" "),
                // The key to the grey band on the bars above.
                Span::styled("   ", Style::new().bg(th.band())),
            ]));
        }
        if insights.history.len() >= 2 {
            let chart = sparkline(
                &insights.history,
                width.saturating_sub(15).clamp(4, 24),
                &app.glyphs,
            );
            lines.push(Line::from(vec![
                label("TREND"),
                Span::styled(chart, Style::new().fg(th.accent)),
                Span::styled(
                    format!(" {}D", insights.history.len()),
                    Style::new().fg(th.dim),
                ),
            ]));
        }
    }
    if let Some(age) = found.cache_age_min {
        lines.push(Line::from(vec![
            label("CACHED"),
            Span::styled(
                format!("{} AGO, NO CALL USED", fmt::duration(age)),
                Style::new().fg(th.dim),
            ),
        ]));
    }
    if lines.len() == 1 {
        lines.clear(); // nothing but the rule: show nothing
    }
    lines
}

/// Squeeze a series into `width` characters of rising/falling blocks.
pub fn sparkline(values: &[f64], width: usize, glyphs: &Glyphs) -> String {
    if values.is_empty() || width == 0 {
        return String::new();
    }
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let levels = glyphs.spark.len();
    (0..width)
        .map(|i| {
            // Average the bucket of samples this column stands for.
            let from = i * values.len() / width;
            let to = ((i + 1) * values.len() / width)
                .max(from + 1)
                .min(values.len());
            let mean = values[from..to].iter().sum::<f64>() / (to - from) as f64;
            let t = if hi - lo > 1e-9 {
                (mean - lo) / (hi - lo)
            } else {
                0.5
            };
            glyphs.spark[((t * (levels - 1) as f64).round() as usize).min(levels - 1)]
        })
        .collect()
}

// ------------------------------------------------------------------ tracks

pub fn tracks(f: &mut Frame, area: Rect, app: &mut App) {
    let th = app.theme;
    let focused = app.focus == Pane::Tracks && app.popup.is_none();
    let count = app.tracks().len();
    let round_trip = app.searched.as_ref().is_some_and(|q| q.ret.is_some());
    let returning = app.board.ret.is_some();

    let plain = title_style(focused, th);
    let arrow = if app.board.descending { "▼" } else { "▲" };
    let title = match app.active_query() {
        Some(q) => {
            let (stage, date) = match (returning, round_trip) {
                (true, _) => (" RETURN", q.ret.unwrap_or(q.depart)),
                (false, true) => (" OUTBOUND", q.depart),
                (false, false) => ("", q.depart),
            };
            Line::from(vec![
                Span::styled(" TRACKS ", plain),
                trip_chip(round_trip, th),
                Span::styled(
                    format!(
                        "{stage} {}  {count} FOUND  BY {} {arrow} ",
                        fmt::day(date),
                        app.board.sort.label()
                    ),
                    plain,
                ),
            ])
        }
        None => Line::from(Span::styled(" TRACKS ", plain)),
    };
    let block = pane_with(title, focused, th);
    let inner = block.inner(area);
    f.render_widget(block, area);
    app.hit.tracks = area;
    if inner.height < 2 {
        return;
    }
    let width = usize::from(inner.width);

    // Flexible columns share whatever the fixed ones leave over.
    let fixed = 3 + 1 + 9 + 2 + 4 + 2 + 7 + 1 + 7 + 1 + 6;
    let spare = width.saturating_sub(fixed);
    let via_width = (spare * 58 / 100).max(9).min(spare);
    let carrier_width = spare.saturating_sub(via_width + 1);
    let rate = app.rate();

    let mut lines: Vec<Line> = Vec::new();
    if let Some(stage) = &app.board.ret {
        let out = &stage.outbound;
        let text = format!(
            " OUT {} {}>{} {} LOCKED - NOW PICK THE RETURN (ESC: BACK)",
            fmt::hhmm(out.dep()),
            out.origin(),
            out.dest(),
            arrival(out),
        );
        lines.push(Line::from(Span::styled(
            fmt::pad(&text, width, app.glyphs.ellipsis),
            Style::new().fg(th.bg).bg(th.info).add_modifier(BOLD),
        )));
    }
    // Column headings. Each one is a button: click it to sort by that
    // column, click again to reverse. The one in force carries the arrow.
    let headings: [(&str, usize, bool); 7] = [
        ("PRICE", 9 + 2, true),
        ("DEP", 4 + 2, false),
        ("ARR", 7 + 1, false),
        ("TIME", 7 + 1, false),
        ("VIA / LAYOVER", via_width + 1, false),
        ("CARRIER", carrier_width, false),
        (" DEAL", 6, false),
    ];
    let quiet = Style::new().fg(th.dim).add_modifier(Modifier::UNDERLINED);
    let mut heading_spans = vec![Span::styled("  # ", quiet)];
    let mut x = inner.x + 4;
    for (i, (name, cell, right_aligned)) in headings.into_iter().enumerate() {
        let sorted = TRACK_COLUMNS[i] == app.board.sort;
        let label = if sorted {
            format!("{name}{arrow}")
        } else {
            name.to_string()
        };
        // Right-aligned headings sit over right-aligned figures; both
        // leave the column's trailing gap clear.
        let text = if right_aligned {
            format!("{:>w$}  ", fmt::clip(&label, cell - 2, ""), w = cell - 2)
        } else {
            fmt::pad(&label, cell, "")
        };
        let style = if sorted {
            quiet.fg(th.bright).add_modifier(BOLD)
        } else {
            quiet
        };
        heading_spans.push(Span::styled(text, style));
        app.hit.track_cols[i] = (x, x + cell as u16);
        x += cell as u16;
    }
    app.hit.track_head = Rect::new(inner.x, inner.y + lines.len() as u16, inner.width, 1);
    lines.push(Line::from(heading_spans));
    let head = lines.len() as u16;
    let visible = usize::from(inner.height.saturating_sub(head));

    // Scroll just enough to keep the selection on screen.
    let sel = app.sel().min(count.saturating_sub(1));
    let mut offset = app.board.offset.min(count.saturating_sub(visible.max(1)));
    if sel < offset {
        offset = sel;
    } else if visible > 0 && sel >= offset + visible {
        offset = sel + 1 - visible;
    }
    app.board.offset = offset;
    app.hit.track_rows = Rect::new(
        inner.x,
        inner.y + head,
        inner.width,
        visible.min(count.saturating_sub(offset)) as u16,
    );

    let app = &*app;
    let note = |text: String, color: Color| Line::from(Span::styled(text, Style::new().fg(color)));
    let mut wrap = true;
    match app.slot() {
        None => {
            lines.push(Line::from(""));
            lines.push(note(" AWAITING ORDERS.".into(), th.dim));
            lines.push(note(
                " 1. SET FROM AND TO  (ENTER ON THE FIELD, OR CLICK AIRPORTS ON THE GLOBE)".into(),
                th.dim,
            ));
            lines.push(note(" 2. SET DATES AND FLEX".into(), th.dim));
            lines.push(note(" 3. PRESS S TO SCAN".into(), th.dim));
        }
        Some(Slot::Pending) => {
            let dots = ".".repeat(1 + app.frame(3.0, 3));
            lines.push(Line::from(""));
            lines.push(note(format!(" ACQUIRING TRACKS{dots}"), th.accent));
        }
        Some(Slot::Failed(error)) => {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!(" {error}"),
                Style::new().fg(th.alert).add_modifier(BOLD),
            )));
            lines.push(note(
                " PRESS S TO TRY AGAIN, OR SHIFT+K TO CHANGE THE API KEY.".into(),
                th.dim,
            ));
        }
        Some(Slot::Ready(_)) if count == 0 => {
            lines.push(Line::from(""));
            lines.push(note(
                " NO FLIGHTS MATCH. LOOSEN STOPS, TIMES OR AIRLINES AND RE-SCAN.".into(),
                th.accent,
            ));
        }
        Some(Slot::Ready(_)) => {
            wrap = false;
            let list = app.tracks();
            let best_by = |key: &dyn Fn(&Itinerary) -> f64| {
                list.iter()
                    .enumerate()
                    .min_by(|a, b| key(a.1).total_cmp(&key(b.1)))
                    .map(|(i, _)| i)
            };
            let cheapest = best_by(&|it| it.price.unwrap_or(f64::MAX));
            let fastest = best_by(&|it| f64::from(it.total_min));
            let best = best_by(&|it| deal_score(it, rate));
            for (i, it) in list.iter().enumerate().skip(offset).take(visible) {
                let tag = match (cheapest == Some(i), fastest == Some(i), best == Some(i)) {
                    (true, true, _) => "ACE",
                    (true, ..) => "LOW",
                    (_, true, _) => "FAST",
                    (_, _, true) => "BEST",
                    _ => "",
                };
                lines.push(track_row(
                    app,
                    it,
                    i,
                    tag,
                    i == sel,
                    via_width,
                    carrier_width,
                    width,
                ));
            }
        }
    }
    let paragraph = Paragraph::new(lines);
    f.render_widget(
        if wrap {
            paragraph.wrap(Wrap { trim: false })
        } else {
            paragraph
        },
        inner,
    );
}

/// "0950+1": arrival time, with a day marker when it lands on a later date.
fn arrival(it: &Itinerary) -> String {
    match it.day_shift() {
        0 => fmt::hhmm(it.arr()),
        n => format!("{}{n:+}", fmt::hhmm(it.arr())),
    }
}

#[allow(clippy::too_many_arguments)]
fn track_row(
    app: &App,
    it: &Itinerary,
    index: usize,
    tag: &str,
    selected: bool,
    via_width: usize,
    carrier_width: usize,
    width: usize,
) -> Line<'static> {
    let th = app.theme;
    let ellipsis = app.glyphs.ellipsis;
    let base = if selected {
        Style::new().bg(th.sel_bg).fg(th.bright).add_modifier(BOLD)
    } else {
        Style::new().bg(th.bg).fg(th.fg)
    };
    let price = it
        .price
        .map_or("N/A".to_string(), |p| fmt::money(p, &app.cfg.currency));
    let price_color = match tag {
        "LOW" | "ACE" => th.good,
        _ if selected => th.bright,
        _ => th.fg,
    };
    // Each stop with its wait. If that will not fit, fall back to the stop
    // codes and the total time on the ground rather than cutting one off.
    let via = if it.layovers.is_empty() {
        "NONSTOP".to_string()
    } else {
        let full = it
            .layovers
            .iter()
            .map(|l| format!("{} {}", l.airport, fmt::duration_short(l.duration_min)))
            .collect::<Vec<_>>()
            .join("  ");
        if full.chars().count() <= via_width {
            full
        } else {
            let codes = it
                .layovers
                .iter()
                .map(|l| l.airport.as_str())
                .collect::<Vec<_>>()
                .join("+");
            let waiting: u32 = it.layovers.iter().map(|l| l.duration_min).sum();
            let compact = format!("{codes} {}", fmt::duration_short(waiting));
            if compact.chars().count() <= via_width {
                compact
            } else {
                codes
            }
        }
    };
    // Airline names when there is room, two-letter codes when there is not.
    let names = it.carriers().join("+").to_uppercase();
    let carrier = if names.chars().count() <= carrier_width {
        names
    } else {
        let mut codes: Vec<&str> = Vec::new();
        for segment in &it.segments {
            if !codes.contains(&segment.airline_code.as_str()) {
                codes.push(&segment.airline_code);
            }
        }
        codes.join("+")
    };
    // The worst layover problem colours the whole VIA cell.
    let via_color = if it.layovers.is_empty() {
        th.info
    } else if it
        .layovers
        .iter()
        .any(|l| l.flag() == Some(LayoverFlag::Tight))
    {
        th.alert
    } else if it.layovers.iter().any(|l| l.flag().is_some()) {
        th.accent
    } else {
        base.fg.unwrap_or(th.fg)
    };
    let tag_color = match tag {
        "LOW" | "ACE" => th.good,
        "FAST" => th.info,
        _ => th.accent,
    };
    let mut spans = vec![
        Span::styled(
            format!("{:>3} ", index + 1),
            base.fg(if selected { th.bright } else { th.dim }),
        ),
        Span::styled(
            format!("{price:>9}  "),
            base.fg(price_color).add_modifier(BOLD),
        ),
        Span::styled(format!("{:<4}  ", fmt::hhmm(it.dep())), base),
        Span::styled(format!("{:<7} ", arrival(it)), base),
        Span::styled(format!("{:<7} ", fmt::duration(it.total_min)), base),
        Span::styled(
            format!("{} ", fmt::pad(&via, via_width, ellipsis)),
            base.fg(via_color),
        ),
        Span::styled(fmt::pad(&carrier, carrier_width, ellipsis), base),
        Span::styled(format!(" {tag:<5}"), base.fg(tag_color).add_modifier(BOLD)),
    ];
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), base));
    Line::from(spans)
}

// ----------------------------------------------------------------- dossier

pub fn dossier(f: &mut Frame, area: Rect, app: &App) {
    let block = pane("DOSSIER", false, app.theme);
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(
        Paragraph::new(dossier_lines(app, inner.width)).wrap(Wrap { trim: false }),
        inner,
    );
}

/// Everything known about the selected itinerary, as a vertical timeline.
pub fn dossier_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let th = app.theme;
    let width = usize::from(width);
    let rule = || Line::from(Span::styled("─".repeat(width), Style::new().fg(th.faint)));
    let Some(selected) = app.selected() else {
        return vec![
            Line::from(""),
            Line::from(Span::styled(
                " NO TRACK SELECTED",
                Style::new().fg(th.dim).add_modifier(BOLD),
            )),
            Line::from(Span::styled(
                " SCAN, THEN PICK A FLIGHT TO SEE",
                Style::new().fg(th.dim),
            )),
            Line::from(Span::styled(
                " EVERY LEG AND LAYOVER HERE.",
                Style::new().fg(th.dim),
            )),
        ];
    };
    let round_trip = app.searched.as_ref().is_some_and(|q| q.ret.is_some());
    let mut lines = Vec::new();
    match &app.board.ret {
        Some(stage) => {
            lines.extend(itinerary_lines(
                app,
                &stage.outbound,
                "OUTBOUND (LOCKED)",
                false,
                width,
            ));
            lines.push(rule());
            lines.extend(itinerary_lines(
                app,
                selected,
                "RETURN (SELECTED)",
                true,
                width,
            ));
        }
        None => {
            let stage = if round_trip { "OUTBOUND" } else { "" };
            lines.extend(itinerary_lines(app, selected, stage, false, width));
        }
    }
    lines.push(rule());

    let label = |text: &str| Span::styled(format!(" {text:<9}"), Style::new().fg(th.dim));
    let price = selected.price.map_or("NOT QUOTED".to_string(), |p| {
        fmt::money(p, &app.cfg.currency)
    });
    let pax = app.searched.as_ref().map_or(1, |q| q.adults);
    // What the figure covers, in the trip type's own colour: the one thing
    // about a price that must not be misread.
    let basis = match (round_trip, app.board.ret.is_some()) {
        (true, true) => "BOTH WAYS, THIS PAIRING",
        (true, false) => "BOTH WAYS, FROM",
        _ => "ONE WAY",
    };
    lines.push(Line::from(vec![
        label("TOTAL"),
        Span::styled(price, Style::new().fg(th.good).add_modifier(BOLD)),
        Span::styled(
            format!("  {basis}"),
            Style::new().fg(th.trip(round_trip)).add_modifier(BOLD),
        ),
        Span::styled(format!("  {pax} PAX"), Style::new().fg(th.dim)),
    ]));
    if let Some(kg) = selected.emissions_kg {
        let versus = selected
            .emissions_vs_typical_pct
            .map_or(String::new(), |pct| format!("  ({pct:+}% VS TYPICAL)"));
        let color = match selected.emissions_vs_typical_pct {
            Some(p) if p > 10 => th.accent,
            Some(p) if p < -5 => th.good,
            _ => th.fg,
        };
        lines.push(Line::from(vec![
            label("CO2"),
            Span::styled(
                format!("{} KG", fmt::thousands(u64::from(kg))),
                Style::new().fg(color),
            ),
            Span::styled(versus, Style::new().fg(th.dim)),
        ]));
    }
    let mut extras: Vec<String> = Vec::new();
    for extra in selected.segments.iter().flat_map(|s| &s.extras) {
        let extra = extra.to_uppercase();
        if !extras.contains(&extra) {
            extras.push(extra);
        }
    }
    for (i, extra) in extras.iter().take(6).enumerate() {
        lines.push(Line::from(vec![
            label(if i == 0 { "ON BOARD" } else { "" }),
            Span::styled(
                fmt::clip(extra, width.saturating_sub(11), app.glyphs.ellipsis),
                Style::new().fg(th.fg),
            ),
        ]));
    }
    lines.push(rule());
    let action = if round_trip && app.board.ret.is_none() {
        " ENTER  CHOOSE RETURN FLIGHT"
    } else {
        " SPACE  FULL-SCREEN DOSSIER"
    };
    lines.push(Line::from(Span::styled(action, Style::new().fg(th.accent))));
    lines.push(Line::from(Span::styled(
        " O      BOOK IN BROWSER",
        Style::new().fg(th.accent),
    )));
    lines
}

/// One journey as a timeline. Every airport is coloured by how far along
/// the journey it is - the same cyan-to-pink ramp the map draws the route
/// in - so the two can be read against each other. `back` is a return
/// flight, which runs the ramp the other way: a place keeps its colour.
fn itinerary_lines(
    app: &App,
    it: &Itinerary,
    stage: &str,
    back: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let th = app.theme;
    let db = airports::db();
    let flights = it.segments.len().max(1);
    let tone = |step: usize| {
        let t = step as f64 / flights as f64;
        if back { 1.0 - t } else { t }
    };
    let round_trip = app.searched.as_ref().is_some_and(|q| q.ret.is_some());
    // "LONDON HEATHROW", not "LONDON LONDON HEATHROW".
    let place = |code: &str, fallback: &str| match db.get(code) {
        Some(a) => {
            let (city, name) = (a.city.to_uppercase(), a.name.to_uppercase());
            if name.contains(&city) {
                name
            } else {
                format!("{city} {name}")
            }
        }
        None => fallback.to_uppercase(),
    };
    let stop = |when, code: &str, name: &str, note: String, step: usize| {
        let stamp = fmt::stamp(when);
        let room = width.saturating_sub(3 + stamp.len() + 1 + code.len() + 1 + note.len());
        let color = th.route(tone(step));
        Line::from(vec![
            Span::styled(" ● ", Style::new().fg(color)),
            Span::styled(
                format!("{stamp} "),
                Style::new().fg(th.bright).add_modifier(BOLD),
            ),
            Span::styled(
                format!("{code} "),
                Style::new().fg(color).add_modifier(BOLD),
            ),
            Span::styled(
                fmt::clip(&place(code, name), room, app.glyphs.ellipsis),
                Style::new().fg(th.fg),
            ),
            Span::styled(note, Style::new().fg(th.accent).add_modifier(BOLD)),
        ])
    };
    // The line joining two stops takes the colour of the flight between.
    let detail = |text: String, color: Color, flight: usize| {
        let rail = th.route_dim((tone(flight) + tone(flight + 1)) / 2.0);
        Line::from(vec![
            Span::styled(" │   ", Style::new().fg(rail)),
            Span::styled(text, Style::new().fg(color)),
        ])
    };

    let stops = match it.stops() {
        0 => "NONSTOP".to_string(),
        1 => "1 STOP".to_string(),
        n => format!("{n} STOPS"),
    };
    let end = |code: &str, step: usize| {
        Span::styled(
            code.to_string(),
            Style::new().fg(th.route(tone(step))).add_modifier(BOLD),
        )
    };
    let mut heading = vec![Span::raw(" "), trip_chip(round_trip, th)];
    if !stage.is_empty() {
        heading.push(Span::styled(
            format!(" {stage}"),
            Style::new().fg(th.fg).add_modifier(BOLD),
        ));
    }
    heading.extend([
        Span::raw("  "),
        end(it.origin(), 0),
        Span::styled(" ► ", Style::new().fg(th.dim)),
        end(it.dest(), flights),
    ]);
    let mut lines = vec![
        Line::from(heading),
        Line::from(Span::styled(
            format!(
                " {}  {stops}  {}",
                fmt::duration(it.total_min),
                it.carriers().join(" + ").to_uppercase()
            ),
            Style::new().fg(th.fg),
        )),
    ];
    for (i, seg) in it.segments.iter().enumerate() {
        lines.push(stop(seg.dep, &seg.from, &seg.from_name, String::new(), i));
        let aircraft = seg
            .aircraft
            .as_deref()
            .map_or(String::new(), |a| format!("  {}", a.to_uppercase()));
        lines.push(detail(
            format!(
                "{}  {}{aircraft}",
                seg.flight_no,
                seg.airline.to_uppercase()
            ),
            th.fg,
            i,
        ));
        let mut facts = vec![fmt::duration(seg.duration_min)];
        if let Some(cabin) = &seg.cabin {
            facts.push(cabin.to_uppercase());
        }
        if let Some(legroom) = &seg.legroom {
            facts.push(format!("LEGROOM {}", legroom.to_uppercase()));
        }
        if seg.overnight {
            facts.push("OVERNIGHT".into());
        }
        lines.push(detail(facts.join("  "), th.dim, i));
        if seg.often_delayed {
            lines.push(detail("! OFTEN DELAYED 30+ MIN".into(), th.alert, i));
        }
        let last = i == it.segments.len() - 1;
        let shift = match (last, it.day_shift()) {
            (true, n) if n != 0 => format!("  {n:+} DAY{}", if n.abs() == 1 { "" } else { "S" }),
            _ => String::new(),
        };
        lines.push(stop(seg.arr, &seg.to, &seg.to_name, shift, i + 1));
        if let Some(layover) = it.layovers.get(i) {
            let (flag, color) = match layover.flag() {
                Some(LayoverFlag::Tight) => ("  [TIGHT CONNECTION]", th.alert),
                Some(LayoverFlag::Overnight) => ("  [OVERNIGHT]", th.accent),
                Some(LayoverFlag::Long) => ("  [LONG WAIT]", th.accent),
                None => ("", th.fg),
            };
            // A change of airport mid-layover is easy to miss and costly.
            let moved = it
                .segments
                .get(i + 1)
                .is_some_and(|next| next.from != seg.to);
            lines.push(Line::from(vec![
                Span::styled(format!(" {}   ", app.glyphs.wait), Style::new().fg(th.dim)),
                Span::styled(
                    format!("LAYOVER {}", fmt::duration(layover.duration_min)),
                    Style::new().fg(color).add_modifier(BOLD),
                ),
                Span::styled(flag, Style::new().fg(color).add_modifier(BOLD)),
                Span::styled(
                    if moved { "  [CHANGE OF AIRPORT]" } else { "" },
                    Style::new().fg(th.alert).add_modifier(BOLD),
                ),
            ]));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::{RICH, SAFE};

    #[test]
    fn sparkline_tracks_the_shape_of_the_series() {
        let rising: Vec<f64> = (0..16).map(f64::from).collect();
        let line = sparkline(&rising, 8, &RICH);
        assert_eq!(line.chars().count(), 8);
        assert_eq!(line.chars().next(), Some('▁'));
        assert_eq!(line.chars().last(), Some('█'));
        // Flat series sit mid-height rather than dividing by zero.
        assert_eq!(sparkline(&[5.0; 4], 4, &SAFE), "▒▒▒▒");
        assert_eq!(sparkline(&[], 4, &RICH), "");
        // More columns than samples must not panic.
        assert_eq!(sparkline(&[1.0, 2.0], 6, &RICH).chars().count(), 6);
    }
}
